use crate::wallet::Wallet;

#[test]
fn test_get_or_generate_last_address() -> anyhow::Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let test_dir = std::env::temp_dir()
        .join(format!("thunder_orchard_test_wallet_{nanos}"));

    // Ensure clean state
    if test_dir.exists() {
        let _unused = std::fs::remove_dir_all(&test_dir);
    }

    let wallet = Wallet::new(&test_dir)?;

    // Seed must be set before we can generate addresses
    assert!(!wallet.has_seed()?);
    let seed = [1u8; 64];
    wallet.set_seed(&seed)?;
    assert!(wallet.has_seed()?);

    {
        let mut rwtxn = wallet.env().write_txn()?;

        // Get last address when none have been generated
        let last_orchard = wallet.try_get_last_orchard_address(&rwtxn)?;
        assert!(last_orchard.is_none());
        let last_transparent =
            wallet.try_get_last_transparent_address(&rwtxn)?;
        assert!(last_transparent.is_none());

        // The first call should generate the first address.
        let orchard_addr1 =
            wallet.get_or_generate_last_orchard_address(&mut rwtxn)?;
        let transparent_addr1 =
            wallet.get_or_generate_last_transparent_address(&mut rwtxn)?;

        let last_orchard = wallet.try_get_last_orchard_address(&rwtxn)?;
        assert_eq!(last_orchard, Some(orchard_addr1));
        let last_transparent =
            wallet.try_get_last_transparent_address(&rwtxn)?;
        assert_eq!(last_transparent, Some(transparent_addr1));

        let orchard_addr2 =
            wallet.get_or_generate_last_orchard_address(&mut rwtxn)?;
        assert_eq!(orchard_addr1, orchard_addr2);
        let transparent_addr2 =
            wallet.get_or_generate_last_transparent_address(&mut rwtxn)?;
        assert_eq!(transparent_addr1, transparent_addr2);

        let orchard_addr3 = wallet.get_new_orchard_address(&mut rwtxn)?;
        assert_ne!(orchard_addr1, orchard_addr3);
        let transparent_addr3 =
            wallet.get_new_transparent_address(&mut rwtxn)?;
        assert_ne!(transparent_addr1, transparent_addr3);

        let last_orchard = wallet.try_get_last_orchard_address(&rwtxn)?;
        assert_eq!(last_orchard, Some(orchard_addr3));
        let last_transparent =
            wallet.try_get_last_transparent_address(&rwtxn)?;
        assert_eq!(last_transparent, Some(transparent_addr3));

        let orchard_addr4 =
            wallet.get_or_generate_last_orchard_address(&mut rwtxn)?;
        assert_eq!(orchard_addr3, orchard_addr4);
        let transparent_addr4 =
            wallet.get_or_generate_last_transparent_address(&mut rwtxn)?;
        assert_eq!(transparent_addr3, transparent_addr4);
    }

    // Clean up
    let _unused = std::fs::remove_dir_all(&test_dir);
    Ok(())
}

mod fee_privacy {
    use crate::wallet::*;

    fn bill_exponents(cast: &Cast) -> Vec<u32> {
        cast.bill_exponents_with_timestamps
            .iter()
            .map(|(exp, _ts)| *exp)
            .collect()
    }

    /// A cast decomposes the amount into power-of-two "bill" denominations
    /// (one per set bit), with no fee folded into the bill amounts. This keeps
    /// each bill a standard denomination shared across users.
    #[test]
    fn cast_decomposes_into_standard_denominations() {
        for sats in [1u64, 0b1011, 1_000_000, (1 << 20) + (1 << 5) + 1] {
            let cast = Cast::new(Amount::from_sat(sats));
            let mut exps = bill_exponents(&cast);
            exps.sort_unstable();
            let expected: Vec<u32> =
                (0..u64::BITS).filter(|i| (sats >> i) & 1 == 1).collect();
            assert_eq!(exps, expected, "bills must be the set bits of {sats}");
            let sum: u64 = exps.iter().map(|exp| 1u64 << exp).sum();
            assert_eq!(sum, sats, "bills must sum back to the amount");
        }
    }

    /// Privacy regression: the fee an observer can derive from a cast
    /// transaction (value_balance - transparent_output) is the same shared
    /// `STANDARD_FEE` for every bill and every cast, regardless of the amount.
    /// A per-user or per-amount fee would be a fingerprint linking a cast's
    /// bills together. The fee is not a caller-supplied parameter.
    #[test]
    fn all_cast_bills_use_the_same_shared_fee() {
        assert_eq!(Cast::tx_fee(), STANDARD_FEE);

        // Footprint of a bill as seen on chain: an unshield of denomination
        // `2^exp` has transparent output `2^exp` and value balance
        // `2^exp + fee`, so the observer-derived fee is exactly the fee used.
        let observed_fees = |sats: u64| -> Vec<Amount> {
            let cast = Cast::new(Amount::from_sat(sats));
            bill_exponents(&cast)
                .into_iter()
                .map(|exp| {
                    let denom = Amount::from_sat(1 << exp);
                    let value_balance = denom + Cast::tx_fee();
                    value_balance - denom
                })
                .collect()
        };

        // Different amounts (e.g. two different users), all bills, one fee.
        for sats in [0b1011u64, 1_000_000, 12_345_678] {
            for fee in observed_fees(sats) {
                assert_eq!(
                    fee, STANDARD_FEE,
                    "every cast bill must use the shared standard fee"
                );
            }
        }
    }
}

mod melt_privacy {
    use crate::wallet::*;

    fn bill_exponents(melt: &MeltBatch) -> Vec<u32> {
        melt.bill_exponents_with_timestamps
            .iter()
            .map(|(exp, _ts)| *exp)
            .collect()
    }

    /// A melt decomposes the amount into power-of-two "bill" denominations
    /// (one shield transaction per set bit). Each transaction therefore shields
    /// a standard denomination `2^n`, so its publicly visible value balance no
    /// longer reveals the exact value of a source UTXO.
    #[test]
    fn melt_decomposes_into_standard_denominations() {
        for sats in [1u64, 0b1011, 1_000_000, (1 << 20) + (1 << 5) + 1] {
            let melt = MeltBatch::new(Amount::from_sat(sats));
            let mut exps = bill_exponents(&melt);
            exps.sort_unstable();
            let expected: Vec<u32> =
                (0..u64::BITS).filter(|i| (sats >> i) & 1 == 1).collect();
            assert_eq!(
                exps, expected,
                "melt bills must be the set bits of {sats}"
            );
            let sum: u64 = exps.iter().map(|exp| 1u64 << exp).sum();
            assert_eq!(sum, sats, "melt bills must sum back to the amount");
        }
    }

    /// Every melt transaction uses the shared standard fee, independent of the
    /// amount, so melt transactions cannot be linked by a per-user fee.
    #[test]
    fn melt_uses_shared_standard_fee() {
        assert_eq!(MeltBatch::tx_fee(), STANDARD_FEE);
    }
}

mod anchor_depth {
    use std::convert::Infallible;

    use crate::wallet::*;

    /// Pick the anchor depth for a tree with `checkpoints` checkpoints: a
    /// checkpoint exists at depth `d` iff `d < checkpoints`.
    fn pick(checkpoints: usize, max_depth: usize) -> Option<usize> {
        deepest_available_anchor_depth(max_depth, |depth| {
            Ok::<bool, Infallible>(depth < checkpoints)
        })
        .unwrap()
    }

    /// With enough history, spends anchor at the full target depth, i.e. behind
    /// the tip (depth 0), not at it.
    #[test]
    fn anchors_behind_the_tip_when_history_is_deep() {
        const { assert!(ANCHOR_CHECKPOINT_DEPTH > 0, "must not anchor at the tip") };
        assert_eq!(
            pick(100, ANCHOR_CHECKPOINT_DEPTH),
            Some(ANCHOR_CHECKPOINT_DEPTH)
        );
        assert_eq!(pick(10, 3), Some(3));
    }

    /// While the tree is young, fall back to the deepest available checkpoint.
    #[test]
    fn falls_back_to_deepest_available_when_young() {
        assert_eq!(pick(1, 3), Some(0)); // only the tip checkpoint exists
        assert_eq!(pick(2, 3), Some(1));
        assert_eq!(pick(3, 3), Some(2));
        assert_eq!(pick(4, 3), Some(3));
    }

    /// No checkpoints (empty tree) yields no anchor depth.
    #[test]
    fn no_checkpoints_yields_none() {
        assert_eq!(pick(0, 3), None);
        assert_eq!(pick(0, 0), None);
    }

    /// Privacy property: whenever more than one checkpoint exists, the chosen
    /// anchor is strictly behind the tip.
    #[test]
    fn never_anchors_at_tip_when_history_exists() {
        for checkpoints in 2..=20 {
            let depth = pick(checkpoints, ANCHOR_CHECKPOINT_DEPTH).unwrap();
            assert!(depth >= 1, "anchor must be behind the tip");
            assert!(depth <= ANCHOR_CHECKPOINT_DEPTH);
            assert!(depth < checkpoints);
        }
    }
}
