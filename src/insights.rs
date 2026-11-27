//! Recomendações em português simples, a partir dos dados já coletados. Função pura, sem I/O.

use crate::guard::{Level, Snapshot};
use crate::health;
use crate::maintenance;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nivel { Bom, Aviso, Ruim }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Botao { AliviarMemoria, FecharOrfaos, OtimizarTudo, VerSistema }

#[derive(Clone, Debug)]
pub struct Insight {
    pub nivel: Nivel,
    pub titulo: String,
    pub detalhe: String,
    /// (ação, rótulo curto do botão)
    pub botao: Option<(Botao, &'static str)>,
}

#[derive(Clone, Debug)]
pub struct Resumo {
    pub nota: u8,
    pub titulo: String,
    pub subtitulo: String,
    /// Ordenados: Ruim, Aviso, Bom (no máximo 2).
    pub itens: Vec<Insight>,
}

struct Acc {
    itens: Vec<(u32, Insight)>,
}

impl Acc {
    fn add(&mut self, peso: u32, nivel: Nivel, titulo: impl Into<String>, detalhe: impl Into<String>, botao: Option<(Botao, &'static str)>) {
        self.itens.push((peso, Insight { nivel, titulo: titulo.into(), detalhe: detalhe.into(), botao }));
    }
}

fn rank(n: Nivel) -> u8 {
    match n { Nivel::Ruim => 0, Nivel::Aviso => 1, Nivel::Bom => 2 }
}

fn eh_navegador(nome: &str) -> bool {
    let n = nome.to_lowercase();
    n == "chrome" || n == "msedge" || n == "firefox"
}

pub fn avaliar(snap: &Snapshot, manut: Option<&maintenance::Status>, health: Option<&health::Report>) -> Resumo {
    let mut a = Acc { itens: Vec::new() };

    // Memória (RAM).
    let ram = snap.mem.used_pct();
    if snap.mem.total > 0 {
        if ram >= 92.0 {
            a.add(30, Nivel::Ruim, format!("Memória quase cheia ({ram:.0}%)"),
                "O PC está prestes a travar. Alivie a memória agora e feche o que não está usando.",
                Some((Botao::AliviarMemoria, "Aliviar")));
        } else if ram >= 85.0 {
            a.add(12, Nivel::Aviso, format!("Memória ficando cheia ({ram:.0}%)"),
                "Ainda funciona, mas pode ficar lento. Aliviar a memória ajuda.",
                Some((Botao::AliviarMemoria, "Aliviar")));
        }
    }

    // Memória pedida além da RAM.
    let commit = snap.mem.commit_vs_ram_pct();
    if commit > 150.0 {
        a.add(25, Nivel::Ruim, "Programas pedem muito mais memória do que o PC tem",
            format!("Os programas pedem {commit:.0}% da RAM. Quando passa disso, o Windows usa o disco como memória, e isso deixa tudo lento. Feche programas que não precisa."),
            Some((Botao::AliviarMemoria, "Aliviar")));
    } else if commit > 110.0 {
        a.add(10, Nivel::Aviso, "Programas pedem mais memória do que o PC tem",
            format!("Os programas pedem {commit:.0}% da RAM. O Windows passa a usar o disco como memória, o que deixa tudo mais lento."),
            Some((Botao::AliviarMemoria, "Aliviar")));
    }

    // Tempo ligado.
    let dias = snap.uptime / 86_400;
    if snap.uptime > 7 * 86_400 {
        a.add(15, Nivel::Ruim, format!("PC ligado há {dias} dias sem reiniciar"),
            "Reiniciar limpa vazamentos de memória acumulados e costuma deixar o PC bem mais rápido.", None);
    } else if snap.uptime > 3 * 86_400 {
        a.add(8, Nivel::Aviso, format!("PC ligado há {dias} dias"),
            "Quando puder, reinicie: isso limpa vazamentos de memória acumulados.", None);
    }

    // Navegador pesado.
    if let Some(b) = snap.top.iter().find(|t| eh_navegador(&t.name) && (t.count >= 25 || t.mb >= 3000.0)) {
        a.add(8, Nivel::Aviso, format!("O navegador está pesado ({} processos, {:.1} GB)", b.count, b.mb / 1024.0),
            "Ative a \"Economia de memória\" nas configurações do navegador e feche as abas que você não está usando.", None);
    }

    // Processador.
    if snap.cpu > 85.0 {
        a.add(10, Nivel::Aviso, format!("Processador muito ocupado ({:.0}%)", snap.cpu),
            "Algum programa está trabalhando pesado. Se o PC estiver lento, feche o que não precisa.", None);
    }

    // Programas esquecidos.
    if snap.orphans > 0 {
        let n = snap.orphans;
        a.add(8, Nivel::Aviso,
            if n == 1 { "1 programa esquecido ocupando memória".to_string() } else { format!("{n} programas esquecidos ocupando memória") },
            "São restos de programas que já foram fechados. Pode fechar sem perder nada.",
            Some((Botao::FecharOrfaos, "Fechar")));
    }

    if snap.level == Level::Critico {
        a.add(10, Nivel::Ruim, "O PC está em nível crítico",
            "A memória está no limite e o PC pode travar a qualquer momento.",
            Some((Botao::AliviarMemoria, "Aliviar")));
    }

    // Manutenção.
    if let Some(m) = manut {
        if m.disk_total_gb > 0.0 {
            let livre = m.disk_free_gb / m.disk_total_gb * 100.0;
            if livre < 10.0 {
                a.add(25, Nivel::Ruim, format!("Pouco espaço no disco C: ({livre:.0}% livre)"),
                    "Com o disco quase cheio o PC fica lento e o Windows pode falhar. Limpe arquivos temporários.",
                    Some((Botao::OtimizarTudo, "Limpar")));
            } else if livre < 20.0 {
                a.add(10, Nivel::Aviso, format!("Espaço no disco C: ficando curto ({livre:.0}% livre)"),
                    "Vale limpar arquivos temporários antes que falte espaço.",
                    Some((Botao::OtimizarTudo, "Limpar")));
            }
        }
        if m.disk_busy_pct.is_some_and(|d| d > 80.0) {
            a.add(10, Nivel::Aviso, "O disco está muito ocupado",
                "Alguma coisa está lendo ou gravando muito no disco agora, o que deixa o PC lento. Costuma passar sozinho.", None);
        }
        if m.temp_mb > 2000.0 {
            a.add(6, Nivel::Aviso, format!("{:.1} GB de arquivos temporários acumulados", m.temp_mb / 1024.0),
                "São arquivos que programas deixaram para trás. Limpar libera espaço.",
                Some((Botao::OtimizarTudo, "Limpar")));
        }
        if m.power_plan.to_lowercase().contains("economia") {
            a.add(8, Nivel::Aviso, "Plano de energia em \"Economia\"",
                "Esse modo reduz o desempenho para gastar menos bateria. Trocar para Equilibrado deixa o PC mais ágil.",
                Some((Botao::OtimizarTudo, "Ajustar")));
        }
        if let Some((nome, n)) = m.handle_hogs.first() {
            a.add(6, Nivel::Aviso, format!("{nome} está acumulando recursos do Windows"),
                format!("Ele está usando {n} recursos internos, bem acima do normal. Fechar e abrir de novo costuma resolver."), None);
        }
        if let Some(s) = &m.ssd {
            if ["Aviso", "Crítico", "Ruim", "Atenção", "problemas"].iter().any(|k| s.contains(k)) {
                a.add(15, Nivel::Aviso, "O disco (SSD) pede atenção",
                    format!("Estado informado: {s}. Faça uma cópia de segurança dos seus arquivos importantes."), None);
            }
        }
        if m.cpu_temp_c.is_some_and(|t| t > 85.0) {
            let t = m.cpu_temp_c.unwrap_or(0.0);
            a.add(12, Nivel::Aviso, format!("O processador está muito quente ({t:.0} °C)"),
                "Deixe o PC em local ventilado e evite apoiar em cama ou sofá. Calor demais deixa tudo mais lento.", None);
        }
        let falhas: Vec<&(bool, String)> = m.selftest.iter()
            .filter(|(ok, nome)| !*ok && !nome.starts_with("Espaço livre") && !nome.starts_with("Plano de energia"))
            .collect();
        if let Some((_, nome)) = falhas.first() {
            let extra = if falhas.len() > 1 { format!(" (e mais {})", falhas.len() - 1) } else { String::new() };
            a.add(5, Nivel::Aviso, "O Ultra Guardião encontrou algo para ajustar",
                format!("{nome}{extra}."), Some((Botao::VerSistema, "Ver")));
        }
    }

    // Saúde do Windows.
    if let Some(h) = health {
        if let Some((t, _)) = &h.last_crash {
            if h.at > 0 && h.at - *t >= 0 && h.at - *t <= 3 * 86_400 {
                a.add(15, Nivel::Aviso, "O PC travou por completo nos últimos dias",
                    "Ele precisou ser desligado à força. Se isso se repetir, veja os detalhes na aba do sistema.",
                    Some((Botao::VerSistema, "Ver")));
            }
        }
        let mut n = 0;
        for c in &h.checks {
            if n >= 2 { break; }
            if c.status == health::Status::Ok { continue; }
            if c.title.starts_with("Programas pedem") || c.title.starts_with("C:") || c.title.starts_with("Ligado há") || c.title.contains("travamento(s) total") {
                continue;
            }
            let (peso, nivel) = if c.status == health::Status::Ruim { (12, Nivel::Ruim) } else { (5, Nivel::Aviso) };
            a.add(peso, nivel, c.title.clone(), c.detail.clone(), Some((Botao::VerSistema, "Ver")));
            n += 1;
        }
    }

    // Nota e ordenação.
    let desconto: u32 = a.itens.iter().map(|(p, _)| *p).sum();
    let nota = 100u32.saturating_sub(desconto).min(100) as u8;
    a.itens.sort_by(|x, y| rank(x.1.nivel).cmp(&rank(y.1.nivel)).then(y.0.cmp(&x.0)));

    let subtitulo = match a.itens.first() {
        Some((_, i)) => format!("Principal motivo: {}.", i.titulo.trim_end_matches('.').to_lowercase_first()),
        None => "Tudo está funcionando bem neste momento.".to_string(),
    };

    let mut itens: Vec<Insight> = a.itens.into_iter().map(|(_, i)| i).collect();
    itens.truncate(8);
    if itens.is_empty() {
        itens.push(Insight {
            nivel: Nivel::Bom,
            titulo: "Memória, disco e processador em ordem".into(),
            detalhe: "Não encontrei nada que precise da sua atenção agora.".into(),
            botao: None,
        });
    }

    let titulo = if nota >= 85 { "Seu PC está ótimo" }
        else if nota >= 65 { "Seu PC está bem, com pontos de atenção" }
        else if nota >= 40 { "Seu PC precisa de cuidado" }
        else { "Seu PC está em risco" };

    Resumo { nota, titulo: titulo.into(), subtitulo, itens }
}

trait LowerFirst { fn to_lowercase_first(&self) -> String; }
impl LowerFirst for str {
    fn to_lowercase_first(&self) -> String {
        let mut c = self.chars();
        match c.next() {
            Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{GB, Memory};

    fn mem(total_gb: f64, avail_gb: f64, commit_gb: f64) -> Memory {
        Memory {
            total: (total_gb * GB) as u64,
            avail: (avail_gb * GB) as u64,
            commit_used: (commit_gb * GB) as u64,
            commit_limit: (total_gb * 2.0 * GB) as u64,
        }
    }

    #[test]
    fn saudavel() {
        let s = Snapshot { mem: mem(16.0, 10.0, 8.0), cpu: 10.0, uptime: 3600, ..Default::default() };
        let r = avaliar(&s, None, None);
        assert!(r.nota >= 85, "nota {}", r.nota);
        assert!(r.itens.iter().all(|i| i.nivel != Nivel::Ruim));
        assert_eq!(r.itens[0].nivel, Nivel::Bom);
    }

    #[test]
    fn ram_e_commit_altos() {
        let s = Snapshot { mem: mem(16.0, 0.8, 28.8), cpu: 10.0, uptime: 3600, ..Default::default() };
        let r = avaliar(&s, None, None);
        assert!(r.nota < 65, "nota {}", r.nota);
        assert!(r.itens.iter().any(|i| matches!(i.botao, Some((Botao::AliviarMemoria, _)))));
        assert_eq!(r.itens[0].nivel, Nivel::Ruim);
    }
}
