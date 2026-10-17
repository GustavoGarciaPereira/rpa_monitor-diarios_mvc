use std::sync::Arc;

use chrono::NaiveDate;
use sqlx::SqlitePool;
use tracing::{debug, info};

use crate::config::Config;
use crate::error::Result;
use crate::model::RawGazette;
use crate::sources::{QueridoDiario, Source};

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

/// Coleta o periodo e grava somente o que ainda nao foi visto.
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
    let source: Arc<dyn Source> =
        Arc::new(QueridoDiario::new(config.page_size, config.max_results_per_keyword)?);

    info!(
        fonte = source.name(),
        %since,
        %until,
        keywords = config.keywords.len(),
        territorios = config.territories.len(),
        "iniciando coleta"
    );

    let items = source
        .fetch(since, until, &config.keywords, &config.territories)
        .await?;

    let mut report = ScanReport {
        collected: items.len(),
        ..Default::default()
    };

    for item in items {
        let persisted = persist(pool, source.as_ref(), &item).await?;

        if persisted.gazette_inserted {
            report.gazettes_new += 1;
        }
        if persisted.alert_inserted {
            report.alerts_new += 1;
            info!(
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

async fn persist(pool: &SqlitePool, source: &dyn Source, item: &RawGazette) -> Result<Persisted> {
    let gazette_id = item.id(source.name());
    let alert_hash = item.alert_hash(source.name(), &item.matched_keyword);
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
    .bind(source.name())
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
