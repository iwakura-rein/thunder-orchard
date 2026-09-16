use error_fatality::{Fatality, Split};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("Bitcoin amount overflow")]
pub struct AmountOverflow;

#[derive(Debug, Error)]
#[error("Bitcoin amount underflow")]
pub struct AmountUnderflow;

/// Non-fatal variants indicate tx rejection reason
#[derive(Debug, Error, Fatality, Split)]
pub enum Authorization {
    #[error("borsh serialization error")]
    #[fatal(true)]
    BorshSerialize(#[from] borsh::io::Error),
    #[error("not enough authorizations")]
    #[fatal(false)]
    NotEnoughAuthorizations,
    #[error("signature verification error")]
    #[fatal(false)]
    SignatureVerification(#[from] frost_ristretto255::Error),
    #[error("too many authorizations")]
    #[fatal(false)]
    TooManyAuthorizations,
    #[error("Orchard bundle proof verification error")]
    #[fatal(false)]
    OrchardProof(#[from] crate::orchard::BundleProofVerificationError),
    #[error("Orchard signature verification error")]
    #[fatal(false)]
    OrchardSignature(#[from] crate::orchard::SignatureVerificationError),
    #[error(
        "wrong key for address: address = {address},
             hash(verifying_key) = {hash_verifying_key}"
    )]
    #[fatal(false)]
    WrongKeyForAddress {
        address: crate::TransparentAddress,
        hash_verifying_key: crate::TransparentAddress,
    },
}

#[derive(Debug, Error)]
pub enum ComputeFee {
    #[error("underfunded; value in ({value_in}) < value out ({value_out})")]
    Underfunded {
        value_in: bitcoin::Amount,
        value_out: bitcoin::Amount,
    },
    #[error("value in overflow")]
    ValueInOverflow(#[source] AmountOverflow),
    #[error("value out overflow")]
    ValueOutOverflow(#[source] AmountOverflow),
}

#[derive(Debug, Error)]
pub enum ParseTransparentAddress {
    #[error("bs58 error")]
    Bs58(#[from] bitcoin::base58::InvalidCharacterError),
    #[error("wrong address length {0} != 20")]
    WrongLength(usize),
}

#[derive(Debug, Error)]
#[error("utreexo error ({0})")]
#[repr(transparent)]
pub struct Utreexo(pub(crate) String);

pub mod withdrawal_bundle {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum Inner {
        #[error(
            "bundle too heavy: weight `{weight}` > max weight `{max_weight}`"
        )]
        BundleTooHeavy { weight: u64, max_weight: u64 },
    }

    #[derive(Debug, Error)]
    #[error("Withdrawal bundle error")]
    #[repr(transparent)]
    pub struct Error(#[from] Inner);
}
pub use withdrawal_bundle::Error as WithdrawalBundle;

#[derive(Debug, Error)]
pub enum ParsePeerAddress {
    #[error("missing port")]
    MissingPort,
    #[error(transparent)]
    Parse(#[from] url::ParseError),
}
