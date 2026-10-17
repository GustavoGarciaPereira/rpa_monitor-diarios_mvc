use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Publicacao crua vinda de uma fonte, ja normalizada para o nosso dominio.
///
/// `matched_keyword` diz **qual** palavra-chave trouxe esta publicacao. Isso e
/// preenchido pela fonte (uma consulta por keyword), e nao por busca no texto:
/// o `querystring` do Querido Diario e um OR, entao o texto nao permite saber
/// qual termo casou.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawGazette {
    /// Identidade da publicacao dentro da fonte. Usamos a URL do PDF:
    /// e estavel entre execucoes e o Querido Diario nao expoe outro id.
    pub external_id: String,
    pub territory_id: String,
    pub territory_name: String,
    pub state_code: String,
    pub published_at: NaiveDate,
    pub url: String,
    pub txt_url: Option<String>,
    pub edition: Option<String>,
    pub is_extra_edition: bool,
    pub excerpts: Vec<String>,
    pub matched_keyword: String,
}

impl RawGazette {
    /// Id deterministico da publicacao no banco: `<source>:<sha256(source|external_id)>`.
    pub fn id(&self, source: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(source.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.external_id.as_bytes());
        format!("{source}:{}", hex::encode(hasher.finalize()))
    }

    /// Hash de deduplicacao do alerta: `sha256(source|external_id|keyword)`.
    ///
    /// A keyword entra normalizada, para que "Licitação" e "licitacao"
    /// nao gerem dois alertas para o mesmo fato.
    pub fn alert_hash(&self, source: &str, keyword: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(source.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.external_id.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(normalize(keyword).as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Melhor trecho para exibir: o que de fato contem a keyword; senao, o maior.
    pub fn best_excerpt(&self, max_chars: usize) -> String {
        let needle = normalize(&self.matched_keyword);
        let chosen = self
            .excerpts
            .iter()
            .filter(|e| !e.trim().is_empty())
            .max_by_key(|e| (normalize(e).contains(&needle), e.chars().count()))
            .map(String::as_str)
            .unwrap_or_default();
        clean_text(chosen, max_chars)
    }
}

/// Dobra um char para comparacao: minusculo e sem acento, sempre 1:1.
///
/// O 1:1 e o que permite mapear posicoes entre o texto original e o dobrado —
/// sem isso, achar a keyword nao daria para voltar ao indice no texto original.
pub fn fold_char(c: char) -> char {
    fold_accent(c.to_lowercase().next().unwrap_or(c))
}

/// Minusculas + remocao de acentos, para comparacao textual.
pub fn normalize(input: &str) -> String {
    input.chars().map(fold_char).collect()
}

fn fold_accent(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

/// Colapsa espacos/quebras de linha e bullet do PDF, e corta em `max_chars`.
pub fn clean_text(input: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(input.len().min(max_chars * 4));
    let mut count = 0usize;
    let mut pending_space = false;

    for ch in input.chars() {
        if ch.is_whitespace() || ch == '\u{2022}' || ch == '\u{2023}' {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
        count += 1;
        if count >= max_chars {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RawGazette {
        RawGazette {
            external_id: "https://data.queridodiario.ok.org.br/4106902/2026-10-14/abc.pdf".into(),
            territory_id: "4106902".into(),
            territory_name: "Curitiba".into(),
            state_code: "PR".into(),
            published_at: NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(),
            url: "https://data.queridodiario.ok.org.br/4106902/2026-10-14/abc.pdf".into(),
            txt_url: None,
            edition: Some("191".into()),
            is_extra_edition: false,
            excerpts: vec!["\n\u{2022}\n\u{2022}\n\nLicitações\n".into()],
            matched_keyword: "licitacao".into(),
        }
    }

    #[test]
    fn normalize_remove_acento_e_caixa() {
        assert_eq!(normalize("Licitação PÚBLICA"), "licitacao publica");
        assert_eq!(normalize("Ação"), "acao");
    }

    #[test]
    fn id_e_deterministico_e_depende_da_fonte() {
        let g = sample();
        assert_eq!(g.id("querido_diario"), g.id("querido_diario"));
        assert_ne!(g.id("querido_diario"), g.id("dou_federal"));
    }

    #[test]
    fn alert_hash_ignora_caixa_e_acento_da_keyword() {
        let g = sample();
        assert_eq!(
            g.alert_hash("querido_diario", "Licitação"),
            g.alert_hash("querido_diario", "licitacao")
        );
        assert_ne!(
            g.alert_hash("querido_diario", "licitacao"),
            g.alert_hash("querido_diario", "contrato")
        );
    }

    #[test]
    fn clean_text_colapsa_bullets_e_corta() {
        let out = clean_text("\n\u{2022}\n\u{2022}\n  Licitações \n do \t municipio ", 100);
        assert_eq!(out, "Licitações do municipio");
        assert_eq!(clean_text("abcdefghij", 4), "abcd");
    }

    #[test]
    fn best_excerpt_prefere_trecho_com_keyword() {
        let mut g = sample();
        g.excerpts = vec![
            "trecho longo sem o termo relevante".into(),
            "abertura de licitação para obra".into(),
        ];
        assert!(g.best_excerpt(200).contains("licitação"));
    }
}
