//! Analista com IA: monta um relatório enxuto do que aconteceu e pede ao Codex CLI
//! um diagnóstico estruturado (em português) com ações que o Guardião sabe aplicar.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::blackbox::{self, Line};
use crate::clock;
use crate::config::Config;
use crate::guard::{Kind, Snapshot};
use crate::health::Report;
use crate::sys::GB;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
const TIMEOUT: Duration = Duration::from_secs(6 * 60);
const KEEP_HISTORY: usize = 30;

pub struct Context {
    pub snap: Snapshot,
    pub health: Option<Report>,
    pub log: Vec<(i64, Kind, String)>,
    pub cfg: Config,
    pub motivo: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Acao {
    pub titulo: String,
    pub explicacao: String,
    pub tipo: String,
    pub alvo: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Analise {
    pub resumo: String,
    pub causa: String,
    pub gravidade: String,
    pub acoes: Vec<Acao>,
    #[serde(default)]
    pub quando: i64,
    #[serde(default)]
    pub modelo: String,
    #[serde(default)]
    pub motivo: String,
    #[serde(default)]
    pub segundos: u64,
}

pub enum Progresso {
    Etapa(String),
    Pronto(Analise),
    Erro(String),
}

const TIPOS: [&str; 7] = ["fechar_processo", "adicionar_orfao", "proteger", "ajustar_limite", "faxina", "reiniciar", "manual"];

fn dir() -> PathBuf {
    let d = crate::config::dir().join("analise");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn historico() -> PathBuf {
    let d = dir().join("historico");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// JSON Schema estrito da resposta (formato exigido pelos "structured outputs").
fn schema() -> String {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["resumo", "causa", "gravidade", "acoes"],
        "properties": {
            "resumo": { "type": "string", "description": "Uma ou duas frases simples sobre o que está acontecendo." },
            "causa": { "type": "string", "description": "A causa raiz mais provável dos travamentos/lentidão." },
            "gravidade": { "type": "string", "enum": ["baixa", "media", "alta"] },
            "acoes": {
                "type": "array",
                "maxItems": 5,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["titulo", "explicacao", "tipo", "alvo"],
                    "properties": {
                        "titulo": { "type": "string" },
                        "explicacao": { "type": "string" },
                        "tipo": { "type": "string", "enum": TIPOS },
                        "alvo": { "type": "string" }
                    }
                }
            }
        }
    })
    .to_string()
}

// ---------------- relatório ----------------

fn fmt_line(l: &Line, off: i64) -> String {
    let top = l.top.iter().map(|(n, mb)| format!("{n} {mb}MB")).collect::<Vec<_>>().join(", ");
    let mut s = format!("{} RAM {:.0}% · commit {:.1} GB · CPU {:.0}% · {} · {}", clock::dd_mm_hhmm(l.t, off), l.ram, l.commit_gb, l.cpu, l.nivel, top);
    if let Some(e) = &l.evento {
        s.push_str(" · EVENTO: ");
        s.push_str(e);
    }
    s
}

/// Linhas da caixa-preta dos dias que cobrem [from, to].
fn blackbox_lines(from: i64, to: i64, off: i64) -> Vec<Line> {
    let folder = blackbox::folder();
    let mut days: Vec<String> = Vec::new();
    let mut t = from - 86400;
    while t <= to + 86400 {
        let d = clock::ymd(t, off);
        if !days.contains(&d) { days.push(d); }
        t += 86400;
    }
    let mut out = Vec::new();
    for d in days {
        let Ok(f) = File::open(folder.join(format!("{d}.jsonl"))) else { continue };
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(line) = serde_json::from_str::<Line>(&l) && line.t >= from && line.t <= to {
                out.push(line);
            }
        }
    }
    out.sort_by_key(|l| l.t);
    out
}

fn relatorio(ctx: &Context, now: i64, off: i64) -> String {
    use std::fmt::Write as _;
    let mut r = String::with_capacity(16 * 1024);
    let s = &ctx.snap;
    let m = &s.mem;
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);

    let _ = writeln!(r, "# Relatório do Ultra Guardião — {}\n", clock::dd_mm_hhmm(now, off));
    let _ = writeln!(r, "## Motivo\n{}\n", if ctx.motivo.trim().is_empty() { "Pedido do usuário." } else { ctx.motivo.trim() });

    let _ = writeln!(r, "## Máquina");
    let _ = writeln!(r, "- RAM física: {:.1} GB · limite de commit (RAM + arquivo de paginação): {:.1} GB", m.total as f64 / GB, m.commit_limit as f64 / GB);
    let _ = writeln!(r, "- Processadores lógicos: {ncpu}\n");

    let _ = writeln!(r, "## Agora");
    let _ = writeln!(r, "- Nível: {} · RAM em uso: {:.0}% ({:.1} GB livres)", s.level.label(), m.used_pct(), m.avail as f64 / GB);
    let _ = writeln!(r, "- Memória pedida (commit): {:.1} GB = {:.0}% da RAM física", m.commit_used as f64 / GB, m.commit_vs_ram_pct());
    let _ = writeln!(r, "- CPU: {:.0}% · ligado há {} · {} processos · {} órfãos", s.cpu, clock::duration(s.uptime), s.nprocs, s.orphans);
    let _ = writeln!(r, "- Guardião já liberou {:.1} GB e fechou {} processos desde que abriu", s.freed_total as f64 / GB, s.closed_total);
    if !s.top.is_empty() {
        let _ = writeln!(r, "\nMaiores consumidores (agrupados por nome):\n");
        let _ = writeln!(r, "| processo | qtd | RAM MB | CPU % |\n|---|---|---|---|");
        for p in s.top.iter().take(15) {
            let _ = writeln!(r, "| {} | {} | {:.0} | {:.0} |", p.name, p.count, p.mb, p.cpu);
        }
    }
    r.push('\n');

    let mut crash_t = None;
    if let Some(h) = &ctx.health {
        let _ = writeln!(r, "## Saúde do Windows (verificada {})", clock::dd_mm_hhmm(h.at, off));
        for c in &h.checks {
            let _ = writeln!(r, "- [{:?}] {} — {}", c.status, c.title, c.detail);
        }
        if let Some((t, line)) = &h.last_crash {
            crash_t = Some(*t);
            let _ = writeln!(r, "\nÚltimo travamento total (Kernel-Power 41): {}", clock::dd_mm_hhmm(*t, off));
            match line {
                Some(l) => { let _ = writeln!(r, "Última leitura da caixa-preta antes dele: {}", fmt_line(l, off)); }
                None => { let _ = writeln!(r, "Sem leitura da caixa-preta antes dele."); }
            }
        }
        r.push('\n');
    }

    if !ctx.log.is_empty() {
        let mut log: Vec<&(i64, Kind, String)> = ctx.log.iter().collect();
        log.sort_by_key(|l| l.0);
        let start = log.len().saturating_sub(30);
        let _ = writeln!(r, "## Ações e alertas recentes do Guardião");
        for (t, k, txt) in &log[start..] {
            let _ = writeln!(r, "- {} [{:?}] {}", clock::hhmm(*t, off), k, txt);
        }
        r.push('\n');
    }

    let recent = blackbox_lines(now - 45 * 60, now, off);
    let start = recent.len().saturating_sub(45);
    let _ = writeln!(r, "## Caixa-preta — últimos 45 minutos (1 linha/min + eventos)");
    if recent.is_empty() { let _ = writeln!(r, "(sem registros)"); }
    for l in &recent[start..] { let _ = writeln!(r, "- {}", fmt_line(l, off)); }
    r.push('\n');

    if let Some(ct) = crash_t {
        let before = blackbox_lines(ct - 6 * 3600, ct, off);
        let start = before.len().saturating_sub(20);
        let _ = writeln!(r, "## Caixa-preta — antes do último travamento total");
        if before.is_empty() { let _ = writeln!(r, "(sem registros nas 6 horas anteriores)"); }
        for l in &before[start..] { let _ = writeln!(r, "- {}", fmt_line(l, off)); }
        r.push('\n');
    }

    let c = &ctx.cfg;
    let _ = writeln!(r, "## Configuração do Guardião");
    let _ = writeln!(r, "- Automático: {} · Atenção em {:.0}% de RAM · Crítico em {:.0}% · CPU alta: {:.0}%", if c.automatico { "sim" } else { "não" }, c.limite_atencao, c.limite_critico, c.limite_cpu);
    let _ = writeln!(r, "- Sugere reiniciar após {} dias ligado", c.dias_para_reiniciar);
    let _ = writeln!(r, "- IA aplica ações seguras sozinha: {} · Modo silencioso (sem notificações): {}", if c.ia_aplicar_auto { "sim" } else { "não" }, if c.silencioso { "sim" } else { "não" });
    let _ = writeln!(r, "- Protegidos (nunca mexer): {}", c.protegidos.join(", "));
    let _ = writeln!(r, "- Fechar quando órfãos e parados: {}", c.orfaos_fechaveis.join(", "));
    let _ = writeln!(r, "- Itens de faxina disponíveis: edge (Edge rodando em 2º plano), paperclip, intel-telemetria, intel-dsa, hibernar (hibernar em vez de suspender)");
    r
}

fn prompt(report: &str) -> String {
    format!(
r#"Você é um diagnosticador de desempenho do Windows ajudando um usuário leigo cujo PC trava ou fica lento.
O app "Ultra Guardião" monitora RAM/CPU, alivia memória, fecha processos órfãos e grava uma caixa-preta por minuto.
O relatório completo está no arquivo relatorio.md na pasta atual e também logo abaixo.

Regras:
- Você PODE rodar comandos PowerShell SOMENTE DE LEITURA para investigar mais (Get-Process, Get-WinEvent, Get-CimInstance, Get-Counter, powercfg /q, driverquery etc.). NUNCA altere nada: não finalize processos, não mude configurações, registro, serviços ou arquivos. Seja rápido: poucos comandos, só os que ajudam.
- Responda em português do Brasil, curto e em linguagem simples (sem jargão).
- Encontre a causa raiz mais provável dos travamentos/lentidão, não apenas os sintomas (ex.: "RAM cheia" é sintoma; "12 processos node.exe esquecidos por ferramentas de IA" é causa).
- Proponha no máximo 5 ações, da mais útil para a menos útil. Cada ação tem um "tipo":
  - "fechar_processo": alvo = nome do .exe (ex.: "node.exe")
  - "adicionar_orfao": alvo = nome do .exe para fechar automaticamente quando ficar órfão
  - "proteger": alvo = nome do .exe que o Guardião nunca deve tocar
  - "ajustar_limite": alvo = "atencao=NN", "critico=NN" ou "cpu=NN" (NN = porcentagem inteira)
  - "faxina": alvo = um de: edge, paperclip, intel-telemetria, intel-dsa, hibernar
  - "reiniciar": alvo = ""
  - "manual": alvo = "" — algo que o usuário precisa fazer à mão (ex.: atualizar um driver pelo site do fabricante); explique o passo a passo em "explicacao"
- Nunca sugira fechar processos do sistema do Windows nem os da lista de protegidos.
- As ações "proteger", "ajustar_limite", "adicionar_orfao" e "faxina" (só as que não pedem administrador: edge, paperclip) serão aplicadas AUTOMATICAMENTE pelo Guardião, sem o usuário ver antes; portanto só proponha-as com alta confiança.
- NUNCA proponha "fechar_processo" nem "reiniciar" para navegadores (chrome, msedge, firefox), WhatsApp, Teams, Zoom, Discord, OneDrive, editores ou qualquer programa com janela que o usuário use; o objetivo é nenhum programa dar erro nem fechar. Prefira "proteger" e "ajustar_limite" (limites mais baixos agem mais cedo).
- Objetivo final: o PC nunca travar e nenhum programa falhar; priorize causa raiz e prevenção.
- gravidade: "baixa", "media" ou "alta".
- A resposta final deve seguir exatamente o esquema JSON pedido.

===== relatorio.md =====
{report}
===== fim do relatório ====="#
    )
}

// ---------------- execução ----------------

fn clean_model(m: &str) -> String {
    m.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')).collect()
}

fn hidden(program: &str) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

fn codex_installed() -> bool {
    hidden("cmd.exe")
        .args(["/D", "/C", "where", "codex"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn kill_tree(child: &mut std::process::Child) {
    let pid = child.id().to_string();
    let _ = hidden("taskkill.exe").args(["/T", "/F", "/PID", &pid]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let _ = child.kill();
    let _ = child.wait();
}

fn tail_chars(s: &str, n: usize) -> String {
    let s = s.trim();
    let count = s.chars().count();
    if count <= n { s.to_string() } else { format!("…{}", s.chars().skip(count - n).collect::<String>()) }
}

fn looks_like_login(s: &str) -> bool {
    let l = s.to_lowercase();
    l.contains("not logged in") || l.contains("codex login") || l.contains("login") || l.contains("unauthorized") || l.contains(" 401") || l.contains("authentication")
}

fn truncate(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n { one } else { format!("{}…", one.chars().take(n).collect::<String>()) }
}

/// Tira o comando real de dentro de `powershell -Command "..."` / `bash -lc '...'` para mostrar algo legível.
fn command_text(item: &serde_json::Value) -> String {
    match item.get("command") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(a)) => {
            let parts: Vec<&str> = a.iter().filter_map(|v| v.as_str()).collect();
            parts.last().map(|s| s.to_string()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn parse_analise(text: &str) -> Option<Analise> {
    let t = text.trim();
    if let Ok(a) = serde_json::from_str::<Analise>(t) { return Some(a); }
    // às vezes vem cercado de ``` ou com texto antes/depois
    let (a, b) = (t.find('{')?, t.rfind('}')?);
    if b <= a { return None; }
    serde_json::from_str::<Analise>(&t[a..=b]).ok()
}

fn normalize(mut a: Analise) -> Analise {
    a.gravidade = match a.gravidade.trim().to_lowercase().as_str() {
        "alta" => "alta",
        "baixa" => "baixa",
        _ => "media",
    }
    .into();
    a.acoes.retain(|x| TIPOS.contains(&x.tipo.as_str()));
    a.acoes.truncate(5);
    for x in &mut a.acoes { x.alvo = x.alvo.trim().to_string(); }
    a
}

fn save_history(a: &Analise, off: i64) {
    let l = clock::local(a.quando, off);
    let name = format!("{:04}-{:02}-{:02}_{:02}{:02}.json", l.y, l.mo, l.d, l.h, l.mi);
    let h = historico();
    if let Ok(s) = serde_json::to_string_pretty(a) {
        let _ = std::fs::write(h.join(name), s);
    }
    let mut files = history_files(&h);
    if files.len() > KEEP_HISTORY {
        let extra = files.len() - KEEP_HISTORY;
        for f in files.drain(..extra) { let _ = std::fs::remove_file(f); }
    }
}

fn history_files(h: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(h)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Bloqueante; chame de uma thread de trabalho. Chama `on` a cada etapa e exatamente uma vez com Pronto ou Erro.
pub fn run(ctx: Context, modelo: &str, on: impl Fn(Progresso)) {
    let started = Instant::now();
    let off = clock::local_offset();
    let now = clock::now_unix();

    on(Progresso::Etapa("Preparando relatório…".into()));
    if !codex_installed() {
        on(Progresso::Erro("Codex não encontrado. Instale com: npm i -g @openai/codex".into()));
        return;
    }

    let d = dir();
    let report = relatorio(&ctx, now, off);
    let out_path = d.join("out.json");
    let _ = std::fs::remove_file(&out_path);
    if std::fs::write(d.join("relatorio.md"), &report).is_err() || std::fs::write(d.join("schema.json"), schema()).is_err() {
        on(Progresso::Erro(format!("Não consegui gravar o relatório em {}.", d.display())));
        return;
    }

    let model = clean_model(modelo);
    let mut line = String::from("/D /S /C \"codex exec");
    if !model.is_empty() { line.push_str(&format!(" -m {model}")); }
    line.push_str(&format!(
        " --sandbox read-only --skip-git-repo-check --output-schema schema.json -o out.json --json -C \"{}\" -\"",
        d.display()
    ));

    on(Progresso::Etapa("Iniciando o Codex…".into()));
    let mut child = match hidden("cmd.exe")
        .raw_arg(&line)
        .current_dir(&d)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            on(Progresso::Erro(format!("Não consegui iniciar o Codex: {e}")));
            return;
        }
    };

    // stdin numa thread para não travar se o pipe encher.
    if let Some(mut stdin) = child.stdin.take() {
        let p = prompt(&report);
        std::thread::spawn(move || {
            let _ = stdin.write_all(p.as_bytes());
            let _ = stdin.flush();
        });
    }

    let stderr_buf = Arc::new(Mutex::new(String::new()));
    if let Some(mut err) = child.stderr.take() {
        let buf = stderr_buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = err.read(&mut chunk) {
                if n == 0 { break; }
                if let Ok(mut b) = buf.lock() {
                    b.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    if b.len() > 16_000 {
                        let cut = b.len() - 8_000;
                        let cut = (cut..b.len()).find(|&i| b.is_char_boundary(i)).unwrap_or(b.len());
                        b.drain(..cut);
                    }
                }
            }
        });
    }

    let (tx, rx) = mpsc::channel::<String>();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() { break; }
            }
        });
    } else {
        drop(tx);
    }

    let deadline = started + TIMEOUT;
    let mut last_step = String::new();
    let mut last_error: Option<String> = None;
    let mut last_message: Option<String> = None;
    let step = |s: String, last: &mut String| {
        if *last != s {
            *last = s.clone();
            on(Progresso::Etapa(s));
        }
    };

    let mut timed_out = false;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        if wait.is_zero() { timed_out = true; break; }
        match rx.recv_timeout(wait) {
            Ok(l) => {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&l) else { continue };
                let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match ty {
                    "thread.started" => step("Conectando ao Codex…".into(), &mut last_step),
                    "item.started" | "item.updated" | "item.completed" => {
                        let Some(item) = v.get("item") else { continue };
                        match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                            "reasoning" => step("Pensando…".into(), &mut last_step),
                            "command_execution" if ty == "item.started" => {
                                step(format!("Investigando: {}", truncate(&command_text(item), 60)), &mut last_step)
                            }
                            "agent_message" => {
                                if ty == "item.completed" && let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                                    last_message = Some(t.to_string());
                                }
                                step("Escrevendo diagnóstico…".into(), &mut last_step)
                            }
                            "error" => {
                                if let Some(m) = item.get("message").and_then(|t| t.as_str()) { last_error = Some(m.to_string()); }
                            }
                            _ => {}
                        }
                    }
                    "error" => {
                        if let Some(m) = v.get("message").and_then(|t| t.as_str()) { last_error = Some(m.to_string()); }
                    }
                    "turn.failed" => {
                        let m = v.get("error").and_then(|e| e.get("message")).and_then(|t| t.as_str()).unwrap_or("falha no turno");
                        last_error = Some(m.to_string());
                    }
                    _ => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => { timed_out = true; break; }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    if timed_out {
        kill_tree(&mut child);
        on(Progresso::Erro("O Codex demorou mais de 6 minutos e foi interrompido. Tente de novo ou use um modelo mais rápido.".into()));
        return;
    }

    // stdout fechou: espera o processo sair (com folga curta).
    let exit_by = deadline.min(Instant::now() + Duration::from_secs(20));
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < exit_by => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => { kill_tree(&mut child); break None; }
            Err(_) => break None,
        }
    };
    // dá um instante para a thread de stderr terminar de ler
    std::thread::sleep(Duration::from_millis(150));
    let stderr = stderr_buf.lock().map(|b| b.clone()).unwrap_or_default();
    let ok_exit = status.is_some_and(|s| s.success());

    let parsed = std::fs::read_to_string(&out_path).ok().and_then(|s| parse_analise(&s))
        .or_else(|| last_message.as_deref().and_then(parse_analise));

    match parsed {
        Some(a) if ok_exit || last_error.is_none() => {
            let mut a = normalize(a);
            a.quando = clock::now_unix();
            a.modelo = if model.is_empty() { "padrão do Codex".into() } else { model };
            a.motivo = ctx.motivo.clone();
            a.segundos = started.elapsed().as_secs();
            save_history(&a, off);
            on(Progresso::Pronto(a));
        }
        _ => {
            let detail = format!("{}\n{}", last_error.clone().unwrap_or_default(), stderr);
            if detail.to_lowercase().contains("não é reconhecido") || detail.to_lowercase().contains("is not recognized") {
                on(Progresso::Erro("Codex não encontrado. Instale com: npm i -g @openai/codex".into()));
            } else if looks_like_login(&detail) {
                on(Progresso::Erro("Faça login no Codex: abra um terminal e rode `codex login`".into()));
            } else {
                let base = if ok_exit { "O Codex não devolveu um diagnóstico válido." } else { "O Codex falhou." };
                let tail = tail_chars(&detail, 300);
                on(Progresso::Erro(if tail.is_empty() { base.to_string() } else { format!("{base} {tail}") }));
            }
        }
    }
}

/// Análise salva mais recente, se houver.
pub fn ultima() -> Option<Analise> {
    let files = history_files(&historico());
    let f = files.last()?;
    serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()
}

/// Abre uma janela de console VISÍVEL com uma sessão interativa do `codex` na pasta da análise.
pub fn abrir_terminal(modelo: &str) -> bool {
    let d = dir();
    let model = clean_model(modelo);
    let has_session = d.join("out.json").exists() || !history_files(&historico()).is_empty();
    let inner = if has_session {
        "codex resume --last".to_string()
    } else {
        let m = if model.is_empty() { String::new() } else { format!(" -m {model}") };
        format!("codex{m} \"Leia relatorio.md e me ajude a investigar travamentos deste PC\"")
    };
    Command::new("cmd.exe")
        .raw_arg(format!("/D /S /K \"{inner}\""))
        .current_dir(&d)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json() {
        let v: serde_json::Value = serde_json::from_str(&schema()).expect("schema JSON");
        assert_eq!(v["additionalProperties"], false);
        assert_eq!(v["properties"]["acoes"]["items"]["required"].as_array().map(|a| a.len()), Some(4));
    }

    #[test]
    fn report_on_empty_context() {
        let ctx = Context { snap: Snapshot::default(), health: None, log: vec![(0, Kind::Info, "x".into())], cfg: Config::default(), motivo: String::new() };
        let r = relatorio(&ctx, clock::now_unix(), 0);
        assert!(r.contains("## Agora"));
        assert!(!prompt(&r).is_empty());
    }

    #[test]
    fn parses_fenced_answer() {
        let a = parse_analise("```json\n{\"resumo\":\"r\",\"causa\":\"c\",\"gravidade\":\"ALTA\",\"acoes\":[{\"titulo\":\"t\",\"explicacao\":\"e\",\"tipo\":\"xx\",\"alvo\":\"\"}]}\n```").map(normalize).expect("parse");
        assert_eq!(a.gravidade, "alta");
        assert!(a.acoes.is_empty());
    }
}
