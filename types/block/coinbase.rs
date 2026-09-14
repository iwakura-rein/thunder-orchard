use borsh::BorshSerialize;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    hashes::{self, MerkleRoot},
    transaction::outputs::Outputs,
};

#[derive(
    BorshSerialize, Clone, Debug, Default, Deserialize, Serialize, ToSchema,
)]
pub struct Coinbase {
    pub memo: Vec<u8>,
    pub outputs: Outputs,
}

impl Coinbase {
    pub(crate) fn compute_merkle_root(&self) -> MerkleRoot {
        let Self { memo, outputs } = self;
        let outputs_commitment = outputs.compute_merkle_root();
        hashes::hash(&(memo, outputs_commitment)).into()
    }
}
