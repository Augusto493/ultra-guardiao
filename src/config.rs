//! Configuração em %LOCALAPPDATA%\UltraGuardiao\config.json. Tudo tem padrão sensato.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Ações automáticas ligadas (aliviar, baixar prioridade, fechar órfãos).
    pub automatico: bool,
    /// % de RAM em uso para entrar em "Atenção" (alivia memória de programas parados).
    pub limite_atencao: f32,
    /// % de RAM em uso para entrar em "Crítico" (também fecha órfãos parados).
    pub limite_critico: f32,
    /// % de CPU sustentada para baixar a prioridade de quem está pesando em segundo plano.
    pub limite_cpu: f32,
    /// Nunca mexer nestes (nome do .exe, sem diferenciar maiúsculas).
    pub protegidos: Vec<String>,
    /// Podem ser fechados quando ficam órfãos (o programa que os abriu já fechou) e parados.
    pub orfaos_fechaveis: Vec<String>,
    /// Sugerir reiniciar depois de tantos dias ligado.
    pub dias_para_reiniciar: u32,
    /// Dias de caixa-preta guardados.
    pub dias_caixa_preta: u32,
    /// Análise com IA (Codex) ligada.
    pub ia_ativa: bool,
    /// Modelo para analisar travamentos e pedidos manuais.
    pub ia_modelo: String,
    /// Modelo barato para análises automáticas de rotina (depois de um Crítico).
    pub ia_modelo_rapido: String,
    /// Análises sem clique (rotina, após travamento total ou ao sair do Crítico). Desligado por padrão: envia dados do PC.
    pub ia_automatica: bool,
    /// Com `ia_automatica`, a cada quantas horas a IA faz uma análise de rotina (0 = sem rotina).
    pub ia_intervalo_horas: u32,
    /// Aplica sozinha as ações seguras sugeridas pela IA (proteger, ajustar limites, faxina sem admin). Desligado por padrão.
    pub ia_aplicar_auto: bool,
    /// Garante que o Guardião abra com o Windows (supervisionado, reinicia se cair).
    pub iniciar_com_windows: bool,
    /// Sem notificações do Windows: tudo vai para o registro e a caixa-preta.
    pub silencioso: bool,
}

/// Programas do dia a dia que o Guardião nunca fecha, nunca "alivia" e nunca desacelera.
const PROTEGIDOS_PADRAO: &[&str] = &[
    "explorer.exe", "dwm.exe", "csrss.exe", "winlogon.exe", "lsass.exe", "services.exe",
    "smss.exe", "wininit.exe", "svchost.exe", "MsMpEng.exe", "NisSrv.exe", "audiodg.exe", "fontdrvhost.exe",
    "sihost.exe", "ctfmon.exe", "UltraGuardiao.exe", "System", "Registry", "Memory Compression",
    "taskhostw.exe", "RuntimeBroker.exe", "ShellExperienceHost.exe", "StartMenuExperienceHost.exe",
    "SearchHost.exe", "TextInputHost.exe", "conhost.exe", "dllhost.exe", "WmiPrvSE.exe", "spoolsv.exe",
    "WhatsApp.exe", "WhatsApp.Root.exe", "WhatsAppDesktop.exe", "chrome.exe", "msedge.exe", "msedgewebview2.exe",
    "firefox.exe", "Teams.exe", "ms-teams.exe", "Zoom.exe", "Discord.exe", "Telegram.exe", "Spotify.exe",
    "OneDrive.exe", "Code.exe", "WindowsTerminal.exe", "OBS.exe", "obs64.exe",
];

impl Default for Config {
    fn default() -> Self {
        Self {
            automatico: true,
            limite_atencao: 85.0,
            limite_critico: 92.0,
            limite_cpu: 85.0,
            protegidos: PROTEGIDOS_PADRAO.iter().map(|s| s.to_string()).collect(),
            orfaos_fechaveis: [
                "node.exe", "claude.exe", "codex.exe", "llama-server.exe", "ollama_llama_server.exe",
                "esbuild.exe", "rg.exe",
            ]
            .map(String::from)
            .to_vec(),
            dias_para_reiniciar: 3,
            dias_caixa_preta: 7,
            ia_ativa: true,
            ia_modelo: "gpt-6-luna".into(),
            ia_modelo_rapido: "gpt-5.6-luna".into(),
            ia_automatica: false,
            ia_intervalo_horas: 3,
            ia_aplicar_auto: false,
            iniciar_com_windows: true,
            silencioso: true,
        }
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join("UltraGuardiao");
    let _ = std::fs::create_dir_all(&d);
    d
}

impl Config {
    pub fn load() -> Self {
        let path = dir().join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                let mut c: Self = serde_json::from_str(&s).unwrap_or_default();
                // Configs antigas ganham os protegidos novos (WhatsApp, Chrome…) sem perder os do usuário.
                let before = c.protegidos.len();
                for p in PROTEGIDOS_PADRAO {
                    if !c.is_protected(p) { c.protegidos.push((*p).to_string()); }
                }
                if c.protegidos.len() != before { c.save(); }
                c
            }
            Err(_) => {
                let c = Self::default();
                c.save();
                c
            }
        }
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(dir().join("config.json"), s);
        }
    }
    pub fn is_protected(&self, name: &str) -> bool {
        self.protegidos.iter().any(|p| p.eq_ignore_ascii_case(name))
    }
    pub fn orphan_ok(&self, name: &str) -> bool {
        self.orfaos_fechaveis.iter().any(|p| p.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ia_sem_clique_desligada_por_padrao() {
        for c in [Config::default(), serde_json::from_str::<Config>("{}").expect("config vazia")] {
            assert!(!c.ia_automatica);
            assert!(!c.ia_aplicar_auto);
            assert!(c.ia_ativa, "o botão manual continua funcionando");
        }
    }

    #[test]
    fn config_existente_mantem_os_valores() {
        let c: Config = serde_json::from_str(r#"{"ia_automatica":true,"ia_aplicar_auto":true}"#).expect("config");
        assert!(c.ia_automatica && c.ia_aplicar_auto);
    }
}
