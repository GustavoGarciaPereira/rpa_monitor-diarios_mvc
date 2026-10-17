mod commands;
mod config;
mod db;
mod error;
mod http;
mod model;
mod sources;

use std::path::PathBuf;

use anyhow::Context;
use chrono::{Duration, Local, NaiveDate};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "monitor-diarios",
    version,
    about = "Monitor de diarios oficiais com alerta por palavra-chave"
)]
struct Cli {
    /// Caminho do arquivo de configuracao
    #[arg(long, global = true, env = "MONITOR_CONFIG", default_value = "config.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Coleta publicacoes no periodo e grava as que ainda nao foram vistas
    Scan {
        /// Data inicial (YYYY-MM-DD). Default: 7 dias antes de --until
        #[arg(long)]
        since: Option<NaiveDate>,

        /// Data final (YYYY-MM-DD). Default: hoje em America/Sao_Paulo
        #[arg(long)]
        until: Option<NaiveDate>,
    },

    /// Lista as cidades cobertas pela fonte, para montar `territories`
    Territories {
        /// Filtra por nome da cidade
        #[arg(long)]
        name: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `.env` e opcional: se nao existir, seguimos so com TOML + env do shell.
    let _ = dotenvy::dotenv();

    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Command::Scan { since, until } => {
            let config = config::Config::load(&cli.config)?;

            // `Local` (nao `Utc`): as datas dos diarios sao do fuso de Brasilia.
            // Com Utc, um scan as 21h de Brasilia cairia no dia seguinte.
            let until = until.unwrap_or_else(|| Local::now().date_naive());
            let since = since.unwrap_or_else(|| until - Duration::days(7));

            if since > until {
                anyhow::bail!("--since ({since}) e posterior a --until ({until})");
            }

            let pool = db::init(&config.database_url)
                .await
                .context("abrindo o banco")?;

            let report = commands::scan::run(&config, &pool, since, until)
                .await
                .context("executando scan")?;

            println!();
            println!(
                "{} novos alertas, {} publicacoes novas — janela {}..{}",
                report.alerts_new, report.gazettes_new, since, until
            );
            println!(
                "{} itens coletados | {} alertas ja conhecidos (deduplicados)",
                report.collected, report.duplicates
            );
            Ok(())
        }

        Command::Territories { name } => {
            commands::territories::run(name.as_deref()).await?;
            Ok(())
        }
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
