use async_trait::async_trait;
use chrono::NaiveDate;
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::Source;
use crate::error::{AppError, Result};
use crate::model::RawGazette;

/// Host canonico. `queridodiario.ok.org.br/api/*` responde 302 para ca,
/// mas ir direto evita um round-trip e o redirect do Cloudflare.
pub const BASE_URL: &str = "https://api.queridodiario.org.br";

const MAX_ATTEMPTS: u32 = 4;
const HTTP_TIMEOUT_SECS: u64 = 90;

/// Fonte Querido Diario (https://queridodiario.org.br).
///
/// Notas de campo, verificadas contra a API real (v0.19.0):
/// * o filtro textual e `querystring`, **nao** `keywords` — `keywords` e
///   ignorado em silencio e a API devolve o pais inteiro com HTTP 200;
/// * a resposta traz `date`, `excerpts: [..]` e **nao** traz `external_id`;
/// * `size` acima de 200 nao ajuda; a paginacao e por `offset`;
/// * a API oscila (timeouts e 520 do Cloudflare), por isso o retry.
pub struct QueridoDiario {
    client: reqwest::Client,
    page_size: usize,
    max_results_per_keyword: usize,
}

impl QueridoDiario {
    pub fn new(page_size: usize, max_results_per_keyword: usize) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .user_agent(concat!(
                "monitor-diarios/",
                env!("CARGO_PKG_VERSION"),
                " (+https://queridodiario.org.br)"
            ))
            .build()?;

        Ok(Self {
            client,
            page_size: page_size.clamp(1, 200),
            max_results_per_keyword: max_results_per_keyword.max(1),
        })
    }

    /// Uma consulta por keyword.
    ///
    /// Fazer OR de todas as keywords numa consulta so seria mais barato, mas a
    /// resposta nao diz qual termo casou — e `matched_keyword` ficaria chutado.
    async fn search(
        &self,
        keyword: &str,
        since: NaiveDate,
        until: NaiveDate,
        territories: &[String],
        offset: usize,
    ) -> Result<GazetteSearchResponse> {
        let url = format!("{BASE_URL}/gazettes");
        let mut params: Vec<(&str, String)> = vec![
            ("querystring", keyword.to_string()),
            ("published_since", since.to_string()),
            ("published_until", until.to_string()),
            ("size", self.page_size.to_string()),
            ("offset", offset.to_string()),
            ("sort_by", "descending_date".to_string()),
            ("excerpt_size", "800".to_string()),
            ("number_of_excerpts", "3".to_string()),
        ];
        for territory in territories {
            params.push(("territory_ids", territory.clone()));
        }

        let response =
            crate::http::get_with_retry(&self.client, &url, &params, MAX_ATTEMPTS).await?;
        let status = response.status();

        if status.is_success() {
            let body = response.text().await?;
            return serde_json::from_str::<GazetteSearchResponse>(&body).map_err(|e| {
                AppError::Parse(format!(
                    "JSON inesperado: {e}; corpo: {}",
                    truncate(&body, 300)
                ))
            });
        }

        let body = response.text().await.unwrap_or_default();
        Err(AppError::Parse(format!(
            "HTTP {} para {url} (keyword={keyword}); corpo: {}",
            status.as_u16(),
            truncate(&body, 300)
        )))
    }
}

#[async_trait]
impl Source for QueridoDiario {
    fn name(&self) -> &'static str {
        "querido_diario"
    }

    async fn fetch(
        &self,
        since: NaiveDate,
        until: NaiveDate,
        keywords: &[String],
        territories: &[String],
    ) -> Result<Vec<RawGazette>> {
        if keywords.is_empty() {
            return Err(AppError::Config("nenhuma keyword para consultar".into()));
        }

        let mut out = Vec::new();

        for keyword in keywords {
            let mut offset = 0usize;
            let mut collected_this_keyword = 0usize;
            let mut total_reported: usize;

            loop {
                let page = self.search(keyword, since, until, territories, offset).await?;
                total_reported = page.total_gazettes as usize;

                let returned = page.gazettes.len();
                collected_this_keyword += returned;

                for item in page.gazettes {
                    out.push(item.into_raw(keyword));
                }

                offset += returned;
                debug!(
                    keyword,
                    offset, returned, total = total_reported, "pagina coletada"
                );

                if returned == 0
                    || offset >= total_reported
                    || collected_this_keyword >= self.max_results_per_keyword
                {
                    break;
                }
            }

            if total_reported > self.max_results_per_keyword {
                warn!(
                    keyword,
                    total = total_reported,
                    limite = self.max_results_per_keyword,
                    "resultado truncado: refine a keyword ou reduza a janela"
                );
            }

            info!(keyword, hits = collected_this_keyword, "keyword consultada");
        }

        Ok(out)
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

// ---------------------------------------------------------------------------
// Contrato da API (v0.19.0). Fonte: https://api.queridodiario.org.br/openapi.json
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GazetteSearchResponse {
    #[serde(default)]
    pub total_gazettes: u64,
    #[serde(default)]
    pub gazettes: Vec<GazetteItem>,
}

#[derive(Debug, Deserialize)]
pub struct GazetteItem {
    pub territory_id: String,
    pub date: NaiveDate,
    pub url: String,
    #[serde(default)]
    pub territory_name: String,
    #[serde(default)]
    pub state_code: String,
    #[serde(default)]
    pub excerpts: Vec<String>,
    #[serde(default)]
    pub edition: Option<String>,
    #[serde(default)]
    pub is_extra_edition: Option<bool>,
    #[serde(default)]
    pub txt_url: Option<String>,
}

impl GazetteItem {
    fn into_raw(self, keyword: &str) -> RawGazette {
        let edition = self.edition.filter(|e| !e.trim().is_empty());
        RawGazette {
            // A API nao expoe id proprio; a URL do arquivo e unica e estavel.
            external_id: self.url.clone(),
            territory_id: self.territory_id,
            territory_name: self.territory_name,
            state_code: self.state_code,
            published_at: self.date,
            url: self.url,
            txt_url: self.txt_url,
            edition,
            is_extra_edition: self.is_extra_edition.unwrap_or(false),
            excerpts: self.excerpts,
            matched_keyword: keyword.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resposta real capturada da API (Curitiba, 2026-10-14, querystring=licitacao).
    const FIXTURE: &str = r#"{
        "total_gazettes": 3,
        "gazettes": [
            {
                "territory_id": "4106902",
                "date": "2026-10-14",
                "scraped_at": "2026-10-16T02:47:19.047064",
                "url": "https://data.queridodiario.ok.org.br/4106902/2026-10-14/48a7f3c553810d14ae2ed2a90b7bf3dbd78f287f.pdf",
                "territory_name": "Curitiba",
                "state_code": "PR",
                "excerpts": ["\n\u2022\n\u2022\n\nLicitações\n\nSECRETARIA DO GOVERNO"],
                "edition": "191",
                "is_extra_edition": false,
                "txt_url": "https://data.queridodiario.ok.org.br/4106902/2026-10-14/48a7f3c553810d14ae2ed2a90b7bf3dbd78f287f.txt"
            },
            {
                "territory_id": "3550308",
                "date": "2026-10-15",
                "scraped_at": "2026-10-16T02:47:23.496365",
                "url": "https://data.queridodiario.ok.org.br/3550308/2026-10-15/aaa.pdf",
                "territory_name": "São Paulo",
                "state_code": "SP",
                "excerpts": [],
                "edition": "",
                "is_extra_edition": null,
                "txt_url": null
            }
        ]
    }"#;

    #[test]
    fn parseia_resposta_real_da_api() {
        let parsed: GazetteSearchResponse = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(parsed.total_gazettes, 3);
        assert_eq!(parsed.gazettes.len(), 2);

        let first = &parsed.gazettes[0];
        assert_eq!(first.territory_id, "4106902");
        assert_eq!(first.date.to_string(), "2026-10-14");
        assert_eq!(first.is_extra_edition, Some(false));
        assert_eq!(first.edition.as_deref(), Some("191"));
    }

    #[test]
    fn edition_vazia_e_null_viram_none() {
        let parsed: GazetteSearchResponse = serde_json::from_str(FIXTURE).unwrap();
        let raw = parsed.gazettes.into_iter().nth(1).unwrap().into_raw("licitacao");
        assert_eq!(raw.edition, None);
        assert!(!raw.is_extra_edition);
        assert_eq!(raw.external_id, raw.url);
        assert_eq!(raw.published_at.to_string(), "2026-10-15");
    }

    #[test]
    fn extrai_excerto_limpo_do_pdf() {
        let parsed: GazetteSearchResponse = serde_json::from_str(FIXTURE).unwrap();
        let raw = parsed.gazettes.into_iter().next().unwrap().into_raw("licitacao");
        let excerpt = raw.best_excerpt(120);
        assert!(excerpt.starts_with("Licitações"), "excerpt: {excerpt:?}");
        assert!(!excerpt.contains('\u{2022}'));
    }

    #[test]
    fn keyword_vazia_falha_cedo() {
        // Guarda de sanidade: sem keyword o `querystring` viria vazio e a API
        // devolveria tudo (sem excerpts), quebrando o filtro silenciosamente.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let source = QueridoDiario::new(10, 10).unwrap();
        let err = rt
            .block_on(source.fetch(
                NaiveDate::from_ymd_opt(2026, 10, 10).unwrap(),
                NaiveDate::from_ymd_opt(2026, 10, 17).unwrap(),
                &[],
                &[],
            ))
            .unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
    }
}
