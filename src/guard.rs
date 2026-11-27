//! O motor anti-travamento: decide o nível e age antes da RAM acabar.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::sys::{self, GB, MB, Memory, Proc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum Level {
    #[default]
    Tranquilo,
    Atencao,
    Critico,
}
impl Level {
    pub fn label(self) -> &'static str {
        match self { Self::Tranquilo => "Tranquilo", Self::Atencao => "Atenção", Self::Critico => "Crítico" }
    }
}

#[derive(Clone, Debug)]
pub struct TopProc {
    pub pid: u32,
    pub name: String,
    pub count: usize,
    pub mb: f64,
    pub cpu: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub mem: Memory,
    pub cpu: f32,
    pub level: Level,
    pub top: Vec<TopProc>,
    pub uptime: u64,
    pub freed_total: u64,
    pub closed_total: u32,
    pub orphans: usize,
    pub nprocs: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Info, Acao, Alerta }

#[derive(Clone, Debug)]
pub struct Event { pub kind: Kind, pub text: String, pub notify: bool }
impl Event {
    fn new(kind: Kind, text: impl Into<String>, notify: bool) -> Self { Self { kind, text: text.into(), notify } }
}

type Id = (u32, u64);

#[derive(Default)]
pub struct Guard {
    pub level: Level,
    last_trim: Option<Instant>,
    last_kill: Option<Instant>,
    last_notify_atencao: Option<Instant>,
    idle_since: HashMap<Id, Instant>,
    lowered: HashSet<Id>,
    cpu_high: u32,
    cpu_low: u32,
    growth: HashMap<Id, VecDeque<(Instant, u64)>>,
    last_growth: Option<Instant>,
    leak_warned: HashSet<Id>,
    restart_warned: Option<Instant>,
    pub freed_total: u64,
    pub closed_total: u32,
    cpu: sys::ProcCpu,
}

fn id(p: &Proc) -> Id { (p.pid, p.created) }

/// O programa que abriu este processo já fechou (ou o número foi reaproveitado).
pub fn is_orphan(p: &Proc, by_pid: &HashMap<u32, &Proc>) -> bool {
    if p.ppid == 0 || p.created == 0 { return false; }
    match by_pid.get(&p.ppid) {
        None => true,
        Some(parent) => parent.created > p.created,
    }
}

impl Guard {
    pub fn tick(&mut self, cfg: &Config, mem: Memory, cpu: f32, procs: &[Proc]) -> (Snapshot, Vec<Event>) {
        let now = Instant::now();
        let mut ev = Vec::new();
        let pcpu = self.cpu.sample(procs);
        let fg = sys::foreground_pid();
        let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        let fg_tree = foreground_tree(fg, &by_pid);

        // Quem está parado (CPU quase zero) e desde quando.
        let alive: HashSet<Id> = procs.iter().map(id).collect();
        self.idle_since.retain(|k, _| alive.contains(k));
        self.lowered.retain(|k| alive.contains(k));
        self.leak_warned.retain(|k| alive.contains(k));
        for p in procs {
            let c = pcpu.get(&p.pid).copied().unwrap_or(0.0);
            if c < 0.3 { self.idle_since.entry(id(p)).or_insert(now); } else { self.idle_since.remove(&id(p)); }
        }

        // Nível pela RAM.
        let used = mem.used_pct();
        let level = if used >= cfg.limite_critico { Level::Critico }
            else if used >= cfg.limite_atencao { Level::Atencao }
            else if used < cfg.limite_atencao - 4.0 { Level::Tranquilo }
            else { self.level.min(Level::Atencao) };
        if level != self.level {
            match level {
                Level::Critico => ev.push(Event::new(Kind::Alerta, format!("RAM em {used:.0}% — risco de travar. Agindo."), true)),
                Level::Atencao => {
                    let notify = self.last_notify_atencao.is_none_or(|t| now.duration_since(t) > Duration::from_secs(1800));
                    if notify { self.last_notify_atencao = Some(now); }
                    ev.push(Event::new(Kind::Alerta, format!("RAM em {used:.0}% — atenção."), notify && !cfg.automatico));
                }
                Level::Tranquilo => ev.push(Event::new(Kind::Info, format!("RAM normalizou ({used:.0}%)."), false)),
            }
            self.level = level;
        }

        let protected = |p: &Proc| cfg.is_protected(&p.name) || fg_tree.contains(&p.pid) || p.pid == std::process::id();

        if cfg.automatico {
            // Atenção/Crítico: aliviar memória dos programas parados.
            let cooldown = if level == Level::Critico { 60 } else { 180 };
            if level >= Level::Atencao && self.last_trim.is_none_or(|t| now.duration_since(t) > Duration::from_secs(cooldown)) {
                self.last_trim = Some(now);
                let (n, freed) = self.trim_idle(procs, &pcpu, &protected);
                if n > 0 {
                    ev.push(Event::new(Kind::Acao, format!("Aliviei {n} programas parados: {:.0} MB de RAM liberados.", freed as f64 / MB), false));
                }
            }
            // Crítico: fechar órfãos parados.
            if level == Level::Critico && self.last_kill.is_none_or(|t| now.duration_since(t) > Duration::from_secs(60)) {
                self.last_kill = Some(now);
                let mut closed = Vec::new();
                for p in procs {
                    if protected(p) || !cfg.orphan_ok(&p.name) || !is_orphan(p, &by_pid) { continue; }
                    let idle = self.idle_since.get(&id(p)).is_some_and(|t| now.duration_since(*t) > Duration::from_secs(180));
                    if idle && sys::kill(p.pid) {
                        closed.push(format!("{} ({:.0} MB)", p.name, p.private as f64 / MB));
                        self.closed_total += 1;
                    }
                }
                if !closed.is_empty() {
                    ev.push(Event::new(Kind::Acao, format!("Fechei {} processos órfãos parados: {}", closed.len(), closed.join(", ")), true));
                }
            }
            // CPU alta sustentada: baixar prioridade de quem pesa em segundo plano.
            if cpu >= cfg.limite_cpu { self.cpu_high += 1; self.cpu_low = 0; } else if cpu < 50.0 { self.cpu_low += 1; self.cpu_high = 0; }
            if self.cpu_high >= 4 {
                self.cpu_high = 0;
                if let Some(p) = procs.iter()
                    .filter(|p| !protected(p) && !self.lowered.contains(&id(p)))
                    .max_by(|a, b| pcpu.get(&a.pid).unwrap_or(&0.0).total_cmp(pcpu.get(&b.pid).unwrap_or(&0.0)))
                    && pcpu.get(&p.pid).copied().unwrap_or(0.0) > 15.0
                    && sys::set_low_priority(p.pid, true)
                {
                    self.lowered.insert(id(p));
                    ev.push(Event::new(Kind::Acao, format!("CPU em {cpu:.0}%: baixei a prioridade de {} (segundo plano).", p.name), false));
                }
            }
            if self.cpu_low >= 20 && !self.lowered.is_empty() {
                for (pid, _) in self.lowered.drain() { sys::set_low_priority(pid, false); }
                ev.push(Event::new(Kind::Info, "CPU acalmou: prioridades restauradas.", false));
            }
        }

        // Vazamento de memória: cresceu muito em 30 min.
        if self.last_growth.is_none_or(|t| now.duration_since(t) > Duration::from_secs(60)) {
            self.last_growth = Some(now);
            self.growth.retain(|k, _| alive.contains(k));
            for p in procs.iter().filter(|p| p.private as f64 > 400.0 * MB) {
                let q = self.growth.entry(id(p)).or_default();
                q.push_back((now, p.private));
                while q.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(1800)) { q.pop_front(); }
                let first = q.front().map(|x| x.1).unwrap_or(p.private);
                if p.private as f64 > 1.5 * GB && p.private.saturating_sub(first) as f64 > 0.8 * GB && self.leak_warned.insert(id(p)) {
                    ev.push(Event::new(Kind::Alerta, format!("{} cresceu {:.1} GB em 30 min (agora {:.1} GB) — parece vazamento. Feche e abra de novo.", p.name, (p.private - first) as f64 / GB, p.private as f64 / GB), true));
                }
            }
        }

        // Muito tempo ligado.
        let up = sys::uptime_secs();
        if up > cfg.dias_para_reiniciar as u64 * 86400 && self.restart_warned.is_none_or(|t| now.duration_since(t) > Duration::from_secs(86400)) {
            self.restart_warned = Some(now);
            ev.push(Event::new(Kind::Alerta, format!("PC ligado há {} sem reiniciar. Reinicie quando puder — limpa vazamentos.", crate::clock::duration(up)), true));
        }

        let orphans = procs.iter().filter(|p| cfg.orphan_ok(&p.name) && is_orphan(p, &by_pid)).count();
        let snap = Snapshot {
            mem,
            cpu,
            level: self.level,
            top: top_groups(procs, &pcpu, 8),
            uptime: up,
            freed_total: self.freed_total,
            closed_total: self.closed_total,
            orphans,
            nprocs: procs.len(),
        };
        (snap, ev)
    }

    fn trim_idle(&mut self, procs: &[Proc], pcpu: &HashMap<u32, f32>, protected: &dyn Fn(&Proc) -> bool) -> (usize, u64) {
        let before = sys::memory().avail;
        let mut n = 0;
        for p in procs {
            if protected(p) || (p.ws as f64) < 80.0 * MB { continue; }
            if pcpu.get(&p.pid).copied().unwrap_or(0.0) > 2.0 { continue; }
            if sys::trim(p.pid) { n += 1; }
        }
        let freed = sys::memory().avail.saturating_sub(before);
        self.freed_total += freed;
        (n, freed)
    }

    /// "Aliviar agora": alivia todos fora do primeiro plano, sem esperar o nível.
    pub fn relieve_now(&mut self, cfg: &Config, procs: &[Proc]) -> Event {
        let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        let fg_tree = foreground_tree(sys::foreground_pid(), &by_pid);
        let pcpu = HashMap::new();
        let protected = |p: &Proc| cfg.is_protected(&p.name) || fg_tree.contains(&p.pid);
        let (n, freed) = self.trim_idle(procs, &pcpu, &protected);
        self.last_trim = Some(Instant::now());
        Event::new(Kind::Acao, format!("Alívio manual: {n} programas, {:.0} MB liberados.", freed as f64 / MB), false)
    }

    /// Fecha todos os órfãos da lista, mesmo fora do Crítico (pedido pelo usuário).
    pub fn close_orphans_now(&mut self, cfg: &Config, procs: &[Proc]) -> Event {
        let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        let mut closed = Vec::new();
        for p in procs {
            if cfg.is_protected(&p.name) || !cfg.orphan_ok(&p.name) || !is_orphan(p, &by_pid) { continue; }
            if sys::kill(p.pid) { closed.push(p.name.clone()); self.closed_total += 1; }
        }
        if closed.is_empty() { Event::new(Kind::Info, "Nenhum processo órfão para fechar.", false) }
        else { Event::new(Kind::Acao, format!("Fechei {} órfãos: {}", closed.len(), closed.join(", ")), false) }
    }
}

/// O processo em primeiro plano e toda a família dele (pais e filhos): nunca mexemos neles.
fn foreground_tree(fg: u32, by_pid: &HashMap<u32, &Proc>) -> HashSet<u32> {
    let mut set = HashSet::new();
    if fg == 0 { return set; }
    // sobe até a raiz do app
    let mut root = fg;
    let mut guard = 0;
    while let Some(p) = by_pid.get(&root) {
        set.insert(root);
        match by_pid.get(&p.ppid) {
            Some(parent) if parent.name.eq_ignore_ascii_case(&p.name) && guard < 16 => { root = p.ppid; guard += 1; }
            _ => break,
        }
    }
    // desce pelos filhos
    let mut stack: Vec<u32> = set.iter().copied().collect();
    while let Some(pid) = stack.pop() {
        for p in by_pid.values() {
            if p.ppid == pid && set.insert(p.pid) { stack.push(p.pid); }
        }
    }
    set
}

/// Agrupa por nome (17 claude.exe viram uma linha) e ordena por RAM.
fn top_groups(procs: &[Proc], pcpu: &HashMap<u32, f32>, n: usize) -> Vec<TopProc> {
    let mut groups: HashMap<String, TopProc> = HashMap::new();
    for p in procs {
        if p.ws == 0 { continue; }
        let g = groups.entry(p.name.to_lowercase()).or_insert(TopProc { pid: p.pid, name: p.name.trim_end_matches(".exe").to_string(), count: 0, mb: 0.0, cpu: 0.0 });
        g.count += 1;
        g.mb += p.ws as f64 / MB;
        g.cpu += pcpu.get(&p.pid).copied().unwrap_or(0.0);
    }
    let mut v: Vec<TopProc> = groups.into_values().collect();
    v.sort_by(|a, b| b.mb.total_cmp(&a.mb));
    v.truncate(n);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(pid: u32, ppid: u32, created: u64) -> Proc {
        Proc { pid, ppid, name: "node.exe".into(), ws: 0, private: 0, cpu_time: 0, created }
    }
    #[test]
    fn orphan_when_parent_gone_or_reused() {
        let parent = p(10, 1, 100);
        let child = p(20, 10, 200);
        let lost = p(30, 99, 300);
        let reused_parent = p(40, 50, 100);
        let newer = p(50, 1, 500);
        let all = [parent, child.clone(), lost.clone(), reused_parent.clone(), newer];
        let by: HashMap<u32, &Proc> = all.iter().map(|x| (x.pid, x)).collect();
        assert!(!is_orphan(&child, &by));
        assert!(is_orphan(&lost, &by));
        assert!(is_orphan(&reused_parent, &by));
    }
}
