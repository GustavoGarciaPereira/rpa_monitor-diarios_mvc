use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::model::RawGazette;
use crate::sources::{FamursSource, QueridoDiario, Source};

#[derive(Debug, Default)]
pub struct ScanReport {
    pub collected: usize,
    pub gazettes_new: usize,
    pub alerts_new: usize,
    pub duplicates: usize,
}

struct Persisted {
    gazette_inserted: bool,
    alert_inserted: bool,
}

/// Coleta o periodo em todas as fontes configuradas e grava o que e novo.
///
/// A deduplicacao acontece no banco, nao na memoria: `INSERT OR IGNORE` no
/// `alert_hash` e a fonte da verdade. Assim, rodar duas vezes seguidas reporta
/// zero novidades, mesmo se o processo morrer no meio.
pub async fn run(
    config: &Config,
    pool: &SqlitePool,
    since: NaiveDate,
    until: NaiveDate,
) -> Result<ScanReport> {
    let sources = build_sources(config)?;

    info!(
        %since,
        %until,
        keywords = config.keywords.len(),
        territorios = config.territories.len(),
        entidades_famurs = config.famurs_entities.len(),
        fontes = sources.len(),
        "iniciando coleta"
    );

    // Uma fonte com problema nao deve derrubar as outras: o monitor prefere
    // entregar dados parciais a nao entregar nada. Se *todas* falharem, erra.
    let mut collected: Vec<(String, RawGazette)> = Vec::new();
    let mut failures = Vec::new();

    for source in &sources {
        let name = source.name();
        match source
            .fetch(since, until, &config.keywords, &config.territories)
            .await
        {
            Ok(items) => {
                info!(fonte = name, itens = items.len(), "fonte coletada");
                collected.extend(items.into_iter().map(|item| (name.to_string(), item)));
            }
            Err(err) => {
                warn!(fonte = name, error = %err, "fonte falhou; seguindo com as demais");
                failures.push(err);
            }
        }
    }

    if collected.is_empty() && !sources.is_empty() && failures.len() == sources.len() {
        if let Some(err) = failures.into_iter().next() {
            return Err(err);
        }
    }

    let mut report = ScanReport {
        collected: collected.len(),
        ..Default::default()
    };

    for (source_name, item) in collected {
        let persisted = persist(pool, &source_name, &item).await?;

        if persisted.gazette_inserted {
            report.gazettes_new += 1;
        }
        if persisted.alert_inserted {
            report.alerts_new += 1;
            info!(
                fonte = %source_name,
                uf = %item.state_code,
                cidade = %item.territory_name,
                data = %item.published_at,
                keyword = %item.matched_keyword,
                url = %item.url,
                "→ nova publicacao"
            );
        } else {
            report.duplicates += 1;
            debug!(url = %item.url, keyword = %item.matched_keyword, "ja alertado antes");
        }
    }

    Ok(report)
}

fn build_sources(config: &Config) -> Result<Vec<Arc<dyn Source>>> {
    let mut sources: Vec<Arc<dyn Source>> = Vec::new();

    sources.push(Arc::new(QueridoDiario::new(
        config.page_size,
        config.max_results_per_keyword,
    )?));

    if !config.famurs_entities.is_empty() {
        // O cliente da FAMURS e compartilhado entre entidades (pool de conexoes).
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .user_agent(concat!("monitor-diarios/", env!("CARGO_PKG_VERSION")))
            .build()?;

        for entity_id in &config.famurs_entities {
            sources.push(Arc::new(FamursSource::new(
                client.clone(),
                entity_id.clone(),
            )));
        }
    }

    Ok(sources)
}

async fn persist(pool: &SqlitePool, source_name: &str, item: &RawGazette) -> Result<Persisted> {
    let gazette_id = item.id(source_name);
    let alert_hash = item.alert_hash(source_name, &item.matched_keyword);
    let excerpt = item.best_excerpt(1000);

    // Transacao por publicacao: gazette + alerta entram juntos ou nao entram.
    // Sem isso, uma falha no meio deixaria a gazette gravada sem alerta e o
    // `scan` seguinte a consideraria "ja vista".
    let mut tx = pool.begin().await?;

    let gazette = sqlx::query(
        r#"
        INSERT OR IGNORE INTO gazettes
            (id, source, external_id, territory_id, territory_name, state_code,
             published_at, url, txt_url, edition, is_extra_edition, excerpt)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
    )
    .bind(&gazette_id)
    .bind(source_name)
    .bind(&item.external_id)
    .bind(&item.territory_id)
    .bind(&item.territory_name)
    .bind(&item.state_code)
    .bind(item.published_at)
    .bind(&item.url)
    .bind(&item.txt_url)
    .bind(&item.edition)
    .bind(item.is_extra_edition as i64)
    .bind(&excerpt)
    .execute(&mut *tx)
    .await?;

    let alert = sqlx::query(
        r#"
        INSERT OR IGNORE INTO alerts (gazette_id, keyword, alert_hash)
        VALUES (?1, ?2, ?3)
        "#,
    )
    .bind(&gazette_id)
    .bind(&item.matched_keyword)
    .bind(&alert_hash)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Persisted {
        gazette_inserted: gazette.rows_affected() > 0,
        alert_inserted: alert.rows_affected() > 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_base() -> Config {
        Config {
            keywords: vec!["+pregao +eletronico".to_string()],
            ..Config::default()
        }
    }

    #[test]
    fn sem_entidades_famurs_so_tem_querido_diario() {
        let sources = build_sources(&config_base()).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name(), "querido_diario");
    }

    #[test]
    fn cada_entidade_famurs_vira_uma_fonte() {
        let config = Config {
            famurs_entities: vec!["64462".to_string(), "64463".to_string()],
            ..config_base()
        };
        let sources = build_sources(&config).unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].name(), "querido_diario");
        assert_eq!(sources[1].name(), "famurs");
        assert_eq!(sources[2].name(), "famurs");
    }
}
