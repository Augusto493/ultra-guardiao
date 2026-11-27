//! Caixa-preta: uma linha JSON por minuto (e por ação) em caixa-preta\AAAA-MM-DD.jsonl.
//! Se o PC travar, depois de religar dá pra ver o que estava acontecendo antes.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::clock;
use crate::guard::Snapshot;
use crate::sys::GB;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Line {
    pub t: i64,
    pub ram: f32,
    pub commit_gb: f32,
    pub cpu: f32,
    pub nivel: String,
    pub top: Vec<(String, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evento: Option<String>,
}

pub struct BlackBox { dir: PathBuf, day: String, file: Option<File>, offset: i64 }

impl BlackBox {
    pub fn new(keep_days: u32) -> Self {
        let dir = crate::config::dir().join("caixa-preta");
        let _ = std::fs::create_dir_all(&dir);
        let offset = clock::local_offset();
        // apaga dias antigos
        let cutoff = clock::ymd(clock::now_unix() - keep_days as i64 * 86400, offset);
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".jsonl") && name.as_str() < format!("{cutoff}.jsonl").as_str() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        Self { dir, day: String::new(), file: None, offset }
    }

    pub fn write(&mut self, snap: &Snapshot, evento: Option<&str>) {
        let t = clock::now_unix();
        let day = clock::ymd(t, self.offset);
        if day != self.day || self.file.is_none() {
            self.file = OpenOptions::new().create(true).append(true).open(self.dir.join(format!("{day}.jsonl"))).ok();
            self.day = day;
        }
        let line = Line {
            t,
            ram: (snap.mem.used_pct() * 10.0).round() / 10.0,
            commit_gb: ((snap.mem.commit_used as f64 / GB) * 10.0).round() as f32 / 10.0,
            cpu: snap.cpu.round(),
            nivel: snap.level.label().into(),
            top: snap.top.iter().take(5).map(|p| (p.name.clone(), p.mb.round() as u32)).collect(),
            evento: evento.map(String::from),
        };
        if let (Some(f), Ok(s)) = (self.file.as_mut(), serde_json::to_string(&line)) {
            let _ = writeln!(f, "{s}");
        }
    }
}

/// A última linha gravada antes de `before` (Unix), procurando nos arquivos mais recentes.
pub fn last_before(before: i64) -> Option<Line> {
    let dir = crate::config::dir().join("caixa-preta");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
    files.sort();
    for f in files.iter().rev() {
        let mut best: Option<Line> = None;
        for l in BufReader::new(File::open(f).ok()?).lines().map_while(Result::ok) {
            if let Ok(line) = serde_json::from_str::<Line>(&l) && line.t < before && line.evento.is_none() {
                best = Some(line);
            }
        }
        if best.is_some() { return best; }
    }
    None
}

pub fn folder() -> PathBuf { crate::config::dir().join("caixa-preta") }
