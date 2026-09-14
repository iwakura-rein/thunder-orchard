use std::{borrow::Borrow, cmp::Ordering, collections::HashMap};

use bitcoin::amount::CheckedSum as _;
use borsh::BorshSerialize;
use rustreexo::accumulator::mem_forest::MemForest;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    authorization::Authorization,
    block::coinbase::Coinbase,
    error,
    hashes::{self, Hash, MerkleRoot, UtreexoNodeHash},
    transaction::{
        AuthorizedTransaction, GetValue, OutPoint, Output, PointedOutput,
        Transaction,
    },
};

// Internal node of a CBMT
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CbmtNode {
    // Commitment to child nodes or leaf value
    commitment: Hash,
    // CBT index, see https://github.com/nervosnetwork/merkle-tree/blob/5d1898263e7167560fdaa62f09e8d52991a1c712/README.md#tree-struct
    // This is required so that `CbmtNode` can be `Ord` correctly
    index: usize,
}

impl PartialOrd for CbmtNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CbmtNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.index.cmp(&other.index)
    }
}

// Marker type for merging branch commitments
struct Merge;

impl merkle_cbt::merkle_tree::Merge for Merge {
    type Item = CbmtNode;

    fn merge(lnode: &Self::Item, rnode: &Self::Item) -> Self::Item {
        // see https://github.com/nervosnetwork/merkle-tree/blob/5d1898263e7167560fdaa62f09e8d52991a1c712/README.md#tree-struct
        assert_eq!(lnode.index + 1, rnode.index);
        let index = (lnode.index - 1) / 2;
        let commitment = hashes::hash(&(&lnode.commitment, &rnode.commitment));
        CbmtNode { commitment, index }
    }
}

// Complete binary merkle tree
type Cbmt = merkle_cbt::CBMT<CbmtNode, Merge>;

#[derive(BorshSerialize, Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Body {
    pub coinbase: Coinbase,
    pub transactions: Vec<Transaction>,
    pub authorizations: Vec<Authorization>,
}

impl Body {
    pub fn new(
        authorized_transactions: Vec<AuthorizedTransaction>,
        coinbase: Coinbase,
    ) -> Self {
        let mut authorizations = Vec::with_capacity(
            authorized_transactions
                .iter()
                .map(|t| t.transaction.inputs.len())
                .sum(),
        );
        let mut transactions =
            Vec::with_capacity(authorized_transactions.len());
        for at in authorized_transactions.into_iter() {
            authorizations.extend(at.authorizations);
            transactions.push(at.transaction);
        }
        Self {
            coinbase,
            transactions,
            authorizations,
        }
    }

    pub fn authorized_transactions(&self) -> Vec<AuthorizedTransaction> {
        let mut authorizations_iter = self.authorizations.iter();
        self.transactions
            .iter()
            .map(|tx| {
                let mut authorizations = Vec::with_capacity(tx.inputs.len());
                for _ in 0..tx.inputs.len() {
                    let auth = authorizations_iter.next().unwrap();
                    authorizations.push(auth.clone());
                }
                AuthorizedTransaction {
                    transaction: tx.clone(),
                    authorizations,
                }
            })
            .collect()
    }

    fn compute_txs_commitment<Tx>(txs: &[Tx]) -> Hash
    where
        Tx: Borrow<Transaction>,
    {
        let n_txs = txs.len();
        let leaves: Vec<_> = txs
            .iter()
            .enumerate()
            .map(|(idx, tx)| CbmtNode {
                commitment: tx.borrow().compute_merkle_root().into(),
                // see https://github.com/nervosnetwork/merkle-tree/blob/5d1898263e7167560fdaa62f09e8d52991a1c712/README.md#tree-struct
                index: (idx + n_txs) - 1,
            })
            .collect();
        Cbmt::build_merkle_root(leaves.as_slice()).commitment
    }

    pub fn compute_merkle_root(&self) -> MerkleRoot {
        let Self {
            coinbase,
            transactions,
            authorizations: _,
        } = self;
        // Borsh encoding for hashing
        #[derive(BorshSerialize)]
        struct HashComponents {
            coinbase_commitment: MerkleRoot,
            txs_commitment: Hash,
        }
        hashes::hash(&HashComponents {
            coinbase_commitment: coinbase.compute_merkle_root(),
            txs_commitment: Self::compute_txs_commitment(transactions),
        })
        .into()
    }

    // Modifies the memforest, without checking tx proofs
    pub fn modify_memforest(
        &self,
        memforest: &mut MemForest<UtreexoNodeHash>,
    ) -> Result<(), String> {
        // New leaves for the accumulator
        let mut accumulator_add = Vec::<UtreexoNodeHash>::new();
        // Accumulator leaves to delete
        let mut accumulator_del = Vec::<UtreexoNodeHash>::new();
        let merkle_root = self.compute_merkle_root();
        for (vout, output) in self.coinbase.outputs.iter().enumerate() {
            let outpoint = OutPoint::Coinbase {
                merkle_root,
                vout: vout as u32,
            };
            let pointed_output = PointedOutput {
                outpoint,
                output: output.clone(),
            };
            accumulator_add.push((&pointed_output).into());
        }
        for transaction in &self.transactions {
            let txid = transaction.txid();
            for (_, utxo_hash) in transaction.inputs.iter() {
                accumulator_del.push(utxo_hash.into());
            }
            for (vout, output) in transaction.outputs.iter().enumerate() {
                let outpoint = OutPoint::Regular {
                    txid,
                    vout: vout as u32,
                };
                let pointed_output = PointedOutput {
                    outpoint,
                    output: output.clone(),
                };
                accumulator_add.push((&pointed_output).into());
            }
        }
        memforest.modify(&accumulator_add, &accumulator_del)
    }

    pub fn get_inputs(&self) -> Vec<OutPoint> {
        self.transactions
            .iter()
            .flat_map(|tx| tx.inputs.iter().map(|(outpoint, _)| outpoint))
            .copied()
            .collect()
    }

    pub fn get_outputs(&self) -> HashMap<OutPoint, Output> {
        let mut outputs = HashMap::new();
        let merkle_root = self.compute_merkle_root();
        for (vout, output) in self.coinbase.outputs.iter().enumerate() {
            let vout = vout as u32;
            let outpoint = OutPoint::Coinbase { merkle_root, vout };
            outputs.insert(outpoint, output.clone());
        }
        for transaction in &self.transactions {
            let txid = transaction.txid();
            for (vout, output) in transaction.outputs.iter().enumerate() {
                let vout = vout as u32;
                let outpoint = OutPoint::Regular { txid, vout };
                outputs.insert(outpoint, output.clone());
            }
        }
        outputs
    }

    pub fn get_coinbase_value(
        &self,
    ) -> Result<bitcoin::Amount, error::AmountOverflow> {
        self.coinbase
            .outputs
            .iter()
            .map(|output| output.get_value())
            .checked_sum()
            .ok_or(error::AmountOverflow)
    }
}
