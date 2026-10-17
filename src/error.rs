use thiserror::Error;

/// Erros de dominio da aplicacao.
///
/// Camadas de baixo (fonte de dados, banco) convertem seus erros para ca.
/// O `main` so orquestra e devolve `anyhow::Result`.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("falha de rede: {0}")]
    Http(#[from] reqwest::Error),

    #[error("falha de banco: {0}")]
    Db(#[from] sqlx::Error),

    #[error("falha nas migracoes: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("configuracao invalida: {0}")]
    Config(String),

    #[error("resposta inesperada da fonte: {0}")]
    Parse(String),

    #[error("falha ao renderizar template: {0}")]
    Template(String),

    #[error("falha de I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("fonte indisponivel apos {attempts} tentativas: {reason}")]
    Unavailable { attempts: u32, reason: String },
}

pub type Result<T> = std::result::Result<T, AppError>;
