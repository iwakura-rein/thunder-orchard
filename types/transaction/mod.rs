use bitcoin::amount::CheckedSum;
use borsh::{self, BorshDeserialize, BorshSerialize};
use educe::Educe;
use rustreexo::accumulator::proof::Proof as UtreexoProof;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    authorization::Authorization,
    error,
    hashes::{
        self, Hash, InputsMerkleRoot, M6id, OutputsMerkleRoot, TxMerkleRoot,
        Txid,
    },
    orchard::{self, BundleAuthorization},
    schema,
};

pub mod inputs;
pub use inputs::Inputs;
pub mod outpoint;
pub use outpoint::{OutPoint, OutPointKey};
pub mod output;
pub use output::{
    Content as OutputContent, Output, Pointed as PointedOutput,
    PointedOutputRef,
};
pub mod outputs;
pub use outputs::Outputs;

pub trait GetValue {
    fn get_value(&self) -> bitcoin::Amount;
}

/// Reference to a tx input.
#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Copy,
    Debug,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    ToSchema,
)]
pub enum InPoint {
    /// Transaction input
    Regular {
        txid: Txid,
        // index of the spend in the inputs to spend_tx
        vin: u32,
    },
    // Created by mainchain withdrawals
    Withdrawal {
        m6id: M6id,
    },
}

#[derive(BorshSerialize, Debug, Deserialize, Educe, Serialize, ToSchema)]
#[educe(
    Clone(bound(orchard::Bundle<Auth>: Clone)),
    Default(bound()),
)]
#[serde(bound(
    deserialize = "orchard::Bundle<Auth>: Deserialize<'de>",
    serialize = "orchard::Bundle<Auth>: Serialize",
))]
#[schema(bound = "")]
pub struct Transaction<Auth = orchard::Authorized>
where
    Auth: BundleAuthorization,
{
    #[schema(value_type = Vec<(OutPoint, String)>)]
    pub inputs: Inputs<(OutPoint, Hash)>,
    /// Utreexo proof for inputs
    #[borsh(skip)]
    #[schema(value_type = schema::UtreexoProof)]
    pub proof: UtreexoProof,
    #[borsh(bound(serialize = "orchard::Bundle<Auth>: BorshSerialize"))]
    #[schema(schema_with =
        <schema::Optional::<
            orchard::Bundle<Auth>
        > as utoipa::PartialSchema>::schema
    )]
    pub orchard_bundle: Option<orchard::Bundle<Auth>>,
    pub outputs: Outputs,
}

pub type MissingOrchardAuthorization = Transaction<
    orchard::InProgress<orchard::BundleProof, orchard::Unauthorized>,
>;

impl<Auth> Transaction<Auth>
where
    Auth: BundleAuthorization,
{
    pub(crate) fn compute_merkle_root(&self) -> TxMerkleRoot {
        let Self {
            inputs,
            proof: _,
            orchard_bundle,
            outputs,
        } = self;
        // Borsh encoding for hashing
        #[derive(BorshSerialize)]
        struct HashComponents {
            inputs_commitment: InputsMerkleRoot,
            orchard_bundle_commitment: Hash,
            outputs_commitment: OutputsMerkleRoot,
        }
        let inputs_commitment = inputs.compute_merkle_root();
        let orchard_bundle_commitment = hashes::hash(
            &orchard_bundle
                .as_ref()
                .map(orchard::BorshSerializeWithoutAuth::wrap_ref),
        );
        let outputs_commitment = outputs.compute_merkle_root();
        hashes::hash(&HashComponents {
            inputs_commitment,
            orchard_bundle_commitment,
            outputs_commitment,
        })
        .into()
    }

    pub fn txid(&self) -> Txid {
        thread_local! {
            static HASHER: std::cell::RefCell<blake3::Hasher> =
                std::cell::RefCell::new(blake3::Hasher::new());
        }

        let Self {
            inputs,
            proof: _,
            outputs,
            orchard_bundle,
        } = self;
        let hash = HASHER.with(|hasher| {
            let mut hasher = hasher.borrow_mut();
            hasher.reset();
            // Inputs
            borsh::to_writer(&mut *hasher, inputs)
                .expect("failed to serialize with borsh to compute a hash");
            // Outputs
            BorshSerialize::serialize(&outputs, &mut *hasher)
                .expect("failed to serialize with borsh to compute a hash");
            // Orchard bundle without auth
            borsh::to_writer(
                &mut *hasher,
                &orchard_bundle
                    .as_ref()
                    .map(orchard::BorshSerializeWithoutAuth::wrap_ref),
            )
            .expect("failed to serialize with borsh to compute a hash");
            hasher.finalize().into()
        });
        Txid(hash)
    }

    /// Canonical encoding as bytes. The canonical encoding is used for hashing,
    /// but other encodings may be used at eg. networking, rpc levels.
    pub fn canonical_bytes(&self) -> borsh::io::Result<Vec<u8>>
    where
        Self: BorshSerialize,
    {
        borsh::to_vec(&self)
    }
}

impl<S> Transaction<orchard::InProgress<orchard::Unproven, S>>
where
    S: orchard::InProgressSignatures,
{
    pub fn create_proof<R>(
        self,
        rng: R,
    ) -> Result<
        Transaction<orchard::InProgress<orchard::BundleProof, S>>,
        orchard::BuildError,
    >
    where
        R: orchard::CryptoRng,
    {
        let Self {
            inputs,
            proof,
            outputs,
            orchard_bundle,
        } = self;
        let orchard_bundle = orchard_bundle
            .map(|bundle| bundle.create_proof(rng))
            .transpose()?;
        let res = Transaction {
            inputs,
            proof,
            outputs,
            orchard_bundle,
        };
        Ok(res)
    }
}

/// Representation of a spent output
#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Debug,
    Deserialize,
    Eq,
    PartialEq,
    Serialize,
    ToSchema,
)]
pub struct SpentOutput<O = Output> {
    pub output: O,
    pub inpoint: InPoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilledTransaction {
    pub transaction: Transaction,
    pub spent_utxos: Vec<Output>,
}

impl FilledTransaction {
    pub fn get_value_in(
        &self,
    ) -> Result<bitcoin::Amount, error::AmountOverflow> {
        self.spent_utxos
            .iter()
            .map(GetValue::get_value)
            .checked_sum()
            .ok_or(error::AmountOverflow)
    }

    pub fn get_value_out(
        &self,
    ) -> Result<bitcoin::Amount, error::AmountOverflow> {
        self.transaction
            .outputs
            .iter()
            .map(GetValue::get_value)
            .checked_sum()
            .ok_or(error::AmountOverflow)
    }

    pub fn get_fee(&self) -> Result<bitcoin::Amount, error::ComputeFee> {
        let value_in = self
            .get_value_in()
            .map_err(error::ComputeFee::ValueInOverflow)?;
        let value_out = self
            .get_value_out()
            .map_err(error::ComputeFee::ValueOutOverflow)?;
        let res = value_in.checked_sub(value_out).ok_or(
            error::ComputeFee::Underfunded {
                value_in,
                value_out,
            },
        )?;
        Ok(res)
    }

    pub fn inputs(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&OutPoint, &Hash, &Output)> {
        self.transaction.inputs.iter().zip(&self.spent_utxos).map(
            |((outpoint, utxo_hash), output)| (outpoint, utxo_hash, output),
        )
    }
}

#[derive(BorshSerialize, Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Authorized<T> {
    pub transaction: T,
    /// Authorizations are called witnesses in Bitcoin.
    pub authorizations: Vec<Authorization>,
}
pub type AuthorizedTransaction = Authorized<Transaction>;

#[cfg(test)]
mod test {
    use crate::{
        address::TransparentAddress,
        transaction::{
            FilledTransaction, GetValue, Output, OutputContent, Outputs,
            Transaction,
        },
    };

    // a withdrawal output must be funded for both its payout and its mainchain
    // fee, since both leave the treasury
    #[test]
    fn withdrawal_value_includes_main_fee() {
        let value = bitcoin::Amount::from_sat(1000);
        let main_fee = bitcoin::Amount::from_sat(300);
        let main_address = "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2"
            .parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()
            .unwrap();
        let withdrawal = Output {
            address: TransparentAddress::ALL_ZEROS,
            content: OutputContent::Withdrawal {
                value,
                main_fee,
                main_address,
            },
        };
        assert_eq!(withdrawal.get_value(), value + main_fee);

        let value_output = |amount| Output {
            address: TransparentAddress::ALL_ZEROS,
            content: OutputContent::Value(amount),
        };
        let withdrawal_tx = |funding| FilledTransaction {
            transaction: Transaction {
                outputs: Outputs(vec![withdrawal.clone()]),
                ..Default::default()
            },
            spent_utxos: vec![value_output(funding)],
        };

        // inputs covering only the payout are insufficient
        assert!(withdrawal_tx(value).get_fee().is_err());
        // inputs covering payout plus mainchain fee fully fund it
        assert_eq!(
            withdrawal_tx(value + main_fee).get_fee().unwrap(),
            bitcoin::Amount::ZERO
        );
    }
}
