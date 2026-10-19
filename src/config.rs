use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// Processo acompanhado no DataJud.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessoMonitorado {
    /// Numero CNJ com 20 digitos, sem pontuacao.
    pub numero: String,
    /// Tribunal em minusculo (`tjsp`, `tjrj`, `tjrs`, `trf4`, `stj`).
    pub tribunal: String,
    /// Nome curto para a UI; ausente usa o numero.
    #[serde(default)]
    pub apelido: Option<String>,
}

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

    /// Entidades do portal da FAMURS (RS). Vazio = nao usa essa fonte.
    #[serde(default)]
    pub famurs_entities: Vec<String>,

    /// Processos acompanhados no DataJud (CNJ).
    #[serde(default)]
    pub processos: Vec<ProcessoMonitorado>,

    /// Chave publica do DataJud. Vazia = erro se houver processos configurados.
    #[serde(default)]
    pub datajud_api_key: String,

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
            famurs_entities: Vec::new(),
            processos: Vec::new(),
            datajud_api_key: String::new(),
            database_url: default_database_url(),
            telegram_token: None,
            telegram_chat_id: None,
            page_size: default_page_size(),
            max_results_per_keyword: default_max_results(),
        }
    }
}

impl Config {
    /// Carrega de um arquivo TOML (ou usa os defaults, se o arquivo nao existir)
    /// e aplica overrides de ambiente.
    ///
    /// Nao valida aqui de proposito: `serve` precisa subir mesmo sem keywords
    /// (no container nao existe `config.toml`). Cada comando chama a validacao
    /// que lhe cabe.
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
        Ok(config)
    }

    /// Regras para coletar: sem keyword nao ha o que buscar.
    pub fn validate_for_scan(&self) -> Result<()> {
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
        for entity in &self.famurs_entities {
            let e = entity.trim();
            if e.is_empty() || !e.chars().all(|c| c.is_ascii_digit()) {
                return Err(AppError::Config(format!(
                    "famurs_entities invalido: {entity:?} (esperado id numerico, ex. 64462)"
                )));
            }
        }
        for processo in &self.processos {
            let numero = processo.numero.trim();
            if numero.chars().count() != 20 || !numero.chars().all(|c| c.is_ascii_digit()) {
                return Err(AppError::Config(format!(
                    "numero de processo invalido: {:?} (esperado 20 digitos, sem pontuacao)",
                    processo.numero
                )));
            }
            if processo.tribunal.trim().is_empty() {
                return Err(AppError::Config(format!(
                    "processo {numero} sem tribunal configurado"
                )));
            }
        }
        self.validate_common()
    }

    /// Servir depende so do banco: a lista de keywords nao e usada pela pagina.
    pub fn validate_for_serve(&self) -> Result<()> {
        self.validate_common()
    }

    fn validate_common(&self) -> Result<()> {
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
        if let Ok(v) = std::env::var("MONITOR_FAMURS_ENTITIES") {
            self.famurs_entities = split_list(&v);
        }
        // Aceita os dois nomes; o prefixado com MONITOR_ tem precedencia.
        if let Ok(v) = std::env::var("DATAJUD_API_KEY") {
            if !v.trim().is_empty() {
                self.datajud_api_key = v;
            }
        }
        if let Ok(v) = std::env::var("MONITOR_DATAJUD_API_KEY") {
            if !v.trim().is_empty() {
                self.datajud_api_key = v;
            }
        }
        if let Ok(v) = std::env::var("MONITOR_PROCESSOS") {
            self.processos = parse_processos_env(&v);
        }
    }
}

/// Le a lista de processos da env.
///
/// Formato: `tribunal:numero[:apelido]`, separado por virgula.
/// `"stj:00011552020248272734:REsp exemplo,tjsp:10047296420238260481"`.
///
/// Entradas invalidas sao ignoradas em vez de derrubar o processo inteiro:
/// uma virgula sobrando nao pode impedir o monitor de subir.
fn parse_processos_env(raw: &str) -> Vec<ProcessoMonitorado> {
    raw.split(',')
        .filter_map(|entry| {
            let parts: Vec<&str> = entry.splitn(3, ':').collect();
            if parts.len() < 2 {
                return None;
            }

            let tribunal = parts[0].trim().to_lowercase();
            let numero = parts[1].trim().to_string();
            if tribunal.is_empty() || numero.is_empty() {
                return None;
            }

            Some(ProcessoMonitorado {
                tribunal,
                numero,
                apelido: parts
                    .get(2)
                    .map(|apelido| apelido.trim().to_string())
                    .filter(|apelido| !apelido.is_empty()),
            })
        })
        .collect()
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
    fn sem_keywords_bloqueia_scan_mas_nao_serve() {
        let cfg = Config::from_toml(r#"keywords = []"#).unwrap();
        assert!(cfg.validate_for_scan().is_err());
        // O container nao carrega config.toml: `serve` tem que subir assim.
        assert!(cfg.validate_for_serve().is_ok());
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
        let err = cfg.validate_for_scan().unwrap_err().to_string();
        assert!(err.contains("territory_id invalido"), "erro foi: {err}");
    }

    #[test]
    fn test_parse_processos_env_uma_entrada() {
        let processos = parse_processos_env("stj:123:apelido");
        assert_eq!(processos.len(), 1);
        assert_eq!(processos[0].tribunal, "stj");
        assert_eq!(processos[0].numero, "123");
        assert_eq!(processos[0].apelido.as_deref(), Some("apelido"));
    }

    #[test]
    fn test_parse_processos_env_varios() {
        let processos = parse_processos_env("stj:111:a,tjsp:222:b");
        assert_eq!(processos.len(), 2);
        assert_eq!(processos[0].numero, "111");
        assert_eq!(processos[1].tribunal, "tjsp");
        assert_eq!(processos[1].numero, "222");
    }

    #[test]
    fn test_parse_processos_env_apelido_opcional() {
        let processos = parse_processos_env("stj:123");
        assert_eq!(processos.len(), 1);
        assert!(processos[0].apelido.is_none());
    }

    #[test]
    fn test_parse_processos_env_vazio() {
        assert!(parse_processos_env("").is_empty());
        // Entradas invalidas sao ignoradas, nao derrubam o resto.
        assert!(parse_processos_env(",,").is_empty());
        assert_eq!(parse_processos_env("lixo,stj:1").len(), 1);
    }

    #[test]
    fn aceita_frase_como_keyword() {
        let cfg = Config::from_toml(r#"keywords = ["dispensa de licitacao"]"#).unwrap();
        assert!(cfg.validate_for_scan().is_ok());
    }
}
