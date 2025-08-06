use sneed::{db::error as db, env::error as env, rwtxn::error as rwtxn};
use thiserror::Error;
use transitive::Transitive;

use crate::types::{
    AmountOverflowError, AmountUnderflowError, BlockHash, M6id, MerkleRoot,
    OutPoint, Txid, UtreexoError, WithdrawalBundleError, orchard,
};

#[derive(Debug, Error)]
#[error(
    "invalid body: expected merkle root {expected}, but computed {computed}"
)]
pub struct InvalidBody {
    pub expected: MerkleRoot,
    pub computed: MerkleRoot,
}

#[derive(Debug, Error)]
pub enum InvalidHeader {
    #[error("expected block hash {expected}, but computed {computed}")]
    BlockHash {
        expected: BlockHash,
        computed: BlockHash,
    },
    #[error(
        "expected previous sidechain block hash {expected:?}, but received {received:?}"
    )]
    PrevSideHash {
        expected: Option<BlockHash>,
        received: Option<BlockHash>,
    },
}

#[derive(Debug, Error, Transitive)]
#[transitive(from(db::Delete, db::Error))]
#[transitive(from(db::Get, db::Error))]
#[transitive(from(db::Last, db::Error))]
#[transitive(from(db::Put, db::Error))]
#[transitive(from(db::TryGet, db::Error))]
pub enum Orchard {
    #[error("Cannot append commitment to frontier: would exceed max depth")]
    AppendCommitment,
    #[error(transparent)]
    Db(#[from] Box<db::Error>),
    #[error("The empty anchor is only allowed if spends are disabled")]
    EmptyAnchor,
    #[error("Invalid anchor (`{anchor}`)")]
    InvalidAnchor { anchor: orchard::Anchor },
    #[error("Nullifier missing (`{nullifier}`)")]
    MissingNullifier { nullifier: orchard::Nullifier },
    #[error("Nullifier double spent (`{nullifier}`)")]
    NullifierDoubleSpent { nullifier: orchard::Nullifier },
}

impl From<db::Error> for Orchard {
    fn from(err: db::Error) -> Self {
        Self::Db(Box::new(err))
    }
}

#[derive(Debug, Error)]
#[error("utxo {outpoint} doesn't exist")]
pub struct NoUtxo {
    pub outpoint: OutPoint,
}

#[derive(Debug, Error, Transitive)]
#[transitive(from(db::Delete, db::Error))]
#[transitive(from(db::Put, db::Error))]
#[transitive(from(db::TryGet, db::Error))]
pub enum ConnectTransaction {
    #[error(transparent)]
    Db(#[from] Box<db::Error>),
    #[error(transparent)]
    NoUtxo(#[from] NoUtxo),
    #[error("Orchard error")]
    Orchard(#[from] Orchard),
}

impl From<db::Error> for ConnectTransaction {
    fn from(err: db::Error) -> Self {
        Self::Db(Box::new(err))
    }
}

#[derive(Debug, Error, Transitive)]
#[transitive(from(db::Get, db::Error))]
#[transitive(from(db::Put, db::Error))]
#[transitive(from(db::TryGet, db::Error))]
pub enum ConnectBlock {
    #[error("error connecting transaction (`{txid}`)")]
    ConnectTransaction {
        txid: Txid,
        source: ConnectTransaction,
    },
    #[error(transparent)]
    Db(#[from] Box<db::Error>),
    #[error(transparent)]
    InvalidBody(#[from] InvalidBody),
    #[error("invalid header: {0}")]
    InvalidHeader(InvalidHeader),
    #[error("Orchard error")]
    Orchard(#[from] Orchard),
    #[error(transparent)]
    Utreexo(#[from] UtreexoError),
}

impl From<db::Error> for ConnectBlock {
    fn from(err: db::Error) -> Self {
        Self::Db(Box::new(err))
    }
}

#[derive(Debug, Error, Transitive)]
#[transitive(
    from(db::Clear, db::Error),
    from(db::Delete, db::Error),
    from(db::Error, sneed::Error),
    from(db::Get, db::Error),
    from(db::Iter, db::Error),
    from(db::IterInit, db::Error),
    from(db::IterItem, db::Error),
    from(db::Last, db::Error),
    from(db::Len, db::Error),
    from(db::Put, db::Error),
    from(db::TryGet, db::Error),
    from(env::CreateDb, env::Error),
    from(env::Error, sneed::Error),
    from(env::WriteTxn, env::Error),
    from(rwtxn::Commit, rwtxn::Error),
    from(rwtxn::Error, sneed::Error)
)]
pub enum Error {
    #[error("failed to verify authorization")]
    AuthorizationError,
    #[error(transparent)]
    AmountOverflow(#[from] AmountOverflowError),
    #[error(transparent)]
    AmountUnderflow(#[from] AmountUnderflowError),
    #[error("body too large")]
    BodyTooLarge,
    #[error(transparent)]
    BorshSerialize(borsh::io::Error),
    #[error("failed to connect block")]
    ConnectBlock(#[from] ConnectBlock),
    #[error(transparent)]
    Db(#[from] Box<sneed::Error>),
    #[error(transparent)]
    InvalidBody(InvalidBody),
    #[error("invalid header: {0}")]
    InvalidHeader(InvalidHeader),
    #[error("deposit block doesn't exist")]
    NoDepositBlock,
    #[error("total fees less than coinbase value")]
    NotEnoughFees,
    #[error("no tip")]
    NoTip,
    #[error("stxo {outpoint} doesn't exist")]
    NoStxo { outpoint: OutPoint },
    #[error("value in is less than value out")]
    NotEnoughValueIn,
    #[error(transparent)]
    NoUtxo(#[from] NoUtxo),
    #[error("Withdrawal bundle event block doesn't exist")]
    NoWithdrawalBundleEventBlock,
    #[error("Orchard error")]
    Orchard(#[from] Orchard),
    #[error(transparent)]
    Utreexo(#[from] UtreexoError),
    #[error("Utreexo proof verification failed for tx {txid}")]
    UtreexoProofFailed { txid: Txid },
    #[error("Computed Utreexo roots do not match the header roots")]
    UtreexoRootsMismatch,
    #[error("utxo double spent")]
    UtxoDoubleSpent,
    #[error("too many sigops")]
    TooManySigops,
    #[error("Unknown withdrawal bundle: {m6id}")]
    UnknownWithdrawalBundle { m6id: M6id },
    #[error(
        "Unknown withdrawal bundle confirmed in {event_block_hash}: {m6id}"
    )]
    UnknownWithdrawalBundleConfirmed {
        event_block_hash: bitcoin::BlockHash,
        m6id: M6id,
    },
    #[error("wrong public key for address")]
    WrongPubKeyForAddress,
    #[error(transparent)]
    WithdrawalBundle(#[from] WithdrawalBundleError),
}

impl From<sneed::Error> for Error {
    fn from(err: sneed::Error) -> Self {
        Self::Db(Box::new(err))
    }
}
