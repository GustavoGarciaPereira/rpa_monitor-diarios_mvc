use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use tracing::{debug, info, warn};

use super::Source;
use crate::config::ProcessoMonitorado;
use crate::error::{AppError, Result};
use crate::model::RawGazette;

/// API publica do DataJud (CNJ). Detalhes em `docs/datajud-findings.md`.
pub const BASE_URL: &str = "https://api-publica.datajud.cnj.jus.br";

/// Cada consulta leva 20-35 s do lado do servidor; 120 s cobre com folga.
const TIMEOUT_SECS: u64 = 120;

/// Pausa entre processos. O servico e publico e compartilhado; cada consulta
/// ja e lenta, entao paralelizar so aumenta a carga sem ganho real.
const PAUSE_BETWEEN_PROCESSOS: Duration = Duration::from_secs(2);

/// Tribunais com indice confirmado na API. STF **nao** esta no DataJud.
pub const TRIBUNAIS_SUPORTADOS: &[&str] = &["tjsp", "tjrj", "tjrs", "trf4", "stj"];

/// Fonte de processos especificos no DataJud.
///
/// Diferente das outras fontes, aqui nao se busca por palavra-chave: o usuario
/// lista os processos que quer acompanhar e cada movimento vira uma publicacao.
pub struct DataJudSource {
    client: reqwest::Client,
    api_key: String,
    processos: Vec<ProcessoMonitorado>,
}

impl DataJudSource {
    pub fn new(api_key: String, processos: Vec<ProcessoMonitorado>) -> Result<Self> {
        Ok(Self {
            client: build_datajud_client()?,
            api_key: normalize_api_key(&api_key),
            processos,
        })
    }

    async fn buscar(&self, processo: &ProcessoMonitorado) -> Result<Vec<RawGazette>> {
        let tribunal = processo.tribunal.trim().to_lowercase();
        let url = format!("{BASE_URL}/api_publica_{tribunal}/_search");

        let body = serde_json::json!({
            "size": 1,
            "query": { "match": { "numeroProcesso": processo.numero.trim() } }
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("APIKey {}", self.api_key.trim()))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();

        // 401/403 nao e "processo indisponivel": e chave errada. Abortar o lote
        // evita 50 timeouts seguidos por causa de configuracao.
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let corpo = response.text().await.unwrap_or_default();
            return Err(AppError::Config(format!(
                "DataJud recusou a API key (HTTP {}): {}",
                status.as_u16(),
                truncate(&corpo, 200)
            )));
        }

        if !status.is_success() {
            let corpo = response.text().await.unwrap_or_default();
            return Err(AppError::Parse(format!(
                "HTTP {} em {url}: {}",
                status.as_u16(),
                truncate(&corpo, 200)
            )));
        }

        let corpo = response.text().await?;
        debug!(bytes = corpo.len(), inicio = %truncate(&corpo, 200), "resposta DataJud");
        gazettes_from_body(&corpo, processo)
    }
}

#[async_trait]
impl Source for DataJudSource {
    fn name(&self) -> &'static str {
        "datajud"
    }

    /// `since`/`until`/`keywords` nao sao usados: o recorte aqui e a lista de
    /// processos do config, e a novidade e garantida pelo `external_id` por
    /// movimento. Filtrar por data esconderia movimento antigo visto agora.
    async fn fetch(
        &self,
        _since: NaiveDate,
        _until: NaiveDate,
        _keywords: &[String],
        _territories: &[String],
    ) -> Result<Vec<RawGazette>> {
        if self.processos.is_empty() {
            return Ok(Vec::new());
        }
        if self.api_key.trim().is_empty() {
            return Err(AppError::Config(
                "datajud_api_key vazia: configure `datajud_api_key` ou a env DATAJUD_API_KEY".into(),
            ));
        }

        let mut out = Vec::new();

        for (index, processo) in self.processos.iter().enumerate() {
            let tribunal = processo.tribunal.trim().to_lowercase();

            if !TRIBUNAIS_SUPORTADOS.contains(&tribunal.as_str()) {
                warn!(
                    tribunal = %tribunal,
                    numero = %processo.numero,
                    "tribunal sem indice no DataJud (STF nao esta na API); processo ignorado"
                );
                continue;
            }

            info!(
                "processo {}/{}: {} {} (aguardando...)",
                index + 1,
                self.processos.len(),
                tribunal.to_uppercase(),
                processo.numero
            );

            match self.buscar(processo).await {
                Ok(mut gazettes) => {
                    info!(
                        numero = %processo.numero,
                        tribunal = %tribunal,
                        movimentos = gazettes.len(),
                        "processo consultado"
                    );
                    out.append(&mut gazettes);
                }
                // Configuracao errada nao melhora na proxima iteracao.
                Err(err @ AppError::Config(_)) => return Err(err),
                Err(err) => {
                    warn!(
                        numero = %processo.numero,
                        tribunal = %tribunal,
                        error = %err,
                        "processo ignorado nesta rodada"
                    );
                }
            }

            if index + 1 < self.processos.len() {
                tokio::time::sleep(PAUSE_BETWEEN_PROCESSOS).await;
            }
        }

        Ok(out)
    }
}

/// Aceita a chave com ou sem o prefixo `APIKey `.
///
/// O exemplo da wiki do CNJ mostra `APIKey <chave>`; se o usuario colar isso
/// no config e o cliente prefixar de novo, o header vira `APIKey APIKey ...` e
/// a API devolve 401.
fn normalize_api_key(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.to_lowercase().starts_with("apikey ") {
        trimmed[7..].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn build_datajud_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .user_agent(concat!("monitor-diarios/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(AppError::from)
}

// ---------------------------------------------------------------------------
// Contrato da API (ver docs/datajud-findings.md)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DataJudResponse {
    /// O Elasticsearch devolve `200` com `timed_out: true` quando a consulta
    /// estoura o tempo — nesse caso `hits.total` vem 0 e NAO significa
    /// "processo inexistente". Tratar como erro evita falso negativo.
    #[serde(default)]
    timed_out: bool,
    hits: Hits,
}

#[derive(Debug, Deserialize)]
struct Hits {
    total: HitTotal,
    #[serde(default)]
    hits: Vec<SourceWrapper>,
}

#[derive(Debug, Deserialize)]
struct HitTotal {
    #[serde(default)]
    value: u32,
}

#[derive(Debug, Deserialize)]
struct SourceWrapper {
    #[serde(rename = "_source")]
    source: ProcessoSource,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessoSource {
    id: String,
    tribunal: String,
    grau: String,
    numero_processo: String,
    #[serde(default)]
    nivel_sigilo: u32,
    #[serde(default)]
    data_ajuizamento: String,
    #[serde(default)]
    movimentos: Vec<Movimento>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Movimento {
    codigo: u32,
    nome: String,
    data_hora: DateTime<Utc>,
    #[serde(default)]
    complementos_tabelados: Vec<Complemento>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Complemento {
    #[serde(default)]
    nome: String,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Converte a resposta de `_search` em publicacoes (uma por movimento).
///
/// `hits.total == 0` nao e erro: o processo simplesmente nao esta no indice.
fn gazettes_from_body(body: &str, processo: &ProcessoMonitorado) -> Result<Vec<RawGazette>> {
    let parsed: DataJudResponse = serde_json::from_str(body)
        .map_err(|e| AppError::Parse(format!("JSON inesperado do DataJud: {e}")))?;

    if parsed.timed_out {
        return Err(AppError::Unavailable {
            attempts: 1,
            reason: "DataJud reportou timed_out; resultado incompleto, tentar de novo".into(),
        });
    }

    if parsed.hits.total.value == 0 || parsed.hits.hits.is_empty() {
        debug!(numero = %processo.numero, "processo nao encontrado no indice");
        return Ok(Vec::new());
    }

    let source = parsed
        .hits
        .hits
        .into_iter()
        .next()
        .map(|wrapper| wrapper.source)
        .ok_or_else(|| AppError::Parse("resposta sem _source".into()))?;

    // `dataAjuizamento` NAO serve como data da publicacao: cada movimento tem
    // a sua. O campo so e validado; se vier torto, seguimos com a data do
    // movimento em vez de derrubar o processo.
    if let Err(err) = parse_data_ajuizamento(&source.data_ajuizamento) {
        warn!(
            numero = %source.numero_processo,
            error = %err,
            "dataAjuizamento invalido; usando a data de cada movimento"
        );
    }

    // A ordem do JSON nao e confiavel: TJSP/TJRS vem decrescente, STJ crescente.
    let mut movimentos = source.movimentos.clone();
    movimentos.sort_by(|a, b| a.data_hora.cmp(&b.data_hora));

    let territorio = format!("{}:{}", source.tribunal, source.grau);
    let apelido = processo.apelido.as_deref().unwrap_or("").trim().to_string();
    let nome = if apelido.is_empty() {
        processo.numero.clone()
    } else {
        apelido.clone()
    };
    let territory_name = format!("{} — {}", source.tribunal, nome);

    let gazettes = movimentos
        .iter()
        .map(|movimento| movement_to_raw(&source, movimento, &territorio, &territory_name, &apelido))
        .collect();

    Ok(gazettes)
}

fn movement_to_raw(
    source: &ProcessoSource,
    movimento: &Movimento,
    territorio: &str,
    territory_name: &str,
    apelido: &str,
) -> RawGazette {
    // Um movimento = uma publicacao. Sem isso, movimento novo no mesmo processo
    // seria deduplicado contra o processo inteiro e nunca alertaria.
    let external_id = format!(
        "{}:{}:{}",
        source.id,
        movimento.codigo,
        movimento.data_hora.to_rfc3339()
    );

    let base = if source.nivel_sigilo != 0 {
        format!("[PROCESSO SIGILOSO] {}", movimento.nome)
    } else {
        describe_movimento(movimento)
    };
    // O apelido do config e contexto, nao classificacao: vai no texto, nao no
    // badge (que passa a mostrar o tipo de movimento).
    let excerpt = if apelido.is_empty() {
        base
    } else {
        format!("[{apelido}] {base}")
    };

    RawGazette {
        external_id,
        territory_id: territorio.to_string(),
        territory_name: territory_name.to_string(),
        state_code: "BR".to_string(),
        // Cada movimento tem a sua data; a de ajuizamento e so do processo.
        published_at: movimento.data_hora.date_naive(),
        // A UI troca esta URL pela consulta publica do tribunal (ver serve.rs).
        url: format!(
            "{BASE_URL}/api_publica_{}/_search",
            source.tribunal.to_lowercase()
        ),
        txt_url: None,
        edition: None,
        is_extra_edition: false,
        excerpts: vec![excerpt],
        matched_keyword: movimento.nome.clone(),
    }
}

fn describe_movimento(movimento: &Movimento) -> String {
    let mut out = movimento.nome.clone();
    let complementos: Vec<&str> = movimento
        .complementos_tabelados
        .iter()
        .map(|c| c.nome.trim())
        .filter(|nome| !nome.is_empty())
        .collect();

    if !complementos.is_empty() {
        out.push_str(" — ");
        out.push_str(&complementos.join(", "));
    }
    out
}

/// `dataAjuizamento` vem como `YYYYMMDDHHMMSS` (nao e ISO 8601).
fn parse_data_ajuizamento(raw: &str) -> Result<NaiveDate> {
    let raw = raw.trim();
    let dia: String = raw.chars().take(8).collect();

    if dia.chars().count() != 8 {
        return Err(AppError::Parse(format!(
            "dataAjuizamento invalido: {raw:?} (esperado YYYYMMDDHHMMSS)"
        )));
    }

    NaiveDate::parse_from_str(&dia, "%Y%m%d")
        .map_err(|e| AppError::Parse(format!("dataAjuizamento invalido: {raw} — {e}")))
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resposta real capturada do TJSP (83 movimentos) em 2026-10-19.
    const REAL: &str = include_str!("../../tests/fixtures/datajud_tjsp.json");

    fn processo_teste() -> ProcessoMonitorado {
        ProcessoMonitorado {
            numero: "10047296420238260481".to_string(),
            tribunal: "tjsp".to_string(),
            apelido: Some("Caso Epitácio".to_string()),
        }
    }

    fn body_sintetico(movimentos: &[(u32, &str, &str)], nivel_sigilo: u32) -> String {
        let movs: Vec<String> = movimentos
            .iter()
            .map(|(codigo, nome, data)| {
                format!(r#"{{"codigo":{codigo},"nome":"{nome}","dataHora":"{data}"}}"#)
            })
            .collect();

        format!(
            r#"{{"hits":{{"total":{{"value":1}},"hits":[{{"_source":{{
                "id":"TJSP_G1_123","tribunal":"TJSP","grau":"G1",
                "numeroProcesso":"123","nivelSigilo":{nivel_sigilo},
                "dataAjuizamento":"20231025114738","movimentos":[{}]}}}}]}}}}"#,
            movs.join(",")
        )
    }

    #[test]
    fn test_parse_datajud_real_response() {
        let gazettes = gazettes_from_body(REAL, &processo_teste()).unwrap();

        assert_eq!(gazettes.len(), 83, "o fixture real tem 83 movimentos");
        assert_eq!(gazettes[0].published_at.to_string(), "2023-10-25");
        assert_eq!(
            gazettes.last().unwrap().published_at.to_string(),
            "2026-10-01",
            "cada movimento usa a propria dataHora, nao a de ajuizamento"
        );
        let datas: std::collections::BTreeSet<_> =
            gazettes.iter().map(|g| g.published_at).collect();
        assert!(datas.len() > 1, "as datas devem variar entre os movimentos");
        assert!(gazettes
            .iter()
            .all(|g| g.external_id.starts_with("TJSP_G1_10047296420238260481:")));
        assert!(gazettes.iter().all(|g| g.excerpts.iter().all(|e| !e.is_empty())));
        assert_eq!(gazettes[0].territory_id, "TJSP:G1");
        assert_eq!(gazettes[0].territory_name, "TJSP — Caso Epitácio");
    }

    #[test]
    fn test_sort_movimentos() {
        let crescente = body_sintetico(
            &[
                (1, "A", "2023-01-01T00:00:00.000Z"),
                (2, "B", "2023-06-01T00:00:00.000Z"),
                (3, "C", "2023-12-01T00:00:00.000Z"),
            ],
            0,
        );
        let decrescente = body_sintetico(
            &[
                (3, "C", "2023-12-01T00:00:00.000Z"),
                (2, "B", "2023-06-01T00:00:00.000Z"),
                (1, "A", "2023-01-01T00:00:00.000Z"),
            ],
            0,
        );

        let a = gazettes_from_body(&crescente, &processo_teste()).unwrap();
        let d = gazettes_from_body(&decrescente, &processo_teste()).unwrap();

        let ids_a: Vec<_> = a.iter().map(|g| g.external_id.clone()).collect();
        let ids_d: Vec<_> = d.iter().map(|g| g.external_id.clone()).collect();

        assert_eq!(ids_a, ids_d, "a ordem de entrada nao pode mudar a saida");
        assert!(ids_a[0].ends_with("2023-01-01T00:00:00+00:00"));
        assert!(ids_a[2].ends_with("2023-12-01T00:00:00+00:00"));
    }

    #[test]
    fn test_external_id_per_movement() {
        let body = body_sintetico(
            &[
                (1, "A", "2023-01-01T00:00:00.000Z"),
                (2, "B", "2023-02-01T00:00:00.000Z"),
                (3, "C", "2023-03-01T00:00:00.000Z"),
            ],
            0,
        );
        let gazettes = gazettes_from_body(&body, &processo_teste()).unwrap();

        assert_eq!(gazettes.len(), 3);
        let mut ids: Vec<_> = gazettes.iter().map(|g| g.external_id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3, "cada movimento precisa de um external_id");
    }

    #[test]
    fn test_sigiloso() {
        let body = body_sintetico(&[(1, "Remessa", "2023-01-01T00:00:00.000Z")], 1);
        let gazettes = gazettes_from_body(&body, &processo_teste()).unwrap();

        assert_eq!(gazettes.len(), 1);
        assert!(
            gazettes[0].excerpts[0].contains("[PROCESSO SIGILOSO]"),
            "veio: {:?}",
            gazettes[0].excerpts[0]
        );
        assert!(gazettes[0].excerpts[0].contains("Remessa"));
    }

    #[test]
    fn badge_mostra_o_movimento_e_apelido_vai_no_texto() {
        let body = body_sintetico(&[(1, "Recebimento", "2023-01-01T00:00:00.000Z")], 0);
        let gazettes = gazettes_from_body(&body, &processo_teste()).unwrap();

        assert_eq!(gazettes[0].matched_keyword, "Recebimento");
        assert!(
            gazettes[0].excerpts[0].starts_with("[Caso Epitácio]"),
            "veio: {:?}",
            gazettes[0].excerpts[0]
        );
    }

    #[test]
    fn test_processo_nao_encontrado() {
        let body = r#"{"hits":{"total":{"value":0},"hits":[]}}"#;
        let gazettes = gazettes_from_body(body, &processo_teste()).unwrap();
        assert!(gazettes.is_empty());
    }

    #[test]
    fn chave_aceita_com_e_sem_prefixo() {
        let chave = "cDZHYzlZa0JadVREZDJCendQbXY6SkJlTzNjLV9TRENyQk1RdnFKZGRQdw==";
        assert_eq!(normalize_api_key(chave), chave);
        assert_eq!(normalize_api_key(&format!("APIKey {chave}")), chave);
        assert_eq!(normalize_api_key(&format!("  apikey   {chave}  ")), chave);
        assert_eq!(normalize_api_key(""), "");
    }

    #[test]
    fn test_timed_out_nao_e_processo_inexistente() {
        // ES devolve 200 com timed_out=true e total 0; nao pode virar "nao achei".
        let body = r#"{"timed_out":true,"hits":{"total":{"value":0},"hits":[]}}"#;
        let err = gazettes_from_body(body, &processo_teste()).unwrap_err();
        assert!(matches!(err, AppError::Unavailable { .. }), "veio: {err:?}");
    }

    #[test]
    fn test_parse_data_ajuizamento() {
        assert_eq!(
            parse_data_ajuizamento("20231025114738").unwrap().to_string(),
            "2023-10-25"
        );
        assert_eq!(
            parse_data_ajuizamento("20231025114738").unwrap(),
            NaiveDate::from_ymd_opt(2023, 10, 25).unwrap()
        );
        assert!(parse_data_ajuizamento("2023").is_err());
        assert!(parse_data_ajuizamento("").is_err());
    }
}
