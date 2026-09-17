use std::path::PathBuf;

use error_fatality::{Fatality, Split};
use sneed::{db::error as db, env::error as env, rwtxn::error as rwtxn};
use thiserror::Error;
use transitive::Transitive;

use crate::types::{
    AmountOverflowError, AmountUnderflowError, BlockHash, Hash, M6id,
    MerkleRoot, OutPoint, TransparentAddress, Txid, UtreexoError, Version,
    WithdrawalBundleError, error, orchard,
};

#[derive(Debug, Error)]
#[error(
    "Computed Utxo hash ({}) for input ({}) does not match input hash ({})",
    const_hex::encode(.computed),
    .outpoint,
    const_hex::encode(.input_hash),
)]
pub struct UtxoHashMismatch {
    pub(in crate::state) computed: Hash,
    pub(in crate::state) outpoint: OutPoint,
    pub(in crate::state) input_hash: Hash,
}

#[derive(Debug, Error, Fatality, Split)]
pub enum ValidateFilledTransaction {
    #[error(transparent)]
    #[fatal(false)]
    AmountOverflow(#[from] AmountOverflowError),
    #[error(transparent)]
    #[fatal(false)]
    AmountUnderflow(#[from] AmountUnderflowError),
    #[error("value in is less than value out")]
    #[fatal(false)]
    NotEnoughValueIn,
    #[error("withdrawal output {outpoint} cannot be spent by a transaction")]
    #[fatal(false)]
    SpendWithdrawalOutput { outpoint: OutPoint },
    #[error(transparent)]
    #[fatal(false)]
    UtxoHashMismatch(#[from] Box<UtxoHashMismatch>),
}

impl From<UtxoHashMismatch> for ValidateFilledTransaction {
    fn from(err: UtxoHashMismatch) -> Self {
        Self::UtxoHashMismatch(Box::new(err))
    }
}

#[derive(Debug, Error)]
#[error("utxo {outpoint} doesn't exist")]
#[repr(transparent)]
pub struct NoUtxo {
    pub outpoint: OutPoint,
}

/// Non-fatal variants indicate tx rejection reason
#[derive(Debug, Error, Fatality)]
pub enum FillTransaction {
    #[error(transparent)]
    #[fatal(true)]
    DbTryGet(#[from] Box<db::TryGet>),
    #[error(transparent)]
    #[fatal(false)]
    NoUtxo(#[from] NoUtxo),
}

impl From<db::TryGet> for FillTransaction {
    fn from(err: db::TryGet) -> Self {
        Self::DbTryGet(Box::new(err))
    }
}

impl Split for FillTransaction {
    type Fatal = db::TryGet;
    type Jfyi = NoUtxo;

    fn split(self) -> std::result::Result<Self::Jfyi, Self::Fatal> {
        match self {
            Self::DbTryGet(err) => Err(*err),
            Self::NoUtxo(jfyi) => Ok(jfyi),
        }
    }
}

/// Non-fatal variants indicate tx rejection reason
#[derive(Debug, Error, Fatality, Split)]
pub enum ValidateOrchardAnchor {
    #[error(transparent)]
    #[fatal(true)]
    DbTryGet(#[from] Box<db::TryGet>),
    #[error("The empty anchor is only allowed if spends are disabled")]
    #[fatal(false)]
    EmptyAnchor,
    #[error("Invalid anchor (`{anchor}`)")]
    #[fatal(false)]
    InvalidAnchor { anchor: orchard::Anchor },
}

impl From<db::TryGet> for ValidateOrchardAnchor {
    fn from(err: db::TryGet) -> Self {
        Self::DbTryGet(Box::new(err))
    }
}

/// Non-fatal variants indicate tx rejection reason
#[derive(Debug, Error, Fatality, Split)]
pub enum ValidateTransaction {
    #[error("failed to verify authorizations")]
    #[fatal(forward)]
    Authorization(#[from] crate::types::AuthorizationError),
    #[error(transparent)]
    #[fatal(forward)]
    Filled(#[from] ValidateFilledTransaction),
    #[error(transparent)]
    #[fatal(forward)]
    FillTransaction(#[from] FillTransaction),
    #[error("failed to validate orchard anchor")]
    #[fatal(forward)]
    OrchardAnchor(#[from] ValidateOrchardAnchor),
    #[error("wrong verifying key hash ({vk_hash}) for address ({address})")]
    #[fatal(false)]
    WrongVerifyingKeyForAddress {
        address: TransparentAddress,
        vk_hash: TransparentAddress,
    },
}

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

#[derive(Debug, Error, Fatality, Split)]
pub enum RegenerateProof {
    #[error(transparent)]
    #[fatal(true)]
    DbTryGet(#[from] db::TryGet),
    #[error("failed to generate proof")]
    #[fatal(false)]
    Prove(#[source] UtreexoError),
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
    #[error("failed to verify authorization")]
    Authorization(#[from] error::Authorization),
    #[error("body too large")]
    BodyTooLarge,
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
    #[error("total fees less than coinbase value")]
    NotEnoughFees,
    #[error("Orchard error")]
    Orchard(#[from] Orchard),
    #[error("other error: {0}")]
    Other(Box<crate::state::Error>),
    #[error("too many sigops")]
    TooManySigops,
    #[error(transparent)]
    Utreexo(#[from] UtreexoError),
    #[error("Computed Utreexo roots do not match the header roots")]
    UtreexoRootsMismatch,
    #[error("utxo double spent")]
    UtxoDoubleSpent,
    #[error("wrong public key for address")]
    WrongPubKeyForAddress,
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
    Authorization(#[from] error::Authorization),
    #[error(transparent)]
    AmountOverflow(#[from] AmountOverflowError),
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
    #[error("failed to fill inputs for tx ({txid})")]
    FillTransaction { source: FillTransaction, txid: Txid },
    #[error(
        "Incompatible DB version ({}). Please clear the DB (`{}`) and re-sync",
        .version,
        .db_path.display()
    )]
    IncompatibleVersion { version: Version, db_path: PathBuf },
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
    #[error("failed to validate tx ({txid})")]
    ValidateFilledTransaction {
        source: ValidateFilledTransaction,
        txid: Txid,
    },
    #[error("failed to validate orchard anchor for tx ({txid})")]
    ValidateOrchardAnchor {
        source: ValidateOrchardAnchor,
        txid: Txid,
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
