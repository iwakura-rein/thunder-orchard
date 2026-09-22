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

    pub(crate) fn compute_merkle_root(&self) -> OutputsMerkleRoot {
        let CbmtNode { commitment, .. } = {
            let n_outputs = self.len();
            let leaves: Vec<CbmtNode> = self
                .iter()
                .enumerate()
                .map(|(idx, output)| CbmtNode {
                    commitment: hashes::hash(&output),
                    // see https://github.com/nervosnetwork/merkle-tree/blob/5d1898263e7167560fdaa62f09e8d52991a1c712/README.md#tree-struct
                    index: (idx + n_outputs) - 1,
                })
                .collect();
            Cbmt::build_merkle_root(leaves.as_slice())
        };
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
