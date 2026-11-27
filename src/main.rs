//! Ultra Guardião: fica na bandeja, vigia RAM/CPU e age antes do PC travar.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analyst;
mod blackbox;
mod cleanup;
mod clock;
mod config;
mod guard;
mod health;
mod insights;
mod maintenance;
mod supervisor;
mod sys;
mod ui;

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rune::{App, AppHandle, AppSpec, Tray, TrayEvent, TrayIcon, TrayItem, TraySpec, WindowOptions};

use config::Config;
use guard::{Guard, Kind, Level};
use ui::Ui;

pub enum Cmd {
    RelieveNow,
    CloseOrphans,
    KillGroup(String),
    SetAuto(bool),
    Health,
    FaxinaCheck,
    FaxinaRun(&'static str, bool),
    FaxinaAll(Vec<&'static str>),
    Autostart(bool),
    OpenBlackBox,
    Analisar { motivo: String, rapido: bool, health: Option<health::Report>, log: Vec<(i64, Kind, String)> },
    /// "Otimizar tudo": alivia memória, fecha órfãos, roda a manutenção segura e pede análise à IA.
    OtimizarTudo,
    /// Resultado de uma análise automática: aplica sozinho só o que é seguro.
    AutoAplicar(analyst::Acao),
    AplicarAcao(analyst::Acao),
    AbrirTerminal,
}

static TX: OnceLock<Sender<Cmd>> = OnceLock::new();
pub fn send(c: Cmd) {
    if let Some(tx) = TX.get() { let _ = tx.send(c); }
}

const APP_ID: &str = "com.ultraguardiao.app";
const TICK: Duration = Duration::from_secs(3);
const CALM_TICK: Duration = Duration::from_secs(8);

/// Só o que não pode derrubar programa nenhum: proteger, ajustar limites, órfãos e faxina sem admin.
/// Nada de fechar processo, reiniciar ou pedir administrador.
fn auto_seguro(a: &analyst::Acao) -> bool {
    match a.tipo.as_str() {
        "proteger" | "adicionar_orfao" | "ajustar_limite" => true,
        "faxina" => cleanup::ITEMS.iter().any(|i| i.id == a.alvo.trim() && !i.admin),
        _ => false,
    }
}

/// Ícone da bandeja: um escudo na cor do nível.
fn icon(level: Level) -> TrayIcon {
    let (r, g, b) = match level { Level::Tranquilo => (46, 204, 113), Level::Atencao => (241, 196, 15), Level::Critico => (231, 76, 60) };
    let n = 32u32;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let fx = x as f32 + 0.5 - 16.0;
            let fy = y as f32 + 0.5;
            // escudo: topo reto com cantos, base em ponta
            let half = if fy < 18.0 { 13.0 } else { 13.0 * (1.0 - (fy - 18.0) / 13.0) };
            let inside = fy > 2.0 && fy < 31.0 && fx.abs() < half;
            if !inside { continue; }
            let edge = fy < 4.5 || fx.abs() > half - 2.5 || fy > 29.0;
            let i = ((y * n + x) * 4) as usize;
            let (cr, cg, cb) = if edge { (r / 2, g / 2, b / 2) } else { (r, g, b) };
            px[i..i + 4].copy_from_slice(&[cr, cg, cb, 255]);
        }
    }
    TrayIcon::rgba(n, n, px)
}

fn make_tray(level: Level, handle: AppHandle<Ui>) -> Option<Tray> {
    let spec = TraySpec::new(APP_ID, "Ultra Guardião", icon(level))
        .tooltip("Ultra Guardião")
        .menu(vec![
            TrayItem::action("abrir", "Abrir painel"),
            TrayItem::action("aliviar", "Aliviar memória agora"),
            TrayItem::action("orfaos", "Fechar processos órfãos"),
            TrayItem::Separator,
            TrayItem::action("sair", "Sair do Guardião"),
        ]);
    Tray::new(spec, move |e| match e {
        TrayEvent::Activate { .. } => handle.open(),
        TrayEvent::Menu(id) => match id.as_str() {
            "abrir" => handle.open(),
            "aliviar" => send(Cmd::RelieveNow),
            "orfaos" => send(Cmd::CloseOrphans),
            "sair" => handle.quit(),
            _ => {}
        },
        _ => {}
    })
    .map_err(|e| eprintln!("bandeja: {e}"))
    .ok()
}

static SILENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn notify(title: &str, body: &str) {
    if SILENT.load(std::sync::atomic::Ordering::Relaxed) { return; }
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let id = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    rune_notify::post(&rune_notify::Notification::new(format!("ug-{id}"), title).body(body));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Modo supervisor: processo minúsculo que mantém o Guardião vivo (e reinicia se cair).
    if args.iter().any(|a| a == "--supervisor") {
        let rest: Vec<String> = args.into_iter().filter(|a| a != "--supervisor").collect();
        supervisor::run(rest);
        return;
    }
    // Aberto à mão (ou por um atalho antigo): passa a rodar sempre sob supervisão.
    if std::env::var_os("UG_SUPERVISED").is_none() && !supervisor::running() && supervisor::spawn_detached(&args) {
        return;
    }

    if !rune::single_instance(APP_ID) { return; }
    let _ = rune_notify::register(&rune_notify::Identity::new(APP_ID, "Ultra Guardião"));
    sys::lower_self();
    supervisor::spawn_watchdog();

    let open_now = args.iter().any(|a| a == "--abrir");
    let (tx, rx) = mpsc::channel::<Cmd>();
    let _ = TX.set(tx);

    let cfg = Config::load();
    SILENT.store(cfg.silencioso, std::sync::atomic::Ordering::Relaxed);
    if cfg.iniciar_com_windows {
        std::thread::spawn(cleanup::ensure_autostart);
    }
    let auto = cfg.automatico;
    let (ia_ativa, ia_modelo) = (cfg.ia_ativa, cfg.ia_modelo.clone());
    let mut app = App::new(AppSpec::<Ui> {
        min_size: Some((440.0, 480.0)),
        ..AppSpec::new("Ultra Guardião", (470.0, 600.0), Ui::default, ui::view)
    })
    .init(move || Ui {
        auto,
        autostart: cleanup::autostart_enabled(),
        offset: clock::local_offset(),
        ia_ativa,
        ia_modelo,
        analise: analyst::ultima(),
        ..Ui::default()
    })
    .keep_running()
    .release_gpu_when_closed()
    .window_options(WindowOptions { min_size: Some((440.0, 480.0)), max_size: Some((620.0, 1000.0)), ..Default::default() });
    if !open_now { app = app.windowless(); }

    let handle = app.handle();
    {
        let h = handle.clone();
        rune_notify::on_click(move |_| h.open());
    }

    // Bandeja (thread própria do Rune), recriada quando o nível muda para trocar a cor.
    let tray: Arc<Mutex<Option<Tray>>> = Arc::new(Mutex::new(make_tray(Level::Tranquilo, handle.clone())));

    let worker_handle = handle.clone();
    let worker_tray = tray.clone();
    std::thread::Builder::new()
        .name("guardiao".into())
        .spawn(move || {
            // Se um tick der pane, o laço recomeça em vez de matar o Guardião.
            let mut cfg = cfg;
            loop {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker(&mut cfg, &rx, worker_handle.clone(), worker_tray.clone())));
                if r.is_ok() { break; }
                worker_handle.update(|s, _| s.push_log(Kind::Alerta, "O vigia teve um erro interno e foi reiniciado sozinho.".into()));
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .ok();

    app.run();
    drop(tray);
}

fn worker(cfg: &mut Config, rx: &mpsc::Receiver<Cmd>, ui: AppHandle<Ui>, tray: Arc<Mutex<Option<Tray>>>) {
    let mut guard = Guard::default();
    let mut cpu = sys::CpuMeter::default();
    cpu.sample();
    let mut bb = blackbox::BlackBox::new(cfg.dias_caixa_preta);
    let mut last_bb: Option<Instant> = None;
    let mut last_level = Level::Tranquilo;
    let mut last_health: Option<Instant> = None;
    let mut next_tick = Instant::now();
    let mut last_snap = guard::Snapshot::default();
    let mut peak_ram: f32 = 0.0;
    let mut last_auto_ai: Option<Instant> = None;
    // Primeira análise de rotina 10 min depois de abrir; depois a cada `ia_intervalo_horas`.
    let mut next_routine = Instant::now() + Duration::from_secs(600);
    let mut tick = TICK;
    let mut next_maint = Instant::now() + Duration::from_secs(45);
    let mut next_report = Instant::now() + Duration::from_secs(24 * 3600);
    let maint_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let log = |ui: &AppHandle<Ui>, kind: Kind, text: String| ui.update(move |s, _| s.push_log(kind, text));

    loop {
        supervisor::beat();
        let wait = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(cmd) => match cmd {
                Cmd::RelieveNow => {
                    let e = guard.relieve_now(&cfg, &sys::processes());
                    log(&ui, e.kind, e.text.clone());
                    bb.write(&ui_snapshot_stub(&guard), Some(&e.text));
                }
                Cmd::OtimizarTudo => {
                    let procs = sys::processes();
                    let a = guard.relieve_now(cfg, &procs);
                    log(&ui, a.kind, a.text.clone());
                    let b = guard.close_orphans_now(cfg, &procs);
                    log(&ui, b.kind, b.text.clone());
                    let resumo = format!("{} {}", a.text, b.text);
                    let (ui2, cfg2) = (ui.clone(), cfg.clone());
                    std::thread::spawn(move || {
                        let mut st = maintenance::collect();
                        let notes = maintenance::run_safe_tasks(&cfg2);
                        st.notes = notes.clone();
                        ui2.update(move |s, _| {
                            s.manut = Some(st);
                            s.push_log(Kind::Acao, "Otimização completa: memória, órfãos e manutenção do sistema verificados.".into());
                            let extra = if notes.is_empty() { String::new() } else { format!(" {}", notes.join(" ")) };
                            for n in notes { s.push_log(Kind::Acao, n); }
                            let ia = if cfg2.ia_ativa && !s.ia_busy {
                                s.start_analysis("Otimização completa pedida no painel.".into(), true);
                                " A IA está analisando e aplica sozinha o que for seguro (leva ~2 min)."
                            } else { "" };
                            s.otim = Some((false, format!("Pronto às {}: {resumo}{extra}{ia}", clock::hhmm(clock::now_unix(), s.offset))));
                        });
                    });
                }
                Cmd::CloseOrphans => {
                    let e = guard.close_orphans_now(&cfg, &sys::processes());
                    log(&ui, e.kind, e.text);
                }
                Cmd::KillGroup(name) => {
                    let procs = sys::processes();
                    let mut n = 0;
                    for p in procs.iter().filter(|p| p.name.eq_ignore_ascii_case(&name)) {
                        if !cfg.is_protected(&p.name) && sys::kill(p.pid) { n += 1; }
                    }
                    let msg = if n > 0 { format!("Você fechou {name} ({n} processos).") } else { format!("Não consegui fechar {name} (protegido ou do sistema).") };
                    log(&ui, Kind::Acao, msg);
                }
                Cmd::SetAuto(on) => {
                    cfg.automatico = on;
                    cfg.save();
                    log(&ui, Kind::Info, if on { "Modo automático ligado.".into() } else { "Modo automático desligado: só aviso, não ajo.".into() });
                }
                Cmd::Health => spawn_health(ui.clone(), false),
                Cmd::FaxinaCheck => spawn_faxina(ui.clone(), None),
                Cmd::FaxinaRun(id, undo) => spawn_faxina(ui.clone(), Some((vec![id], undo))),
                Cmd::FaxinaAll(ids) => spawn_faxina(ui.clone(), Some((ids, false))),
                Cmd::Autostart(on) => {
                    let ok = cleanup::set_autostart(on);
                    let real = cleanup::autostart_enabled();
                    ui.update(move |s, _| s.autostart = real);
                    if !ok { log(&ui, Kind::Alerta, "Não consegui mudar a inicialização automática.".into()); }
                }
                Cmd::Analisar { motivo, rapido, health, log } => {
                    let modelo = if rapido { cfg.ia_modelo_rapido.clone() } else { cfg.ia_modelo.clone() };
                    // Análises automáticas (tudo menos o botão do painel) aplicam sozinhas o que é seguro.
                    let aplicar_auto = cfg.ia_aplicar_auto && !motivo.starts_with("Pedido manual");
                    let ctx = analyst::Context { snap: last_snap.clone(), health, log, cfg: cfg.clone(), motivo };
                    let ui2 = ui.clone();
                    std::thread::spawn(move || {
                        analyst::run(ctx, &modelo, |p| match p {
                            analyst::Progresso::Etapa(e) => ui2.update(move |s, _| s.ia_etapa = e),
                            analyst::Progresso::Pronto(a) => {
                                if a.gravidade == "alta" { notify(&format!("Diagnóstico pronto ({})", a.gravidade), &a.resumo); }
                                if aplicar_auto {
                                    for acao in a.acoes.iter().filter(|x| auto_seguro(x)) { send(Cmd::AutoAplicar(acao.clone())); }
                                }
                                let msg = format!("IA: {}", a.resumo);
                                ui2.update(move |s, _| {
                                    s.analise = Some(a);
                                    s.ia_busy = false;
                                    s.ia_feitas.clear();
                                    s.ia_confirm = None;
                                    s.push_log(Kind::Info, msg);
                                });
                            }
                            analyst::Progresso::Erro(e) => ui2.update(move |s, _| {
                                s.ia_busy = false;
                                s.push_log(Kind::Alerta, format!("Análise com IA falhou: {e}"));
                                s.ia_etapa = e;
                            }),
                        });
                    });
                }
                Cmd::AplicarAcao(a) => {
                    let msg = apply_ai_action(cfg, &a, &ui);
                    log(&ui, Kind::Acao, msg);
                }
                Cmd::AutoAplicar(a) => {
                    // Revalida: nunca aplica sozinho algo fora da lista segura.
                    // Proteger um programa da lista de órfãos desligaria a limpeza dele: isso fica manual.
                    let alvo = a.alvo.trim().to_lowercase();
                    let alvo_exe = if alvo.ends_with(".exe") { alvo } else { format!("{alvo}.exe") };
                    if auto_seguro(&a) && !(a.tipo == "proteger" && cfg.orphan_ok(&alvo_exe)) {
                        let msg = apply_ai_action(cfg, &a, &ui);
                        log(&ui, Kind::Acao, format!("Automático — {msg}"));
                    }
                }
                Cmd::AbrirTerminal => {
                    if !analyst::abrir_terminal(&cfg.ia_modelo) {
                        log(&ui, Kind::Alerta, "Não consegui abrir o terminal do Codex.".into());
                    }
                }
                Cmd::OpenBlackBox => {
                    let _ = std::process::Command::new("explorer.exe").arg(blackbox::folder()).spawn();
                }
            },
            Err(RecvTimeoutError::Timeout) => {
                next_tick = Instant::now() + tick;
                let procs = sys::processes();
                let (snap, events) = guard.tick(&cfg, sys::memory(), cpu.sample(), &procs);

                // Caixa-preta: a cada minuto e a cada evento.
                if last_bb.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                    last_bb = Some(Instant::now());
                    bb.write(&snap, None);
                }
                for e in &events {
                    bb.write(&snap, Some(&e.text));
                    if e.notify { notify(match e.kind { Kind::Alerta => "Ultra Guardião — atenção", _ => "Ultra Guardião" }, &e.text); }
                }

                // IA automática: ao sair do Crítico, no máximo a cada 2 horas, com o modelo barato.
                if snap.level == Level::Critico { peak_ram = peak_ram.max(snap.mem.used_pct()); }
                if last_level == Level::Critico && snap.level != Level::Critico && cfg.ia_ativa && cfg.ia_automatica
                    && last_auto_ai.is_none_or(|t| t.elapsed() >= Duration::from_secs(2 * 3600))
                {
                    last_auto_ai = Some(Instant::now());
                    let motivo = format!("O PC passou pelo nível Crítico (RAM chegou a {peak_ram:.0}%) e acabou de normalizar.");
                    peak_ram = 0.0;
                    ui.update(move |s, _| s.start_analysis(motivo, true));
                }

                // IA de rotina: sozinha a cada N horas, com o modelo barato, aplicando só o que é seguro.
                if cfg.ia_ativa && cfg.ia_automatica && cfg.ia_intervalo_horas > 0 && Instant::now() >= next_routine {
                    next_routine = Instant::now() + Duration::from_secs(cfg.ia_intervalo_horas as u64 * 3600);
                    let motivo = "Análise de rotina: otimizar o PC e prevenir travamentos antes que aconteçam.".to_string();
                    ui.update(move |s, _| s.start_analysis(motivo, true));
                }

                // Manutenção segura (disco, temp, energia, autodiagnóstico): a cada 10 min, fora do laço.
                if Instant::now() >= next_maint && !maint_busy.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    next_maint = Instant::now() + Duration::from_secs(600);
                    let (ui2, cfg2, busy) = (ui.clone(), cfg.clone(), maint_busy.clone());
                    std::thread::spawn(move || {
                        let mut st = maintenance::collect();
                        let notes = maintenance::run_safe_tasks(&cfg2);
                        st.notes = notes.clone();
                        ui2.update(move |s, _| {
                            s.manut = Some(st);
                            for n in notes { s.push_log(Kind::Acao, n); }
                        });
                        busy.store(false, std::sync::atomic::Ordering::Relaxed);
                    });
                }

                // Relatório diário curto, em arquivo (nada de notificação).
                if Instant::now() >= next_report {
                    next_report = Instant::now() + Duration::from_secs(24 * 3600);
                    maintenance::write_daily_report(&format!(
                        "Nível {} · RAM {:.0}% · CPU {:.0}% · {} processos · liberou {:.1} GB · fechou {} órfãos · ligado há {}",
                        snap.level.label(), snap.mem.used_pct(), snap.cpu, snap.nprocs,
                        snap.freed_total as f64 / sys::GB, snap.closed_total, clock::duration(snap.uptime)
                    ));
                }

                // Ritmo: rápido sob pressão, calmo quando está tudo bem (poupa CPU).
                tick = if snap.level >= Level::Atencao || snap.cpu >= 60.0 { TICK } else { CALM_TICK };

                // Bandeja: cor por nível, dica com números.
                if let Ok(mut t) = tray.lock() {
                    if snap.level != last_level {
                        last_level = snap.level;
                        *t = None;
                        *t = make_tray(snap.level, ui.clone());
                    }
                    if let Some(t) = t.as_ref() {
                        t.set_tooltip(format!("Ultra Guardião — {}\nRAM {:.0}% · CPU {:.0}%", snap.level.label(), snap.mem.used_pct(), snap.cpu));
                    }
                }

                // Saúde: ao abrir e a cada 6 horas.
                if last_health.is_none_or(|t| t.elapsed() >= Duration::from_secs(6 * 3600)) {
                    last_health = Some(Instant::now());
                    spawn_health(ui.clone(), cfg.ia_ativa && cfg.ia_automatica);
                }

                last_snap = snap.clone();
                ui.update(move |s, _| {
                    s.snap = snap;
                    for e in events { s.push_log(e.kind, e.text); }
                });
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn ui_snapshot_stub(g: &Guard) -> guard::Snapshot {
    guard::Snapshot { mem: sys::memory(), level: g.level, ..Default::default() }
}

fn spawn_health(ui: AppHandle<Ui>, auto_ai: bool) {
    ui.update(|s, _| s.health_busy = true);
    std::thread::spawn(move || {
        let r = health::run();
        let crash_new = r.last_crash.as_ref().map(|c| c.0);
        ui.update(move |s, _| {
            s.health = Some(r);
            s.health_busy = false;
            if let Some(t) = crash_new {
                let day = clock::now_unix() - t < 86400;
                if day && !s.log.iter().any(|l| l.2.starts_with("Travamento total")) {
                    s.push_log(Kind::Alerta, format!("Travamento total detectado em {}. Veja a aba Saúde.", clock::dd_mm_hhmm(t, s.offset)));
                }
                // Cada travamento total é analisado pela IA uma única vez.
                if auto_ai && crash_unseen(t) {
                    s.start_analysis(format!("Travamento total (desligamento forçado) em {}.", clock::dd_mm_hhmm(t, s.offset)), false);
                }
            }
        });
    });
}

fn spawn_faxina(ui: AppHandle<Ui>, run: Option<(Vec<&'static str>, bool)>) {
    ui.update(|s, _| s.faxina_busy = true);
    std::thread::spawn(move || {
        let mut msg = None;
        if let Some((ids, undo)) = run {
            let ok = if ids.len() == 1 {
                cleanup::ITEMS.iter().find(|i| i.id == ids[0]).is_some_and(|i| cleanup::run(i, undo))
            } else {
                cleanup::apply_all(&ids)
            };
            let what = if undo { "Desfeito" } else { "Aplicado" };
            msg = Some(if ok { format!("Faxina: {what} — {}.", ids.join(", ")) } else { format!("Faxina: não concluiu ({}) — permissão negada?", ids.join(", ")) });
        }
        let states: Vec<cleanup::State> = cleanup::ITEMS.iter().map(cleanup::check).collect();
        ui.update(move |s, _| {
            s.faxina = states;
            s.faxina_busy = false;
            if let Some(m) = msg { s.push_log(Kind::Acao, m); }
        });
    });
}

/// Marca o travamento como analisado; true só na primeira vez que vemos este.
fn crash_unseen(t: i64) -> bool {
    let f = config::dir().join("ultimo_travamento_analisado.txt");
    let seen: i64 = std::fs::read_to_string(&f).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    if t <= seen { return false; }
    let _ = std::fs::write(&f, t.to_string());
    true
}

/// Executa uma ação sugerida pela IA. Só tipos conhecidos; nada de comando livre.
fn apply_ai_action(cfg: &mut Config, a: &analyst::Acao, ui: &AppHandle<Ui>) -> String {
    let alvo = a.alvo.trim().to_string();
    let exe = if alvo.to_lowercase().ends_with(".exe") { alvo.clone() } else { format!("{alvo}.exe") };
    match a.tipo.as_str() {
        "fechar_processo" => {
            if cfg.is_protected(&exe) { return format!("{exe} está protegido; não fechei."); }
            let n = sys::processes().iter().filter(|p| p.name.eq_ignore_ascii_case(&exe)).filter(|p| sys::kill(p.pid)).count();
            format!("IA → fechei {exe} ({n} processos).")
        }
        "adicionar_orfao" => {
            if cfg.is_protected(&exe) { return format!("{exe} está protegido; não adicionei."); }
            if !cfg.orphan_ok(&exe) { cfg.orfaos_fechaveis.push(exe.clone()); cfg.save(); }
            format!("IA → {exe} agora é fechado quando ficar órfão e parado.")
        }
        "proteger" => {
            if !cfg.is_protected(&exe) { cfg.protegidos.push(exe.clone()); cfg.save(); }
            format!("IA → {exe} protegido: o Guardião nunca mexe nele.")
        }
        "ajustar_limite" => {
            let Some((k, v)) = alvo.split_once('=') else { return format!("Ajuste inválido: {alvo}") };
            let Ok(v) = v.trim().parse::<f32>() else { return format!("Ajuste inválido: {alvo}") };
            let v = v.clamp(50.0, 98.0);
            // Faixas seguras: a IA nunca relaxa os limites a ponto de agir tarde demais.
            let v = match k.trim() { "atencao" => v.clamp(70.0, 90.0), "critico" => v.clamp(80.0, 95.0), _ => v.clamp(70.0, 95.0) };
            match k.trim() {
                "atencao" => cfg.limite_atencao = v.min(cfg.limite_critico - 2.0),
                "critico" => cfg.limite_critico = v.max(cfg.limite_atencao + 2.0),
                "cpu" => cfg.limite_cpu = v,
                _ => return format!("Ajuste inválido: {alvo}"),
            }
            cfg.save();
            format!("IA → limite {} ajustado para {v:.0}%.", k.trim())
        }
        "faxina" => match cleanup::ITEMS.iter().find(|i| i.id == alvo) {
            Some(item) => { spawn_faxina(ui.clone(), Some((vec![item.id], false))); format!("IA → aplicando faxina: {}.", item.title) }
            None => format!("Faxina desconhecida: {alvo}"),
        },
        "reiniciar" => {
            let ok = sys::run_hidden("shutdown.exe", &["/r", "/t", "60", "/c", "Ultra Guardião: reiniciando em 60 s. Salve seu trabalho. Para cancelar: shutdown /a"]).is_some();
            if ok { "IA → reinício agendado em 60 s (cancele com: shutdown /a).".into() } else { "Não consegui agendar o reinício.".into() }
        }
        other => format!("Ação \"{other}\" é manual: {}", a.explicacao),
    }
}
