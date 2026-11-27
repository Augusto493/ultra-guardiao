//! Saúde do sistema: lê os registros do Windows e aponta o que está errado.

use std::collections::HashMap;

use crate::blackbox;
use crate::clock;
use crate::sys::{self, GB};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status { Ok, Aviso, Ruim }

#[derive(Clone, Debug)]
pub struct Check { pub status: Status, pub title: String, pub detail: String }

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub checks: Vec<Check>,
    /// Antes do último travamento total: hora e o que a caixa-preta viu.
    pub last_crash: Option<(i64, Option<blackbox::Line>)>,
    pub at: i64,
}

struct Ev { t: i64, data: Vec<(String, String)> }

/// Saída do wevtutil em UTF-16 (`/uni:true`): sem isso ele escreve na página ANSI
/// e nomes com acento viram lixo ao ler como UTF-8.
fn wevtutil(args: &[&str]) -> Option<String> {
    let mut full: Vec<&str> = args.to_vec();
    full.push("/uni:true");
    let (_, b) = crate::sys::run_timeout_raw("wevtutil.exe", &full, 40)?;
    if b.len() >= 2 && b[0] == 0xFF && b[1] == 0xFE {
        let w: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(String::from_utf16_lossy(&w))
    } else {
        Some(String::from_utf8_lossy(&b).into_owned())
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Eventos de um log com estes IDs nos últimos `days` dias.
fn query(log: &str, ids: &[u32], days: u32, extra: &str) -> Vec<Ev> {
    let idq = ids.iter().map(|i| format!("EventID={i}")).collect::<Vec<_>>().join(" or ");
    let q = format!("*[System[({idq}){extra} and TimeCreated[timediff(@SystemTime) <= {}]]]", days as u64 * 86_400_000);
    let Some(xml) = wevtutil(&["qe", log, &format!("/q:{q}"), "/f:xml", "/c:300", "/rd:true"]) else { return vec![] };
    xml.split("<Event ").skip(1).map(|e| {
        let t = e.split("SystemTime='").nth(1).and_then(|s| clock::parse_iso_utc(s)).unwrap_or(0);
        let mut data = Vec::new();
        for d in e.split("<Data").skip(1) {
            let Some((tag, rest)) = d.split_once('>') else { continue };
            let name = tag.split("Name='").nth(1).and_then(|s| s.split('\'').next()).unwrap_or("").to_string();
            // <Data Name='x'/> é vazio; não engolir o próximo campo.
            let val = if tag.ends_with('/') { "" } else { rest.split("</Data>").next().unwrap_or("") };
            data.push((name, unescape(val)));
        }
        Ev { t, data }
    }).collect()
}

fn top_names(evs: &[Ev], field: impl Fn(&Ev) -> Option<String>) -> String {
    let mut m: HashMap<String, usize> = HashMap::new();
    for e in evs { if let Some(n) = field(e) { *m.entry(n).or_default() += 1; } }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.into_iter().take(3).map(|(n, c)| format!("{n} ({c}x)")).collect::<Vec<_>>().join(", ")
}

pub fn run() -> Report {
    let off = clock::local_offset();
    let mut checks = Vec::new();

    // 1. Travamento total / desligamento forçado.
    let kp = query("System", &[41], 30, " and Provider[@Name='Microsoft-Windows-Kernel-Power']");
    let last_crash = kp.first().map(|e| (e.t, blackbox::last_before(e.t - 20)));
    checks.push(match kp.len() {
        0 => Check { status: Status::Ok, title: "Sem travamentos totais".into(), detail: "Nenhum desligamento forçado em 30 dias.".into() },
        n => Check {
            status: if n > 2 { Status::Ruim } else { Status::Aviso },
            title: format!("{n} travamento(s) total(is) em 30 dias"),
            detail: format!("Último: {}. Se foi ao acordar da suspensão, aplique \"Hibernar em vez de suspender\" na Faxina.", clock::dd_mm_hhmm(kp[0].t, off)),
        },
    });

    // 2. Programas que pararam de responder.
    let hangs = query("Application", &[1002], 7, " and Provider[@Name='Application Hang']");
    checks.push(if hangs.is_empty() {
        Check { status: Status::Ok, title: "Nenhum programa congelou".into(), detail: "Últimos 7 dias.".into() }
    } else {
        Check { status: if hangs.len() > 5 { Status::Ruim } else { Status::Aviso }, title: format!("{} congelamento(s) de programa em 7 dias", hangs.len()), detail: top_names(&hangs, |e| e.data.first().map(|d| d.1.clone())) }
    });

    // 3. Programas que fecharam sozinhos.
    let crashes = query("Application", &[1000], 7, " and Provider[@Name='Application Error']");
    checks.push(if crashes.is_empty() {
        Check { status: Status::Ok, title: "Nenhum programa fechou sozinho".into(), detail: "Últimos 7 dias.".into() }
    } else {
        Check { status: if crashes.len() > 5 { Status::Ruim } else { Status::Aviso }, title: format!("{} fechamento(s) inesperado(s) em 7 dias", crashes.len()), detail: top_names(&crashes, |e| e.data.first().map(|d| d.1.clone())) }
    });

    // 4. Atualizações que falham (drivers).
    // 0x80073d02 = app da Loja aberta durante a atualização; o Windows tenta de novo sozinho.
    let wu: Vec<Ev> = query("System", &[20], 7, " and Provider[@Name='Microsoft-Windows-WindowsUpdateClient']")
        .into_iter().filter(|e| !e.data.iter().any(|d| d.0 == "errorCode" && d.1.eq_ignore_ascii_case("0x80073d02"))).collect();
    checks.push(if wu.is_empty() {
        Check { status: Status::Ok, title: "Atualizações em dia".into(), detail: "Nenhuma falha de instalação em 7 dias.".into() }
    } else {
        let names = top_names(&wu, |e| e.data.iter().find(|d| d.0 == "updateTitle").map(|d| d.1.split(" - ").take(2).collect::<Vec<_>>().join(" - ")));
        Check { status: Status::Ruim, title: format!("{} atualização(ões) falharam em 7 dias", wu.len()), detail: format!("{names}. Baixe os drivers no site da Acer/Intel.") }
    });

    // 5. Tempo ligado.
    let up = sys::uptime_secs();
    checks.push(Check {
        status: if up > 3 * 86400 { Status::Aviso } else { Status::Ok },
        title: format!("Ligado há {}", clock::duration(up)),
        detail: if up > 3 * 86400 { "Reinicie: limpa vazamentos de memória acumulados.".into() } else { "Tudo certo.".into() },
    });

    // 6. Memória pedida vs física.
    let m = sys::memory();
    let ratio = m.commit_vs_ram_pct();
    checks.push(Check {
        status: if ratio > 100.0 { Status::Ruim } else if ratio > 85.0 { Status::Aviso } else { Status::Ok },
        title: format!("Programas pedem {:.1} GB de {:.1} GB de RAM", m.commit_used as f64 / GB, m.total as f64 / GB),
        detail: if ratio > 100.0 { "Passou da RAM física: o Windows está usando o disco como memória — é isso que trava.".into() } else { "Dentro da RAM física.".into() },
    });

    // 7. Disco.
    let (free, total) = sys::disk_c();
    let pct = if total > 0 { free as f64 / total as f64 * 100.0 } else { 100.0 };
    checks.push(Check {
        status: if pct < 10.0 { Status::Ruim } else if pct < 20.0 { Status::Aviso } else { Status::Ok },
        title: format!("C: com {:.0} GB livres ({pct:.0}%)", free as f64 / GB),
        detail: if pct < 15.0 { "Pouco espaço: libere arquivos.".into() } else { "Espaço suficiente.".into() },
    });

    // 8. Drivers de vídeo antigos.
    if let Some(out) = sys::powershell("Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name + '|' + $_.DriverDate.ToString('yyyy-MM-dd') }") {
        let today = clock::ymd(clock::now_unix(), off);
        let year: i64 = today[..4].parse().unwrap_or(2026);
        for l in out.lines().filter(|l| l.contains('|')) {
            let (name, date) = l.split_once('|').unwrap_or((l, ""));
            let y: i64 = date.get(..4).and_then(|s| s.parse().ok()).unwrap_or(year);
            let old = year - y >= 2;
            checks.push(Check {
                status: if old { Status::Aviso } else { Status::Ok },
                title: format!("Vídeo: {}", name.trim()),
                detail: if old { format!("Driver de {date} — antigo. Atualize pelo site do fabricante.") } else { format!("Driver de {date}.") },
            });
        }
    }

    Report { checks, last_crash, at: clock::now_unix() }
}
