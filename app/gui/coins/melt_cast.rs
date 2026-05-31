use std::sync::Arc;

use bitcoin::Amount;
use eframe::egui::{self, Button, InnerResponse};
use parking_lot::RwLock;
use poll_promise::Promise;
use thunder_orchard::{
    types::Txid,
    wallet::{self, MeltBatch},
};

use crate::{
    app::App,
    gui::util::{UiExt, show_btc_amount},
};

#[derive(Debug, Default)]
struct MeltInput {
    amount_input: String,
}

impl MeltInput {
    /// Returns `Some(amount)` if melt is clicked
    fn show(
        &mut self,
        app: Option<&App>,
        ui: &mut egui::Ui,
    ) -> InnerResponse<Option<Amount>> {
        let Some(app) = app else {
            return InnerResponse::new(None, ui.response());
        };
        let transparent_balance = match app.wallet.get_balance() {
            Ok(balance) => balance.available_transparent,
            Err(err) => {
                let err = anyhow::Error::from(err);
                return InnerResponse::new(
                    None,
                    ui.monospace_selectable_multiline(format!("{err:#}")),
                );
            }
        };
        ui.monospace_selectable_singleline(
            false,
            format!(
                "Transparent balance available: {}",
                show_btc_amount(transparent_balance)
            ),
        );
        let amount_edit = egui::TextEdit::singleline(&mut self.amount_input)
            .hint_text("melt amount")
            .desired_width(80.);
        ui.add(amount_edit);
        ui.label("BTC");
        let amount = bitcoin::Amount::from_str_in(
            &self.amount_input,
            bitcoin::Denomination::Bitcoin,
        );
        ui.monospace_selectable_singleline(
            false,
            format!("Fee per tx: {}", show_btc_amount(wallet::STANDARD_FEE)),
        );
        // One transaction per set bit of the amount, each charged the standard
        // fee.
        let total_fee = amount.as_ref().ok().and_then(|amount| {
            wallet::STANDARD_FEE
                .checked_mul(amount.to_sat().count_ones() as u64)
        });
        if let Some(total_fee) = total_fee {
            ui.monospace_selectable_singleline(
                false,
                format!("Total fee: {}", show_btc_amount(total_fee)),
            );
        }
        let total_spend = match (amount.as_ref(), total_fee.as_ref()) {
            (Ok(amount), Some(total_fee)) => amount.checked_add(*total_fee),
            (_, _) => None,
        };
        ui.vertical_centered(|ui| {
            if ui
                .add_enabled(
                    total_spend.is_some_and(|total_spend| {
                        total_spend <= transparent_balance
                    }),
                    Button::new("Melt"),
                )
                .clicked()
            {
                Some(amount.unwrap())
            } else {
                None
            }
        })
    }
}

struct Melting {
    txs: Arc<RwLock<Vec<Txid>>>,
    fut: Promise<anyhow::Result<()>>,
}

impl Melting {
    fn new(app: &App, mut melt_batch: MeltBatch) -> Self {
        let _rt_guard = app.runtime.enter();
        let txs = Arc::new(RwLock::new(Vec::new()));
        let fut = Promise::spawn_async({
            let app = app.clone();
            let txs = Arc::clone(&txs);
            async move {
                while let Some(tx_fn) = melt_batch.next_tx().await {
                    let accumulator = app.node.get_tip_accumulator()?;
                    let tx = tx_fn(&accumulator, &app.wallet)?;
                    let txid = tx.txid();
                    let () = app.sign_and_send(tx)?;
                    txs.write().push(txid);
                }
                Ok(())
            }
        });
        Self { txs, fut }
    }

    /// If completed, returns `true` if successful, `false` if unsuccessful.
    fn show(&mut self, ui: &mut egui::Ui) -> Option<bool> {
        ui.heading("Melting...");
        self.txs.read().iter().for_each(|txid| {
            ui.monospace_selectable_singleline(false, txid.to_string());
        });
        match self.fut.ready() {
            Some(Ok(())) => Some(true),
            Some(Err(err)) => {
                ui.monospace_selectable_multiline(format!("{err:#}"));
                Some(false)
            }
            None => None,
        }
    }
}

enum MeltInner {
    MeltInput(MeltInput),
    Melting(Melting),
}

impl Default for MeltInner {
    fn default() -> Self {
        Self::MeltInput(MeltInput::default())
    }
}

impl MeltInner {
    fn show(&mut self, app: Option<&App>, ui: &mut egui::Ui) {
        match self {
            Self::MeltInput(melt_input) => {
                if let Some(amount) = melt_input.show(app, ui).inner {
                    let melt_batch = MeltBatch::new(amount);
                    let melting = Melting::new(app.unwrap(), melt_batch);
                    *self = Self::Melting(melting);
                }
            }
            Self::Melting(melting) => {
                if let Some(true) = melting.show(ui) {
                    *self = Self::MeltInput(MeltInput::default())
                }
            }
        }
    }
}

#[derive(Default)]
#[repr(transparent)]
pub struct Melt(MeltInner);

impl Melt {
    pub fn show(&mut self, app: Option<&App>, ui: &mut egui::Ui) {
        self.0.show(app, ui);
    }
}

#[derive(Debug, Default)]
struct CastInput {
    amount_input: String,
}

impl CastInput {
    /// Returns `Some(amount)` if cast is clicked
    fn show(
        &mut self,
        app: Option<&App>,
        ui: &mut egui::Ui,
    ) -> InnerResponse<Option<Amount>> {
        let Some(app) = app else {
            return InnerResponse::new(None, ui.response());
        };
        let shielded_balance = match app.wallet.get_balance() {
            Ok(balance) => balance.available_shielded,
            Err(err) => {
                let err = anyhow::Error::from(err);
                return InnerResponse::new(
                    None,
                    ui.monospace_selectable_multiline(format!("{err:#}")),
                );
            }
        };
        ui.monospace_selectable_singleline(
            false,
            format!(
                "Shielded balance available: {}",
                show_btc_amount(shielded_balance)
            ),
        );
        let amount_edit = egui::TextEdit::singleline(&mut self.amount_input)
            .hint_text("cast amount")
            .desired_width(80.);
        ui.add(amount_edit);
        ui.label("BTC");
        let amount = bitcoin::Amount::from_str_in(
            &self.amount_input,
            bitcoin::Denomination::Bitcoin,
        );
        ui.monospace_selectable_singleline(
            false,
            format!("Fee per tx: {}", show_btc_amount(wallet::STANDARD_FEE)),
        );
        // One transaction per set bit of the amount, each charged the standard
        // fee.
        let total_fee = amount.as_ref().ok().and_then(|amount| {
            wallet::STANDARD_FEE
                .checked_mul(amount.to_sat().count_ones() as u64)
        });
        if let Some(total_fee) = total_fee {
            ui.monospace_selectable_singleline(
                false,
                format!("Total fee: {}", show_btc_amount(total_fee)),
            );
        }
        let total_spend = match (amount.as_ref(), total_fee.as_ref()) {
            (Ok(amount), Some(total_fee)) => amount.checked_add(*total_fee),
            (_, _) => None,
        };
        ui.vertical_centered(|ui| {
            if ui
                .add_enabled(
                    total_spend.is_some_and(|total_spend| {
                        total_spend <= shielded_balance
                    }),
                    Button::new("Cast"),
                )
                .clicked()
            {
                Some(amount.unwrap())
            } else {
                None
            }
        })
    }
}

struct Casting {
    txs: Arc<RwLock<Vec<Txid>>>,
    fut: Promise<anyhow::Result<()>>,
}

impl Casting {
    fn new(app: &App, mut cast: wallet::Cast) -> Self {
        let _rt_guard = app.runtime.enter();
        let txs = Arc::new(RwLock::new(Vec::new()));
        let fut = Promise::spawn_async({
            let app = app.clone();
            let txs = Arc::clone(&txs);
            async move {
                while let Some(tx_fn) = cast.next_tx().await {
                    let accumulator = app.node.get_tip_accumulator()?;
                    let tx = tx_fn(&accumulator, &app.wallet)?;
                    let txid = tx.txid();
                    let () = app.sign_and_send(tx)?;
                    txs.write().push(txid);
                }
                Ok(())
            }
        });
        Self { txs, fut }
    }

    /// If completed, returns `true` if successful, `false` if unsuccessful.
    fn show(&mut self, ui: &mut egui::Ui) -> Option<bool> {
        ui.heading("Casting...");
        self.txs.read().iter().for_each(|txid| {
            ui.monospace_selectable_singleline(false, txid.to_string());
        });
        match self.fut.ready() {
            Some(Ok(())) => Some(true),
            Some(Err(err)) => {
                ui.monospace_selectable_multiline(format!("{err:#}"));
                Some(false)
            }
            None => None,
        }
    }
}

enum CastInner {
    CastInput(CastInput),
    Casting(Casting),
}

impl CastInner {
    fn show(&mut self, app: Option<&App>, ui: &mut egui::Ui) {
        match self {
            Self::CastInput(cast_input) => {
                if let Some(amount) = cast_input.show(app, ui).inner {
                    let cast = wallet::Cast::new(amount);
                    let casting = Casting::new(app.unwrap(), cast);
                    *self = Self::Casting(casting);
                }
            }
            Self::Casting(casting) => {
                if let Some(true) = casting.show(ui) {
                    *self = Self::CastInput(CastInput::default())
                }
            }
        }
    }
}

impl Default for CastInner {
    fn default() -> Self {
        Self::CastInput(CastInput::default())
    }
}

#[derive(Default)]
#[repr(transparent)]
pub struct Cast(CastInner);

impl Cast {
    pub fn show(&mut self, app: Option<&App>, ui: &mut egui::Ui) {
        self.0.show(app, ui);
    }
}
