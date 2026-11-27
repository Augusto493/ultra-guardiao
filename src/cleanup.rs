//! Faxina: ajustes únicos e reversíveis (cada um tem "Desfazer").

use crate::sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State { Aplicado, Pendente, NaoExiste, Verificando }

pub struct Item {
    pub id: &'static str,
    pub title: &'static str,
    pub detail: &'static str,
    pub admin: bool,
    /// Script que imprime "1" (aplicado), "0" (pendente) ou "-" (não existe neste PC).
    check: &'static str,
    apply: &'static str,
    undo: &'static str,
}

const RUN: &str = r"$r='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'; $a='HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'; if(!(Test-Path $a)){New-Item $a | Out-Null}; $names=(Get-Item $r).Property | Where-Object { $_ -like $pat };";

pub const ITEMS: &[Item] = &[
    Item {
        id: "edge",
        title: "Edge abrindo escondido com o Windows",
        detail: "O Edge carrega em segundo plano ao ligar, mesmo sem você usar. Desliga só a inicialização.",
        admin: false,
        check: "$pat='MicrosoftEdgeAutoLaunch_*'; RUN if(!$names){'-'} elseif(($names | Where-Object { $v=(Get-ItemProperty $a -Name $_ -EA 0).$_; -not $v -or $v[0] -ne 3 })){'0'} else {'1'}",
        apply: "$pat='MicrosoftEdgeAutoLaunch_*'; RUN $names | ForEach-Object { Set-ItemProperty $a -Name $_ -Value ([byte[]](3,0,0,0,0,0,0,0,0,0,0,0)) -Type Binary }",
        undo: "$pat='MicrosoftEdgeAutoLaunch_*'; RUN $names | ForEach-Object { Set-ItemProperty $a -Name $_ -Value ([byte[]](2,0,0,0,0,0,0,0,0,0,0,0)) -Type Binary }",
    },
    Item {
        id: "paperclip",
        title: "Paperclip iniciando com o Windows",
        detail: "Abre oculto a cada boot. Desligue se não usa todo dia (dá pra abrir manualmente).",
        admin: false,
        check: "$pat='br.paperclip*'; RUN if(!$names){'-'} elseif(($names | Where-Object { $v=(Get-ItemProperty $a -Name $_ -EA 0).$_; -not $v -or $v[0] -ne 3 })){'0'} else {'1'}",
        apply: "$pat='br.paperclip*'; RUN $names | ForEach-Object { Set-ItemProperty $a -Name $_ -Value ([byte[]](3,0,0,0,0,0,0,0,0,0,0,0)) -Type Binary }",
        undo: "$pat='br.paperclip*'; RUN $names | ForEach-Object { Set-ItemProperty $a -Name $_ -Value ([byte[]](2,0,0,0,0,0,0,0,0,0,0,0)) -Type Binary }",
    },
    Item {
        id: "intel-telemetria",
        title: "Telemetria da Intel (4 serviços)",
        detail: "Coletam dados de uso e o QUEENCREEK vive caindo. Não fazem falta. Pede permissão de administrador.",
        admin: true,
        check: "$s=Get-Service IntelTelemetryAgent,IntelCollectorService,SystemUsageReportSvc_QUEENCREEK,ESRV_SVC_QUEENCREEK -EA 0; if(!$s){'-'} elseif($s | Where-Object StartType -ne 'Disabled'){'0'} else {'1'}",
        apply: "$s=Get-Service IntelTelemetryAgent,IntelCollectorService,SystemUsageReportSvc_QUEENCREEK,ESRV_SVC_QUEENCREEK -EA 0; $s | Stop-Service -Force -EA 0; $s | Set-Service -StartupType Disabled",
        undo: "$s=Get-Service IntelTelemetryAgent,IntelCollectorService,SystemUsageReportSvc_QUEENCREEK,ESRV_SVC_QUEENCREEK -EA 0; $s | Set-Service -StartupType Automatic; $s | Start-Service -EA 0",
    },
    Item {
        id: "intel-dsa",
        title: "Assistente de drivers da Intel sempre ligado",
        detail: "Passa a abrir só quando você pedir (manual). Continua funcionando. Pede administrador.",
        admin: true,
        check: "$s=Get-Service DSAService,DSAUpdateService -EA 0; if(!$s){'-'} elseif($s | Where-Object StartType -eq 'Automatic'){'0'} else {'1'}",
        apply: "$s=Get-Service DSAService,DSAUpdateService -EA 0; $s | Stop-Service -Force -EA 0; $s | Set-Service -StartupType Manual",
        undo: "$s=Get-Service DSAService,DSAUpdateService -EA 0; $s | Set-Service -StartupType Automatic; $s | Start-Service -EA 0",
    },
    Item {
        id: "hibernar",
        title: "Hibernar em vez de suspender",
        detail: "O PC travou ao acordar da suspensão. Hibernar desliga de verdade e volta de onde parou. Pede administrador.",
        admin: true,
        // LIDACTION é oculto: `/q` não lista, só `/qh`.
        check: "$o=powercfg /qh SCHEME_CURRENT SUB_BUTTONS LIDACTION | Select-String '0x0000000' ; $v=$o | ForEach-Object { ($_ -split ':')[-1].Trim() }; if($v.Count -ge 2 -and $v[-1] -eq '0x00000002' -and $v[-2] -eq '0x00000002'){'1'} else {'0'}",
        apply: "powercfg /hibernate on; powercfg /change standby-timeout-ac 0; powercfg /change standby-timeout-dc 0; powercfg /change hibernate-timeout-ac 60; powercfg /change hibernate-timeout-dc 20; powercfg /setacvalueindex SCHEME_CURRENT SUB_BUTTONS LIDACTION 2; powercfg /setdcvalueindex SCHEME_CURRENT SUB_BUTTONS LIDACTION 2; powercfg /setacvalueindex SCHEME_CURRENT SUB_BUTTONS SBUTTONACTION 2; powercfg /setdcvalueindex SCHEME_CURRENT SUB_BUTTONS SBUTTONACTION 2; powercfg /setactive SCHEME_CURRENT",
        undo: "powercfg /change standby-timeout-ac 30; powercfg /change standby-timeout-dc 10; powercfg /change hibernate-timeout-ac 0; powercfg /change hibernate-timeout-dc 0; powercfg /setacvalueindex SCHEME_CURRENT SUB_BUTTONS LIDACTION 1; powercfg /setdcvalueindex SCHEME_CURRENT SUB_BUTTONS LIDACTION 1; powercfg /setacvalueindex SCHEME_CURRENT SUB_BUTTONS SBUTTONACTION 1; powercfg /setdcvalueindex SCHEME_CURRENT SUB_BUTTONS SBUTTONACTION 1; powercfg /setactive SCHEME_CURRENT",
    },
];

fn expand(script: &str) -> String { script.replace("RUN ", RUN) }

pub fn check(item: &Item) -> State {
    match sys::powershell(&expand(item.check)).map(|s| s.trim().to_string()).as_deref() {
        Some(s) if s.ends_with('1') => State::Aplicado,
        Some(s) if s.ends_with('-') => State::NaoExiste,
        _ => State::Pendente,
    }
}

pub fn run(item: &Item, undo: bool) -> bool {
    let script = expand(if undo { item.undo } else { item.apply });
    if item.admin { sys::powershell_admin(&script) } else { sys::powershell_ok(&script) }
}

/// Todos os pendentes de uma vez, com um único pedido de administrador.
pub fn apply_all(ids: &[&str]) -> bool {
    let items: Vec<&Item> = ITEMS.iter().filter(|i| ids.contains(&i.id)).collect();
    let mut ok = true;
    for i in items.iter().filter(|i| !i.admin) { ok &= run(i, false); }
    let admin: Vec<String> = items.iter().filter(|i| i.admin).map(|i| expand(i.apply)).collect();
    if !admin.is_empty() { ok &= sys::powershell_admin(&admin.join("; ")); }
    ok
}

// ---- Iniciar com o Windows (o próprio Guardião) ----

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

const TASK: &str = "UltraGuardiao";

/// Valor que vai na inicialização: o supervisor, que mantém o Guardião vivo.
fn run_value() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --supervisor", exe.display()))
}

pub fn autostart_enabled() -> bool {
    sys::run_hidden("reg.exe", &["query", RUN_KEY, "/v", "UltraGuardiao"]).is_some_and(|s| s.contains("UltraGuardiao"))
}

/// Já aponta para este executável, com `--supervisor`?
fn autostart_current() -> bool {
    let Some(want) = run_value() else { return false };
    sys::run_hidden("reg.exe", &["query", RUN_KEY, "/v", "UltraGuardiao"])
        .is_some_and(|s| s.to_lowercase().contains(&want.to_lowercase()))
}

pub fn set_autostart(on: bool) -> bool {
    if on {
        let Some(val) = run_value() else { return false };
        let ok = sys::run_hidden("reg.exe", &["add", RUN_KEY, "/v", "UltraGuardiao", "/t", "REG_SZ", "/d", &val, "/f"]).is_some();
        register_task();
        ok
    } else {
        unregister_task();
        sys::run_hidden("reg.exe", &["delete", RUN_KEY, "/v", "UltraGuardiao", "/f"]).is_some()
    }
}

/// Tarefa agendada "ao entrar no Windows": começa na hora (sem o atraso da pasta Inicializar),
/// roda mesmo na bateria e sem limite de tempo. Sem administrador: se o Windows negar, a chave
/// de inicialização acima continua garantindo a abertura.
fn register_task() -> bool {
    let Some(val) = run_value() else { return false };
    let exe = val.trim_end_matches(" --supervisor").trim_matches('"').replace('\'', "''");
    sys::powershell_ok(&format!(
        "$a=New-ScheduledTaskAction -Execute '{exe}' -Argument '--supervisor'; \
         $t=New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME; \
         $s=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew; \
         Register-ScheduledTask -TaskName '{TASK}' -Action $a -Trigger $t -Settings $s -Force | Out-Null"
    ))
}

fn unregister_task() -> bool {
    sys::powershell_ok(&format!("Unregister-ScheduledTask -TaskName '{TASK}' -Confirm:$false -ErrorAction SilentlyContinue"))
}

/// Chamado ao iniciar: garante que a abertura com o Windows continua apontando para cá.
/// Não mexe quando rodando de uma pasta de desenvolvimento (`\debug\`).
pub fn ensure_autostart() {
    let Some(val) = run_value() else { return };
    if val.contains("\\debug\\") { return; }
    if !autostart_current() { set_autostart(true); }
    else if sys::powershell(&format!("if(Get-ScheduledTask -TaskName '{TASK}' -ErrorAction SilentlyContinue){{'1'}}")).is_none_or(|s| !s.contains('1')) {
        register_task();
    }
}
