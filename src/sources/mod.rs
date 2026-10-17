use async_trait::async_trait;
use chrono::NaiveDate;

use crate::error::Result;
use crate::model::RawGazette;

pub mod querido_diario;

pub use querido_diario::QueridoDiario;

/// Contrato de uma fonte de diarios oficiais.
///
/// Adicionar uma fonte nova = criar um modulo aqui e implementar esta trait.
/// Nada mais no resto do sistema precisa mudar.
#[async_trait]
pub trait Source: Send + Sync {
    /// Identificador estavel da fonte (entra no hash de deduplicacao).
    fn name(&self) -> &'static str;

    /// Coleta publicacoes no periodo que casem com as keywords.
    ///
    /// `territories` vazio significa busca nacional (quando a fonte suportar).
    /// Cada item retornado deve trazer `matched_keyword` preenchido.
    async fn fetch(
        &self,
        since: NaiveDate,
        until: NaiveDate,
        keywords: &[String],
        territories: &[String],
    ) -> Result<Vec<RawGazette>>;
}
