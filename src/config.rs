use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// Configuracao do monitor.
///
/// Precedencia: `config.toml` < variaveis de ambiente.
/// Segredos (Telegram) devem vir **somente** do ambiente.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Palavras-chave monitoradas. Viram `querystring` na fonte.
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Ids IBGE de 7 digitos. Vazio = busca nacional.
    #[serde(default)]
    pub territories: Vec<String>,

    /// URL do SQLite, ex.: `sqlite:monitor.db`.
    #[serde(default = "default_database_url")]
    pub database_url: String,

    /// Reservado para o dia 3; hoje apenas logamos.
    #[serde(default)]
    pub telegram_token: Option<String>,

    #[serde(default)]
    pub telegram_chat_id: Option<String>,

    /// Itens por pagina na fonte (a API aceita ate 200).
    #[serde(default = "default_page_size")]
    pub page_size: usize,

    /// Teto de itens por keyword, para nao varrer o pais inteiro por engano.
    #[serde(default = "default_max_results")]
    pub max_results_per_keyword: usize,
}

fn default_database_url() -> String {
    "sqlite:monitor.db".to_string()
}

fn default_page_size() -> usize {
    200
}

fn default_max_results() -> usize {
    2000
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keywords: Vec::new(),
            territories: Vec::new(),
            database_url: default_database_url(),
            telegram_token: None,
            telegram_chat_id: None,
            page_size: default_page_size(),
            max_results_per_keyword: default_max_results(),
        }
    }
}

impl Config {
    /// Carrega de um arquivo TOML (ou usa os defaults, se o arquivo nao existir),
    /// aplica overrides de ambiente e valida.
    pub fn load(path: &Path) -> Result<Self> {
        let mut config = if path.exists() {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| AppError::Config(format!("lendo {}: {e}", path.display())))?;
            Self::from_toml(&raw)
                .map_err(|e| AppError::Config(format!("TOML invalido em {}: {e}", path.display())))?
        } else {
            Self::default()
        };

        config.apply_env();
        config.validate()?;
        Ok(config)
    }

    /// Parser puro, sem tocar em ambiente — facilita teste.
    pub fn from_toml(raw: &str) -> std::result::Result<Self, toml::de::Error> {
        toml::from_str(raw)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("DATABASE_URL") {
            if !v.trim().is_empty() {
                self.database_url = v;
            }
        }
        if let Ok(v) = std::env::var("TELEGRAM_TOKEN") {
            if !v.trim().is_empty() {
                self.telegram_token = Some(v);
            }
        }
        if let Ok(v) = std::env::var("TELEGRAM_CHAT_ID") {
            if !v.trim().is_empty() {
                self.telegram_chat_id = Some(v);
            }
        }
        if let Ok(v) = std::env::var("MONITOR_KEYWORDS") {
            self.keywords = split_list(&v);
        }
        if let Ok(v) = std::env::var("MONITOR_TERRITORIES") {
            self.territories = split_list(&v);
        }
    }

    fn validate(&self) -> Result<()> {
        if self.keywords.is_empty() {
            return Err(AppError::Config(
                "nenhuma keyword configurada: preencha `keywords` no config.toml \
                 ou a env MONITOR_KEYWORDS"
                    .into(),
            ));
        }
        if self.keywords.iter().any(|k| k.trim().is_empty()) {
            return Err(AppError::Config(
                "keyword vazia na configuracao; remova a entrada".into(),
            ));
        }
        for territory in &self.territories {
            let t = territory.trim();
            if t.len() != 7 || !t.chars().all(|c| c.is_ascii_digit()) {
                return Err(AppError::Config(format!(
                    "territory_id invalido: {territory:?} (esperado 7 digitos IBGE, ex. 4106902)"
                )));
            }
        }
        if self.database_url.trim().is_empty() {
            return Err(AppError::Config("database_url vazia".into()));
        }
        Ok(())
    }
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
keywords = ["licitacao", "pregao eletronico"]
territories = ["4106902", "3304557"]
database_url = "sqlite:teste.db"
"#;

    #[test]
    fn parseia_toml_completo() {
        let cfg = Config::from_toml(SAMPLE).unwrap();
        assert_eq!(cfg.keywords.len(), 2);
        assert_eq!(cfg.territories, vec!["4106902", "3304557"]);
        assert_eq!(cfg.database_url, "sqlite:teste.db");
        assert_eq!(cfg.page_size, default_page_size());
    }

    #[test]
    fn aplica_defaults_em_campos_ausentes() {
        let cfg = Config::from_toml(r#"keywords = ["x"]"#).unwrap();
        assert!(cfg.territories.is_empty());
        assert_eq!(cfg.database_url, "sqlite:monitor.db");
        assert!(cfg.telegram_token.is_none());
    }

    #[test]
    fn rejeita_keywords_vazias() {
        let cfg = Config::from_toml(r#"keywords = []"#).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejeita_territory_id_malformado() {
        let cfg = Config::from_toml(
            r#"
keywords = ["x"]
territories = ["355030"]
"#,
        )
        .unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("territory_id invalido"), "erro foi: {err}");
    }

    #[test]
    fn aceita_frase_como_keyword() {
        let cfg = Config::from_toml(r#"keywords = ["dispensa de licitacao"]"#).unwrap();
        assert!(cfg.validate().is_ok());
    }
}
