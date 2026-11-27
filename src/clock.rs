//! Hora local sem dependências: Unix + fuso lido do Windows.

use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Dias desde 1970-01-01 para uma data civil (algoritmo de Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Diferença entre a hora local e UTC, em segundos.
pub fn local_offset() -> i64 {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: SYSTEMTIME válido.
    unsafe { GetLocalTime(&mut st) };
    let local = days_from_civil(st.wYear as i64, st.wMonth as i64, st.wDay as i64) * 86400
        + st.wHour as i64 * 3600
        + st.wMinute as i64 * 60
        + st.wSecond as i64;
    let off = local - now_unix();
    // arredonda para 15 min
    (off as f64 / 900.0).round() as i64 * 900
}

/// "2026-11-09T22:49:49.1234Z" -> Unix.
pub fn parse_iso_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let h: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let se: i64 = s.get(17..19)?.parse().ok()?;
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se)
}

pub struct Local { pub y: i64, pub mo: i64, pub d: i64, pub h: i64, pub mi: i64 }
pub fn local(unix: i64, offset: i64) -> Local {
    let t = unix + offset;
    let (y, mo, d) = civil_from_days(t.div_euclid(86400));
    let r = t.rem_euclid(86400);
    Local { y, mo, d, h: r / 3600, mi: (r % 3600) / 60 }
}

pub fn hhmm(unix: i64, offset: i64) -> String {
    let l = local(unix, offset);
    format!("{:02}:{:02}", l.h, l.mi)
}
pub fn dd_mm_hhmm(unix: i64, offset: i64) -> String {
    let l = local(unix, offset);
    format!("{:02}/{:02} {:02}:{:02}", l.d, l.mo, l.h, l.mi)
}
pub fn ymd(unix: i64, offset: i64) -> String {
    let l = local(unix, offset);
    format!("{:04}-{:02}-{:02}", l.y, l.mo, l.d)
}

pub fn duration(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 { format!("{d}d {h}h") } else if h > 0 { format!("{h}h {m}min") } else { format!("{m}min") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn iso_round_trip() {
        let t = parse_iso_utc("2026-11-09T22:49:49.123Z").unwrap();
        assert_eq!(hhmm(t, -3 * 3600), "19:49");
        assert_eq!(ymd(t, 0), "2026-11-09");
    }
}
