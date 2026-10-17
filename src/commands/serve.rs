use std::net::SocketAddr;
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

pub async fn fetch_stats(pool: &SqlitePool) -> Result<Stats> {
    let total_gazettes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gazettes")
        .fetch_one(pool)
        .await?;
    let total_alerts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts")
        .fetch_one(pool)
        .await?;
    let total_cities: i64 =
        sqlx::query_scalar("SELECT COUNT(DISTINCT territory_id) FROM gazettes")
            .fetch_one(pool)
            .await?;

    Ok(Stats {
        total_gazettes,
        total_alerts,
        total_cities,
    })
}

pub async fn fetch_gazettes(pool: &SqlitePool, filters: &Filters) -> Result<Vec<GazetteRow>> {
    let city = clean_filter(filters.city.as_deref());
    let keyword = clean_filter(filters.keyword.as_deref());
    let days = filters.effective_days();

    let mut sql = String::from(
        r#"
        SELECT g.published_at, g.territory_name, g.excerpt, g.url,
               GROUP_CONCAT(DISTINCT a.keyword) AS keywords
        FROM gazettes g
        JOIN alerts a ON a.gazette_id = g.id
        WHERE 1 = 1
        "#,
    );

    if city.is_some() {
        sql.push_str(" AND g.territory_name LIKE '%' || ? || '%'");
    }
    if keyword.is_some() {
        sql.push_str(" AND a.keyword LIKE '%' || ? || '%'");
    }

    // `-3 hours`: o banco guarda datas de Brasilia; `date('now')` e UTC e
    // viraria o dia as 21h, encolhendo a janela em um dia.
    sql.push_str(" AND g.published_at >= date('now', '-3 hours', '-' || ? || ' days')");
    sql.push_str(" GROUP BY g.id ORDER BY g.published_at DESC LIMIT 200");

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

        out.push(GazetteRow {
            date: published_at.format("%d/%m/%Y").to_string(),
            city,
            keywords,
            // `chars()` e nao slicing: cortar por byte panica em UTF-8.
            excerpt: excerpt
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>()
                .trim()
                .to_string(),
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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    rss_mb: f64,
    template_env: Arc<minijinja::Environment<'static>>,
}

async fn handler_index(
    State(state): State<AppState>,
    Query(filters): Query<Filters>,
) -> std::result::Result<Html<String>, AppError> {
    let (stats, gazettes, last_update) = tokio::join!(
        fetch_stats(&state.pool),
        fetch_gazettes(&state.pool, &filters),
        fetch_last_update(&state.pool),
    );

    let view = FilterView {
        city: filters.city.clone().unwrap_or_default(),
        keyword: filters.keyword.clone().unwrap_or_default(),
        days: filters.days.clamp(1, 3650),
    };

    let context = minijinja::context! {
        stats => stats?,
        gazettes => gazettes?,
        filters => view,
        rss_mb => format!("{:.1}", state.rss_mb),
        last_update => last_update?,
    };

    let template = state.template_env.get_template("index.html")?;
    Ok(Html(template.render(context)?))
}

async fn handler_health() -> &'static str {
    "ok"
}

// ---------------------------------------------------------------------------
// Servidor
// ---------------------------------------------------------------------------

pub async fn run(config: &Config, pool: SqlitePool, addr: SocketAddr) -> Result<()> {
    let mut env = minijinja::Environment::new();
    env.add_template("index.html", include_str!("../../templates/index.html"))?;

    let state = AppState {
        pool,
        rss_mb: read_rss_mb(),
        template_env: Arc::new(env),
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
        let stats = fetch_stats(&pool).await.unwrap();
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
        assert_eq!(rows[0].excerpt.chars().count(), 200);
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
}
