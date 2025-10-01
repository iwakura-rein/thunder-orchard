use std::{collections::HashMap, io::Cursor};

use bitcoin::amount::CheckedSum;
use borsh::{self, BorshDeserialize, BorshSerialize};
use educe::Educe;
use heed::{BoxedError, BytesDecode, BytesEncode};
use rustreexo::accumulator::{
    mem_forest::MemForest, node_hash::BitcoinNodeHash, proof::Proof,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    authorization::Authorization,
    types::{
        AmountOverflowError, Hash, M6id, MerkleRoot, TransparentAddress, Txid,
        hash, hash_with_scratch_buffer,
        orchard::{self, BundleAuthorization},
    },
};

pub trait GetValue {
    fn get_value(&self) -> bitcoin::Amount;
}

fn borsh_serialize_bitcoin_outpoint<W>(
    outpoint: &bitcoin::OutPoint,
    writer: &mut W,
) -> borsh::io::Result<()>
where
    W: borsh::io::Write,
{
    let bitcoin::OutPoint { txid, vout } = outpoint;
    let txid_bytes: &[u8; 32] = txid.as_ref();
    borsh::BorshSerialize::serialize(&(txid_bytes, vout), writer)
}

fn borsh_deserialize_bitcoin_outpoint<R>(
    reader: &mut R,
) -> borsh::io::Result<bitcoin::OutPoint>
where
    R: borsh::io::Read,
{
    use bitcoin::hashes::Hash as BitcoinHash;
    let (txid_bytes, vout): ([u8; 32], u32) =
        <([u8; 32], u32) as BorshDeserialize>::deserialize_reader(reader)?;
    Ok(bitcoin::OutPoint {
        txid: bitcoin::Txid::from_byte_array(txid_bytes),
        vout,
    })
}

#[derive(
    BorshSerialize,
    BorshDeserialize,
    Clone,
    Copy,
    Debug,
    Deserialize,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    ToSchema,
)]
pub enum OutPoint {
    // Created by transactions.
    Regular {
        txid: Txid,
        vout: u32,
    },
    // Created by block bodies.
    Coinbase {
        merkle_root: MerkleRoot,
        vout: u32,
    },
    // Created by mainchain deposits.
    #[schema(value_type = crate::types::schema::BitcoinOutPoint)]
    Deposit(
        #[borsh(
            serialize_with = "borsh_serialize_bitcoin_outpoint",
            deserialize_with = "borsh_deserialize_bitcoin_outpoint"
        )]
        bitcoin::OutPoint,
    ),
}

impl std::fmt::Display for OutPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Regular { txid, vout } => write!(f, "regular {txid} {vout}"),
            Self::Coinbase { merkle_root, vout } => {
                write!(f, "coinbase {merkle_root} {vout}")
            }
            Self::Deposit(bitcoin::OutPoint { txid, vout }) => {
                write!(f, "deposit {txid} {vout}")
            }
        }
    }
}

const OUTPOINT_KEY_SIZE: usize = 37;

/// Fixed-width key for OutPoint based on its canonical Borsh encoding.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OutPointKey([u8; OUTPOINT_KEY_SIZE]);

impl OutPointKey {
    /// Get the raw key bytes
    #[inline]
    pub fn as_bytes(&self) -> &[u8; OUTPOINT_KEY_SIZE] {
        &self.0
    }
}

impl From<OutPoint> for OutPointKey {
    #[inline]
    fn from(op: OutPoint) -> Self {
        let mut key = [0u8; OUTPOINT_KEY_SIZE];
        let mut cursor = Cursor::new(&mut key[..]);
        BorshSerialize::serialize(&op, &mut cursor)
            .expect("serializing OutPoint into key buffer should never fail");
        debug_assert_eq!(cursor.position() as usize, OUTPOINT_KEY_SIZE);
        Self(key)
    }
}

impl From<&OutPoint> for OutPointKey {
    #[inline]
    fn from(op: &OutPoint) -> Self {
        <Self as From<OutPoint>>::from(*op)
    }
}

impl From<OutPointKey> for OutPoint {
    #[inline]
    fn from(key: OutPointKey) -> Self {
        let mut cursor = Cursor::new(&key.0[..]);
        OutPoint::deserialize_reader(&mut cursor)
            .expect("deserializing OutPointKey should never fail")
    }
}

impl From<&OutPointKey> for OutPoint {
    #[inline]
    fn from(key: &OutPointKey) -> Self {
        <Self as From<OutPointKey>>::from(*key)
    }
}

impl Ord for OutPointKey {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for OutPointKey {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<[u8]> for OutPointKey {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

// Database key encoding traits for direct LMDB usage
impl<'a> BytesEncode<'a> for OutPointKey {
    type EItem = OutPointKey;

    #[inline]
    fn bytes_encode(
        item: &'a Self::EItem,
    ) -> Result<std::borrow::Cow<'a, [u8]>, BoxedError> {
        Ok(std::borrow::Cow::Borrowed(item.as_ref()))
    }
}

impl<'a> BytesDecode<'a> for OutPointKey {
    type DItem = OutPointKey;

    #[inline]
    fn bytes_decode(bytes: &'a [u8]) -> Result<Self::DItem, BoxedError> {
        if bytes.len() != OUTPOINT_KEY_SIZE {
            return Err(format!(
                "OutPointKey must be exactly {OUTPOINT_KEY_SIZE} bytes"
            )
            .into());
        }
        let mut key = [0u8; OUTPOINT_KEY_SIZE];
        key.copy_from_slice(bytes);
        let mut cursor = Cursor::new(&key[..]);
        let _ = OutPoint::deserialize_reader(&mut cursor)
            .map_err(|err| -> BoxedError { Box::new(err) })?;
        Ok(OutPointKey(key))
    }
}

#[cfg(test)]
mod tests {
    use super::{OUTPOINT_KEY_SIZE, OutPoint, OutPointKey};

    #[test]
    fn check_outpoint_key_size() -> anyhow::Result<()> {
        use anyhow::ensure;
        use bitcoin::hashes::Hash as BitcoinHash;

        let variants = [
            OutPoint::Regular {
                txid: Default::default(),
                vout: u32::MAX,
            },
            OutPoint::Coinbase {
                merkle_root: Default::default(),
                vout: u32::MAX,
            },
            OutPoint::Deposit(bitcoin::OutPoint {
                txid: bitcoin::Txid::from_byte_array([0; 32]),
                vout: u32::MAX,
            }),
        ];

        for op in variants {
            let serialized = borsh::to_vec(&op)?;
            ensure!(
                serialized.len() == OUTPOINT_KEY_SIZE,
                "unexpected serialized size: {}",
                serialized.len()
            );

            let key = OutPointKey::from(op);
            let decoded = OutPoint::from(key);
            ensure!(decoded == op);
        }

        Ok(())
    }
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

mod content {
    use serde::{Deserialize, Serialize};
    use utoipa::{PartialSchema, ToSchema};

    /// Default representation for Serde
    #[derive(Deserialize, Serialize)]
    enum DefaultRepr {
        Value(bitcoin::Amount),
        Withdrawal {
            value: bitcoin::Amount,
            main_fee: bitcoin::Amount,
            main_address: bitcoin::Address<bitcoin::address::NetworkUnchecked>,
        },
    }

    /// Human-readable representation for Serde
    #[derive(Deserialize, Serialize, ToSchema)]
    #[schema(as = OutputContent, description = "")]
    enum HumanReadableRepr {
        #[schema(value_type = u64)]
        Value(
            #[serde(with = "bitcoin::amount::serde::as_sat")] bitcoin::Amount,
        ),
        Withdrawal {
            #[serde(with = "bitcoin::amount::serde::as_sat")]
            #[serde(rename = "value_sats")]
            #[schema(value_type = u64)]
            value: bitcoin::Amount,
            #[serde(with = "bitcoin::amount::serde::as_sat")]
            #[serde(rename = "main_fee_sats")]
            #[schema(value_type = u64)]
            main_fee: bitcoin::Amount,
            #[schema(value_type = crate::types::schema::BitcoinAddr)]
            main_address: bitcoin::Address<bitcoin::address::NetworkUnchecked>,
        },
    }

    type SerdeRepr = serde_with::IfIsHumanReadable<
        serde_with::FromInto<DefaultRepr>,
        serde_with::FromInto<HumanReadableRepr>,
    >;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Content {
        Value(bitcoin::Amount),
        Withdrawal {
            value: bitcoin::Amount,
            main_fee: bitcoin::Amount,
            main_address: bitcoin::Address<bitcoin::address::NetworkUnchecked>,
        },
    }

    impl borsh::BorshSerialize for Content {
        fn serialize<W: borsh::io::Write>(
            &self,
            writer: &mut W,
        ) -> borsh::io::Result<()> {
            match self {
                Content::Value(amount) => {
                    borsh::BorshSerialize::serialize(&0u8, writer)?;
                    borsh::BorshSerialize::serialize(&amount.to_sat(), writer)
                }
                Content::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                } => {
                    borsh::BorshSerialize::serialize(&1u8, writer)?;
                    borsh::BorshSerialize::serialize(&value.to_sat(), writer)?;
                    borsh::BorshSerialize::serialize(
                        &main_fee.to_sat(),
                        writer,
                    )?;
                    // Serialize address as script bytes
                    let script = main_address
                        .as_unchecked()
                        .assume_checked_ref()
                        .script_pubkey();
                    borsh::BorshSerialize::serialize(&script.as_bytes(), writer)
                }
            }
        }
    }

    impl borsh::BorshDeserialize for Content {
        fn deserialize(buf: &mut &[u8]) -> borsh::io::Result<Self> {
            let variant: u8 = borsh::BorshDeserialize::deserialize(buf)?;
            match variant {
                0 => {
                    let sats: u64 = borsh::BorshDeserialize::deserialize(buf)?;
                    Ok(Content::Value(bitcoin::Amount::from_sat(sats)))
                }
                1 => {
                    let value_sats: u64 =
                        borsh::BorshDeserialize::deserialize(buf)?;
                    let main_fee_sats: u64 =
                        borsh::BorshDeserialize::deserialize(buf)?;
                    let script_bytes: Vec<u8> =
                        borsh::BorshDeserialize::deserialize(buf)?;

                    let script = bitcoin::ScriptBuf::from_bytes(script_bytes);
                    let checked_address = bitcoin::Address::from_script(
                        &script,
                        bitcoin::Network::Bitcoin,
                    )
                    .map_err(|e| {
                        borsh::io::Error::new(
                            borsh::io::ErrorKind::InvalidData,
                            e,
                        )
                    })?;
                    let address = checked_address.as_unchecked().clone();

                    Ok(Content::Withdrawal {
                        value: bitcoin::Amount::from_sat(value_sats),
                        main_fee: bitcoin::Amount::from_sat(main_fee_sats),
                        main_address: address,
                    })
                }
                _ => Err(borsh::io::Error::new(
                    borsh::io::ErrorKind::InvalidData,
                    format!("Invalid Content variant: {}", variant),
                )),
            }
        }

        fn deserialize_reader<R: borsh::io::Read>(
            reader: &mut R,
        ) -> borsh::io::Result<Self> {
            let mut variant_buf = [0u8; 1];
            reader.read_exact(&mut variant_buf)?;
            let variant = variant_buf[0];

            match variant {
                0 => {
                    let mut sats_buf = [0u8; 8];
                    reader.read_exact(&mut sats_buf)?;
                    let sats = u64::from_le_bytes(sats_buf);
                    Ok(Content::Value(bitcoin::Amount::from_sat(sats)))
                }
                1 => {
                    let mut value_buf = [0u8; 8];
                    reader.read_exact(&mut value_buf)?;
                    let value_sats = u64::from_le_bytes(value_buf);

                    let mut fee_buf = [0u8; 8];
                    reader.read_exact(&mut fee_buf)?;
                    let main_fee_sats = u64::from_le_bytes(fee_buf);

                    // Read script length first
                    let mut len_buf = [0u8; 4];
                    reader.read_exact(&mut len_buf)?;
                    let script_len = u32::from_le_bytes(len_buf) as usize;

                    let mut script_bytes = vec![0u8; script_len];
                    reader.read_exact(&mut script_bytes)?;

                    let script = bitcoin::ScriptBuf::from_bytes(script_bytes);
                    let checked_address = bitcoin::Address::from_script(
                        &script,
                        bitcoin::Network::Bitcoin,
                    )
                    .map_err(|e| {
                        borsh::io::Error::new(
                            borsh::io::ErrorKind::InvalidData,
                            e,
                        )
                    })?;
                    let address = checked_address.as_unchecked().clone();

                    Ok(Content::Withdrawal {
                        value: bitcoin::Amount::from_sat(value_sats),
                        main_fee: bitcoin::Amount::from_sat(main_fee_sats),
                        main_address: address,
                    })
                }
                _ => Err(borsh::io::Error::new(
                    borsh::io::ErrorKind::InvalidData,
                    format!("Invalid Content variant: {}", variant),
                )),
            }
        }
    }

    impl Content {
        pub fn is_value(&self) -> bool {
            matches!(self, Self::Value(_))
        }
        pub fn is_withdrawal(&self) -> bool {
            matches!(self, Self::Withdrawal { .. })
        }

        pub(in crate::types) fn schema_ref() -> utoipa::openapi::Ref {
            utoipa::openapi::Ref::new("OutputContent")
        }
    }

    impl crate::wallet::GetValue for Content {
        #[inline(always)]
        fn get_value(&self) -> bitcoin::Amount {
            match self {
                Self::Value(value) => *value,
                Self::Withdrawal { value, .. } => *value,
            }
        }
    }

    impl From<Content> for DefaultRepr {
        fn from(content: Content) -> Self {
            match content {
                Content::Value(value) => Self::Value(value),
                Content::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                } => Self::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                },
            }
        }
    }

    impl From<Content> for HumanReadableRepr {
        fn from(content: Content) -> Self {
            match content {
                Content::Value(value) => Self::Value(value),
                Content::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                } => Self::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                },
            }
        }
    }

    impl From<DefaultRepr> for Content {
        fn from(repr: DefaultRepr) -> Self {
            match repr {
                DefaultRepr::Value(value) => Self::Value(value),
                DefaultRepr::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                } => Self::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                },
            }
        }
    }

    impl From<HumanReadableRepr> for Content {
        fn from(repr: HumanReadableRepr) -> Self {
            match repr {
                HumanReadableRepr::Value(value) => Self::Value(value),
                HumanReadableRepr::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                } => Self::Withdrawal {
                    value,
                    main_fee,
                    main_address,
                },
            }
        }
    }

    impl<'de> Deserialize<'de> for Content {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            <SerdeRepr as serde_with::DeserializeAs<'de, _>>::deserialize_as(
                deserializer,
            )
        }
    }

    impl Serialize for Content {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            <SerdeRepr as serde_with::SerializeAs<_>>::serialize_as(
                self, serializer,
            )
        }
    }

    impl PartialSchema for Content {
        fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
            <HumanReadableRepr as PartialSchema>::schema()
        }
    }

    impl ToSchema for Content {
        fn name() -> std::borrow::Cow<'static, str> {
            <HumanReadableRepr as ToSchema>::name()
        }
    }
}
pub use content::Content;

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
pub struct Output {
    pub address: TransparentAddress,
    #[schema(schema_with = Content::schema_ref)]
    pub content: Content,
}

impl GetValue for Output {
    #[inline(always)]
    fn get_value(&self) -> bitcoin::Amount {
        self.content.get_value()
    }
}

#[derive(
    BorshSerialize,
    Clone,
    Debug,
    Deserialize,
    Eq,
    PartialEq,
    Serialize,
    ToSchema,
)]
pub struct PointedOutput {
    pub outpoint: OutPoint,
    pub output: Output,
}

impl From<&PointedOutput> for BitcoinNodeHash {
    fn from(pointed_output: &PointedOutput) -> Self {
        Self::new(hash(pointed_output))
    }
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
    pub inputs: Vec<(OutPoint, Hash)>,
    /// Utreexo proof for inputs
    #[borsh(skip)]
    #[schema(value_type = crate::types::schema::UtreexoProof)]
    pub proof: Proof,
    pub outputs: Vec<Output>,
    #[borsh(bound(serialize = "orchard::Bundle<Auth>: BorshSerialize"))]
    #[schema(schema_with =
        <crate::types::schema::Optional::<
            orchard::Bundle<Auth>
        > as utoipa::PartialSchema>::schema
    )]
    pub orchard_bundle: Option<orchard::Bundle<Auth>>,
}

impl<Auth> Transaction<Auth>
where
    Auth: BundleAuthorization,
{
    pub fn txid(&self) -> Txid {
        use smallvec::SmallVec;
        thread_local! {
            static SCRATCH: std::cell::RefCell<SmallVec<[u8; 512]>> =
                std::cell::RefCell::new(SmallVec::new());
        }
        let Self {
            inputs,
            proof: _,
            outputs,
            orchard_bundle,
        } = self;
        let hash = SCRATCH.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.clear();
            // Inputs
            borsh::to_writer(&mut *buf, inputs)
                .expect("failed to serialize with borsh to compute a hash");
            // Outputs
            BorshSerialize::serialize(&outputs, &mut *buf)
                .expect("failed to serialize with borsh to compute a hash");
            // Orchard bundle without auth
            if let Some(orchard_bundle) = orchard_bundle {
                orchard_bundle
                    .borsh_serialize_without_auth(&mut *buf)
                    .expect("failed to serialize with borsh to compute a hash");
            }
            blake3::hash(&buf).into()
        });
        Txid(hash)
    }
}

impl<S> Transaction<orchard::InProgress<orchard::Unproven, S>>
where
    S: orchard::InProgressSignatures,
{
    pub fn create_proof(
        self,
    ) -> Result<
        Transaction<orchard::InProgress<orchard::BundleProof, S>>,
        orchard::BuildError,
    > {
        let Self {
            inputs,
            proof,
            outputs,
            orchard_bundle,
        } = self;
        let orchard_bundle = orchard_bundle
            .map(|bundle| bundle.create_proof(rand::rngs::OsRng))
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
)]
pub struct SpentOutput {
    pub output: Output,
    pub inpoint: InPoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilledTransaction {
    pub transaction: Transaction,
    pub spent_utxos: Vec<Output>,
}

impl FilledTransaction {
    pub fn get_value_in(&self) -> Result<bitcoin::Amount, AmountOverflowError> {
        self.spent_utxos
            .iter()
            .map(GetValue::get_value)
            .checked_sum()
            .ok_or(AmountOverflowError)
    }

    pub fn get_value_out(
        &self,
    ) -> Result<bitcoin::Amount, AmountOverflowError> {
        self.transaction
            .outputs
            .iter()
            .map(GetValue::get_value)
            .checked_sum()
            .ok_or(AmountOverflowError)
    }

    pub fn get_fee(
        &self,
    ) -> Result<Option<bitcoin::Amount>, AmountOverflowError> {
        let value_in = self.get_value_in()?;
        let value_out = self.get_value_out()?;
        if value_in < value_out {
            Ok(None)
        } else {
            Ok(Some(value_in - value_out))
        }
    }
}

#[derive(BorshSerialize, Clone, Debug, Deserialize, Serialize)]
pub struct AuthorizedTransaction {
    pub transaction: Transaction,
    /// Authorization is called witness in Bitcoin.
    pub authorizations: Vec<Authorization>,
}

#[derive(BorshSerialize, Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Body {
    pub coinbase: Vec<Output>,
    pub transactions: Vec<Transaction>,
    pub authorizations: Vec<Authorization>,
}

impl Body {
    pub fn new(
        authorized_transactions: Vec<AuthorizedTransaction>,
        coinbase: Vec<Output>,
    ) -> Self {
        let mut authorizations = Vec::with_capacity(
            authorized_transactions
                .iter()
                .map(|t| t.transaction.inputs.len())
                .sum(),
        );
        let mut transactions =
            Vec::with_capacity(authorized_transactions.len());
        for at in authorized_transactions.into_iter() {
            authorizations.extend(at.authorizations);
            transactions.push(at.transaction);
        }
        Self {
            coinbase,
            transactions,
            authorizations,
        }
    }

    pub fn authorized_transactions(&self) -> Vec<AuthorizedTransaction> {
        let mut authorizations_iter = self.authorizations.iter();
        self.transactions
            .iter()
            .map(|tx| {
                let mut authorizations = Vec::with_capacity(tx.inputs.len());
                for _ in 0..tx.inputs.len() {
                    let auth = authorizations_iter.next().unwrap();
                    authorizations.push(auth.clone());
                }
                AuthorizedTransaction {
                    transaction: tx.clone(),
                    authorizations,
                }
            })
            .collect()
    }

    pub fn compute_merkle_root(&self) -> MerkleRoot {
        // FIXME: Compute actual merkle root instead of just a hash.
        hash_with_scratch_buffer(&(&self.coinbase, &self.transactions)).into()
    }

    // Modifies the memforest, without checking tx proofs
    pub fn modify_memforest(
        &self,
        memforest: &mut MemForest<BitcoinNodeHash>,
    ) -> Result<(), String> {
        // New leaves for the accumulator
        let mut accumulator_add = Vec::<BitcoinNodeHash>::new();
        // Accumulator leaves to delete
        let mut accumulator_del = Vec::<BitcoinNodeHash>::new();
        let merkle_root = self.compute_merkle_root();
        for (vout, output) in self.coinbase.iter().enumerate() {
            let outpoint = OutPoint::Coinbase {
                merkle_root,
                vout: vout as u32,
            };
            let pointed_output = PointedOutput {
                outpoint,
                output: output.clone(),
            };
            accumulator_add.push((&pointed_output).into());
        }
        for transaction in &self.transactions {
            let txid = transaction.txid();
            for (_, utxo_hash) in transaction.inputs.iter() {
                accumulator_del.push(utxo_hash.into());
            }
            for (vout, output) in transaction.outputs.iter().enumerate() {
                let outpoint = OutPoint::Regular {
                    txid,
                    vout: vout as u32,
                };
                let pointed_output = PointedOutput {
                    outpoint,
                    output: output.clone(),
                };
                accumulator_add.push((&pointed_output).into());
            }
        }
        memforest.modify(&accumulator_add, &accumulator_del)
    }

    pub fn get_inputs(&self) -> Vec<OutPoint> {
        self.transactions
            .iter()
            .flat_map(|tx| tx.inputs.iter().map(|(outpoint, _)| outpoint))
            .copied()
            .collect()
    }

    pub fn get_outputs(&self) -> HashMap<OutPoint, Output> {
        let mut outputs = HashMap::new();
        let merkle_root = self.compute_merkle_root();
        for (vout, output) in self.coinbase.iter().enumerate() {
            let vout = vout as u32;
            let outpoint = OutPoint::Coinbase { merkle_root, vout };
            outputs.insert(outpoint, output.clone());
        }
        for transaction in &self.transactions {
            let txid = transaction.txid();
            for (vout, output) in transaction.outputs.iter().enumerate() {
                let vout = vout as u32;
                let outpoint = OutPoint::Regular { txid, vout };
                outputs.insert(outpoint, output.clone());
            }
        }
        outputs
    }

    pub fn get_coinbase_value(
        &self,
    ) -> Result<bitcoin::Amount, AmountOverflowError> {
        self.coinbase
            .iter()
            .map(|output| output.get_value())
            .checked_sum()
            .ok_or(AmountOverflowError)
    }
}
