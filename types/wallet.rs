use bitcoin::Amount;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct Balance {
    #[serde(
        rename = "total_shielded_sats",
        with = "bitcoin::amount::serde::as_sat"
    )]
    #[schema(value_type = u64)]
    pub total_shielded: Amount,
    #[serde(
        rename = "total_transparent_sats",
        with = "bitcoin::amount::serde::as_sat"
    )]
    #[schema(value_type = u64)]
    pub total_transparent: Amount,
    #[serde(
        rename = "available_shielded_sats",
        with = "bitcoin::amount::serde::as_sat"
    )]
    #[schema(value_type = u64)]
    pub available_shielded: Amount,
    #[serde(
        rename = "available_transparent_sats",
        with = "bitcoin::amount::serde::as_sat"
    )]
    #[schema(value_type = u64)]
    pub available_transparent: Amount,
}

impl Balance {
    /// Get the total balance
    pub fn total(&self) -> Amount {
        self.total_shielded + self.total_transparent
    }

    /// Get the total available amount
    pub fn available(&self) -> Amount {
        self.available_shielded + self.available_transparent
    }
}
