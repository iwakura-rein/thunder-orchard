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

#[allow(clippy::duplicated_attributes)]
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

#[allow(clippy::duplicated_attributes)]
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
    #[error("Utreexo proof verification failed")]
    UtreexoProofFailed,
}

impl From<db::Error> for ConnectTransaction {
    fn from(err: db::Error) -> Self {
        Self::Db(Box::new(err))
    }
}

#[allow(clippy::duplicated_attributes)]
#[derive(Debug, Error, Transitive)]
#[transitive(from(db::Delete, db::Error))]
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
    #[error("failed to verify authorization")]
    AuthorizationError,
    #[error("total fees less than coinbase value")]
    NotEnoughFees,
    #[error("wrong public key for address")]
    WrongPubKeyForAddress,
    #[error("Computed Utreexo roots do not match the header roots")]
    UtreexoRootsMismatch,
    #[error("too many sigops")]
    TooManySigops,
    #[error("body too large")]
    BodyTooLarge,
    #[error("utxo double spent")]
    UtxoDoubleSpent,
    #[error("other error: {0}")]
    Other(Box<crate::state::Error>),
}

impl From<db::Error> for ConnectBlock {
    fn from(err: db::Error) -> Self {
        Self::Db(Box::new(err))
    }
}

#[derive(Debug, Error)]
#[error("pending withdrawal bundle {0} unknown in withdrawal_bundles")]
#[repr(transparent)]
pub struct PendingWithdrawalBundleUnknown(pub M6id);

#[allow(clippy::duplicated_attributes)]
#[derive(Debug, Error, Transitive)]
#[transitive(
    from(db::Delete, db::Error),
    from(db::Put, db::Error),
    from(db::TryGet, db::Error)
)]
pub enum ConnectWithdrawalBundleSubmitted {
    #[error(
        "confirmed withdrawal bundle {} resubmitted in {}",
        .m6id,
        .event_block_hash,
    )]
    ConfirmedResubmitted {
        event_block_hash: bitcoin::BlockHash,
        m6id: M6id,
    },
    #[error(transparent)]
    Db(Box<db::Error>),
    #[error(
        "dropped withdrawal bundle {0} marked as pending in withdrawal_bundles"
    )]
    DroppedPending(M6id),
    #[error(transparent)]
    NoUtxo(#[from] NoUtxo),
    #[error(transparent)]
    PendingWithdrawalBundleUnknown(#[from] PendingWithdrawalBundleUnknown),
    #[error(
        "withdrawal bundle {} submitted in {} resubmitted in {}",
        m6id,
        submitted_block_height,
        event_block_hash
    )]
    Resubmitted {
        event_block_hash: bitcoin::BlockHash,
        m6id: M6id,
        submitted_block_height: u32,
    },
    #[error(
        "unknown confirmed withdrawal bundle {} marked as failed in {}",
        .m6id,
        .failed_block_height,
    )]
    UnknownConfirmedFailed {
        m6id: M6id,
        failed_block_height: u32,
    },
    #[error(
        "unknown withdrawal bundle {} marked as dropped in {}",
        .m6id,
        .dropped_block_height,
    )]
    UnknownDropped {
        m6id: M6id,
        dropped_block_height: u32,
    },
    #[error(
        "unknown withdrawal bundle {} marked as pending in {}",
        .m6id,
        .pending_block_height,
    )]
    UnknownPending {
        m6id: M6id,
        pending_block_height: u32,
    },
}

impl From<db::Error> for ConnectWithdrawalBundleSubmitted {
    fn from(err: db::Error) -> Self {
        Self::Db(Box::new(err))
    }
}

#[allow(clippy::duplicated_attributes)]
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
    ConnectWithdrawalBundleSubmitted(#[from] ConnectWithdrawalBundleSubmitted),
    #[error(transparent)]
    Db(Box<sneed::Error>),
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
    PendingWithdrawalBundleUnknown(#[from] PendingWithdrawalBundleUnknown),
    #[error(transparent)]
    Utreexo(#[from] UtreexoError),
    #[error("Utreexo proof verification failed for tx {txid}")]
    UtreexoProofFailed { txid: Txid },
    #[error("Computed Utreexo roots do not match the header roots")]
    UtreexoRootsMismatch,
    #[error("utxo double spent")]
    UtxoDoubleSpent,
    #[error(
        "Computed Utxo hash ({}) for input ({}) does not match input hash ({})",
        hex::encode(.computed),
        .outpoint,
        hex::encode(.input_hash),
    )]
    UtxoHashMismatch {
        computed: crate::types::Hash,
        outpoint: OutPoint,
        input_hash: crate::types::Hash,
    },
    #[error("too many sigops")]
    TooManySigops,
    #[error(
        "protocol would be insolvent after confirming unexpected withdrawal bundle {} in {}; bundle outpoint {} already spent",
        .m6id,
        .event_block_hash,
        .outpoint,
    )]
    UnexpectedWithdrawalBundleInsolvency {
        event_block_hash: bitcoin::BlockHash,
        m6id: M6id,
        outpoint: OutPoint,
    },
    #[error("Unknown withdrawal bundle: {m6id}")]
    UnknownWithdrawalBundle { m6id: M6id },
    #[error(
        "Unknown withdrawal bundle confirmed in {event_block_hash}: {m6id}"
    )]
    UnknownWithdrawalBundleConfirmed {
        event_block_hash: bitcoin::BlockHash,
        m6id: M6id,
    },
    #[error(
        "Unknown confirmed withdrawal bundle reconfirmed in {event_block_hash}: {m6id}"
    )]
    UnknownWithdrawalBundleReconfirmed {
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
