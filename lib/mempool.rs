use std::collections::VecDeque;

use fallible_iterator::FallibleIterator as _;
use heed::types::SerdeBincode;
use sneed::{DatabaseUnique, RoTxn, RwTxn, RwTxnError, UnitKey, db, env};
use thiserror::Error;
use transitive::Transitive;

use crate::types::{
    Accumulator, AuthorizedTransaction, Body, OutPoint, Txid, UtreexoError,
    VERSION, Version, orchard::Nullifier,
};

#[derive(Debug, Error, Transitive)]
#[transitive(
    from(db::error::Delete, db::Error),
    from(db::error::Get, db::Error),
    from(db::error::IterInit, db::Error),
    from(db::error::IterItem, db::Error),
    from(db::error::Put, db::Error),
    from(db::error::TryGet, db::Error),
    from(env::error::CreateDb, env::Error),
    from(env::error::WriteTxn, env::Error)
)]
pub enum Error {
    #[error(transparent)]
    Db(#[from] Box<db::Error>),
    #[error("Database env error")]
    DbEnv(#[from] Box<env::Error>),
    #[error("Database write error")]
    DbWrite(#[from] RwTxnError),
    #[error(
        "can't add transaction (`{}`), nullifier (`{}`) already used by (`{}`)",
        .new_txid,
        .nullifier,
        .old_txid,
    )]
    NullifierDoubleSpent {
        new_txid: Txid,
        nullifier: Nullifier,
        old_txid: Txid,
    },
    #[error(transparent)]
    Utreexo(#[from] UtreexoError),
    #[error("can't add transaction (`{txid}`), utxo double spent")]
    UtxoDoubleSpent { txid: Txid },
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

#[derive(Clone)]
pub struct MemPool {
    pub transactions:
        DatabaseUnique<SerdeBincode<Txid>, SerdeBincode<AuthorizedTransaction>>,
    pub spent_utxos: DatabaseUnique<SerdeBincode<OutPoint>, SerdeBincode<Txid>>,
    pub used_nullifiers:
        DatabaseUnique<SerdeBincode<Nullifier>, SerdeBincode<Txid>>,
    _version: DatabaseUnique<UnitKey, SerdeBincode<Version>>,
}

impl MemPool {
    pub const NUM_DBS: u32 = 4;

    pub fn new(env: &sneed::Env) -> Result<Self, Error> {
        let mut rwtxn = env.write_txn()?;
        let transactions =
            DatabaseUnique::create(env, &mut rwtxn, "transactions")?;
        let spent_utxos =
            DatabaseUnique::create(env, &mut rwtxn, "spent_utxos")?;
        let used_nullifiers =
            DatabaseUnique::create(env, &mut rwtxn, "used_nullifiers")?;
        let version =
            DatabaseUnique::create(env, &mut rwtxn, "mempool_version")?;
        if version.try_get(&rwtxn, &())?.is_none() {
            version.put(&mut rwtxn, &(), &*VERSION)?;
        }
        rwtxn.commit().map_err(RwTxnError::from)?;
        Ok(Self {
            transactions,
            spent_utxos,
            used_nullifiers,
            _version: version,
        })
    }

    pub fn put(
        &self,
        rwtxn: &mut RwTxn,
        transaction: &AuthorizedTransaction,
    ) -> Result<(), Error> {
        let txid = transaction.transaction.txid();
        if self.transactions.contains_key(rwtxn, &txid)? {
            tracing::debug!(%txid, "transaction already in mempool");
            return Ok(());
        }
        tracing::debug!("adding transaction {txid} to mempool");
        for (outpoint, _) in &transaction.transaction.inputs {
            if self.spent_utxos.try_get(rwtxn, outpoint)?.is_some() {
                return Err(Error::UtxoDoubleSpent {
                    txid: transaction.transaction.txid(),
                });
            }
            self.spent_utxos.put(rwtxn, outpoint, &txid)?;
        }
        if let Some(orchard_bundle) = &transaction.transaction.orchard_bundle {
            for nullifier in orchard_bundle.nullifiers() {
                if let Some(old_txid) =
                    self.used_nullifiers.try_get(rwtxn, nullifier)?
                {
                    let err = Error::NullifierDoubleSpent {
                        new_txid: txid,
                        nullifier: *nullifier,
                        old_txid,
                    };
                    return Err(err);
                }
                self.used_nullifiers.put(rwtxn, nullifier, &txid)?;
            }
        }
        self.transactions.put(rwtxn, &txid, transaction)?;
        Ok(())
    }

    pub fn delete(&self, rwtxn: &mut RwTxn, txid: Txid) -> Result<(), Error> {
        let mut pending_deletes = VecDeque::from([txid]);
        while let Some(txid) = pending_deletes.pop_front() {
            if let Some(tx) = self.transactions.try_get(rwtxn, &txid)? {
                for (outpoint, _) in &tx.transaction.inputs {
                    self.spent_utxos.delete(rwtxn, outpoint)?;
                }
                self.transactions.delete(rwtxn, &txid)?;
                for vout in 0..tx.transaction.outputs.len() {
                    let outpoint = OutPoint::Regular {
                        txid,
                        vout: vout as u32,
                    };
                    if let Some(child_txid) =
                        self.spent_utxos.try_get(rwtxn, &outpoint)?
                    {
                        pending_deletes.push_back(child_txid);
                    }
                }
                if let Some(orchard_bundle) = tx.transaction.orchard_bundle {
                    for nullifier in orchard_bundle.nullifiers() {
                        self.used_nullifiers.delete(rwtxn, nullifier)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn take(
        &self,
        rotxn: &RoTxn,
        number: usize,
    ) -> Result<Vec<AuthorizedTransaction>, Error> {
        self.transactions
            .iter(rotxn)?
            .take(number)
            .map(|(_, transaction)| Ok(transaction))
            .collect()
            .map_err(Error::from)
    }

    pub fn take_all(
        &self,
        rotxn: &RoTxn,
    ) -> Result<Vec<AuthorizedTransaction>, Error> {
        self.transactions
            .iter(rotxn)?
            .map(|(_, transaction)| Ok(transaction))
            .collect()
            .map_err(Error::from)
    }

    /// regenerate utreexo proofs for all txs in the mempool
    pub fn regenerate_proofs(
        &self,
        rwtxn: &mut RwTxn,
        accumulator: &Accumulator,
    ) -> Result<(), Error> {
        let txids: Vec<_> = self.transactions.iter_keys(rwtxn)?.collect()?;
        for txid in txids {
            let mut tx = self.transactions.get(rwtxn, &txid)?;
            let targets: Vec<_> = tx
                .transaction
                .inputs
                .iter()
                .map(|(_, utxo_hash)| utxo_hash.into())
                .collect();
            tx.transaction.proof = accumulator.prove(&targets)?;
            self.transactions.put(rwtxn, &txid, &tx)?;
        }
        Ok(())
    }

    /// Remove conflicting txs and regenerate proofs after applying a block
    pub fn connect_block(
        &self,
        rwtxn: &mut RwTxn,
        accumulator: &Accumulator,
        body: &Body,
    ) -> Result<(), Error> {
        for tx in &body.transactions {
            let () = self.delete(rwtxn, tx.txid())?;
            if let Some(orchard_bundle) = &tx.orchard_bundle {
                for nullifier in orchard_bundle.nullifiers() {
                    if let Some(txid) =
                        self.used_nullifiers.try_get(rwtxn, nullifier)?
                    {
                        let () = self.delete(rwtxn, txid)?;
                    }
                }
            }
        }
        self.regenerate_proofs(rwtxn, accumulator)
    }
}
