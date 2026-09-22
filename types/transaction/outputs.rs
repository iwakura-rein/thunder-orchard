use std::cmp::Ordering;

use borsh::BorshSerialize;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    hashes::{self, Hash, OutputsMerkleRoot},
    transaction::output::Output,
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

#[derive(
    BorshSerialize, Clone, Debug, Default, Deserialize, Serialize, ToSchema,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Outputs(pub Vec<Output>);

impl Outputs {
    #[inline(always)]
    pub fn as_slice(&self) -> &[Output] {
        self.0.as_slice()
    }

    fn merkle_leaves(&self) -> Vec<CbmtNode> {
        let n_outputs = self.len();
        self.iter()
            .enumerate()
            .map(|(idx, output)| CbmtNode {
                commitment: hashes::hash(&output),
                // see https://github.com/nervosnetwork/merkle-tree/blob/5d1898263e7167560fdaa62f09e8d52991a1c712/README.md#tree-struct
                index: (idx + n_outputs) - 1,
            })
            .collect()
    }

    pub(crate) fn compute_merkle_root(&self) -> OutputsMerkleRoot {
        let CbmtNode { commitment, .. } =
            Cbmt::build_merkle_root(self.merkle_leaves().as_slice());
        commitment.into()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline(always)]
    pub fn iter(&self) -> std::slice::Iter<'_, Output> {
        self.0.iter()
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline(always)]
    pub fn push(&mut self, output: Output) {
        self.0.push(output)
    }

    #[inline(always)]
    pub fn remove(&mut self, index: usize) -> Output {
        self.0.remove(index)
    }
}

impl From<Vec<Output>> for Outputs {
    #[inline(always)]
    fn from(outputs: Vec<Output>) -> Self {
        Self(outputs)
    }
}

impl IntoIterator for Outputs {
    type IntoIter = <Vec<Output> as IntoIterator>::IntoIter;
    type Item = Output;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Outputs {
    type IntoIter = <&'a Vec<Output> as IntoIterator>::IntoIter;
    type Item = &'a Output;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod test {
    use crate::{
        address::TransparentAddress,
        transaction::{
            output::{self, Output},
            outputs::{Cbmt, Outputs},
        },
    };

    #[test]
    fn test_merkle_proof() -> anyhow::Result<()> {
        use rand::{
            Rng as _, RngExt as _, SeedableRng as _, rngs::ChaCha20Rng,
        };
        const N_OUTPUTS: usize = 10;
        let max_output_value = bitcoin::Amount::from_sat(
            bitcoin::Amount::MAX_MONEY.to_sat() / N_OUTPUTS as u64,
        );
        let mut rng = ChaCha20Rng::from_rng(&mut rand::rng());
        let outputs: Outputs = (0..N_OUTPUTS)
            .map(|_| {
                let mut address = TransparentAddress([0; 20]);
                rng.fill_bytes(&mut address.0);
                let value_sats = rng.random_range(1..max_output_value.to_sat());
                let value = bitcoin::Amount::from_sat(value_sats);
                Output {
                    address,
                    content: output::Content::Value(value),
                }
            })
            .collect::<Vec<_>>()
            .into();
        let merkle_leaves = outputs.merkle_leaves();
        let merkle_tree = Cbmt::build_merkle_tree(merkle_leaves.as_slice());
        let merkle_root = merkle_tree.root();
        let root_commitment = merkle_root.commitment;
        anyhow::ensure!(root_commitment == outputs.compute_merkle_root().0);
        // select a random output
        let output_idx = rng.random_range(0..outputs.len());
        let output = merkle_leaves[output_idx].clone();
        let merkle_proof = merkle_tree
            .build_proof(&[output_idx as u32])
            .ok_or_else(|| anyhow::anyhow!("generating merkle proof failed"))?;
        if !merkle_proof.verify(&merkle_root, &[output]) {
            anyhow::bail!("verifying merkle proof failed")
        }
        Ok(())
    }
}
