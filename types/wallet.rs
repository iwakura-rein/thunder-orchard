use std::collections::BTreeMap;

use bitcoin::Amount;
use serde::{Deserialize, Serialize};
use serde_with::{MapPreventDuplicates, serde_as};
use utoipa::ToSchema;

use crate::address::TransparentAddress;

/// Block heights used to select coins
#[derive(Clone, Copy, Debug)]
pub struct CoinSelectionHeights {
    pub main_height: u32,
    pub spend_height: u32,
}

/// Destinations of a transparent transfer. Each address takes a value in sats.
#[serde_as]
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[schema(value_type = BTreeMap<String, u64>)]
pub struct TransparentTransferDests(
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    pub  BTreeMap<TransparentAddress, u64>,
);

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
