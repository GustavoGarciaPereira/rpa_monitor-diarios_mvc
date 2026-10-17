use std::time::Duration;

use async_trait::async_trait;
use chrono::NaiveDate;
use scraper::{Html, Selector};
use tracing::{debug, warn};

use super::Source;
use crate::error::{AppError, Result};
use crate::model::{fold_char, normalize, RawGazette};

/// Portal SIGPub da FAMURS — Diario Oficial dos Municipios do RS.
/// Detalhes do reconhecimento em `docs/fontes-rs.md`.
pub const ORIGIN: &str = "https://www.diariomunicipal.com.br";
pub const BASE_URL: &str = "https://www.diariomunicipal.com.br/famurs";

/// Pausa entre requisicoes de materia. Educacao com o portal, nao rate limit.
const REQUEST_PAUSE: Duration = Duration::from_millis(500);

/// Trava de seguranca caso o portal ignore `pagina` e repita a primeira.
const MAX_PAGES: usize = 200;

/// Fonte de um municipio que publica no Diario Oficial dos Municipios do RS.
///
/// `entity_id` e o id da entidade no SIGPub (ex.: `64462` = Prefeitura de Sao
/// Sepe), obtido no select "Municipio (entidade)" da busca do portal.
pub struct FamursSource {
    client: reqwest::Client,
    entity_id: String,
}

impl FamursSource {
    pub fn new(client: reqwest::Client, entity_id: String) -> Self {
        Self { client, entity_id }
    }

    async fn fetch_page(
        &self,
        since: NaiveDate,
        until: NaiveDate,
        page: usize,
    ) -> Result<Listing> {
        let url = format!("{BASE_URL}/pesquisar");
        let params: Vec<(&str, String)> = vec![
            ("busca_avancada[entidade]", self.entity_id.clone()),
            ("busca_avancada[dataInicio]", since.to_string()),
            ("busca_avancada[dataFim]", until.to_string()),
            ("busca_avancada[ordenacao]", "recente".to_string()),
            ("busca_avancada[pagina]", page.to_string()),
        ];

        let response = crate::http::get_with_retry(&self.client, &url, &params, 4).await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(AppError::Parse(format!(
                "HTTP {} na listagem (pagina {page}): {}",
                status.as_u16(),
                truncate(&body, 200)
            )));
        }

        Ok(parse_listing(&body))
    }

    /// Texto integral da materia. O portal serve HTML, nao PDF.
    async fn fetch_text(&self, external_id: &str) -> Result<String> {
        let url = format!("{BASE_URL}/download/materia/{external_id}");
        let params: Vec<(&str, String)> = Vec::new();

        let response = crate::http::get_with_retry(&self.client, &url, &params, 4).await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(AppError::Parse(format!(
                "HTTP {} em {url}: {}",
                status.as_u16(),
                truncate(&body, 200)
            )));
        }

        Ok(html_to_text(&body))
    }
}

#[async_trait]
impl Source for FamursSource {
    fn name(&self) -> &'static str {
        "famurs"
    }

    async fn fetch(
        &self,
        since: NaiveDate,
        until: NaiveDate,
        keywords: &[String],
        _territories: &[String],
    ) -> Result<Vec<RawGazette>> {
        if keywords.is_empty() {
            return Err(AppError::Config("nenhuma keyword para consultar".into()));
        }

        let mut out = Vec::new();
        let mut page = 1usize;
        let mut seen = 0usize;

        loop {
            let listing = self.fetch_page(since, until, page).await?;

            if listing.cards.is_empty() {
                break;
            }
            let returned = listing.cards.len();
            seen += returned;
            let Listing { cards, total } = listing;
            debug!(
                entidade = %self.entity_id,
                page, returned, total = ?total, "pagina de materias"
            );

            for card in cards {
                // A listagem nao traz o texto: uma requisicao por materia.
                let text = match self.fetch_text(&card.external_id).await {
                    Ok(text) => text,
                    Err(err) => {
                        warn!(
                            external_id = %card.external_id,
                            title = %card.title,
                            error = %err,
                            "materia ignorada (falha ao baixar o texto)"
                        );
                        tokio::time::sleep(REQUEST_PAUSE).await;
                        continue;
                    }
                };
                tokio::time::sleep(REQUEST_PAUSE).await;

                let folded = normalize(&text);
                for keyword in keywords {
                    if !matches_keyword(&folded, keyword) {
                        continue;
                    }
                    out.push(card.to_raw(keyword, &text, &self.entity_id));
                }
            }

            if let Some(total) = total {
                if seen >= total {
                    break;
                }
            }
            page += 1;
            if page > MAX_PAGES {
                warn!(
                    page,
                    "limite de paginas atingido; pode haver materias nao coletadas"
                );
                break;
            }
        }

        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

pub struct Listing {
    pub cards: Vec<Card>,
    /// Total informado em "Mostrando 1 a 25 de 94 materias".
    pub total: Option<usize>,
}

pub struct Card {
    pub external_id: String,
    pub url: String,
    pub title: String,
    pub entity_name: String,
    pub published_at: NaiveDate,
    pub edition: Option<String>,
}

impl Card {
    fn to_raw(&self, keyword: &str, text: &str, entity_id: &str) -> RawGazette {
        RawGazette {
            external_id: self.external_id.clone(),
            // Nao temos o IBGE: usamos o id da entidade no SIGPub.
            territory_id: entity_id.to_string(),
            territory_name: self.entity_name.clone(),
            state_code: "RS".to_string(),
            published_at: self.published_at,
            url: self.url.clone(),
            txt_url: Some(format!("{BASE_URL}/download/materia/{}", self.external_id)),
            edition: self.edition.clone(),
            is_extra_edition: false,
            excerpts: vec![window_around(text, keyword, 500, 500)],
            matched_keyword: keyword.to_string(),
        }
    }
}

pub fn parse_listing(body: &str) -> Listing {
    let doc = Html::parse_document(body);

    let card_sel = selector("li.materia-card");
    let title_sel = selector("h2.materia-card__titulo a");
    let code_sel = selector("dl.materia-card__dados code");
    let entity_sel = selector(".materia-card__entidade");
    let summary_sel = selector("p.busca-avancada__resumo strong");
    let row_sel = selector("dl.materia-card__dados > div");
    let dt_sel = selector("dt");
    let dd_sel = selector("dd");

    let total = doc
        .select(&summary_sel)
        .filter_map(|el| el.text().next())
        .filter_map(|text| text.trim().replace('.', "").parse::<usize>().ok())
        .next_back();

    let mut cards = Vec::new();

    for card in doc.select(&card_sel) {
        let Some(code) = text_of(&card, &code_sel) else {
            continue;
        };
        let external_id = code.trim().to_string();
        if external_id.is_empty() {
            continue;
        }

        let Some(title_el) = card.select(&title_sel).next() else {
            continue;
        };
        let title = collapse(&title_el.text().collect::<Vec<_>>().join(" "));
        let href = title_el.value().attr("href").unwrap_or("").to_string();
        if href.is_empty() {
            continue;
        }

        let Some(published_at) = field_date(&card, &row_sel, &dt_sel, &dd_sel, "circula") else {
            debug!(external_id, "materia sem data de circulacao; ignorada");
            continue;
        };

        let entity_name = card
            .select(&entity_sel)
            .next()
            .map(|el| collapse(&el.text().collect::<Vec<_>>().join(" ")))
            .unwrap_or_default();

        let edition = field_text(&card, &row_sel, &dt_sel, &dd_sel, "edicao")
            .filter(|value| !value.is_empty());

        cards.push(Card {
            url: absolute(&href),
            title,
            entity_name: city_from_entity(&entity_name),
            published_at,
            edition,
            external_id,
        });
    }

    Listing { cards, total }
}

fn selector(raw: &str) -> Selector {
    // Seletores sao constantes do modulo; um erro aqui e bug de programacao.
    Selector::parse(raw).expect("seletor CSS invalido")
}

fn text_of(element: &scraper::ElementRef, sel: &Selector) -> Option<String> {
    element
        .select(sel)
        .next()
        .map(|el| collapse(&el.text().collect::<Vec<_>>().join(" ")))
}

/// Le o `<dd>` de uma linha cujo `<dt>` comeca com `label_prefix`.
fn field_text(
    card: &scraper::ElementRef,
    rows: &Selector,
    dt: &Selector,
    dd: &Selector,
    label_prefix: &str,
) -> Option<String> {
    let needle = normalize(label_prefix);
    for row in card.select(rows) {
        let label = row
            .select(dt)
            .next()
            .map(|el| normalize(&el.text().collect::<Vec<_>>().join(" ")))
            .unwrap_or_default();
        if !label.starts_with(&needle) {
            continue;
        }
        return row.select(dd).next().map(|el| {
            collapse(&el.text().collect::<Vec<_>>().join(" "))
                .trim()
                .to_string()
        });
    }
    None
}

fn field_date(
    card: &scraper::ElementRef,
    rows: &Selector,
    dt: &Selector,
    dd: &Selector,
    label_prefix: &str,
) -> Option<NaiveDate> {
    let raw = field_text(card, rows, dt, dd, label_prefix)?;
    NaiveDate::parse_from_str(raw.trim(), "%d/%m/%Y").ok()
}

fn absolute(href: &str) -> String {
    if href.starts_with("http") {
        return href.to_string();
    }
    // O portal devolve `/famurs/materia/...`; BASE_URL ja inclui `/famurs`.
    if href.starts_with('/') {
        format!("{ORIGIN}{href}")
    } else {
        format!("{BASE_URL}/{href}")
    }
}

/// "Prefeitura Municipal de Sao Sepe" -> "Sao Sepe".
pub fn city_from_entity(entity: &str) -> String {
    let trimmed = entity.trim();
    for prefix in [
        "Prefeitura Municipal de ",
        "Prefeitura Municipal do ",
        "Prefeitura Municipal da ",
        "Prefeitura de ",
        "Prefeitura do ",
        "Prefeitura da ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim().to_string();
        }
    }
    trimmed.to_string()
}

/// Texto visivel do corpo da materia (sem menu, cabecalho ou rodape).
pub fn html_to_text(body: &str) -> String {
    let doc = Html::parse_document(body);
    let content_sel = selector("div.materia");

    let raw = if let Some(content) = doc.select(&content_sel).next() {
        content.text().collect::<Vec<_>>().join(" ")
    } else {
        doc.root_element().text().collect::<Vec<_>>().join(" ")
    };

    collapse(&raw)
}

/// Colapsa espacos/quebras de linha preservando o conteudo.
fn collapse(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

// ---------------------------------------------------------------------------
// Casamento de keyword
// ---------------------------------------------------------------------------

/// Termos que precisam aparecer no texto, normalizados.
///
/// As keywords da configuracao usam a sintaxe do Querido Diario (`+termo` para
/// exigir). Aqui o `+` e removido e **todos** os termos passam a ser exigidos:
/// buscar a string literal "+pregao +eletronico" nunca casaria com o texto.
fn keyword_terms(keyword: &str) -> Vec<String> {
    let mut terms: Vec<String> = keyword
        .split_whitespace()
        .map(|term| term.trim_start_matches('+'))
        .map(normalize)
        .filter(|term| term.chars().count() >= 3)
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

/// `folded_text` ja deve vir normalizado (minusculo, sem acento).
fn matches_keyword(folded_text: &str, keyword: &str) -> bool {
    let terms = keyword_terms(keyword);
    if terms.is_empty() {
        return folded_text.contains(&normalize(keyword));
    }
    terms.iter().all(|term| folded_text.contains(term.as_str()))
}

/// Janela de texto ao redor do primeiro termo da keyword.
///
/// A listagem do portal nao traz o texto completo; o excerpt precisa mostrar o
/// trecho que casou, senao vira um recorte aleatorio da materia.
pub fn window_around(text: &str, keyword: &str, before: usize, after: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let folded: String = chars.iter().copied().map(fold_char).collect();

    let fallback = || chars.iter().take(before + after).collect::<String>();

    let terms = keyword_terms(keyword);
    if terms.is_empty() {
        let needle = normalize(keyword);
        return match folded.find(&needle) {
            Some(pos) => slice_around(&chars, &folded, pos, needle.chars().count(), before, after),
            None => fallback(),
        };
    }

    // Ancorar no termo que realmente aparece, e no que aparece primeiro.
    // Os termos vem ordenados alfabeticamente; pegar `terms[0]` erraria em
    // metade dos casos ("dispensa" antes de "licitacao").
    let mut best: Option<(usize, usize)> = None;
    for term in &terms {
        if let Some(pos) = folded.find(term.as_str()) {
            if best.is_none_or(|(best_pos, _)| pos < best_pos) {
                best = Some((pos, term.chars().count()));
            }
        }
    }

    match best {
        Some((pos, len)) => slice_around(&chars, &folded, pos, len, before, after),
        None => fallback(),
    }
}

fn slice_around(
    chars: &[char],
    folded: &str,
    byte_pos: usize,
    needle_len: usize,
    before: usize,
    after: usize,
) -> String {
    let anchor = folded[..byte_pos].chars().count();
    let start = anchor.saturating_sub(before);
    let end = (anchor + needle_len + after).min(chars.len());

    chars[start..end]
        .iter()
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"
    <p class="busca-avancada__resumo">
      Mostrando <strong>1</strong> a <strong>2</strong> de <strong>2</strong> matérias.
    </p>
    <ul class="lista-materias">
      <li class="materia-card">
        <h2 class="materia-card__titulo">
          <a href="/famurs/materia/D6898EDE" target="_blank">RETIFICAÇÃO DE EDITAL</a>
        </h2>
        <p class="materia-card__origem">
          <span class="materia-card__entidade">Prefeitura Municipal de São Sepé</span>
          <span class="materia-card__separador">•</span>
          <span class="materia-card__orgao">Secretaria Municipal de Administração</span>
        </p>
        <dl class="materia-card__dados">
          <div><dt>Circulação</dt><dd>13/10/2026</dd></div>
          <div><dt>Edição</dt><dd> 4434 </dd></div>
          <div><dt>Código identificador</dt><dd><code>D6898EDE</code></dd></div>
        </dl>
      </li>
      <li class="materia-card">
        <h2 class="materia-card__titulo">
          <a href="/famurs/materia/8DD435CD">AVISO DE LICITAÇÃO</a>
        </h2>
        <p class="materia-card__origem">
          <span class="materia-card__entidade">Prefeitura Municipal de São Sepé</span>
        </p>
        <dl class="materia-card__dados">
          <div><dt>Circulação</dt><dd>10/10/2026</dd></div>
          <div><dt>Edição</dt><dd>4431</dd></div>
          <div><dt>Código identificador</dt><dd><code>8DD435CD</code></dd></div>
        </dl>
      </li>
    </ul>
    "#;

    #[test]
    fn parseia_a_listagem_real() {
        let listing = parse_listing(LISTING);
        assert_eq!(listing.total, Some(2));
        assert_eq!(listing.cards.len(), 2);

        let first = &listing.cards[0];
        assert_eq!(first.external_id, "D6898EDE");
        assert_eq!(first.title, "RETIFICAÇÃO DE EDITAL");
        assert_eq!(first.published_at.to_string(), "2026-10-13");
        assert_eq!(first.edition.as_deref(), Some("4434"));
        assert_eq!(first.entity_name, "São Sepé");
        assert_eq!(
            first.url,
            "https://www.diariomunicipal.com.br/famurs/materia/D6898EDE"
        );
    }

    #[test]
    fn extrai_texto_do_corpo_da_materia() {
        let html = r#"
        <html><body>
        <div class="folha"><article class="documento">
          <header class="cabecalho"><h1>PREFEITURA MUNICIPAL DE SÃO SEPÉ</h1></header>
          <div class="materia">
            <p>AVISO DE LICITAÇÃO</p>
            <p>O Prefeito torna público o Pregão Eletrônico nº 50/2026.</p>
          </div>
        </article></div>
        </body></html>"#;
        let text = html_to_text(html);
        assert!(text.contains("AVISO DE LICITAÇÃO"));
        assert!(!text.contains("PREFEITURA MUNICIPAL DE SÃO SEPÉ"));
    }

    #[test]
    fn casa_keyword_com_operador_e_acento() {
        let text = normalize("O Prefeito torna público o Pregão Eletrônico nº 50/2026");
        assert!(matches_keyword(&text, "+pregao +eletronico"));
        assert!(matches_keyword(&text, "pregao"));
        // Falta um dos termos exigidos.
        assert!(!matches_keyword(&text, "+pregao +dispensa"));
        // Termo inexistente.
        assert!(!matches_keyword(&text, "+contratacao +direta"));
    }

    #[test]
    fn janela_centra_no_termo() {
        let text = format!("{} AVISO DE LICITAÇÃO {}", "a".repeat(400), "b".repeat(400));
        let out = window_around(&text, "+dispensa +licitacao", 100, 100);
        assert!(normalize(&out).contains("licitacao"), "veio: {out:?}");
        assert!(out.chars().count() <= 220);
    }

    #[test]
    fn janela_cai_no_inicio_sem_termo() {
        let text = "z".repeat(500);
        let out = window_around(&text, "+licitacao", 100, 100);
        assert_eq!(out.chars().count(), 200);
    }

    #[test]
    fn limpa_prefixo_da_entidade() {
        assert_eq!(city_from_entity("Prefeitura Municipal de São Sepé"), "São Sepé");
        assert_eq!(city_from_entity("Câmara Municipal de Vereadores"), "Câmara Municipal de Vereadores");
    }
}
