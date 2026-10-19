use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::model::RawGazette;
use crate::sources::{DataJudSource, FamursSource, QueridoDiario, Source};

#[derive(Debug, Default)]
pub struct ScanReport {
    pub collected: usize,
    pub gazettes_new: usize,
    pub alerts_new: usize,
    pub duplicates: usize,
}

/// O que rodar numa execucao.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escopo {
    /// Diarios e depois processos.
    Tudo,
    /// So Querido Diario + FAMURS (rapido).
    SoDiarios,
    /// So DataJud (lento, serial).
    SoProcessos,
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
    escopo: Escopo,
) -> Result<ScanReport> {
    let mut report = ScanReport::default();

    // Diarios primeiro: e rapido e cobre a maior parte do uso.
    if escopo != Escopo::SoProcessos {
        let mut items = coletar_diarios(config, since, until).await?;
        aplicar(pool, &mut items, &mut report).await?;
    }

    // Processos depois, serial: cada consulta leva ~30 s do lado do DataJud.
    if escopo != Escopo::SoDiarios {
        if config.processos.is_empty() {
            if escopo == Escopo::SoProcessos {
                return Err(AppError::Config(
                    "nenhum processo configurado: preencha `processos` no config.toml".into(),
                ));
            }
        } else {
            info!(
                processos = config.processos.len(),
                estimativa_segundos = config.processos.len() * 30,
                "iniciando scan de {} processos — pode levar ~{}s",
                config.processos.len(),
                config.processos.len() * 30
            );

            let mut items = coletar_processos(config, since, until).await?;
            aplicar(pool, &mut items, &mut report).await?;
        }
    }

    Ok(report)
}

async fn coletar_diarios(
    config: &Config,
    since: NaiveDate,
    until: NaiveDate,
) -> Result<Vec<(String, RawGazette)>> {
    let sources = build_sources_diarios(config)?;

    info!(
        %since,
        %until,
        keywords = config.keywords.len(),
        territorios = config.territories.len(),
        entidades_famurs = config.famurs_entities.len(),
        fontes = sources.len(),
        "iniciando coleta de diarios"
    );

    coletar(sources, config, since, until).await
}

async fn coletar_processos(
    config: &Config,
    since: NaiveDate,
    until: NaiveDate,
) -> Result<Vec<(String, RawGazette)>> {
    let sources = build_sources_processos(config)?;
    coletar(sources, config, since, until).await
}

/// Uma fonte com problema nao deve derrubar as outras: o monitor prefere
/// entregar dados parciais a nao entregar nada. Se *todas* falharem, erra.
async fn coletar(
    sources: Vec<Arc<dyn Source>>,
    config: &Config,
    since: NaiveDate,
    until: NaiveDate,
) -> Result<Vec<(String, RawGazette)>> {
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
            Err(err @ AppError::Config(_)) => {
                // Configuracao errada nao melhora rodando as outras fontes.
                warn!(fonte = name, error = %err, "configuracao invalida; abortando");
                return Err(err);
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

    Ok(collected)
}

async fn aplicar(
    pool: &SqlitePool,
    items: &mut Vec<(String, RawGazette)>,
    report: &mut ScanReport,
) -> Result<()> {
    report.collected += items.len();

    for (source_name, item) in items.drain(..) {
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

    Ok(())
}

fn build_sources_diarios(config: &Config) -> Result<Vec<Arc<dyn Source>>> {
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

fn build_sources_processos(config: &Config) -> Result<Vec<Arc<dyn Source>>> {
    let mut sources: Vec<Arc<dyn Source>> = Vec::new();

    if !config.processos.is_empty() {
        sources.push(Arc::new(DataJudSource::new(
            config.datajud_api_key.clone(),
            config.processos.clone(),
        )?));
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
    use crate::config::ProcessoMonitorado;

    fn config_base() -> Config {
        Config {
            keywords: vec!["+pregao +eletronico".to_string()],
            ..Config::default()
        }
    }

    #[test]
    fn sem_entidades_famurs_so_tem_querido_diario() {
        let sources = build_sources_diarios(&config_base()).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name(), "querido_diario");
    }

    #[test]
    fn cada_entidade_famurs_vira_uma_fonte() {
        let config = Config {
            famurs_entities: vec!["64462".to_string(), "64463".to_string()],
            ..config_base()
        };
        let sources = build_sources_diarios(&config).unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].name(), "querido_diario");
        assert_eq!(sources[1].name(), "famurs");
        assert_eq!(sources[2].name(), "famurs");
    }

    #[test]
    fn diarios_e_processos_ficam_separados() {
        let config = Config {
            processos: vec![ProcessoMonitorado {
                numero: "00011552020248272734".to_string(),
                tribunal: "stj".to_string(),
                apelido: "x".to_string(),
            }],
            ..config_base()
        };

        let diarios = build_sources_diarios(&config).unwrap();
        assert!(diarios.iter().all(|source| source.name() != "datajud"));

        let processos = build_sources_processos(&config).unwrap();
        assert_eq!(processos.len(), 1);
        assert_eq!(processos[0].name(), "datajud");
    }

    #[test]
    fn sem_processos_nao_cria_fonte_datajud() {
        assert!(build_sources_processos(&config_base()).unwrap().is_empty());
    }
}
