//! A janela (Rune). Só existe enquanto está aberta; fechada, não gasta GPU.

use std::collections::VecDeque;

use rune::tok::{self, radius, space, type_};
use rune::{Color, Cx, El, ScrollSpec, TextStyle, col, el, oklch, oklcha, row, spacer, text};

use crate::analyst::{Acao, Analise};
use crate::cleanup::{self, State as FState};
use crate::clock;
use crate::guard::{Kind, Level, Snapshot};
use crate::health::{Report, Status};
use crate::sys::GB;
use crate::{Cmd, send};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab { #[default] Agora, Acoes, Saude, Sistema, Faxina, Analise }

#[derive(Default)]
pub struct Ui {
    pub snap: Snapshot,
    pub log: VecDeque<(i64, Kind, String)>,
    pub tab: Tab,
    pub health: Option<Report>,
    pub health_busy: bool,
    pub faxina: Vec<FState>,
    pub faxina_busy: bool,
    pub auto: bool,
    pub autostart: bool,
    pub confirm_kill: Option<String>,
    pub offset: i64,
    pub analise: Option<Analise>,
    pub ia_busy: bool,
    pub ia_etapa: String,
    pub ia_modelo: String,
    pub ia_ativa: bool,
    pub ia_confirm: Option<usize>,
    pub ia_feitas: Vec<usize>,
    pub manut: Option<crate::maintenance::Status>,
    /// "Otimizar tudo": (em andamento?, mensagem para mostrar no cartão principal).
    pub otim: Option<(bool, String)>,
}

impl Ui {
    pub fn push_log(&mut self, kind: Kind, text: String) {
        self.log.push_front((clock::now_unix(), kind, text));
        self.log.truncate(60);
    }
    /// Pede uma análise à IA com o que a janela já sabe (saúde e ações recentes).
    pub fn start_analysis(&mut self, motivo: String, rapido: bool) {
        if self.ia_busy || !self.ia_ativa { return; }
        self.ia_busy = true;
        self.ia_etapa = "Preparando relatório…".into();
        send(Cmd::Analisar { motivo, rapido, health: self.health.clone(), log: self.log.iter().cloned().collect() });
    }
}

// ---------- tipografia: três tamanhos ----------
const T_BIG: TextStyle = TextStyle::new(18.0, 600.0); // títulos e números de destaque
const T_BODY: TextStyle = type_::BODY; // 13: texto corrido
const T_SMALL: TextStyle = type_::LABEL; // 12: apoio e rótulos

// ---------- espaçamento (grade 4/8) ----------
const G4: f32 = space::S1;
const G8: f32 = space::S2;
const G12: f32 = space::S3;
const G16: f32 = 16.0;
const G20: f32 = 20.0;

// ---------- cores (oklch, dessaturadas) ----------
fn green() -> Color { oklch(0.74, 0.12, 155.0) }
fn yellow() -> Color { oklch(0.81, 0.12, 82.0) }
fn red() -> Color { oklch(0.68, 0.15, 25.0) }
fn level_color(l: Level) -> Color { match l { Level::Tranquilo => green(), Level::Atencao => yellow(), Level::Critico => red() } }
fn pct_color(p: f32, warn: f32, crit: f32) -> Color { if p >= crit { red() } else if p >= warn { yellow() } else { green() } }
fn status_color(s: Status) -> Color { match s { Status::Ok => green(), Status::Aviso => yellow(), Status::Ruim => red() } }
fn accent() -> Color { oklch(0.68, 0.13, 262.0) }

// ---------- componentes ----------
#[derive(Clone, Copy, PartialEq)]
enum Variant { Primary, Secondary, Danger }

fn button(key: impl Into<String>, label: impl Into<String>, t: tok::Theme, v: Variant, f: impl Fn(&mut Ui) + 'static) -> El {
    let label = label.into();
    let (fill, hover, fg) = match v {
        Variant::Primary => (accent(), oklch(0.73, 0.13, 262.0), oklch(0.16, 0.02, 262.0)),
        Variant::Secondary => (t.control_off, t.hover, t.text),
        Variant::Danger => (red().alpha(0.18), red().alpha(0.28), red()),
    };
    row().key(key.into()).role("button").label(label.clone()).h(32.0).px(G16).center().radius(radius::MD)
        .fill(fill).hover(hover)
        .on_press(f)
        .child(text(label).font(TextStyle::new(13.0, 600.0)).color(fg))
}

fn small_button(key: impl Into<String>, label: impl Into<String>, t: tok::Theme, color: Option<Color>, f: impl Fn(&mut Ui) + 'static) -> El {
    let label = label.into();
    row().key(key.into()).role("button").label(label.clone()).h(24.0).px(G8 + G4).center().radius(radius::SM)
        .fill(color.map(|c| c.alpha(0.16)).unwrap_or(t.control_off)).hover(t.hover)
        .on_press(f)
        .child(text(label).font(T_SMALL).color(color.unwrap_or(t.text)))
}

fn pill(label: impl Into<String>, color: Color) -> El {
    row().h(24.0).px(G12).gap(6.0).center().radius(12.0).fill(color.alpha(0.14))
        .child(el().size(6.0, 6.0).radius(3.0).fill(color))
        .child(text(label.into()).font(T_SMALL).color(color))
}

fn card(t: tok::Theme) -> El {
    col().w_full().px(G16).py(G16).radius(radius::LG).fill(t.card).border(1.0, t.card_border)
}

fn section(title: &str, t: tok::Theme) -> El {
    text(title.to_uppercase()).font(type_::SECTION.tracking(0.6)).color(t.text_secondary)
}

fn meter(key: &str, title: &str, value: String, pct: f32, color: Color, t: tok::Theme) -> El {
    let frac = (pct / 100.0).clamp(0.0, 1.0);
    col().key(key.to_string()).w_full().gap(G8)
        .child(row().w_full().items_center().gap(G8)
            .child(text(title.to_string()).font(T_SMALL).color(t.text_secondary).grow(1.0).shrink())
            .child(text(value).font(T_BODY).color(t.text)))
        .child(el().w_full().h(6.0).radius(3.0).fill(t.control_track)
            .child(el().key(format!("{key}-bar")).w_frac(frac.max(0.02)).h(6.0).radius(3.0).fill(color).motion(tok::motion::SNAPPY)))
}

fn kv(label: &str, value: String, color: Color, t: tok::Theme) -> El {
    row().w_full().items_center().gap(G8)
        .child(text(label.to_string()).font(T_SMALL).color(t.text_secondary).grow(1.0).shrink())
        .child(text(value).font(T_BODY).color(color).wrap())
}

fn empty_state(title: &str, sub: &str, t: tok::Theme) -> El {
    card(t).gap(G4).items_center().py(G20 + G12)
        .child(el().size(8.0, 8.0).radius(4.0).fill(green().alpha(0.7)))
        .child(el().size(1.0, G8))
        .child(text(title.to_string()).font(T_BIG).color(t.text))
        .child(text(sub.to_string()).font(T_BODY).color(t.text_secondary).wrap())
}

fn tab_button(tab: Tab, label: &'static str, cur: Tab, t: tok::Theme) -> El {
    let on = tab == cur;
    row().key(format!("tab-{label}")).role("tab").label(label).checked(on).h(28.0).grow(1.0).px(G4).center().radius(radius::SM + 1.0)
        .fill(if on { t.card } else { oklcha(0.0, 0.0, 0.0, 0.0) }).hover(if on { t.card } else { t.hover })
        .on_press(move |s: &mut Ui| {
            s.tab = tab;
            if tab == Tab::Faxina && s.faxina.is_empty() { s.faxina_busy = true; send(Cmd::FaxinaCheck); }
            if tab == Tab::Saude && s.health.is_none() && !s.health_busy { s.health_busy = true; send(Cmd::Health); }
        })
        .child(text(label).font(if on { TextStyle::new(12.0, 600.0) } else { T_SMALL }).color(if on { t.text } else { t.text_secondary }))
}

pub fn view(s: &Ui, _cx: &mut Cx<Ui>) -> El {
    let t = tok::theme(true);
    let snap = &s.snap;
    let lc = level_color(snap.level);

    let header = row().key("header").w_full().items_center().gap(G8)
        .child(col().grow(1.0).shrink().gap(2.0)
            .child(text("Ultra Guardião").font(T_BIG).color(t.text))
            .child(text(format!("Ligado há {} · {} processos", clock::duration(snap.uptime), snap.nprocs)).font(T_SMALL).color(t.text_secondary)))
        .child(pill(snap.level.label(), lc).key("level-pill"));

    let tabs = row().key("tabs").w_full().gap(2.0).px(2.0).py(2.0).radius(radius::MD).fill(t.control_track)
        .child(tab_button(Tab::Agora, "Agora", s.tab, t))
        .child(tab_button(Tab::Acoes, "Ações", s.tab, t))
        .child(tab_button(Tab::Saude, "Saúde", s.tab, t))
        .child(tab_button(Tab::Sistema, "Sistema", s.tab, t))
        .child(tab_button(Tab::Faxina, "Faxina", s.tab, t))
        .child(tab_button(Tab::Analise, "Análise", s.tab, t));

    let body = match s.tab {
        Tab::Agora => agora(s, t),
        Tab::Acoes => acoes(s, t),
        Tab::Saude => saude(s, t),
        Tab::Sistema => sistema(s, t),
        Tab::Faxina => faxina(s, t),
        Tab::Analise => analise(s, t),
    };

    let footer = row().key("footer").w_full().items_center().gap(G8)
        .child(text("Guardião automático").font(T_SMALL).color(t.text_secondary))
        .child(spacer())
        .child(small_button("auto", if s.auto { "Ligado" } else { "Desligado" }, t, Some(if s.auto { green() } else { yellow() }), |s: &mut Ui| {
            s.auto = !s.auto;
            send(Cmd::SetAuto(s.auto));
        }));

    col().key("root").w_full().h_full().fill(t.bg).px(G16).py(G16).gap(G12)
        .child(header)
        .child(tabs)
        .child(col().key(format!("body-{:?}", s.tab)).w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(body))
        .child(footer)
}

fn agora(s: &Ui, t: tok::Theme) -> El {
    let m = &s.snap.mem;
    let used = m.used_pct();
    let commit = m.commit_vs_ram_pct();
    let resumo = crate::insights::avaliar(&s.snap, s.manut.as_ref(), s.health.as_ref());

    // Cartão principal: nota, veredito e botão único.
    let nota_cor = if resumo.nota >= 85 { green() } else if resumo.nota >= 65 { yellow() } else { red() };
    let hero = col().key("hero").w_full().px(G16).py(G16).gap(G16).radius(20.0).fill(t.card).border(1.0, nota_cor.alpha(0.35))
        .child(row().w_full().items_center().gap(G16)
            .child(row().items_center().gap(G4)
                .child(text(format!("{}", resumo.nota)).font(TextStyle::new(44.0, 700.0)).color(nota_cor))
                .child(text("/100").font(T_SMALL).color(t.text_secondary)))
            .child(col().grow(1.0).shrink().gap(G4)
                .child(text(resumo.titulo.clone()).font(T_BIG).color(t.text).wrap())
                .child(text(resumo.subtitulo.clone()).font(T_BODY).color(t.text_secondary).wrap())))
        .child(button(
            "optimize-all",
            if s.otim.as_ref().is_some_and(|o| o.0) { "Otimizando…" } else { "Otimizar tudo agora" },
            t,
            Variant::Primary,
            |s: &mut Ui| {
                if s.otim.as_ref().is_some_and(|o| o.0) { return; }
                s.otim = Some((true, "Otimizando: liberando memória, fechando órfãos e verificando o sistema…".into()));
                send(Cmd::OtimizarTudo);
            },
        ).w_full().h(40.0))
        .child(match &s.otim {
            Some((busy, msg)) => text(msg.clone()).font(T_BODY).color(if *busy { t.text_secondary } else { green() }).wrap(),
            None => text(if s.ia_ativa {
                "Libera memória, fecha órfãos sem janela, limpa o que for seguro e pede análise da IA (envia dados do PC ao Codex)"
            } else {
                "Libera memória, fecha órfãos sem janela e limpa o que for seguro"
            }).font(T_SMALL).color(t.text_secondary).wrap(),
        });

    // Grade 2x2 de métricas.
    let ptxt = |v: f64| format!("{v:.1}").replace('.', ",");
    let (disk_val, disk_leg, disk_pct) = match &s.manut {
        Some(mm) => {
            let total = mm.disk_total_gb.max(0.001);
            let up = (((total - mm.disk_free_gb) / total) * 100.0).clamp(0.0, 100.0) as f32;
            (format!("{:.0} GB", mm.disk_free_gb), format!("livres de {:.0} GB", mm.disk_total_gb), up)
        }
        None => ("—".to_string(), "verificando…".to_string(), 0.0),
    };
    let grid = col().key("grid").w_full().gap(G8)
        .child(row().w_full().gap(G8)
            .child(metric("ram", "RAM", format!("{used:.0}%"), format!("{} de {} GB", ptxt((m.total - m.avail) as f64 / GB), ptxt(m.total as f64 / GB)), used, pct_color(used, 85.0, 92.0), t))
            .child(metric("cpu", "Processador", format!("{:.0}%", s.snap.cpu), "uso agora".to_string(), s.snap.cpu, pct_color(s.snap.cpu, 70.0, 90.0), t)))
        .child(row().w_full().gap(G8)
            .child(metric("disk", "Disco C:", disk_val, disk_leg, disk_pct, pct_color(disk_pct, 85.0, 95.0), t))
            .child(metric("commit", "Memória pedida", format!("{commit:.0}%"), "Acima de 100% = usando o disco como memória".to_string(), commit.min(130.0) / 1.3, pct_color(commit, 90.0, 105.0), t)));

    // Recomendações.
    let mut recs = col().key("recs").w_full().gap(G8).child(section("Recomendações", t));
    for (i, ins) in resumo.itens.iter().enumerate() {
        let c = match ins.nivel { crate::insights::Nivel::Bom => green(), crate::insights::Nivel::Aviso => yellow(), crate::insights::Nivel::Ruim => red() };
        let mut r = row().w_full().gap(G12).items_center()
            .child(el().size(10.0, 10.0).radius(5.0).fill(c).mt(4.0))
            .child(col().grow(1.0).shrink().gap(2.0)
                .child(text(ins.titulo.clone()).font(TextStyle::new(13.0, 600.0)).color(t.text).wrap())
                .child(text(ins.detalhe.clone()).font(T_SMALL).color(t.text_secondary).wrap()));
        if let Some((b, label)) = ins.botao {
            r = r.child(small_button(format!("ins-btn-{i}"), label, t, Some(c), move |s: &mut Ui| match b {
                crate::insights::Botao::AliviarMemoria => send(Cmd::RelieveNow),
                crate::insights::Botao::FecharOrfaos => send(Cmd::CloseOrphans),
                crate::insights::Botao::OtimizarTudo => send(Cmd::OtimizarTudo),
                crate::insights::Botao::VerSistema => s.tab = Tab::Sistema,
            }));
        }
        recs = recs.child(col().key(format!("ins-{i}")).w_full().px(G12).py(G12).radius(radius::LG).fill(t.card).border(1.0, c.alpha(0.3)).child(r));
    }

    let mut list = card(t).gap(G4).child(
        row().w_full().pb(G8)
            .child(text("MAIORES CONSUMIDORES").font(type_::SECTION.tracking(0.6)).color(t.text_secondary))
            .child(spacer())
            .child(text("RAM · CPU").font(type_::SECTION.tracking(0.6)).color(t.text_secondary)),
    );
    if s.snap.top.is_empty() {
        list = list.child(text("Nenhum processo relevante no momento.").font(T_BODY).color(t.text_secondary));
    }
    for p in &s.snap.top {
        let name = if p.count > 1 { format!("{} ×{}", p.name, p.count) } else { p.name.clone() };
        let confirming = s.confirm_kill.as_deref() == Some(p.name.as_str());
        let pname = p.name.clone();
        let kill = if confirming {
            small_button(format!("kill-{}", p.name), "Confirmar", t, Some(red()), move |s: &mut Ui| {
                s.confirm_kill = None;
                send(Cmd::KillGroup(format!("{pname}.exe")));
            })
        } else {
            small_button(format!("kill-{}", p.name), "Fechar", t, None, move |s: &mut Ui| s.confirm_kill = Some(pname.clone()))
        };
        list = list.child(
            row().key(format!("p-{}", p.name)).w_full().h(32.0).items_center().gap(G8)
                .child(text(name).font(T_BODY).color(t.text).truncate().grow(1.0).shrink())
                .child(text(format!("{:.0} MB", p.mb)).font(type_::MONO_CAPTION).color(t.text))
                .child(text(format!("{:>3.0}%", p.cpu)).font(type_::MONO_CAPTION).color(t.text_secondary).min_w(34.0))
                .child(kill),
        );
    }

    let stats = text(format!(
        "Desde que abriu: {:.1} GB aliviados · {} órfãos fechados",
        s.snap.freed_total as f64 / GB, s.snap.closed_total
    )).font(T_SMALL).color(t.text_secondary).wrap();

    col().w_full().gap(G12).child(hero).child(grid).child(recs).child(list).child(stats)
}

/// Cartão-métrica compacto: rótulo, número grande, legenda e barra fina.
fn metric(key: &str, title: &str, value: String, legend: String, pct: f32, color: Color, t: tok::Theme) -> El {
    let frac = (pct / 100.0).clamp(0.0, 1.0);
    col().key(key.to_string()).grow(1.0).shrink().min_w(0.0).px(G12).py(G12).gap(G4).radius(radius::LG).fill(t.card).border(1.0, t.card_border)
        .child(text(title.to_string()).font(T_SMALL).color(t.text_secondary))
        .child(text(value).font(TextStyle::new(28.0, 700.0)).color(color))
        .child(text(legend).font(T_SMALL).color(t.text_secondary).wrap())
        .child(el().size(1.0, G4))
        .child(el().w_full().h(4.0).radius(2.0).fill(t.control_track)
            .child(el().key(format!("{key}-bar")).w_frac(frac.max(0.02)).h(4.0).radius(2.0).fill(color).motion(tok::motion::SNAPPY)))
}

fn acoes(s: &Ui, t: tok::Theme) -> El {
    if s.log.is_empty() {
        return col().w_full().gap(G12)
            .child(empty_state("Tudo calmo por aqui", "Quando o Guardião agir, o que ele fez aparece nesta lista.", t))
            .child(button("open-bb", "Abrir caixa-preta", t, Variant::Secondary, |_| send(Cmd::OpenBlackBox)));
    }
    let mut c = card(t).gap(G12);
    for (i, (when, kind, msg)) in s.log.iter().enumerate() {
        let color = match kind { Kind::Info => t.text_secondary, Kind::Acao => green(), Kind::Alerta => yellow() };
        c = c.child(row().key(format!("log-{i}")).w_full().gap(G8)
            .child(text(clock::hhmm(*when, s.offset)).font(type_::MONO_CAPTION).color(t.text_secondary))
            .child(el().size(6.0, 6.0).radius(3.0).fill(color).mt(5.0))
            .child(text(msg.clone()).font(T_BODY).color(t.text).wrap().grow(1.0).shrink()));
    }
    col().w_full().gap(G12).child(c)
        .child(button("open-bb", "Abrir caixa-preta", t, Variant::Secondary, |_| send(Cmd::OpenBlackBox)))
}

fn saude(s: &Ui, t: tok::Theme) -> El {
    let mut out = col().w_full().gap(G12);
    let top = row().w_full().items_center().gap(G8)
        .child(text(match (&s.health, s.health_busy) {
            (_, true) => "Verificando o Windows…".to_string(),
            (Some(r), _) => format!("Verificado às {}", clock::hhmm(r.at, s.offset)),
            (None, _) => "Ainda não verificado".to_string(),
        }).font(T_SMALL).color(t.text_secondary).grow(1.0).shrink())
        .child(small_button("health-run", "Verificar agora", t, None, |s: &mut Ui| {
            if !s.health_busy { s.health_busy = true; send(Cmd::Health); }
        }));
    out = out.child(top);
    let Some(r) = &s.health else { return out };

    if let Some((when, line)) = &r.last_crash {
        let mut c = card(t).gap(G8).border(1.0, red().alpha(0.4))
            .child(text(format!("Último travamento: {}", clock::dd_mm_hhmm(*when, s.offset))).font(T_BODY).color(red()));
        match line {
            Some(l) => {
                c = c.child(text(format!(
                    "Caixa-preta às {}: RAM {:.0}%, CPU {:.0}%, {:.1} GB pedidos.",
                    clock::hhmm(l.t, s.offset), l.ram, l.cpu, l.commit_gb
                )).font(T_BODY).color(t.text).wrap());
                let top = l.top.iter().map(|(n, mb)| format!("{n} {mb} MB")).collect::<Vec<_>>().join(", ");
                c = c.child(text(format!("Maiores: {top}")).font(T_SMALL).color(t.text_secondary).wrap());
            }
            None => { c = c.child(text("A caixa-preta ainda não estava gravando nessa hora. No próximo, vai mostrar a causa.").font(T_BODY).color(t.text_secondary).wrap()); }
        }
        out = out.child(c);
    }

    let mut c = card(t).gap(G16);
    for (i, ch) in r.checks.iter().enumerate() {
        c = c.child(row().key(format!("chk-{i}")).w_full().gap(G12)
            .child(el().size(8.0, 8.0).radius(4.0).fill(status_color(ch.status)).mt(5.0))
            .child(col().grow(1.0).shrink().gap(2.0)
                .child(text(ch.title.clone()).font(T_BODY).color(t.text).wrap())
                .child(text(ch.detail.clone()).font(T_SMALL).color(t.text_secondary).wrap())));
    }
    out.child(c)
}

fn sistema(s: &Ui, t: tok::Theme) -> El {
    let mut out = col().w_full().gap(G12);
    let Some(m) = &s.manut else {
        return out.child(card(t).child(text("Verificando…").font(T_BODY).color(t.text_secondary)));
    };
    out = out.child(text(format!("Atualizado às {}", clock::hhmm(m.at, s.offset))).font(T_SMALL).color(t.text_secondary));

    // Disco e memória
    let total = m.disk_total_gb.max(0.001);
    let used_pct = (((total - m.disk_free_gb) / total) * 100.0).clamp(0.0, 100.0) as f32;
    let mut c = card(t).gap(G16)
        .child(meter("sys-disk", "Disco C:", format!("{:.0} GB livres de {:.0} GB", m.disk_free_gb, m.disk_total_gb), used_pct, pct_color(used_pct, 85.0, 95.0), t));
    if let Some(b) = m.disk_busy_pct {
        c = c.child(meter("sys-busy", "Atividade do disco", format!("{b:.0}%"), b, pct_color(b, 70.0, 95.0), t));
    }
    c = c.child(meter("sys-commit", "Memória pedida", format!("{:.0}% da RAM", m.commit_pct), m.commit_pct.min(130.0) / 1.3, pct_color(m.commit_pct, 90.0, 105.0), t));
    out = out.child(c);

    // Detalhes
    let temp = if m.temp_mb >= 1024.0 { format!("{:.1} GB", m.temp_mb / 1024.0) } else { format!("{:.0} MB", m.temp_mb) };
    let temp_color = if m.temp_mb >= 4096.0 { yellow() } else { t.text };
    let mut d = card(t).gap(G12)
        .child(kv("Arquivos temporários", temp, temp_color, t))
        .child(kv("Plano de energia", m.power_plan.clone(), t.text, t));
    if let Some(ssd) = &m.ssd { d = d.child(kv("SSD", ssd.clone(), t.text, t)); }
    if let Some(c) = m.cpu_temp_c { d = d.child(kv("Temperatura da CPU", format!("{c:.0} °C"), pct_color(c, 80.0, 92.0), t)); }
    out = out.child(d);

    // Programas que iniciam com o Windows
    if !m.startup.is_empty() {
        let mut c = card(t).gap(G8).child(section("Iniciam com o Windows", t));
        for (i, n) in m.startup.iter().take(8).enumerate() {
            c = c.child(text(n.clone()).key(format!("st-{i}")).font(T_BODY).color(t.text).truncate());
        }
        if m.startup.len() > 8 {
            c = c.child(text(format!("e mais {}", m.startup.len() - 8)).font(T_SMALL).color(t.text_secondary));
        }
        out = out.child(c);
    }

    // Processos com muitos handles
    if !m.handle_hogs.is_empty() {
        let mut c = card(t).gap(G8).child(section("Muitos handles abertos", t));
        for (i, (n, h)) in m.handle_hogs.iter().enumerate() {
            c = c.child(row().key(format!("hh-{i}")).w_full().items_center().gap(G8)
                .child(text(n.clone()).font(T_BODY).color(t.text).truncate().grow(1.0).shrink())
                .child(text(format!("{h}")).font(type_::MONO_CAPTION).color(if *h >= 10_000 { yellow() } else { t.text_secondary })));
        }
        out = out.child(c);
    }

    // Autodiagnóstico
    if !m.selftest.is_empty() {
        let mut c = card(t).gap(G8).child(section("Autodiagnóstico", t));
        for (i, (ok, msg)) in m.selftest.iter().enumerate() {
            let color = if *ok { green() } else { red() };
            c = c.child(row().key(format!("sf-{i}")).w_full().gap(G8)
                .child(text(if *ok { "✓" } else { "✗" }).font(T_BODY).color(color))
                .child(text(msg.clone()).font(T_BODY).color(if *ok { t.text } else { color }).wrap().grow(1.0).shrink()));
        }
        out = out.child(c);
    }

    // Feito automaticamente
    if !m.notes.is_empty() {
        let mut c = card(t).gap(G8).child(section("Feito automaticamente", t));
        for (i, n) in m.notes.iter().enumerate() {
            c = c.child(row().key(format!("nt-{i}")).w_full().gap(G8)
                .child(el().size(6.0, 6.0).radius(3.0).fill(green()).mt(5.0))
                .child(text(n.clone()).font(T_BODY).color(t.text).wrap().grow(1.0).shrink()));
        }
        out = out.child(c);
    }
    out
}

fn faxina(s: &Ui, t: tok::Theme) -> El {
    let mut c = card(t).gap(G16);
    let pending: Vec<&'static str> = cleanup::ITEMS.iter().zip(s.faxina.iter()).filter(|(_, st)| **st == FState::Pendente).map(|(i, _)| i.id).collect();
    for (i, item) in cleanup::ITEMS.iter().enumerate() {
        let st = s.faxina.get(i).copied().unwrap_or(FState::Verificando);
        let (label, color) = match st {
            FState::Aplicado => ("Feito", green()),
            FState::Pendente => ("Pendente", yellow()),
            FState::NaoExiste => ("Já está ok", green()),
            FState::Verificando => ("…", t.text_secondary),
        };
        let id = item.id;
        let action = match (st, s.faxina_busy) {
            (_, true) | (FState::Verificando | FState::NaoExiste, _) => el(),
            (FState::Aplicado, _) => small_button(format!("undo-{id}"), "Desfazer", t, None, move |s: &mut Ui| { s.faxina_busy = true; send(Cmd::FaxinaRun(id, true)); }),
            (FState::Pendente, _) => small_button(format!("do-{id}"), "Aplicar", t, Some(green()), move |s: &mut Ui| { s.faxina_busy = true; send(Cmd::FaxinaRun(id, false)); }),
        };
        c = c.child(row().key(format!("fx-{id}")).w_full().gap(G12).items_center()
            .child(col().grow(1.0).shrink().gap(2.0)
                .child(row().gap(G8).items_center()
                    .child(text(item.title).font(T_BODY).color(t.text))
                    .child(text(label).font(T_SMALL).color(color)))
                .child(text(if item.admin { format!("{} 🛡", item.detail) } else { item.detail.to_string() }).font(T_SMALL).color(t.text_secondary).wrap()))
            .child(action));
    }
    let mut out = col().w_full().gap(G12);
    if s.faxina_busy {
        out = out.child(text("Trabalhando… se o Windows pedir permissão, aceite.").font(T_SMALL).color(t.text_secondary).wrap());
    } else if !pending.is_empty() {
        out = out.child(button("fx-all", format!("Aplicar os {} pendentes", pending.len()), t, Variant::Primary, move |s: &mut Ui| {
            s.faxina_busy = true;
            send(Cmd::FaxinaAll(pending.clone()));
        }));
    }
    out = out.child(c);
    out.child(card(t).child(row().w_full().items_center().gap(G12)
        .child(col().grow(1.0).shrink().gap(2.0)
            .child(text("Iniciar com o Windows").font(T_BODY).color(t.text))
            .child(text("Fica no relógio, perto do Wi-Fi, gastando quase nada.").font(T_SMALL).color(t.text_secondary).wrap()))
        .child(small_button("autostart", if s.autostart { "Ligado" } else { "Desligado" }, t, Some(if s.autostart { green() } else { yellow() }), |s: &mut Ui| {
            s.autostart = !s.autostart;
            send(Cmd::Autostart(s.autostart));
        }))))
}

fn gravidade_color(g: &str) -> Color { match g { "alta" => red(), "media" => yellow(), _ => green() } }

fn action_button(i: usize, a: &Acao, s: &Ui, t: tok::Theme) -> El {
    if s.ia_feitas.contains(&i) {
        return text("Feito ✓").font(T_SMALL).color(green());
    }
    let risky = matches!(a.tipo.as_str(), "fechar_processo" | "reiniciar");
    let label = match a.tipo.as_str() {
        "manual" => return el(),
        "fechar_processo" => format!("Fechar {}", a.alvo.trim_end_matches(".exe")),
        "reiniciar" => "Reiniciar em 60 s".to_string(),
        "adicionar_orfao" => "Vigiar órfãos".to_string(),
        "proteger" => "Proteger".to_string(),
        "ajustar_limite" => "Ajustar".to_string(),
        "faxina" => "Aplicar faxina".to_string(),
        _ => return el(),
    };
    let acao = a.clone();
    if risky && s.ia_confirm != Some(i) {
        return small_button(format!("ia-do-{i}"), label, t, Some(yellow()), move |s: &mut Ui| s.ia_confirm = Some(i));
    }
    let label = if risky { "Confirmar".to_string() } else { label };
    small_button(format!("ia-do-{i}"), label, t, Some(if risky { red() } else { green() }), move |s: &mut Ui| {
        s.ia_confirm = None;
        s.ia_feitas.push(i);
        send(Cmd::AplicarAcao(acao.clone()));
    })
}

fn analise(s: &Ui, t: tok::Theme) -> El {
    let mut out = col().w_full().gap(G12);
    let top = row().w_full().items_center().gap(G8)
        .child(col().grow(1.0).shrink().gap(2.0)
            .child(text("Diagnóstico com IA").font(T_BODY).color(t.text))
            .child(text(if s.ia_ativa { format!("Codex · {}", s.ia_modelo) } else { "Desligada no config.json (ia_ativa)".to_string() }).font(T_SMALL).color(t.text_secondary)))
        .child(small_button("ia-term", "Terminal", t, None, |_| send(Cmd::AbrirTerminal)))
        .child(if s.ia_busy { el() } else {
            small_button("ia-run", "Analisar agora", t, Some(accent()), |s: &mut Ui| s.start_analysis("Pedido manual pelo painel.".into(), false))
        });
    out = out.child(top);
    if s.ia_ativa {
        out = out.child(text("\"Analisar agora\" e \"Terminal\" usam o Codex: dados deste PC (programas, uso, registros do Windows) são enviados ao serviço de IA. Detalhes no README.").font(T_SMALL).color(t.text_secondary).wrap());
    }

    if s.ia_busy {
        out = out.child(card(t).child(row().w_full().items_center().gap(G12)
            .child(el().key("ia-pulse").size(8.0, 8.0).radius(4.0).fill(accent()))
            .child(text(s.ia_etapa.clone()).font(T_BODY).color(t.text).wrap().grow(1.0).shrink())));
    }

    let Some(a) = &s.analise else {
        return out.child(empty_state("Nenhuma análise ainda", "Toque em \"Analisar agora\": a IA lê a caixa-preta e os registros do Windows e diz a causa e o que fazer. Ela só roda sem clique se você ativar ia_automatica no config.json.", t));
    };

    let gc = gravidade_color(&a.gravidade);
    let meta = format!("{} · {} · {} s · {}", clock::dd_mm_hhmm(a.quando, s.offset), a.modelo, a.segundos, a.motivo);
    out = out.child(card(t).gap(G8).border(1.0, gc.alpha(0.4))
        .child(row().w_full().items_center().child(pill(format!("Gravidade {}", a.gravidade), gc)).child(spacer()))
        .child(text(a.resumo.clone()).font(T_BIG).color(t.text).wrap())
        .child(text(format!("Causa provável: {}", a.causa)).font(T_BODY).color(t.text).wrap())
        .child(text(meta).font(T_SMALL).color(t.text_secondary).wrap()));

    for (i, ac) in a.acoes.iter().enumerate() {
        out = out.child(card(t).key(format!("ia-acao-{i}")).child(row().w_full().gap(G12).items_center()
            .child(col().grow(1.0).shrink().gap(2.0)
                .child(text(ac.titulo.clone()).font(T_BODY).color(t.text).wrap())
                .child(text(ac.explicacao.clone()).font(T_SMALL).color(t.text_secondary).wrap()))
            .child(action_button(i, ac, s, t))));
    }
    out
}
