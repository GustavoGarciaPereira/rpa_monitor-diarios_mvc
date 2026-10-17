use std::time::Duration;

use reqwest::{Client, Response, StatusCode};
use tracing::warn;

use crate::error::{AppError, Result};

/// GET com timeout, retry e backoff exponencial.
///
/// A API do Querido Diario oscila: durante o levantamento ~1/3 das chamadas
/// estourou timeout e apareceram 503/520 do Cloudflare. Retornar o `Response`
/// final (mesmo 4xx) deixa a decisao de erro para o chamador.
pub async fn get_with_retry(
    client: &Client,
    url: &str,
    params: &[(&str, String)],
    max_attempts: u32,
) -> Result<Response> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match client.get(url).query(params).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || !is_retryable(status) || attempt >= max_attempts {
                    return Ok(response);
                }
                warn!(
                    status = status.as_u16(),
                    attempt,
                    url,
                    "resposta transitoria, repetindo"
                );
                sleep_backoff(attempt).await;
            }
            Err(err) => {
                if attempt >= max_attempts {
                    return Err(AppError::Unavailable {
                        attempts: attempt,
                        reason: err.to_string(),
                    });
                }
                warn!(error = %err, attempt, url, "falha de rede, repetindo");
                sleep_backoff(attempt).await;
            }
        }
    }
}

fn is_retryable(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
}

async fn sleep_backoff(attempt: u32) {
    let secs = 2u64.pow(attempt.min(3));
    tokio::time::sleep(Duration::from_secs(secs)).await;
}
