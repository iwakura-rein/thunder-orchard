use std::path::PathBuf;

use sneed::{db::error as db, env::error as env, rwtxn::error as rwtxn};
use thiserror::Error;
use transitive::Transitive;

use crate::types::{Txid, UtreexoError, Version};

#[derive(Debug, Error)]
pub enum TxRejected {
    #[error("nullifier already used by tx ({conflicts_with})")]
    NullifierDoubleSpent { conflicts_with: Txid },
    #[error(
        "input ({double_spent_vin}) already spent by tx ({conflicts_with})"
    )]
    UtxoDoubleSpent {
        double_spent_vin: usize,
        conflicts_with: Txid,
    },
}

pub mod insert {
    use error_fatality::{Fatality, Split};
    use sneed::db::error as db;
    use thiserror::Error;

    use crate::mempool::error::TxRejected;

    #[derive(Debug, Error)]
    pub enum Fatal {
        #[error(transparent)]
        DbPut(#[from] db::Put),
        #[error(transparent)]
        DbTryGet(#[from] db::TryGet),
    }

    /// Non-fatal variants indicate tx rejection reason
    #[derive(Debug, Error, Fatality)]
    pub enum Error {
        #[error(transparent)]
        #[fatal(true)]
        DbPut(#[from] db::Put),
        #[error(transparent)]
        #[fatal(true)]
        DbTryGet(#[from] db::TryGet),
        #[error(transparent)]
        #[fatal(false)]
        TxRejected(#[from] TxRejected),
    }

    impl Split for Error {
        type Fatal = Fatal;
        type Jfyi = TxRejected;

        fn split(self) -> std::result::Result<Self::Jfyi, Self::Fatal> {
            match self {
                Self::DbPut(err) => Err(Fatal::DbPut(err)),
                Self::DbTryGet(err) => Err(Fatal::DbTryGet(err)),
                Self::TxRejected(jfyi) => Ok(jfyi),
            }
        }
    }
}
pub use insert::Error as Insert;

#[allow(clippy::duplicated_attributes)]
#[derive(Debug, Error, Transitive)]
#[transitive(
    from(db::Delete, db::Error),
    from(db::Get, db::Error),
    from(db::IterInit, db::Error),
    from(db::IterItem, db::Error),
    from(db::Put, db::Error),
    from(db::TryGet, db::Error),
    from(env::CreateDb, env::Error),
    from(env::WriteTxn, env::Error)
)]
pub enum Error {
    #[error(transparent)]
    Db(#[from] Box<db::Error>),
    #[error("Database env error")]
    DbEnv(#[from] Box<env::Error>),
    #[error("Database write error")]
    DbWrite(#[from] rwtxn::Error),
    #[error(
        "Incompatible DB version ({}). Please clear the DB (`{}`) and re-sync",
        .version,
        .db_path.display()
    )]
    IncompatibleVersion { version: Version, db_path: PathBuf },
    #[error(transparent)]
    Utreexo(#[from] UtreexoError),
}

impl From<db::Error> for Error {
    fn from(err: db::Error) -> Self {
        Self::Db(Box::new(err))
    }
}

impl From<env::Error> for Error {
    fn from(err: env::Error) -> Self {
        Self::DbEnv(Box::new(err))
    }
}

impl From<insert::Fatal> for Error {
    fn from(err: insert::Fatal) -> Self {
        match err {
            insert::Fatal::DbPut(err) => err.into(),
            insert::Fatal::DbTryGet(err) => err.into(),
        }
    }
}
