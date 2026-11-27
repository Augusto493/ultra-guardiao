//! Leituras e ações do Windows: memória, CPU, processos. Tudo via Win32, sem dependências pesadas.

use std::collections::{HashMap, HashSet};
use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::{BOOL, CloseHandle, FILETIME, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32EmptyWorkingSet, K32GetPerformanceInfo, K32GetProcessMemoryInfo, PERFORMANCE_INFORMATION,
    PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetProcessTimes, GetSystemTimes, NORMAL_PRIORITY_CLASS, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    SetPriorityClass, TerminateProcess,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible};

pub const GB: f64 = 1024.0 * 1024.0 * 1024.0;
pub const MB: f64 = 1024.0 * 1024.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Memory {
    pub total: u64,
    pub avail: u64,
    pub commit_used: u64,
    pub commit_limit: u64,
}
impl Memory {
    pub fn used_pct(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (1.0 - self.avail as f64 / self.total as f64) as f32 * 100.0 }
    }
    /// Memória pedida pelos programas em relação à RAM física (passa de 100% quando já está usando o disco).
    pub fn commit_vs_ram_pct(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.commit_used as f64 / self.total as f64) as f32 * 100.0 }
    }
}

pub fn memory() -> Memory {
    // SAFETY: estruturas zeradas com o tamanho preenchido, como a API exige.
    unsafe {
        let mut ms: MEMORYSTATUSEX = zeroed();
        ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        GlobalMemoryStatusEx(&mut ms);
        let mut pi: PERFORMANCE_INFORMATION = zeroed();
        pi.cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
        K32GetPerformanceInfo(&mut pi, pi.cb);
        let page = pi.PageSize as u64;
        Memory {
            total: ms.ullTotalPhys,
            avail: ms.ullAvailPhys,
            commit_used: pi.CommitTotal as u64 * page,
            commit_limit: pi.CommitLimit as u64 * page,
        }
    }
}

fn ft(f: FILETIME) -> u64 { ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64 }

/// Mede a CPU total entre duas chamadas.
#[derive(Default)]
pub struct CpuMeter { last: Option<(u64, u64, u64)> }
impl CpuMeter {
    pub fn sample(&mut self) -> f32 {
        let (mut idle, mut kernel, mut user): (FILETIME, FILETIME, FILETIME) = unsafe { (zeroed(), zeroed(), zeroed()) };
        // SAFETY: três FILETIME válidos.
        unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        let now = (ft(idle), ft(kernel), ft(user));
        let pct = match self.last {
            Some((i, k, u)) => {
                let di = now.0.saturating_sub(i) as f64;
                let total = (now.1.saturating_sub(k) + now.2.saturating_sub(u)) as f64;
                if total <= 0.0 { 0.0 } else { ((1.0 - di / total) * 100.0).clamp(0.0, 100.0) as f32 }
            }
            None => 0.0,
        };
        self.last = Some(now);
        pct
    }
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// RAM física em uso agora.
    pub ws: u64,
    /// Memória privada pedida (inclui o que foi pro disco).
    pub private: u64,
    /// Tempo de CPU acumulado (100 ns).
    pub cpu_time: u64,
    /// Quando o processo começou (FILETIME).
    pub created: u64,
}

pub fn processes() -> Vec<Proc> {
    let mut out = Vec::with_capacity(400);
    // SAFETY: snapshot fechado no fim; entry com dwSize preenchido.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE { return out; }
        let mut e: PROCESSENTRY32W = zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            let mut p = Proc { pid: e.th32ProcessID, ppid: e.th32ParentProcessID, name, ws: 0, private: 0, cpu_time: 0, created: 0 };
            if p.pid > 4 {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, p.pid);
                if !h.is_null() {
                    let mut mc: PROCESS_MEMORY_COUNTERS_EX = zeroed();
                    mc.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
                    if K32GetProcessMemoryInfo(h, &mut mc as *mut _ as *mut PROCESS_MEMORY_COUNTERS, mc.cb) != 0 {
                        p.ws = mc.WorkingSetSize as u64;
                        p.private = mc.PrivateUsage as u64;
                    }
                    let (mut c, mut x, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = (zeroed(), zeroed(), zeroed(), zeroed());
                    if GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) != 0 {
                        p.cpu_time = ft(k) + ft(u);
                        p.created = ft(c);
                    }
                    CloseHandle(h);
                }
            }
            out.push(p);
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// CPU por processo entre duas amostras, em % da máquina inteira.
#[derive(Default)]
pub struct ProcCpu { last: HashMap<(u32, u64), u64>, last_at: Option<std::time::Instant> }
impl ProcCpu {
    pub fn sample(&mut self, procs: &[Proc]) -> HashMap<u32, f32> {
        let now = std::time::Instant::now();
        let elapsed = self.last_at.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        let mut out = HashMap::with_capacity(procs.len());
        let mut next = HashMap::with_capacity(procs.len());
        for p in procs {
            let key = (p.pid, p.created);
            if let (Some(prev), true) = (self.last.get(&key), elapsed > 0.0) {
                let d = p.cpu_time.saturating_sub(*prev) as f64 / 1e7;
                out.insert(p.pid, ((d / (elapsed * ncpu)) * 100.0).clamp(0.0, 100.0) as f32);
            }
            next.insert(key, p.cpu_time);
        }
        self.last = next;
        self.last_at = Some(now);
        out
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: handle aberto por OpenProcess.
        unsafe { CloseHandle(self.0) };
    }
}
fn open(pid: u32, access: u32) -> Option<Handle> {
    // SAFETY: chamada simples; null = falha.
    let h = unsafe { OpenProcess(access, 0, pid) };
    if h.is_null() { None } else { Some(Handle(h)) }
}

/// Devolve à RAM as páginas paradas do processo (elas vão pro disco se precisar).
pub fn trim(pid: u32) -> bool {
    open(pid, PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION)
        // SAFETY: handle válido com SET_QUOTA.
        .map(|h| unsafe { K32EmptyWorkingSet(h.0) } != 0)
        .unwrap_or(false)
}

pub fn kill(pid: u32) -> bool {
    // SAFETY: handle válido com TERMINATE.
    open(pid, PROCESS_TERMINATE).map(|h| unsafe { TerminateProcess(h.0, 1) } != 0).unwrap_or(false)
}

pub fn set_low_priority(pid: u32, low: bool) -> bool {
    let class = if low { BELOW_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS };
    // SAFETY: handle válido com SET_INFORMATION.
    open(pid, PROCESS_SET_INFORMATION).map(|h| unsafe { SetPriorityClass(h.0, class) } != 0).unwrap_or(false)
}

/// O processo da janela em primeiro plano (o que você está usando).
pub fn foreground_pid() -> u32 {
    // SAFETY: GetForegroundWindow pode devolver null; GetWindowThreadProcessId aceita.
    unsafe {
        let w = GetForegroundWindow();
        if w.is_null() { return 0; }
        let mut pid = 0u32;
        GetWindowThreadProcessId(w, &mut pid);
        pid
    }
}

/// Processos donos de alguma janela visível (minimizada conta). `None` se o Windows não deixou listar.
pub fn visible_window_pids() -> Option<HashSet<u32>> {
    unsafe extern "system" fn collect(w: HWND, out: LPARAM) -> BOOL {
        // SAFETY: `out` é o HashSet passado abaixo, vivo durante todo o EnumWindows.
        unsafe {
            if IsWindowVisible(w) != 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(w, &mut pid);
                (*(out as *mut HashSet<u32>)).insert(pid);
            }
        }
        1
    }
    let mut set = HashSet::<u32>::new();
    // SAFETY: callback com a assinatura exigida; o ponteiro só é usado durante a chamada.
    let ok = unsafe { EnumWindows(Some(collect), &mut set as *mut HashSet<u32> as LPARAM) } != 0;
    ok.then_some(set)
}

pub fn uptime_secs() -> u64 {
    // SAFETY: sem argumentos.
    unsafe { GetTickCount64() / 1000 }
}

/// Espaço livre e total do C:.
pub fn disk_c() -> (u64, u64) {
    let path: Vec<u16> = "C:\\".encode_utf16().chain(Some(0)).collect();
    let (mut free, mut total, mut _tfree) = (0u64, 0u64, 0u64);
    // SAFETY: caminho terminado em zero e saídas válidas.
    unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, &mut total, &mut _tfree) };
    (free, total)
}

/// Roda um comando escondido com prazo: se passar do tempo, mata e devolve None.
/// Nenhuma chamada externa pode pendurar o Guardião.
pub fn run_timeout(program: &str, args: &[&str], secs: u64) -> Option<(bool, String)> {
    run_timeout_raw(program, args, secs).map(|(ok, b)| (ok, String::from_utf8_lossy(&b).into_owned()))
}

/// Igual a `run_timeout`, mas devolve os bytes (para saídas em UTF-16).
pub fn run_timeout_raw(program: &str, args: &[&str], secs: u64) -> Option<(bool, Vec<u8>)> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let mut child = std::process::Command::new(program)
        .args(args)
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let status = status?;
    let buf = reader.join().unwrap_or_default();
    Some((status.success(), buf))
}

/// Roda um comando sem abrir janela de console e devolve a saída (prazo de 30 s).
pub fn run_hidden(program: &str, args: &[&str]) -> Option<String> {
    run_timeout(program, args, 30).map(|(_, s)| s)
}

/// Roda e diz se terminou sem erro (código 0 e nada no fluxo de erro).
pub fn powershell_ok(script: &str) -> bool {
    let s = format!("$ErrorActionPreference='Stop'; try {{ {script} }} catch {{ exit 1 }}");
    run_timeout("powershell.exe", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &s], 60)
        .is_some_and(|(ok, _)| ok)
}

pub fn powershell(script: &str) -> Option<String> {
    run_timeout("powershell.exe", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script], 60).map(|(_, s)| s)
}

/// O próprio Guardião fica leve: prioridade baixa e modo de eficiência (EcoQoS).
pub fn lower_self() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetProcessInformation,
    };
    // SAFETY: pseudo-handle do próprio processo e estrutura inicializada.
    unsafe {
        let me = GetCurrentProcess();
        SetPriorityClass(me, BELOW_NORMAL_PRIORITY_CLASS);
        let st = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        };
        SetProcessInformation(me, ProcessPowerThrottling, &st as *const _ as *const _, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32);
    }
}

/// Roda um script PowerShell como administrador (um pedido do UAC). Espera terminar.
pub fn powershell_admin(script: &str) -> bool {
    let encoded = encode_ps(script);
    let outer = format!(
        "$p = Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -Wait -PassThru -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-EncodedCommand','{encoded}'; exit $p.ExitCode"
    );
    run_timeout("powershell.exe", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &outer], 180)
        .is_some_and(|(ok, _)| ok)
}

/// Base64 de UTF-16LE, o formato do -EncodedCommand.
fn encode_ps(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_utf16() {
        // "a" em UTF-16LE = 61 00 -> "YQA="
        assert_eq!(super::encode_ps("a"), "YQA=");
    }
}

/// Quantos handles (arquivos, chaves, eventos…) o processo tem abertos; 0 se não der para ler.
pub fn handle_count(pid: u32) -> u32 {
    use windows_sys::Win32::System::Threading::GetProcessHandleCount;
    let Some(h) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return 0 };
    let mut n = 0u32;
    // SAFETY: handle válido com QUERY_LIMITED e saída u32 válida.
    if unsafe { GetProcessHandleCount(h.0, &mut n) } != 0 { n } else { 0 }
}
