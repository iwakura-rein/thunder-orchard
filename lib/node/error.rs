use error_fatality::{Fatality, Split};
use sneed::{db::error as db, env::error as env, rwtxn::error as rwtxn};
use thiserror::Error;
use transitive::Transitive;

use crate::{
    archive,
    mempool::error as mempool,
    net::error as net,
    node::{mainchain_task, net_task},
    state::error as state,
    types::{AmountOverflowError, AmountUnderflowError, UtreexoError, proto},
};

/// Non-fatal variants indicate tx rejection reason
#[derive(Debug, Error, Fatality, Split)]
pub enum SubmitTransaction {
    #[error(transparent)]
    #[fatal(true)]
    CommitRwTxn(#[from] rwtxn::Commit),
    #[error(transparent)]
    #[fatal(true)]
    EnvWriteTxn(#[from] env::WriteTxn),
    #[error("failed to insert tx into mempool")]
    #[fatal(forward)]
    MempoolInsert(#[from] mempool::Insert),
    #[error("failed to regenerate proof")]
    #[fatal(forward)]
    RegenerateProof(#[from] state::RegenerateProof),
    #[error("failed to validate transaction")]
    #[fatal(forward)]
    Validate(#[from] state::ValidateTransaction),
}

#[allow(clippy::duplicated_attributes)]
#[derive(Debug, Error, Transitive)]
#[transitive(
    from(env::ReadTxn, env::Error),
    from(env::WriteTxn, env::Error),
    from(rwtxn::Commit, rwtxn::Error)
)]
pub enum Error {
    #[error("address parse error")]
    AddrParse(#[from] std::net::AddrParseError),
    #[error(transparent)]
    AmountOverflow(#[from] AmountOverflowError),
    #[error(transparent)]
    AmountUnderflow(#[from] AmountUnderflowError),
    #[error("archive error")]
    Archive(#[from] archive::Error),
    #[error("CUSF mainchain proto error")]
    CusfMainchain(#[from] proto::Error),
    #[error(transparent)]
    Db(#[from] db::Error),
    #[error("Database env error")]
    DbEnv(#[from] env::Error),
    #[error("Database write error")]
    DbWrite(#[from] rwtxn::Error),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("error requesting mainchain ancestors")]
    MainchainAncestors(#[source] mainchain_task::ResponseError),
    #[error("mempool error")]
    MemPool(#[from] mempool::Error),
    #[error("net error")]
    Net(#[from] Box<net::Error>),
    #[error("net task error")]
    NetTask(#[source] Box<net_task::Error>),
    #[error("No CUSF mainchain wallet client")]
    NoCusfMainchainWalletClient,
    #[error("peer info stream closed")]
    PeerInfoRxClosed,
    #[error("Receive mainchain task response cancelled")]
    ReceiveMainchainTaskResponse,
    #[error("Send mainchain task request failed")]
    SendMainchainTaskRequest,
    #[error("state error")]
    State(#[source] Box<state::Error>),
    #[error(transparent)]
    Utreexo(#[from] UtreexoError),
    #[error("Verify BMM error")]
    VerifyBmm(anyhow::Error),
}

impl From<net::Error> for Error {
    fn from(err: net::Error) -> Self {
        Self::Net(Box::new(err))
    }
}

impl From<net_task::Error> for Error {
    fn from(err: net_task::Error) -> Self {
        Self::NetTask(Box::new(err))
    }
}

impl From<state::Error> for Error {
    fn from(err: state::Error) -> Self {
        Self::State(Box::new(err))
    }
}

impl From<state::RegenerateProof> for Error {
    fn from(err: state::RegenerateProof) -> Self {
        match err {
            state::RegenerateProof::DbTryGet(err) => {
                db::Error::from(err).into()
            }
            state::RegenerateProof::Prove(err) => err.into(),
        }
    }
}
