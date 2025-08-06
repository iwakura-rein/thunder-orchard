use borsh::BorshSerialize;
use rayon::{
    iter::{IntoParallelRefIterator as _, ParallelIterator as _},
    slice::ParallelSlice as _,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::types::{
    AuthorizedTransaction, Body, Transaction, TransparentAddress, orchard,
};

pub use ed25519_dalek::{
    Signature, SignatureError, Signer, SigningKey, Verifier, VerifyingKey,
};

pub fn get_address(verifying_key: &VerifyingKey) -> TransparentAddress {
    let mut hasher = blake3::Hasher::new();
    let mut reader = hasher.update(&verifying_key.to_bytes()).finalize_xof();
    let mut output: [u8; 20] = [0; 20];
    reader.fill(&mut output);
    TransparentAddress(output)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("borsh serialization error")]
    BorshSerialize(#[from] borsh::io::Error),
    #[error("ed25519_dalek error")]
    DalekError(#[from] SignatureError),
    #[error("not enough authorizations")]
    NotEnoughAuthorizations,
    #[error("too many authorizations")]
    TooManyAuthorizations,
    #[error("Orchard bundle proof verification error")]
    OrchardProof(#[from] orchard::BundleProofVerificationError),
    #[error("Orchard signature verification error")]
    OrchardSignature(#[from] orchard::SignatureVerificationError),
    #[error(
        "wrong key for address: address = {address},
             hash(verifying_key) = {hash_verifying_key}"
    )]
    WrongKeyForAddress {
        address: TransparentAddress,
        hash_verifying_key: TransparentAddress,
    },
}

fn borsh_serialize_verifying_key<W>(
    vk: &VerifyingKey,
    writer: &mut W,
) -> borsh::io::Result<()>
where
    W: borsh::io::Write,
{
    borsh::BorshSerialize::serialize(&vk.to_bytes(), writer)
}

fn borsh_serialize_signature<W>(
    sig: &Signature,
    writer: &mut W,
) -> borsh::io::Result<()>
where
    W: borsh::io::Write,
{
    borsh::BorshSerialize::serialize(&sig.to_bytes(), writer)
}

#[derive(
    BorshSerialize,
    Debug,
    Clone,
    Deserialize,
    Eq,
    PartialEq,
    Serialize,
    ToSchema,
)]
pub struct Authorization {
    #[borsh(serialize_with = "borsh_serialize_verifying_key")]
    #[schema(value_type = String)]
    pub verifying_key: VerifyingKey,
    #[borsh(serialize_with = "borsh_serialize_signature")]
    #[schema(value_type = String)]
    pub signature: Signature,
}

pub fn verify_authorized_transaction(
    transaction: &AuthorizedTransaction,
) -> Result<(), Error> {
    let () = verify_orchard(&transaction.transaction)?;

    let tx_bytes_canonical = borsh::to_vec(&transaction.transaction)?;
    let messages: Vec<_> = std::iter::repeat_n(
        tx_bytes_canonical.as_slice(),
        transaction.authorizations.len(),
    )
    .collect();
    let (verifying_keys, signatures): (Vec<VerifyingKey>, Vec<Signature>) =
        transaction
            .authorizations
            .iter()
            .map(
                |Authorization {
                     verifying_key,
                     signature,
                 }| (verifying_key, signature),
            )
            .unzip();
    ed25519_dalek::verify_batch(&messages, &signatures, &verifying_keys)?;
    Ok(())
}

// Verify orchard authorization
fn verify_orchard(transaction: &Transaction) -> Result<(), Error> {
    if let Some(orchard_bundle) = &transaction.orchard_bundle {
        let txid = transaction.txid();
        let bvk = orchard_bundle.binding_validating_key();
        let binding_sig = orchard_bundle.authorization().binding_signature();
        let () = bvk.verify(txid.as_slice(), binding_sig)?;
        let () = orchard_bundle.verify_proof()?;
    };
    Ok(())
}

pub fn verify_authorizations(body: &Body) -> Result<(), Error> {
    // TODO: batch orchard verifications
    let () = body.transactions.par_iter().try_for_each(verify_orchard)?;
    let verifications_required =
        body.transactions.par_iter().map(|tx| tx.inputs.len()).sum();
    match body.authorizations.len().cmp(&verifications_required) {
        std::cmp::Ordering::Less => return Err(Error::NotEnoughAuthorizations),
        std::cmp::Ordering::Equal => (),
        std::cmp::Ordering::Greater => {
            return Err(Error::TooManyAuthorizations);
        }
    }
    if verifications_required == 0 {
        return Ok(());
    }
    // pairs of serialized txs, and the number of inputs
    let serialized_transactions_inputs: Vec<(Vec<u8>, usize)> = body
        .transactions
        .par_iter()
        .map(|tx| Ok((borsh::to_vec(tx)?, tx.inputs.len())))
        .collect::<Result<_, Error>>()?;
    let messages =
        serialized_transactions_inputs
            .iter()
            .flat_map(|(tx, n_inputs)| {
                std::iter::repeat_n(tx.as_slice(), *n_inputs)
            });
    let pairs = body.authorizations.iter().zip(messages).collect::<Vec<_>>();
    assert_eq!(pairs.len(), body.authorizations.len());
    const CHUNK_SIZE: usize = 1 << 14;
    pairs.par_chunks(CHUNK_SIZE).try_for_each(|chunk| {
        let (signatures, verifying_keys, messages): (
            Vec<Signature>,
            Vec<VerifyingKey>,
            Vec<&[u8]>,
        ) = chunk
            .iter()
            .map(|(auth, msg)| (auth.signature, auth.verifying_key, msg))
            .collect();
        ed25519_dalek::verify_batch(&messages, &signatures, &verifying_keys)
    })?;
    Ok(())
}

impl Authorization {
    pub fn get_address(&self) -> TransparentAddress {
        get_address(&self.verifying_key)
    }

    pub fn verify_transaction(
        transaction: &AuthorizedTransaction,
    ) -> Result<(), Error> {
        verify_authorized_transaction(transaction)?;
        Ok(())
    }

    pub fn verify_body(body: &Body) -> Result<(), Error> {
        verify_authorizations(body)?;
        Ok(())
    }
}

pub fn sign_orchard(
    signing_keys: &[orchard::SpendAuthorizingKey],
    transaction: Transaction<
        orchard::InProgress<orchard::BundleProof, orchard::Unauthorized>,
    >,
) -> Result<Transaction, orchard::BuildError> {
    let sighash: [u8; 32] = transaction.txid().0;
    let Transaction {
        inputs,
        proof,
        outputs,
        orchard_bundle,
    } = transaction;
    let orchard_bundle = orchard_bundle
        .map(|bundle| {
            bundle.apply_signatures(rand::rngs::OsRng, sighash, signing_keys)
        })
        .transpose()?;
    let transaction = Transaction {
        inputs,
        proof,
        outputs,
        orchard_bundle,
    };
    Ok(transaction)
}

pub fn sign(
    signing_key: &SigningKey,
    transaction: &Transaction,
) -> Result<Signature, Error> {
    let tx_bytes_canonical = borsh::to_vec(&transaction)?;
    Ok(signing_key.sign(&tx_bytes_canonical))
}

pub fn authorize(
    addresses_signing_keys: &[(TransparentAddress, &SigningKey)],
    transaction: Transaction,
) -> Result<AuthorizedTransaction, Error> {
    let mut authorizations: Vec<Authorization> =
        Vec::with_capacity(addresses_signing_keys.len());
    let tx_bytes_canonical = borsh::to_vec(&transaction)?;
    for (address, signing_key) in addresses_signing_keys {
        let hash_verifying_key = get_address(&signing_key.verifying_key());
        if *address != hash_verifying_key {
            return Err(Error::WrongKeyForAddress {
                address: *address,
                hash_verifying_key,
            });
        }
        let authorization = Authorization {
            verifying_key: signing_key.verifying_key(),
            signature: signing_key.sign(&tx_bytes_canonical),
        };
        authorizations.push(authorization);
    }
    Ok(AuthorizedTransaction {
        authorizations,
        transaction,
    })
}
