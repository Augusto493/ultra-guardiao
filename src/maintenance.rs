//! Manutenção segura: lê a saúde do PC (disco, energia, inicialização, handles, SSD) e faz só o que
//! é inofensivo e sem administrador. Nada aqui fecha programas, mexe em registro ou em serviços.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::{clock, config, sys};

const PLANO_EQUILIBRADO: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub at: i64,
    pub disk_free_gb: f64,
    pub disk_total_gb: f64,
    pub disk_busy_pct: Option<f32>,
    pub commit_pct: f32,
    pub temp_mb: f64,
    pub power_plan: String,
    pub startup: Vec<String>,
    pub handle_hogs: Vec<(String, u32)>,
    pub ssd: Option<String>,
    pub cpu_temp_c: Option<f32>,
    pub selftest: Vec<(bool, String)>,
    pub notes: Vec<String>,
}

// ---- Funções puras (testáveis) ----

/// "GUID do Esquema de Energia: 381b...  (Equilibrado)" -> "Equilibrado".
fn parse_plan_name(out: &str) -> String {
    let line = out.lines().find(|l| l.contains('(')).unwrap_or("");
    match (line.rfind('('), line.rfind(')')) {
        (Some(a), Some(b)) if b > a => line[a + 1..b].trim().to_string(),
        _ => String::new(),
    }
}

fn is_power_saver(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("economia") || n.contains("power saver")
}

/// Nomes de valores de `reg query ...\Run`: linhas "    Nome    REG_SZ    comando".
fn parse_reg_names(out: &str) -> Vec<String> {
    let mut v = Vec::new();
    for line in out.lines() {
        if !line.starts_with(' ') { continue; }
        if let Some(i) = line.find(" REG_") {
            let name = line[..i].trim();
            if !name.is_empty() { v.push(name.to_string()); }
        }
    }
    v
}

fn parse_f32(s: &str) -> Option<f32> {
    s.trim().replace(',', ".").parse::<f32>().ok().filter(|x| x.is_finite())
}

/// Soma o tamanho dos arquivos (sem seguir atalhos/links), com limite de profundidade e de itens.
fn dir_size(dir: &Path, depth: u32, budget: &mut u32) -> u64 {
    if depth > 12 { return 0; }
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut total = 0u64;
    for e in rd.flatten() {
        if *budget == 0 { break; }
        *budget -= 1;
        let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
        let ft = md.file_type();
        if ft.is_symlink() { continue; }
        if ft.is_dir() { total += dir_size(&e.path(), depth + 1, budget); } else { total += md.len(); }
    }
    total
}

/// Apaga arquivos mais velhos que `age` dentro de `dir` (e pastas que ficarem vazias). Ignora erros
/// e arquivos em uso. Nunca sai de `dir`: não segue links. Devolve os bytes liberados.
fn clean_old(dir: &Path, age: Duration, depth: u32, budget: &mut u32) -> u64 {
    if depth > 12 { return 0; }
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let Some(limit) = SystemTime::now().checked_sub(age) else { return 0 };
    let mut freed = 0u64;
    for e in rd.flatten() {
        if *budget == 0 { break; }
        *budget -= 1;
        let p = e.path();
        let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
        let ft = md.file_type();
        if ft.is_symlink() { continue; }
        if ft.is_dir() {
            freed += clean_old(&p, age, depth + 1, budget);
            // remove_dir falha em pasta com conteúdo, então só some se ficou vazia
            if md.modified().is_ok_and(|m| m < limit) { let _ = std::fs::remove_dir(&p); }
        } else if md.modified().is_ok_and(|m| m < limit) && std::fs::remove_file(&p).is_ok() {
            freed += md.len();
        }
    }
    freed
}

fn mb(bytes: u64) -> f64 { bytes as f64 / sys::MB }

fn temp_dir() -> Option<std::path::PathBuf> {
    let t = std::path::PathBuf::from(std::env::var_os("TEMP")?);
    // trava de segurança: precisa ser uma pasta de verdade, não uma raiz de disco
    if t.components().count() >= 3 && t.is_dir() { Some(t) } else { None }
}

fn active_plan() -> String {
    sys::run_timeout("powercfg.exe", &["/getactivescheme"], 15)
        .map(|(_, s)| parse_plan_name(&s))
        .unwrap_or_default()
}

// ---- Coleta ----

fn disk_busy() -> Option<f32> {
    let s = sys::powershell(
        "(Get-Counter '\\PhysicalDisk(_Total)\\% Disk Time' -SampleInterval 1 -MaxSamples 2).CounterSamples[0].CookedValue.ToString([cultureinfo]::InvariantCulture)",
    )?;
    parse_f32(s.lines().last().unwrap_or(""))
}

fn ssd_info() -> Option<String> {
    let s = sys::powershell(
        "$d=Get-PhysicalDisk -ErrorAction Stop | Select-Object -First 1; $r=$d | Get-StorageReliabilityCounter -ErrorAction SilentlyContinue; \
         \"$($d.HealthStatus)|$($r.Temperature)|$($r.Wear)\"",
    )?;
    let line = s.lines().rev().find(|l| l.contains('|'))?.trim().to_string();
    let mut it = line.split('|');
    let health = it.next()?.trim();
    if health.is_empty() { return None; }
    let mut out = match health {
        "Healthy" => "Saudável".to_string(),
        "Warning" => "Atenção".to_string(),
        "Unhealthy" => "Com problemas".to_string(),
        o => o.to_string(),
    };
    if let Some(t) = it.next().and_then(parse_f32).filter(|t| *t > 0.0) { out.push_str(&format!(" · {t:.0} °C")); }
    if let Some(w) = it.next().and_then(parse_f32) { out.push_str(&format!(" · desgaste {w:.0}%")); }
    Some(out)
}

fn cpu_temp() -> Option<f32> {
    let s = sys::powershell(
        "$t=Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature -ErrorAction Stop | Select-Object -First 1; \
         (($t.CurrentTemperature/10)-273.15).ToString([cultureinfo]::InvariantCulture)",
    )?;
    parse_f32(s.lines().last().unwrap_or("")).filter(|t| *t > 0.0 && *t < 150.0)
}

fn startup_apps() -> Vec<String> {
    let mut v = Vec::new();
    for key in [
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
        r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run",
    ] {
        if let Some((_, out)) = sys::run_timeout("reg.exe", &["query", key], 15) {
            for n in parse_reg_names(&out) {
                if !v.contains(&n) { v.push(n); }
            }
        }
    }
    v
}

fn handle_hogs() -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = sys::processes()
        .into_iter()
        .filter(|p| p.pid > 4)
        .map(|p| (p.name, sys::handle_count(p.pid)))
        .filter(|(_, n)| *n > 10_000)
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.truncate(5);
    v
}

fn codex_checks() -> [(bool, String); 2] {
    let installed = sys::run_timeout("where.exe", &["codex"], 15).is_some_and(|(ok, s)| ok && !s.trim().is_empty());
    let logged = installed
        && sys::run_timeout("cmd.exe", &["/c", "codex", "login", "status"], 30).is_some_and(|(ok, s)| {
            let l = s.to_lowercase();
            ok && !l.contains("not logged") && !l.contains("not signed")
        });
    [
        (installed, if installed { "Codex instalado".into() } else { "Codex não encontrado".into() }),
        (logged, if logged { "Codex logado".into() } else { "Codex sem login (rode: codex login)".into() }),
    ]
}

fn config_writable() -> bool {
    let p = config::dir().join(".teste-escrita");
    let ok = std::fs::write(&p, b"ok").is_ok();
    let _ = std::fs::remove_file(&p);
    ok
}

fn task_exists() -> bool {
    sys::powershell("if(Get-ScheduledTask -TaskName 'UltraGuardiao' -ErrorAction SilentlyContinue){'1'}")
        .is_some_and(|s| s.contains('1'))
}

fn run_key_present() -> bool {
    sys::run_timeout(
        "reg.exe",
        &["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", "UltraGuardiao"],
        15,
    )
    .is_some_and(|(ok, s)| ok && s.contains("UltraGuardiao"))
}

/// Lê tudo (bloqueante, alguns segundos). As consultas lentas rodam em paralelo.
pub fn collect() -> Status {
    let (free, total) = sys::disk_c();
    let mem = sys::memory();
    let commit_pct = if mem.commit_limit == 0 { 0.0 } else { (mem.commit_used as f64 / mem.commit_limit as f64 * 100.0) as f32 };
    let mut st = Status {
        at: clock::now_unix(),
        disk_free_gb: free as f64 / sys::GB,
        disk_total_gb: total as f64 / sys::GB,
        commit_pct,
        ..Default::default()
    };

    let (busy, ssd, cpu_t, codex, task) = std::thread::scope(|s| {
        let a = s.spawn(disk_busy);
        let b = s.spawn(ssd_info);
        let c = s.spawn(cpu_temp);
        let d = s.spawn(codex_checks);
        let e = s.spawn(task_exists);
        (
            a.join().unwrap_or(None),
            b.join().unwrap_or(None),
            c.join().unwrap_or(None),
            d.join().unwrap_or_else(|_| [(false, "Codex: falha ao verificar".into()), (false, "Codex: falha ao verificar".into())]),
            e.join().unwrap_or(false),
        )
    });
    st.disk_busy_pct = busy;
    st.ssd = ssd;
    st.cpu_temp_c = cpu_t;

    let mut budget = 300_000u32;
    st.temp_mb = temp_dir().map(|t| mb(dir_size(&t, 0, &mut budget))).unwrap_or(0.0);
    st.power_plan = active_plan();
    st.startup = startup_apps();
    st.handle_hogs = handle_hogs();

    let free_pct = if total == 0 { 0.0 } else { free as f64 / total as f64 * 100.0 };
    let [inst, logged] = codex;
    st.selftest.push(inst);
    st.selftest.push(logged);
    st.selftest.push((config_writable(), "Pasta de configuração gravável".into()));
    st.selftest.push((task, "Tarefa agendada UltraGuardiao existe".into()));
    st.selftest.push((run_key_present(), "Chave de inicialização (Run) presente".into()));
    st.selftest.push((free_pct > 10.0, format!("Espaço livre em C: {free_pct:.0}% (mínimo 10%)")));
    st.selftest.push((
        !is_power_saver(&st.power_plan),
        format!("Plano de energia: {}", if st.power_plan.is_empty() { "desconhecido" } else { &st.power_plan }),
    ));
    st
}

// ---- Ações seguras ----

pub fn run_safe_tasks(_cfg: &config::Config) -> Vec<String> {
    let mut log = Vec::new();

    // 1) Pouco espaço no C: -> limpa só temporários antigos.
    let (free, total) = sys::disk_c();
    if total > 0 && (free as f64 / total as f64) < 0.12 {
        let mut freed = 0u64;
        let mut budget = 300_000u32;
        if let Some(t) = temp_dir() { freed += clean_old(&t, Duration::from_secs(2 * 86400), 0, &mut budget); }
        if let Some(la) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from) {
            for sub in ["CrashDumps", "D3DSCache"] {
                let mut b = 100_000u32;
                freed += clean_old(&la.join(sub), Duration::from_secs(7 * 86400), 0, &mut b);
            }
        }
        log.push(format!(
            "Disco C: com {:.0}% livre: temporários antigos apagados, {:.0} MB liberados.",
            free as f64 / total as f64 * 100.0,
            mb(freed)
        ));
    }

    // 2) Plano de energia "Economia de energia" -> Equilibrado.
    let plan = active_plan();
    if is_power_saver(&plan) {
        let ok = sys::run_timeout("powercfg.exe", &["/setactive", PLANO_EQUILIBRADO], 15).is_some_and(|(ok, _)| ok);
        let _ = sys::run_timeout("powercfg.exe", &["/change", "disk-timeout-ac", "0"], 15);
        log.push(if ok {
            format!("Plano de energia \"{plan}\" trocado para Equilibrado; disco não desliga mais na tomada.")
        } else {
            format!("Plano de energia \"{plan}\" está ativo e não consegui trocar para Equilibrado.")
        });
    }

    // 3) Commit alto: não há ação segura, só avisa.
    let m = sys::memory();
    if m.commit_limit > 0 {
        let pct = m.commit_used as f64 / m.commit_limit as f64 * 100.0;
        if pct > 85.0 {
            log.push(format!("Aviso: memória comprometida em {pct:.0}% do limite (RAM + arquivo de paginação); sem ação segura automática."));
        }
    }
    log
}

// ---- Relatório diário ----

pub fn write_daily_report(text: &str) {
    use std::io::Write;
    let path = config::dir().join("relatorio-diario.txt");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 200 * 1024) {
        let _ = std::fs::rename(&path, config::dir().join("relatorio-diario.antigo.txt"));
    }
    let stamp = clock::dd_mm_hhmm(clock::now_unix(), clock::local_offset());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = write!(f, "=== {stamp} ===\r\n{}\r\n\r\n", text.trim_end());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plano_de_energia() {
        let s = "GUID do Esquema de Energia: 381b4222-f694-41f0-9685-ff5bb260df2e  (Equilibrado)\r\n";
        assert_eq!(parse_plan_name(s), "Equilibrado");
        assert_eq!(parse_plan_name("lixo"), "");
        assert!(is_power_saver("Economia de energia"));
        assert!(is_power_saver("Power saver"));
        assert!(!is_power_saver("Equilibrado"));
    }

    #[test]
    fn nomes_da_chave_run() {
        let s = "\r\nHKEY_CURRENT_USER\\...\\Run\r\n    OneDrive    REG_SZ    \"C:\\x\\OneDrive.exe\" /background\r\n    Meu App    REG_SZ    c:\\a.exe\r\n";
        assert_eq!(parse_reg_names(s), vec!["OneDrive", "Meu App"]);
    }

    #[test]
    fn soma_de_tamanho() {
        let d = std::env::temp_dir().join(format!("ug-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(d.join("sub"));
        let _ = std::fs::write(d.join("a"), [0u8; 100]);
        let _ = std::fs::write(d.join("sub").join("b"), [0u8; 50]);
        let mut b = 1000;
        assert_eq!(dir_size(&d, 0, &mut b), 150);
        // nada é velho o bastante: nada é apagado
        let mut b = 1000;
        assert_eq!(clean_old(&d, Duration::from_secs(86400), 0, &mut b), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn numeros() {
        assert_eq!(parse_f32(" 12,5 \r\n"), Some(12.5));
        assert_eq!(parse_f32("x"), None);
    }
}
