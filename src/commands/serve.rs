use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tracing::info;

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::model::{fold_char, normalize};

/// Template embutido no binario (producao). Em dev o HTML vem do disco.
const INDEX_HTML: &str = include_str!("../../templates/index.html");

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub total_gazettes: i64,
    pub total_alerts: i64,
    pub total_cities: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GazetteRow {
    pub date: String,
    pub city: String,
    pub keywords: Vec<String>,
    pub excerpt: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Filters {
    pub city: Option<String>,
    pub keyword: Option<String>,
    #[serde(default = "default_days")]
    pub days: u32,
}

fn default_days() -> u32 {
    7
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            city: None,
            keyword: None,
            days: default_days(),
        }
    }
}

impl Filters {
    /// Evita `date('now', '-0 days')` (que zera a lista) e janelas absurdas.
    fn effective_days(&self) -> i64 {
        i64::from(self.days.clamp(1, 3650))
    }
}

/// Versao dos filtros pronta para o template: sempre strings, nunca `None`.
#[derive(Debug, Clone, Serialize)]
struct FilterView {
    city: String,
    keyword: String,
    days: u32,
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Condicoes comuns a lista e aos cards de estatistica.
///
/// Os mesmos filtros precisam valer para os dois: senao os numeros do topo
/// contradizem a lista logo abaixo.
fn filter_parts(filters: &Filters) -> (String, Option<String>, Option<String>, i64) {
    let city = clean_filter(filters.city.as_deref());
    let keyword = clean_filter(filters.keyword.as_deref());
    let days = filters.effective_days();

    let mut sql = String::new();
    if city.is_some() {
        sql.push_str(" AND g.territory_name LIKE '%' || ? || '%'");
    }
    if keyword.is_some() {
        sql.push_str(" AND a.keyword LIKE '%' || ? || '%'");
    }
    // `-3 hours`: o banco guarda datas de Brasilia; `date('now')` e UTC e
    // viraria o dia as 21h, encolhendo a janela em um dia.
    sql.push_str(" AND g.published_at >= date('now', '-3 hours', '-' || ? || ' days')");

    (sql, city, keyword, days)
}

/// Numeros do topo, sempre respeitando os filtros ativos.
pub async fn fetch_stats(pool: &SqlitePool, filters: &Filters) -> Result<Stats> {
    let (conditions, city, keyword, days) = filter_parts(filters);

    let sql = format!(
        r#"
        SELECT COUNT(DISTINCT g.id)           AS gazettes,
               COUNT(a.id)                    AS alerts,
               COUNT(DISTINCT g.territory_name) AS cities
        FROM gazettes g
        JOIN alerts a ON a.gazette_id = g.id
        WHERE 1 = 1{conditions}
        "#
    );

    let mut query = sqlx::query(&sql);
    if let Some(value) = &city {
        query = query.bind(value);
    }
    if let Some(value) = &keyword {
        query = query.bind(value);
    }
    query = query.bind(days);

    let row = query.fetch_one(pool).await?;

    Ok(Stats {
        total_gazettes: row.try_get("gazettes")?,
        total_alerts: row.try_get("alerts")?,
        total_cities: row.try_get("cities")?,
    })
}

/// Cidades que existem no banco, para alimentar o `<select>` do filtro.
pub async fn fetch_cities(pool: &SqlitePool) -> Result<Vec<String>> {
    let cities = sqlx::query_scalar(
        "SELECT DISTINCT territory_name FROM gazettes ORDER BY territory_name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;

    Ok(cities)
}

pub async fn fetch_gazettes(pool: &SqlitePool, filters: &Filters) -> Result<Vec<GazetteRow>> {
    let (conditions, city, keyword, days) = filter_parts(filters);

    let mut sql = String::from(
        r#"
        SELECT g.published_at, g.territory_name, g.excerpt, g.url,
               GROUP_CONCAT(DISTINCT a.keyword) AS keywords
        FROM gazettes g
        JOIN alerts a ON a.gazette_id = g.id
        WHERE 1 = 1
        "#,
    );

    sql.push_str(&conditions);
    sql.push_str(" GROUP BY g.id ORDER BY g.published_at DESC LIMIT 1000");

    let mut query = sqlx::query(&sql);
    if let Some(value) = &city {
        query = query.bind(value);
    }
    if let Some(value) = &keyword {
        query = query.bind(value);
    }
    query = query.bind(days);

    let rows = query.fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let published_at: NaiveDate = row.try_get("published_at")?;
        let city: String = row.try_get("territory_name")?;
        let excerpt: Option<String> = row.try_get("excerpt")?;
        let url: String = row.try_get("url")?;
        let keywords: Option<String> = row.try_get("keywords")?;

        let mut keywords: Vec<String> = keywords
            .unwrap_or_default()
            .split(',')
            .map(pretty_keyword)
            .filter(|k| !k.is_empty())
            .collect();
        keywords.sort();
        keywords.dedup();

        let raw_excerpt = excerpt.unwrap_or_default();
        let excerpt_text = excerpt_for(&raw_excerpt, &keywords, EXCERPT_RADIUS);

        out.push(GazetteRow {
            date: published_at.format("%d/%m/%Y").to_string(),
            city,
            keywords,
            excerpt: excerpt_text,
            url,
        });
    }

    Ok(out)
}

pub async fn fetch_last_update(pool: &SqlitePool) -> Result<String> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT MAX(found_at) FROM gazettes")
            .fetch_one(pool)
            .await?;

    let Some(raw) = raw else {
        return Ok("—".to_string());
    };

    // `CURRENT_TIMESTAMP` grava UTC; exibimos no horario local.
    Ok(
        match NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S") {
            Ok(naive) => Utc
                .from_utc_datetime(&naive)
                .with_timezone(&Local)
                .format("%d/%m/%Y %H:%M")
                .to_string(),
            Err(_) => raw,
        },
    )
}

fn clean_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// `+pregao +eletronico` -> `pregao eletronico`, so para exibicao.
fn pretty_keyword(raw: &str) -> String {
    raw.split_whitespace()
        .map(|term| term.trim_start_matches('+'))
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Chars de contexto de cada lado da keyword na janela do excerpt.
const EXCERPT_RADIUS: usize = 100;

/// Trecho de ~200 chars centrado na keyword que de fato aparece no texto.
///
/// Cortar os primeiros 200 chars escondia justamente o termo que fez a
/// publicacao casar (ele costuma estar no meio do trecho guardado).
fn excerpt_for(text: &str, keywords: &[String], radius: usize) -> String {
    if text.is_empty() {
        return String::new();
    }

    let chars: Vec<char> = text.chars().collect();
    // `fold_char` e 1:1 com o original, entao indices do texto dobrado valem
    // para `chars`.
    let folded: String = chars.iter().copied().map(fold_char).collect();

    for keyword in keywords {
        if let Some((start, end)) = anchor_span(&folded, keyword, radius, chars.len()) {
            return render_window(&chars, start, end);
        }
    }

    // Nenhuma keyword no texto: comportamento antigo (inicio do trecho).
    chars.iter().take(radius * 2).collect::<String>().trim().to_string()
}

/// Posicao (em chars) da janela que envolve a keyword encontrada.
fn anchor_span(
    folded: &str,
    keyword: &str,
    radius: usize,
    total_chars: usize,
) -> Option<(usize, usize)> {
    for candidate in keyword_candidates(keyword) {
        if let Some(byte_pos) = folded.find(candidate.as_str()) {
            let anchor = folded[..byte_pos].chars().count();
            let keyword_len = candidate.chars().count();
            let start = anchor.saturating_sub(radius);
            let end = (anchor + keyword_len + radius).min(total_chars);
            return Some((start, end));
        }
    }
    None
}

/// Candidatos de busca, do mais especifico ao mais generico.
///
/// A frase inteira primeiro. Se ela nao aparece (comum: a API casa os termos
/// separadamente), tenta cada termo isolado, do mais longo ao mais curto.
fn keyword_candidates(keyword: &str) -> Vec<String> {
    let phrase = normalize(keyword);
    let mut candidates = Vec::new();
    if !phrase.is_empty() {
        candidates.push(phrase);
    }

    let mut terms: Vec<String> = keyword
        .split_whitespace()
        .map(normalize)
        .filter(|term| term.chars().count() >= 4)
        .collect();
    terms.sort_by_key(|term| std::cmp::Reverse(term.chars().count()));
    candidates.extend(terms);

    candidates
}

fn render_window(chars: &[char], start: usize, end: usize) -> String {
    let mut out = String::new();
    if start > 0 {
        out.push_str("...");
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push_str("...");
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    rss_mb: f64,
    template_env: Arc<minijinja::Environment<'static>>,
    /// Caminho do template lido do disco a cada request; `None` = embutido.
    live_template: Option<PathBuf>,
}

async fn handler_index(
    State(state): State<AppState>,
    Query(filters): Query<Filters>,
) -> std::result::Result<Html<String>, AppError> {
    let (stats, gazettes, last_update, cities) = tokio::join!(
        fetch_stats(&state.pool, &filters),
        fetch_gazettes(&state.pool, &filters),
        fetch_last_update(&state.pool),
        fetch_cities(&state.pool),
    );

    let view = FilterView {
        city: filters.city.clone().unwrap_or_default(),
        keyword: filters.keyword.clone().unwrap_or_default(),
        days: filters.days.clamp(1, 3650),
    };

    let context = minijinja::context! {
        stats => stats?,
        gazettes => gazettes?,
        cities => cities?,
        filters => view,
        rss_mb => format!("{:.1}", state.rss_mb),
        last_update => last_update?,
    };

    let html = render_index(&state.template_env, state.live_template.as_deref(), context)?;
    Ok(Html(html))
}

async fn handler_health() -> &'static str {
    "ok"
}

// ---------------------------------------------------------------------------
// Servidor
// ---------------------------------------------------------------------------

pub async fn run(config: &Config, pool: SqlitePool, addr: SocketAddr) -> Result<()> {
    let mut env = minijinja::Environment::new();
    env.add_template("index.html", INDEX_HTML)?;

    let live_template = live_template_path();
    if let Some(path) = &live_template {
        info!(
            caminho = %path.display(),
            "hot-reload do template ativo: edite o HTML e recarregue o browser"
        );
    }

    let state = AppState {
        pool,
        rss_mb: read_rss_mb(),
        template_env: Arc::new(env),
        live_template,
    };

    let app = Router::new()
        .route("/", get(handler_index))
        .route("/health", get(handler_health))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(
        keywords = config.keywords.len(),
        territorios = config.territories.len(),
        "servidor iniciado"
    );
    // `axum::serve` nunca retorna: o usuario precisa da URL antes disso.
    println!("servindo em http://{addr}");

    axum::serve(listener, app).await?;
    Ok(())
}

/// Caminho do template lido do disco a cada request.
///
/// Ativo em build de debug (dev) e quando `MONITOR_TEMPLATE_FILE` aponta um
/// arquivo: editar o HTML e so recarregar o browser, sem recompilar. Em
/// release o template embutido e usado, sem I/O por request.
fn live_template_path() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("MONITOR_TEMPLATE_FILE") {
        let raw = raw.trim();
        if !raw.is_empty() {
            return Some(PathBuf::from(raw));
        }
    }
    if cfg!(debug_assertions) {
        return Some(PathBuf::from("templates/index.html"));
    }
    None
}

/// Renderiza o index, preferindo o arquivo em disco quando configurado.
///
/// Se o arquivo sumir ou ficar ilegivel, cai no embutido em vez de derrubar a
/// pagina — durante o dev isso evita um 500 a cada save incompleto.
fn render_index(
    env: &minijinja::Environment<'static>,
    live_template: Option<&Path>,
    context: minijinja::Value,
) -> Result<String> {
    if let Some(path) = live_template {
        if let Ok(source) = std::fs::read_to_string(path) {
            return minijinja::Environment::new()
                .render_str(&source, context)
                .map_err(Into::into);
        }
    }

    let template = env.get_template("index.html")?;
    Ok(template.render(context)?)
}

fn read_rss_mb() -> f64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0.0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            if let Some(kb) = rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<f64>().ok())
            {
                return kb / 1024.0;
            }
        }
    }
    0.0
}

// ---------------------------------------------------------------------------
// Integracao com axum / minijinja
// ---------------------------------------------------------------------------

impl From<minijinja::Error> for AppError {
    fn from(err: minijinja::Error) -> Self {
        AppError::Template(err.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::error!(error = %self, "erro ao responder requisicao");
        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    async fn test_pool() -> SqlitePool {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        // Um unico conexao: cada conexao de `:memory:` teria um banco proprio.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    /// `date('now', '-1 day')` mantem o teste independente do relogio.
    async fn add_gazette(pool: &SqlitePool, id: &str, city: &str, days_ago: i64, excerpt: &str) {
        sqlx::query(
            r#"
            INSERT INTO gazettes
                (id, source, external_id, territory_id, territory_name, state_code,
                 published_at, url, is_extra_edition, excerpt)
            VALUES (?1, 'test', ?1, '0000000', ?2, 'PR',
                    date('now', '-' || ?3 || ' day'), 'https://example.test/' || ?1, 0, ?4)
            "#,
        )
        .bind(id)
        .bind(city)
        .bind(days_ago)
        .bind(excerpt)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn add_alert(pool: &SqlitePool, gazette_id: &str, keyword: &str) {
        sqlx::query("INSERT INTO alerts (gazette_id, keyword, alert_hash) VALUES (?1, ?2, ?3)")
            .bind(gazette_id)
            .bind(keyword)
            .bind(format!("{gazette_id}|{keyword}"))
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_fetch_stats_empty() {
        let pool = test_pool().await;
        let stats = fetch_stats(&pool, &Filters::default()).await.unwrap();
        assert_eq!(stats.total_gazettes, 0);
        assert_eq!(stats.total_alerts, 0);
        assert_eq!(stats.total_cities, 0);
    }

    #[tokio::test]
    async fn test_fetch_gazettes_agrupa_keywords() {
        let pool = test_pool().await;
        add_gazette(&pool, "g1", "Curitiba", 1, "abertura de licitacao").await;
        add_gazette(&pool, "g2", "Salvador", 2, "contrato firmado").await;
        add_alert(&pool, "g1", "+pregao +eletronico").await;
        add_alert(&pool, "g1", "+dispensa +licitacao").await;
        add_alert(&pool, "g2", "+dispensa +licitacao").await;

        let rows = fetch_gazettes(&pool, &Filters::default()).await.unwrap();
        assert_eq!(rows.len(), 2, "uma linha por publicacao, nao por alerta");

        let curitiba = rows.iter().find(|r| r.city == "Curitiba").unwrap();
        assert_eq!(curitiba.keywords.len(), 2);
        assert_eq!(curitiba.date.split('/').count(), 3, "formato DD/MM/YYYY");
    }

    #[tokio::test]
    async fn test_fetch_gazettes_filtra_keyword() {
        let pool = test_pool().await;
        add_gazette(&pool, "g1", "Curitiba", 1, "pregao eletronico").await;
        add_gazette(&pool, "g2", "Salvador", 1, "contrato").await;
        add_alert(&pool, "g1", "+pregao +eletronico").await;
        add_alert(&pool, "g2", "+dispensa +licitacao").await;

        let filters = Filters {
            keyword: Some("licitacao".into()),
            ..Filters::default()
        };
        let rows = fetch_gazettes(&pool, &filters).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].city, "Salvador");
    }

    #[tokio::test]
    async fn test_fetch_gazettes_filtra_cidade() {
        let pool = test_pool().await;
        add_gazette(&pool, "g1", "Curitiba", 1, "a").await;
        add_gazette(&pool, "g2", "Salvador", 1, "b").await;
        add_alert(&pool, "g1", "licitacao").await;
        add_alert(&pool, "g2", "licitacao").await;

        let filters = Filters {
            city: Some("curit".into()),
            ..Filters::default()
        };
        let rows = fetch_gazettes(&pool, &filters).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].city, "Curitiba");
    }

    #[tokio::test]
    async fn test_fetch_gazettes_trunca_utf8_sem_panico() {
        let pool = test_pool().await;
        let excerpt = "Á".repeat(300);
        add_gazette(&pool, "g1", "Curitiba", 1, &excerpt).await;
        add_alert(&pool, "g1", "licitacao").await;

        let rows = fetch_gazettes(&pool, &Filters::default()).await.unwrap();
        // Sem keyword no texto: corte normal (2 * raio), sem panico em UTF-8.
        assert_eq!(rows[0].excerpt.chars().count(), 200);
    }

    #[tokio::test]
    async fn test_excerpt_centers_on_keyword() {
        let pool = test_pool().await;
        let text = format!(
            "{} dispensa licitacao para compra de materiais {}",
            "a".repeat(500),
            "b".repeat(500)
        );
        add_gazette(&pool, "g1", "Curitiba", 1, &text).await;
        add_alert(&pool, "g1", "+dispensa +licitacao").await;

        let rows = fetch_gazettes(&pool, &Filters::default()).await.unwrap();
        let excerpt = &rows[0].excerpt;
        let len = excerpt.chars().count();

        assert!(
            normalize(excerpt).contains("dispensa licitacao"),
            "o trecho deve conter a keyword, veio: {excerpt:?}"
        );
        assert!((150..=250).contains(&len), "tamanho fora da faixa: {len}");
        assert!(excerpt.starts_with("..."), "deveria marcar corte no inicio: {excerpt:?}");
        assert!(excerpt.ends_with("..."), "deveria marcar corte no fim: {excerpt:?}");
    }

    #[test]
    fn excerpt_usa_termo_quando_a_frase_nao_aparece() {
        // Caso real: a API casa "+dispensa +licitacao" em documentos que so
        // trazem "licitacao" — a frase inteira nao existe no texto.
        let text = format!("{} AVISO DE LICITAÇÃO {}", "x".repeat(300), "y".repeat(300));
        let out = excerpt_for(&text, &["dispensa licitacao".to_string()], EXCERPT_RADIUS);
        assert!(normalize(&out).contains("licitacao"), "veio: {out:?}");
        assert!((150..=250).contains(&out.chars().count()));
    }

    #[test]
    fn excerpt_cai_no_inicio_quando_nenhuma_keyword_aparece() {
        let text = "z".repeat(500);
        let out = excerpt_for(&text, &["licitacao".to_string()], EXCERPT_RADIUS);
        assert_eq!(out.chars().count(), 200);
        assert_eq!(out, "z".repeat(200));
    }

    #[tokio::test]
    async fn test_fetch_stats_respeitam_o_filtro() {
        let pool = test_pool().await;
        add_gazette(&pool, "g1", "Curitiba", 1, "pregao eletronico").await;
        add_gazette(&pool, "g2", "Salvador", 1, "contrato").await;
        add_alert(&pool, "g1", "+pregao +eletronico").await;
        add_alert(&pool, "g2", "+dispensa +licitacao").await;

        let all = fetch_stats(&pool, &Filters::default()).await.unwrap();
        assert_eq!(all.total_gazettes, 2);
        assert_eq!(all.total_alerts, 2);
        assert_eq!(all.total_cities, 2);

        // Os cards precisam acompanhar o filtro, senao contradizem a lista.
        let by_city = Filters {
            city: Some("curit".into()),
            ..Filters::default()
        };
        let stats = fetch_stats(&pool, &by_city).await.unwrap();
        assert_eq!(stats.total_gazettes, 1);
        assert_eq!(stats.total_alerts, 1);
        assert_eq!(stats.total_cities, 1);

        let by_keyword = Filters {
            keyword: Some("dispensa".into()),
            ..Filters::default()
        };
        let stats = fetch_stats(&pool, &by_keyword).await.unwrap();
        assert_eq!(stats.total_alerts, 1);
    }

    #[tokio::test]
    async fn test_fetch_cities_ordenadas_e_sem_repeticao() {
        let pool = test_pool().await;
        add_gazette(&pool, "g1", "Salvador", 1, "a").await;
        add_gazette(&pool, "g2", "Curitiba", 1, "b").await;
        add_gazette(&pool, "g3", "Curitiba", 2, "c").await;
        add_alert(&pool, "g1", "licitacao").await;
        add_alert(&pool, "g2", "licitacao").await;
        add_alert(&pool, "g3", "licitacao").await;

        let cities = fetch_cities(&pool).await.unwrap();
        assert_eq!(cities, vec!["Curitiba", "Salvador"]);
    }

    #[tokio::test]
    async fn test_fetch_last_update_vazio() {
        let pool = test_pool().await;
        assert_eq!(fetch_last_update(&pool).await.unwrap(), "—");
    }

    #[test]
    fn pretty_keyword_remove_operadores() {
        assert_eq!(pretty_keyword("+pregao +eletronico"), "pregao eletronico");
        assert_eq!(pretty_keyword("licitacao"), "licitacao");
    }

    #[test]
    fn days_fora_da_faixa_e_normalizado() {
        let zero = Filters {
            days: 0,
            ..Filters::default()
        };
        assert_eq!(zero.effective_days(), 1);
    }

    #[test]
    fn render_usa_arquivo_do_disco_quando_existe() {
        let path = std::env::temp_dir().join(format!("monitor_tpl_{}.html", std::process::id()));
        std::fs::write(&path, "DISCO {{ city }}").unwrap();

        let mut env = minijinja::Environment::new();
        env.add_template("index.html", "EMBUTIDO").unwrap();

        let html =
            render_index(&env, Some(&path), minijinja::context! { city => "Curitiba" }).unwrap();
        assert_eq!(html, "DISCO Curitiba");

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn render_cai_no_embutido_quando_o_arquivo_some() {
        // Arquivo inexistente: a pagina continua no ar com o template embutido.
        let path = std::env::temp_dir().join("monitor_tpl_nao_existe_12345.html");

        let mut env = minijinja::Environment::new();
        env.add_template("index.html", "EMBUTIDO").unwrap();

        let html = render_index(&env, Some(&path), minijinja::context! {}).unwrap();
        assert_eq!(html, "EMBUTIDO");
    }
}
