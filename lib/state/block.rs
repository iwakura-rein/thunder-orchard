//! Connect and disconnect blocks

use rayon::prelude::*;
use rustreexo::accumulator::node_hash::BitcoinNodeHash;
use sneed::{RoTxn, RwTxn};

use crate::{
    state::{Error, PrevalidatedBlock, State, error},
    types::{
        Accumulator, AccumulatorDiff, AmountOverflowError, Body, GetValue as _,
        Header, InPoint, OutPoint, OutPointKey, Output, PointedOutput,
        SpentOutput, Transaction, orchard,
    },
    wallet::Authorization,
};

/// Calculate total number of inputs across all transactions in a block body
fn calculate_total_inputs(body: &Body) -> usize {
    body.transactions.iter().map(|t| t.inputs.len()).sum()
}

pub fn validate(
    state: &State,
    rotxn: &RoTxn,
    header: &Header,
    body: &Body,
) -> Result<bitcoin::Amount, Error> {
    let tip_hash = state.try_get_tip(rotxn)?;
    if header.prev_side_hash != tip_hash {
        let err = error::InvalidHeader::PrevSideHash {
            expected: tip_hash,
            received: header.prev_side_hash,
        };
        return Err(Error::InvalidHeader(err));
    };
    let height = state.try_get_height(rotxn)?.map_or(0, |height| height + 1);
    if body.authorizations.len() > State::body_sigops_limit(height) {
        return Err(Error::TooManySigops);
    }
    let body_size =
        borsh::object_length(&body).map_err(Error::BorshSerialize)?;
    if body_size > State::body_size_limit(height) {
        return Err(Error::BodyTooLarge);
    }
    let mut accumulator = state.utreexo_accumulator.get(rotxn, &())?;
    let mut accumulator_diff = AccumulatorDiff::default();
    let mut coinbase_value = bitcoin::Amount::ZERO;
    let merkle_root = body.compute_merkle_root();
    if merkle_root != header.merkle_root {
        let err = error::InvalidBody {
            expected: merkle_root,
            computed: header.merkle_root,
        };
        return Err(Error::InvalidBody(err));
    }
    for (vout, output) in body.coinbase.iter().enumerate() {
        coinbase_value = coinbase_value
            .checked_add(output.get_value())
            .ok_or(AmountOverflowError)?;
        let outpoint = OutPoint::Coinbase {
            merkle_root,
            vout: vout as u32,
        };
        let pointed_output = PointedOutput {
            outpoint,
            output: output.clone(),
        };
        accumulator_diff.insert((&pointed_output).into());
    }
    let mut total_fees = bitcoin::Amount::ZERO;
    let filled_transactions: Vec<_> = body
        .transactions
        .iter()
        .map(|t| state.fill_transaction(rotxn, t))
        .collect::<Result<_, _>>()?;
    let total_inputs = calculate_total_inputs(body);

    // Collect all inputs as fixed-width keys for efficient double-spend detection via sort-and-scan
    let mut all_input_keys = Vec::with_capacity(total_inputs);
    for filled_transaction in &filled_transactions {
        for (outpoint, _) in &filled_transaction.transaction.inputs {
            all_input_keys.push(OutPointKey::from_outpoint(outpoint));
        }
    }

    // Sort and check for duplicate outpoints (double-spend detection)
    all_input_keys.par_sort_unstable();
    if all_input_keys.windows(2).any(|w| w[0] == w[1]) {
        return Err(Error::UtxoDoubleSpent);
    }

    // Process transactions for utreexo and fee validation
    for filled_transaction in &filled_transactions {
        let txid = filled_transaction.transaction.txid();
        // hashes of spent utxos, used to verify the utreexo proof
        let mut spent_utxo_hashes = Vec::<BitcoinNodeHash>::with_capacity(
            filled_transaction.transaction.inputs.len(),
        );
        for (_outpoint, utxo_hash) in &filled_transaction.transaction.inputs {
            spent_utxo_hashes.push(utxo_hash.into());
            accumulator_diff.remove(utxo_hash.into());
        }
        for (vout, output) in
            filled_transaction.transaction.outputs.iter().enumerate()
        {
            let outpoint = OutPoint::Regular {
                txid,
                vout: vout as u32,
            };
            let pointed_output = PointedOutput {
                outpoint,
                output: output.clone(),
            };
            accumulator_diff.insert((&pointed_output).into());
        }
        total_fees = total_fees
            .checked_add(state.validate_filled_transaction(filled_transaction)?)
            .ok_or(AmountOverflowError)?;
        // verify utreexo proof
        if !accumulator
            .verify(&filled_transaction.transaction.proof, &spent_utxo_hashes)?
        {
            return Err(Error::UtreexoProofFailed { txid });
        }
    }
    if coinbase_value > total_fees {
        return Err(Error::NotEnoughFees);
    }
    let spent_utxos = filled_transactions
        .iter()
        .flat_map(|t| t.spent_utxos.iter());
    for (authorization, spent_utxo) in
        body.authorizations.iter().zip(spent_utxos)
    {
        if authorization.get_address() != spent_utxo.address {
            return Err(Error::WrongPubKeyForAddress);
        }
    }
    if Authorization::verify_body(body).is_err() {
        return Err(Error::AuthorizationError);
    }
    let () = accumulator.apply_diff(accumulator_diff)?;
    let roots: Vec<BitcoinNodeHash> = accumulator.get_roots();
    if roots != header.roots {
        return Err(Error::UtreexoRootsMismatch);
    }
    Ok(total_fees)
}

pub fn prevalidate(
    state: &State,
    rotxn: &RoTxn,
    header: &Header,
    body: &Body,
) -> Result<PrevalidatedBlock, Error> {
    let tip_hash = state.try_get_tip(rotxn)?;
    if header.prev_side_hash != tip_hash {
        let err = error::InvalidHeader::PrevSideHash {
            expected: tip_hash,
            received: header.prev_side_hash,
        };
        return Err(Error::InvalidHeader(err));
    };
    let height = state.try_get_height(rotxn)?.map_or(0, |height| height + 1);
    if body.authorizations.len() > State::body_sigops_limit(height) {
        return Err(Error::TooManySigops);
    }
    let body_size =
        borsh::object_length(&body).map_err(Error::BorshSerialize)?;
    if body_size > State::body_size_limit(height) {
        return Err(Error::BodyTooLarge);
    }
    let mut accumulator = state.utreexo_accumulator.get(rotxn, &())?;
    let mut accumulator_diff = AccumulatorDiff::default();
    let mut coinbase_value = bitcoin::Amount::ZERO;
    let merkle_root = body.compute_merkle_root();
    if merkle_root != header.merkle_root {
        let err = error::InvalidBody {
            expected: merkle_root,
            computed: header.merkle_root,
        };
        return Err(Error::InvalidBody(err));
    }
    for (vout, output) in body.coinbase.iter().enumerate() {
        coinbase_value = coinbase_value
            .checked_add(output.get_value())
            .ok_or(AmountOverflowError)?;
        let outpoint = OutPoint::Coinbase {
            merkle_root,
            vout: vout as u32,
        };
        let pointed_output = PointedOutput {
            outpoint,
            output: output.clone(),
        };
        accumulator_diff.insert((&pointed_output).into());
    }
    let mut total_fees = bitcoin::Amount::ZERO;
    let filled_transactions: Vec<_> = body
        .transactions
        .iter()
        .map(|t| state.fill_transaction(rotxn, t))
        .collect::<Result<_, _>>()?;
    let total_inputs = calculate_total_inputs(body);

    // Collect all inputs as fixed-width keys for efficient double-spend detection via sort-and-scan
    let mut all_input_keys = Vec::with_capacity(total_inputs);
    for filled_transaction in &filled_transactions {
        for (outpoint, _) in &filled_transaction.transaction.inputs {
            all_input_keys.push(OutPointKey::from_outpoint(outpoint));
        }
    }

    // Sort and check for duplicate outpoints (double-spend detection)
    all_input_keys.par_sort_unstable();
    if all_input_keys.windows(2).any(|w| w[0] == w[1]) {
        return Err(Error::UtxoDoubleSpent);
    }

    // Process transactions for utreexo and fee validation
    for filled_transaction in &filled_transactions {
        let txid = filled_transaction.transaction.txid();
        // hashes of spent utxos, used to verify the utreexo proof
        let mut spent_utxo_hashes = Vec::<BitcoinNodeHash>::with_capacity(
            filled_transaction.transaction.inputs.len(),
        );
        for (_outpoint, utxo_hash) in &filled_transaction.transaction.inputs {
            spent_utxo_hashes.push(utxo_hash.into());
            accumulator_diff.remove(utxo_hash.into());
        }
        for (vout, output) in
            filled_transaction.transaction.outputs.iter().enumerate()
        {
            let outpoint = OutPoint::Regular {
                txid,
                vout: vout as u32,
            };
            let pointed_output = PointedOutput {
                outpoint,
                output: output.clone(),
            };
            accumulator_diff.insert((&pointed_output).into());
        }
        total_fees = total_fees
            .checked_add(state.validate_filled_transaction(filled_transaction)?)
            .ok_or(AmountOverflowError)?;
        // verify utreexo proof
        if !accumulator
            .verify(&filled_transaction.transaction.proof, &spent_utxo_hashes)?
        {
            return Err(Error::UtreexoProofFailed { txid });
        }
    }
    if coinbase_value > total_fees {
        return Err(Error::NotEnoughFees);
    }
    let spent_utxos = filled_transactions
        .iter()
        .flat_map(|t| t.spent_utxos.iter());
    for (authorization, spent_utxo) in
        body.authorizations.iter().zip(spent_utxos)
    {
        if authorization.get_address() != spent_utxo.address {
            return Err(Error::WrongPubKeyForAddress);
        }
    }
    if Authorization::verify_body(body).is_err() {
        return Err(Error::AuthorizationError);
    }
    let () = accumulator.apply_diff(accumulator_diff.clone())?;
    let roots: Vec<BitcoinNodeHash> = accumulator.get_roots();
    if roots != header.roots {
        return Err(Error::UtreexoRootsMismatch);
    }
    Ok(PrevalidatedBlock {
        filled_transactions,
        computed_merkle_root: merkle_root,
        total_fees,
        coinbase_value,
        next_height: height,
        accumulator_diff,
    })
}

pub fn connect_prevalidated(
    state: &State,
    rwtxn: &mut RwTxn,
    header: &Header,
    body: &Body,
    prevalidated: PrevalidatedBlock,
) -> Result<Option<orchard::Frontier>, error::ConnectBlock> {
    let merkle_root = prevalidated.computed_merkle_root;

    let mut accumulator = state.utreexo_accumulator.get(rwtxn, &())?;
    let accumulator_diff = prevalidated.accumulator_diff;

    let mut frontier = state
        .orchard
        .frontier()
        .get(rwtxn, &())
        .map_err(error::Orchard::from)?;

    // Coalesce UTXO/STXO mutations for sorted application
    use rayon::prelude::*;
    let mut utxo_deletes: Vec<OutPointKey> = Vec::new();
    let mut stxo_puts: Vec<(OutPointKey, SpentOutput)> = Vec::new();
    let mut utxo_puts: Vec<(OutPointKey, Output)> = Vec::new();

    // Collect coinbase UTXOs
    for (vout, output) in body.coinbase.iter().enumerate() {
        let outpoint = OutPoint::Coinbase {
            merkle_root,
            vout: vout as u32,
        };
        let key = OutPointKey::from_outpoint(&outpoint);
        utxo_puts.push((key, output.clone()));
    }

    // Collect TX mutations
    for filled_transaction in &prevalidated.filled_transactions {
        let txid = filled_transaction.transaction.txid();

        // Inputs: delete UTXOs and create STXOs
        for (vin, (outpoint, _utxo_hash)) in
            filled_transaction.transaction.inputs.iter().enumerate()
        {
            let spent_utxo = &filled_transaction.spent_utxos[vin];
            let key = OutPointKey::from_outpoint(outpoint);
            utxo_deletes.push(key);
            stxo_puts.push((
                key,
                SpentOutput {
                    output: spent_utxo.clone(),
                    inpoint: InPoint::Regular {
                        txid,
                        vin: vin as u32,
                    },
                },
            ));
        }

        // Outputs: create UTXOs
        for (vout, output) in
            filled_transaction.transaction.outputs.iter().enumerate()
        {
            let outpoint = OutPoint::Regular {
                txid,
                vout: vout as u32,
            };
            let key = OutPointKey::from_outpoint(&outpoint);
            utxo_puts.push((key, output.clone()));
        }

        // Handle orchard bundle if present
        if let Some(orchard_bundle) =
            filled_transaction.transaction.orchard_bundle.as_ref()
        {
            let () =
                connect_orchard(state, rwtxn, orchard_bundle, &mut frontier)
                    .map_err(|err| error::ConnectBlock::ConnectTransaction {
                        txid,
                        source: error::ConnectTransaction::Orchard(err),
                    })?;
        }
    }

    // Sort operations by key to improve B+tree locality
    utxo_deletes.par_sort_unstable();
    stxo_puts.par_sort_unstable_by_key(|(k, _)| *k);
    utxo_puts.par_sort_unstable_by_key(|(k, _)| *k);

    // Apply deletes first
    for key in &utxo_deletes {
        let _ = state.utxos.delete(rwtxn, key)?;
    }
    // Then STXO puts
    for (key, spent_output) in &stxo_puts {
        state.stxos.put(rwtxn, key, spent_output)?;
    }
    // Finally UTXO puts
    for (key, output) in &utxo_puts {
        state.utxos.put(rwtxn, key, output)?;
    }

    // Update tip and height using precomputed values (no redundant DB reads)
    let block_hash = header.hash();
    state.tip.put(rwtxn, &(), &block_hash)?;
    state.height.put(rwtxn, &(), &prevalidated.next_height)?;

    // Apply accumulator diff and update orchard state
    let () = accumulator.apply_diff(accumulator_diff)?;
    state.utreexo_accumulator.put(rwtxn, &(), &accumulator)?;
    let () = state.orchard.put_frontier(rwtxn, &frontier)?;
    let root_changed: bool = state.orchard.put_historical_root(
        rwtxn,
        block_hash,
        frontier.root(),
    )?;
    let res = if root_changed { Some(frontier) } else { None };
    Ok(res)
}

/// Connect the orchard components of a transaction
fn connect_orchard(
    state: &State,
    rwtxn: &mut RwTxn,
    bundle: &orchard::Bundle<orchard::Authorized>,
    frontier: &mut orchard::Frontier,
) -> Result<(), error::Orchard> {
    // Update frontier
    {
        for cmx in bundle.extracted_note_commitments() {
            if !frontier.append(cmx) {
                return Err(error::Orchard::AppendCommitment);
            }
        }
    }
    // Store nullifiers
    for nullifier in bundle.nullifiers() {
        if state.orchard.nullifiers().contains_key(rwtxn, nullifier)? {
            return Err(error::Orchard::NullifierDoubleSpent {
                nullifier: *nullifier,
            });
        }
        state.orchard.put_nullifier(rwtxn, nullifier)?;
    }
    Ok(())
}

fn connect_transaction(
    state: &State,
    rwtxn: &mut RwTxn,
    transaction: &Transaction,
    accumulator_diff: &mut AccumulatorDiff,
    frontier: &mut orchard::Frontier,
) -> Result<(), error::ConnectTransaction> {
    let txid = transaction.txid();
    for (vin, (outpoint, utxo_hash)) in transaction.inputs.iter().enumerate() {
        let spent_output = state
            .utxos
            .try_get(rwtxn, &OutPointKey::from_outpoint(outpoint))?
            .ok_or(error::NoUtxo {
                outpoint: *outpoint,
            })?;
        accumulator_diff.remove(utxo_hash.into());
        state
            .utxos
            .delete(rwtxn, &OutPointKey::from_outpoint(outpoint))?;
        let spent_output = SpentOutput {
            output: spent_output,
            inpoint: InPoint::Regular {
                txid,
                vin: vin as u32,
            },
        };
        state.stxos.put(
            rwtxn,
            &OutPointKey::from_outpoint(outpoint),
            &spent_output,
        )?;
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
        accumulator_diff.insert((&pointed_output).into());
        let key = OutPointKey::from_outpoint(&outpoint);
        state.utxos.put(rwtxn, &key, output)?;
    }
    if let Some(orchard_bundle) = transaction.orchard_bundle.as_ref() {
        let () = connect_orchard(state, rwtxn, orchard_bundle, frontier)?;
    }
    Ok(())
}

/// Returns data that must be archived in order to disconnect to the new tip.
/// Returns `Ok(Some(_))` if connecting a block changed the orchard frontier.
pub fn connect(
    state: &State,
    rwtxn: &mut RwTxn,
    header: &Header,
    body: &Body,
) -> Result<Option<orchard::Frontier>, error::ConnectBlock> {
    let tip_hash = state.try_get_tip(rwtxn)?;
    if tip_hash != header.prev_side_hash {
        let err = error::InvalidHeader::PrevSideHash {
            expected: tip_hash,
            received: header.prev_side_hash,
        };
        return Err(error::ConnectBlock::InvalidHeader(err));
    }
    let merkle_root = body.compute_merkle_root();
    if merkle_root != header.merkle_root {
        let err = error::InvalidBody {
            expected: merkle_root,
            computed: header.merkle_root,
        };
        return Err(err.into());
    }
    let mut accumulator = state.utreexo_accumulator.get(rwtxn, &())?;
    let mut accumulator_diff = AccumulatorDiff::default();
    let mut frontier = state
        .orchard
        .frontier()
        .get(rwtxn, &())
        .map_err(error::Orchard::from)?;
    for (vout, output) in body.coinbase.iter().enumerate() {
        let outpoint = OutPoint::Coinbase {
            merkle_root,
            vout: vout as u32,
        };
        let pointed_output = PointedOutput {
            outpoint,
            output: output.clone(),
        };
        accumulator_diff.insert((&pointed_output).into());
        let key = OutPointKey::from_outpoint(&outpoint);
        state.utxos.put(rwtxn, &key, output)?;
    }
    for transaction in &body.transactions {
        let () = connect_transaction(
            state,
            rwtxn,
            transaction,
            &mut accumulator_diff,
            &mut frontier,
        )
        .map_err(|err| error::ConnectBlock::ConnectTransaction {
            txid: transaction.txid(),
            source: err,
        })?;
    }
    let block_hash = header.hash();
    let height = state.try_get_height(rwtxn)?.map_or(0, |height| height + 1);
    state.tip.put(rwtxn, &(), &block_hash)?;
    state.height.put(rwtxn, &(), &height)?;
    let () = accumulator.apply_diff(accumulator_diff)?;
    state.utreexo_accumulator.put(rwtxn, &(), &accumulator)?;
    let () = state.orchard.put_frontier(rwtxn, &frontier)?;
    let root_changed: bool = state.orchard.put_historical_root(
        rwtxn,
        block_hash,
        frontier.root(),
    )?;
    let res = if root_changed { Some(frontier) } else { None };
    Ok(res)
}

/// Disconnect the orchard components of a transaction.
/// The note commitments merkle frontier is not reverted, and must be restored
/// from a checkpoint.
fn disconnect_orchard(
    state: &State,
    rwtxn: &mut RwTxn,
    bundle: &orchard::Bundle<orchard::Authorized>,
) -> Result<(), error::Orchard> {
    // Delete used nullifiers
    for nullifier in bundle.nullifiers() {
        let _: bool = state.orchard.delete_nullifier(rwtxn, nullifier)?;
    }
    Ok(())
}

fn disconnect_transaction(
    state: &State,
    rwtxn: &mut RwTxn,
    transaction: &Transaction,
    accumulator_diff: &mut AccumulatorDiff,
) -> Result<(), Error> {
    if let Some(orchard_bundle) = transaction.orchard_bundle.as_ref() {
        let () = disconnect_orchard(state, rwtxn, orchard_bundle)?;
    }
    let txid = transaction.txid();
    // delete UTXOs, last-to-first
    transaction.outputs.iter().enumerate().rev().try_for_each(
        |(vout, output)| {
            let outpoint = OutPoint::Regular {
                txid,
                vout: vout as u32,
            };
            let pointed_output = PointedOutput {
                outpoint,
                output: output.clone(),
            };
            accumulator_diff.remove((&pointed_output).into());
            if state
                .utxos
                .delete(rwtxn, &OutPointKey::from_outpoint(&outpoint))?
            {
                Ok::<_, Error>(())
            } else {
                Err(error::NoUtxo { outpoint }.into())
            }
        },
    )?;
    // unspend STXOs, last-to-first
    transaction
        .inputs
        .iter()
        .rev()
        .try_for_each(|(outpoint, utxo_hash)| {
            if let Some(spent_output) = state
                .stxos
                .try_get(rwtxn, &OutPointKey::from_outpoint(outpoint))?
            {
                accumulator_diff.insert(utxo_hash.into());
                state
                    .stxos
                    .delete(rwtxn, &OutPointKey::from_outpoint(outpoint))?;
                state.utxos.put(
                    rwtxn,
                    &OutPointKey::from_outpoint(outpoint),
                    &spent_output.output,
                )?;
                Ok(())
            } else {
                Err(Error::NoStxo {
                    outpoint: *outpoint,
                })
            }
        })
}

pub fn disconnect_tip(
    state: &State,
    rwtxn: &mut RwTxn,
    header: &Header,
    body: &Body,
    prev_accumulator: &Accumulator,
    prev_note_commitments_merkle_frontier: Option<&orchard::Frontier>,
) -> Result<(), Error> {
    let tip_hash = state.tip.try_get(rwtxn, &())?.ok_or(Error::NoTip)?;
    if tip_hash != header.hash() {
        let err = error::InvalidHeader::BlockHash {
            expected: tip_hash,
            computed: header.hash(),
        };
        return Err(Error::InvalidHeader(err));
    }
    let merkle_root = body.compute_merkle_root();
    if merkle_root != header.merkle_root {
        let err = error::InvalidBody {
            expected: merkle_root,
            computed: header.merkle_root,
        };
        return Err(Error::InvalidBody(err));
    }
    let mut accumulator = state.utreexo_accumulator.get(rwtxn, &())?;
    tracing::debug!("Got acc");
    let mut accumulator_diff = AccumulatorDiff::default();
    // revert txs, last-to-first
    body.transactions.iter().rev().try_for_each(|tx| {
        disconnect_transaction(state, rwtxn, tx, &mut accumulator_diff)
    })?;
    // delete coinbase UTXOs, last-to-first
    body.coinbase
        .iter()
        .enumerate()
        .rev()
        .try_for_each(|(vout, output)| {
            let outpoint = OutPoint::Coinbase {
                merkle_root,
                vout: vout as u32,
            };
            let pointed_output = PointedOutput {
                outpoint,
                output: output.clone(),
            };
            accumulator_diff.remove((&pointed_output).into());
            if state
                .utxos
                .delete(rwtxn, &OutPointKey::from_outpoint(&outpoint))?
            {
                Ok::<_, Error>(())
            } else {
                Err(error::NoUtxo { outpoint }.into())
            }
        })?;
    let height = state
        .try_get_height(rwtxn)?
        .expect("Height should not be None");
    match (header.prev_side_hash, height) {
        (None, 0) => {
            state.tip.delete(rwtxn, &())?;
            state.height.delete(rwtxn, &())?;
        }
        (None, _) | (_, 0) => return Err(Error::NoTip),
        (Some(prev_side_hash), height) => {
            state.tip.put(rwtxn, &(), &prev_side_hash)?;
            state.height.put(rwtxn, &(), &(height - 1))?;
        }
    }
    let () = accumulator.apply_diff(accumulator_diff)?;
    // Accumulator is restored from archive, instead of computed during
    // disconnect
    state
        .utreexo_accumulator
        .put(rwtxn, &(), prev_accumulator)?;
    if let Some(frontier) = prev_note_commitments_merkle_frontier {
        state.orchard.put_frontier(rwtxn, frontier)?;
    }
    let _: bool = state.orchard.delete_historical_root(rwtxn, tip_hash)?;
    Ok(())
}
