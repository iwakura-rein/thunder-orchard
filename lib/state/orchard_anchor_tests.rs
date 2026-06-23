//! Regression test for forged-anchor orchard spends.
//!
//! An attacker can forge a note they own, build a private one-leaf commitment
//! tree, and take that tree's root as the bundle `anchor`. The Halo2 proof only
//! proves the spend is consistent with whatever anchor the bundle declares, so
//! it verifies. Consensus must additionally reject any anchor that is not a
//! known historical note-commitment tree root, otherwise arbitrary transparent
//! value can be unshielded out of nothing.
//!
//! This test builds such a forged-anchor unshielding transaction and asserts it
//! is rejected by both the mempool path and the block-validation path.

use bitcoin::hashes::Hash as _;
use bytemuck::TransparentWrapper as _;
use incrementalmerkletree::{Hashable, Level};
use rustreexo::accumulator::node_hash::BitcoinNodeHash;
use sneed::RoTxn;

use crate::{
    authorization,
    state::{
        State,
        error::{self, Error},
    },
    types::{
        AccumulatorDiff, AuthorizedTransaction, Body, Header, OutPoint, Output,
        OutputContent, PointedOutput, Transaction, TransparentAddress,
        orchard as o,
    },
};

// The forged note's value: 1,000,000 BTC in sats. Fits in i64 value_balance.
const FORGED_SATS: u64 = 1_000_000 * 100_000_000;

/// Temp directory for an LMDB env, removed on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "thunder_orchard_anchor_test_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _unused = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a position-0 merkle path and the resulting anchor for a single leaf,
/// without ever inserting the leaf into the chain's note-commitment tree.
fn forged_path_and_anchor(
    cmx: &o::ExtractedNoteCommitment,
) -> (o::MerklePath, o::Anchor) {
    // 32 = orchard::NOTE_COMMITMENT_TREE_DEPTH. The auth path for the leftmost
    // leaf is just the empty subtree root at each level.
    const DEPTH: u8 = 32;
    let auth_path: Vec<o::MerkleHashOrchard> = (0..DEPTH)
        .map(|i| <o::MerkleHashOrchard as Hashable>::empty_root(Level::from(i)))
        .collect();
    let auth_path: [o::MerkleHashOrchard; DEPTH as usize] =
        auth_path.try_into().unwrap();
    let merkle_path = o::MerklePath::from_parts(0, auth_path);
    let raw_anchor = merkle_path.root(cmx.0);
    let anchor = o::Anchor::wrap(raw_anchor);
    (merkle_path, anchor)
}

/// Forge a note of arbitrary value owned by the attacker, plus a spendable
/// merkle path and anchor for a tree that exists only in the attacker's head.
fn forge_spend() -> (
    orchard::keys::FullViewingKey,
    orchard::keys::SpendingKey,
    o::Note,
    o::MerklePath,
    o::Anchor,
) {
    let sk = loop {
        let bytes: [u8; 32] = rand::random();
        if let Some(sk) =
            orchard::keys::SpendingKey::from_bytes(bytes).into_option()
        {
            break sk;
        }
    };
    let fvk = orchard::keys::FullViewingKey::from(&sk);
    let recipient = fvk.address_at(0u32, orchard::keys::Scope::External);

    // `rho` need only be a valid Pallas base element; the circuit does not check
    // that it came from a real prior nullifier.
    let rho = loop {
        let bytes: [u8; 32] = rand::random();
        if let Some(rho) = orchard::note::Rho::from_bytes(&bytes).into_option()
        {
            break rho;
        }
    };
    let rseed = loop {
        let bytes: [u8; 32] = rand::random();
        if let Some(rseed) =
            orchard::note::RandomSeed::from_bytes(bytes, &rho).into_option()
        {
            break rseed;
        }
    };
    let note = orchard::Note::from_parts(
        recipient,
        orchard::value::NoteValue::from_raw(FORGED_SATS),
        rho,
        rseed,
    )
    .into_option()
    .expect("valid forged note");

    let raw_cmx: orchard::note::ExtractedNoteCommitment =
        note.commitment().into();
    let cmx = o::ExtractedNoteCommitment::from(raw_cmx);
    let (path, anchor) = forged_path_and_anchor(&cmx);
    (fvk, sk, o::Note::wrap(note), path, anchor)
}

/// Build the attacker's unshielding transaction: spend the forged note,
/// value_balance = +FORGED_SATS, and emit a transparent UTXO of that value.
fn build_attack_tx(
    attacker_addr: TransparentAddress,
    empty_utreexo_proof: rustreexo::accumulator::proof::Proof,
) -> AuthorizedTransaction {
    let (fvk, sk, note, path, anchor) = forge_spend();

    let flags = o::BundleFlags::ENABLED;
    let mut builder = o::Builder::new(flags, false, anchor);
    builder
        .add_spend(fvk.clone(), note, path)
        .expect("add_spend");
    // One zero-value output so value_balance = FORGED_SATS - 0 = +FORGED_SATS.
    let ovk = fvk.to_ovk(orchard::keys::Scope::Internal);
    let out_addr =
        o::Address::wrap(fvk.address_at(1u32, orchard::keys::Scope::External));
    builder
        .add_output(
            Some(ovk.clone()),
            out_addr,
            o::NoteValue::from_raw(0),
            [0u8; 512],
        )
        .expect("add_output");

    let (bundle, _meta) = builder
        .build(rand::rngs::OsRng, Some(ovk))
        .expect("build")
        .expect("non-empty bundle");
    let bundle = bundle.create_proof(rand::rngs::OsRng).expect("prove");

    let outputs = vec![Output {
        address: attacker_addr,
        content: OutputContent::Value(bitcoin::Amount::from_sat(FORGED_SATS)),
    }];
    let tx = Transaction {
        inputs: Vec::new(),
        proof: empty_utreexo_proof,
        outputs,
        orchard_bundle: Some(bundle),
    };
    let spend_auth_key = orchard::keys::SpendAuthorizingKey::from(&sk);
    let tx = authorization::sign_orchard(&[spend_auth_key], tx)
        .expect("sign_orchard");

    // No transparent inputs -> no ed25519 authorizations needed.
    AuthorizedTransaction {
        transaction: tx,
        authorizations: Vec::new(),
    }
}

/// Compute the utreexo roots a valid header must declare for this body.
fn expected_roots(
    state: &State,
    rotxn: &RoTxn,
    body: &Body,
) -> Vec<BitcoinNodeHash> {
    let mut acc = state.get_accumulator(rotxn).unwrap();
    let mut diff = AccumulatorDiff::default();
    let merkle_root = body.compute_merkle_root();
    for (vout, output) in body.coinbase.iter().enumerate() {
        let outpoint = OutPoint::Coinbase {
            merkle_root,
            vout: vout as u32,
        };
        diff.insert(
            (&PointedOutput {
                outpoint,
                output: output.clone(),
            })
                .into(),
        );
    }
    for tx in &body.transactions {
        let txid = tx.txid();
        for (_op, utxo_hash) in &tx.inputs {
            diff.remove(utxo_hash.into());
        }
        for (vout, output) in tx.outputs.iter().enumerate() {
            let outpoint = OutPoint::Regular {
                txid,
                vout: vout as u32,
            };
            diff.insert(
                (&PointedOutput {
                    outpoint,
                    output: output.clone(),
                })
                    .into(),
            );
        }
    }
    acc.apply_diff(diff).unwrap();
    acc.get_roots()
}

#[test]
fn forged_anchor_rejected_in_block() {
    let tmp = TempDir::new();
    let env = {
        let mut opts = heed::EnvOpenOptions::new();
        opts.map_size(1024 * 1024 * 1024).max_dbs(State::NUM_DBS);
        unsafe { sneed::Env::open(&opts, &tmp.0) }.unwrap()
    };
    let state = State::new(&env).unwrap();

    let attacker_addr = TransparentAddress([0x11; 20]);
    let empty_proof = {
        let rotxn = env.read_txn().unwrap();
        state
            .get_utreexo_proof(&rotxn, std::iter::empty::<&PointedOutput>())
            .unwrap()
    };

    let auth_tx = build_attack_tx(attacker_addr, empty_proof);

    // Mempool path must reject the forged anchor.
    {
        let rotxn = env.read_txn().unwrap();
        let err = state
            .validate_transaction(&rotxn, &auth_tx)
            .expect_err("mempool must reject forged anchor");
        assert!(
            matches!(err, Error::Orchard(error::Orchard::InvalidAnchor { .. })),
            "expected InvalidAnchor, got: {err:?}"
        );
    }

    // Block-validation path must reject the same transaction. Before the fix,
    // `validate_block` accepted it and minted FORGED_SATS out of nothing.
    let body = Body::new(vec![auth_tx], Vec::new());
    let header = {
        let rotxn = env.read_txn().unwrap();
        Header {
            merkle_root: body.compute_merkle_root(),
            prev_side_hash: state.try_get_tip(&rotxn).unwrap(), // None at genesis
            prev_main_hash: bitcoin::BlockHash::from_byte_array([0u8; 32]),
            roots: expected_roots(&state, &rotxn, &body),
        }
    };
    {
        let rotxn = env.read_txn().unwrap();
        let err = state
            .validate_block(&rotxn, &header, &body)
            .expect_err("block path must reject forged anchor");
        assert!(
            matches!(err, Error::Orchard(error::Orchard::InvalidAnchor { .. })),
            "expected InvalidAnchor, got: {err:?}"
        );
    }
}

// A block that does not change the orchard root must still archive its
// frontier, otherwise a reorg disconnecting a later orchard block cannot
// restore the tree and nodes diverge.
#[test]
fn frontier_archived_when_orchard_root_unchanged() {
    let tmp = TempDir::new();
    let env = {
        let mut opts = heed::EnvOpenOptions::new();
        opts.map_size(1024 * 1024 * 1024).max_dbs(State::NUM_DBS);
        unsafe { sneed::Env::open(&opts, &tmp.0) }.unwrap()
    };
    let state = State::new(&env).unwrap();

    let connect_empty = |rwtxn: &mut sneed::RwTxn| {
        let body = Body::new(Vec::new(), Vec::new());
        let header = Header {
            merkle_root: body.compute_merkle_root(),
            prev_side_hash: state.try_get_tip(rwtxn).unwrap(),
            prev_main_hash: bitcoin::BlockHash::from_byte_array([0u8; 32]),
            roots: expected_roots(&state, rwtxn, &body),
        };
        state.apply_block(rwtxn, &header, &body).unwrap()
    };

    let mut rwtxn = env.write_txn().unwrap();
    // First empty block establishes the (empty) orchard root.
    let _first = connect_empty(&mut rwtxn);
    // Second empty block does not change the orchard root.
    let archived = connect_empty(&mut rwtxn);
    rwtxn.commit().unwrap();

    assert!(
        archived.is_some(),
        "frontier must be archived even when the orchard root is unchanged"
    );
}
