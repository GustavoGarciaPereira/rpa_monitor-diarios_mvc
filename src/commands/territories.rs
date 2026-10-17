use std::time::Duration;

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::sources::querido_diario::BASE_URL;

#[derive(Debug, Deserialize)]
struct CitiesResponse {
    #[serde(default)]
    cities: Vec<City>,
}

#[derive(Debug, Deserialize)]
struct City {
    territory_id: String,
    territory_name: String,
    state_code: String,
    #[serde(default)]
    level: String,
}

/// Lista as cidades cobertas pela fonte, para o usuario montar `territories`.
///
/// `--name` filtra por nome (a API faz busca por similaridade).
pub async fn run(name: Option<&str>) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .user_agent(concat!("monitor-diarios/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(name) = name {
        params.push(("city_name", name.to_string()));
    }

    let url = format!("{BASE_URL}/cities");
    let response = crate::http::get_with_retry(&client, &url, &params, 4).await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Parse(format!(
            "HTTP {} em /cities: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        )));
    }

    let parsed: CitiesResponse = response.json().await?;
    if parsed.cities.is_empty() {
        println!("Nenhuma cidade encontrada.");
        return Ok(());
    }

    println!("{:<9}  {:<34} {:<3}  NIVEL", "ID", "CIDADE", "UF");
    println!("{}", "-".repeat(58));
    for city in &parsed.cities {
        println!(
            "{:<9}  {:<34} {:<3}  {}",
            city.territory_id,
            truncate(&city.territory_name, 34),
            city.state_code,
            city.level
        );
    }

    println!();
    println!(
        "{} cidade(s). Copie os IDs para `territories` no config.toml.",
        parsed.cities.len()
    );
    println!("Nivel 0 = sem dados publicados; 1-3 = cobertura crescente.");
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}
