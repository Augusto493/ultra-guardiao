//! Supervisor + watchdog: o Guardião se reinicia sozinho se cair ou travar.
//!
//! `UltraGuardiao.exe --supervisor` é um processo minúsculo, sem janela, que mantém o Guardião
//! de verdade rodando. Se o Guardião cair (código ≠ 0) ou o watchdog dele detectar que o laço
//! principal parou, o supervisor sobe outro em poucos segundos. "Sair do Guardião" (código 0)
//! encerra tudo de propósito.

use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
use windows_sys::Win32::System::Threading::{CreateMutexW, OpenMutexW};

const MUTEX: &str = "Local\\UltraGuardiaoSupervisor";
const SYNCHRONIZE: u32 = 0x0010_0000;
const ERROR_ALREADY_EXISTS: u32 = 183;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
/// Código de saída do Guardião quando o watchdog o derruba por travamento do laço principal.
const EXIT_HUNG: i32 = 3;
/// Sem batimento por tanto tempo = laço travado.
const HUNG_AFTER_SECS: i64 = 150;

static BEAT: AtomicI64 = AtomicI64::new(0);

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn log(msg: &str) {
    use std::io::Write;
    let f = crate::config::dir().join("supervisor.log");
    // Mantém o arquivo pequeno.
    if std::fs::metadata(&f).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&f, crate::config::dir().join("supervisor.old.log"));
    }
    if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
        let _ = writeln!(h, "{} {msg}", now());
    }
}

/// Já existe um supervisor rodando?
pub fn running() -> bool {
    let name = wide(MUTEX);
    // SAFETY: nome terminado em zero; o handle é fechado logo em seguida.
    unsafe {
        let h = OpenMutexW(SYNCHRONIZE, 0, name.as_ptr());
        if h.is_null() { return false; }
        CloseHandle(h);
        true
    }
}

/// Sobe um supervisor separado do processo atual e devolve true se conseguiu.
pub fn spawn_detached(args: &[String]) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    std::process::Command::new(exe)
        .arg("--supervisor")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .is_ok()
}

/// Laço do supervisor. `first_args` vão só para a primeira execução (ex.: `--abrir`).
pub fn run(first_args: Vec<String>) {
    let name = wide(MUTEX);
    // SAFETY: nome terminado em zero; o handle vive até o fim do processo.
    let _mutex = unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() || GetLastError() == ERROR_ALREADY_EXISTS { return; }
        h
    };
    crate::sys::lower_self();
    let Ok(exe) = std::env::current_exe() else { return };
    log("supervisor iniciado");

    let mut first = true;
    let mut crashes: Vec<Instant> = Vec::new();
    loop {
        let mut cmd = std::process::Command::new(&exe);
        cmd.env("UG_SUPERVISED", "1").creation_flags(CREATE_NO_WINDOW);
        if first { cmd.args(&first_args); }
        first = false;
        let started = Instant::now();
        match cmd.spawn().and_then(|mut c| c.wait()) {
            Ok(st) if st.success() => {
                log("Guardião encerrado normalmente; supervisor também sai");
                return;
            }
            Ok(st) => {
                let code = st.code().unwrap_or(-1);
                log(&format!("Guardião caiu (código {code}{}) depois de {} s; reiniciando", if code == EXIT_HUNG { ", travado" } else { "" }, started.elapsed().as_secs()));
            }
            Err(e) => log(&format!("não consegui iniciar o Guardião: {e}")),
        }
        crashes.retain(|t| t.elapsed() < Duration::from_secs(300));
        crashes.push(Instant::now());
        // Caindo em loop? Espera mais para não gastar CPU.
        let wait = if crashes.len() >= 5 { 60 } else { 3 };
        std::thread::sleep(Duration::from_secs(wait));
    }
}

/// O laço principal chama isto a cada volta.
pub fn beat() { BEAT.store(now(), Ordering::Relaxed); }

/// Vigia o batimento. Se parar, derruba o processo com código ≠ 0 para o supervisor subir outro.
pub fn spawn_watchdog() {
    beat();
    std::thread::Builder::new()
        .name("watchdog".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(20));
            let age = now() - BEAT.load(Ordering::Relaxed);
            if age > HUNG_AFTER_SECS {
                log(&format!("watchdog: laço principal parado há {age} s; reiniciando"));
                // Só derruba se houver supervisor para subir outro.
                if std::env::var_os("UG_SUPERVISED").is_some() || running() { std::process::exit(EXIT_HUNG); }
                beat();
            }
        })
        .ok();
}
