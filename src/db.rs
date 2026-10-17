use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::error::{AppError, Result};

/// Abre o pool SQLite (criando o arquivo se preciso) e roda as migracoes.
///
/// `create_if_missing` e obrigatorio: `SqlitePool::connect("sqlite:x.db")`
/// falha quando o arquivo nao existe, o que quebraria o primeiro `scan`.
pub async fn init(url: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(url)
        .map_err(|e| AppError::Config(format!("database_url invalida ({url}): {e}")))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
