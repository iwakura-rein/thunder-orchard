use std::{collections::HashMap, sync::Arc, task::Poll};

use bitcoin::Amount;
use eframe::egui::{self, Button, InnerResponse};
use parking_lot::RwLock;
use poll_promise::Promise;
use thunder_orchard::{
    types::{OutPoint, Output, Txid},
    util::Watchable,
    wallet::{self, MeltBatch, Wallet},
};

use crate::{
    app::App,
    gui::util::{UiExt, show_btc_amount},
    util::PromiseStream,
};

use super::utxo_selector::UtxoSelector;

struct MeltSelectUtxosInner {
    rt_handle: tokio::runtime::Handle,
    wallet: Box<Wallet>,
    utxos: HashMap<OutPoint, Output>,
    wallet_updated: PromiseStream<<Wallet as Watchable<()>>::WatchStream>,
}

struct MeltSelectUtxos {
    inner: Option<Result<Box<MeltSelectUtxosInner>, String>>,
    utxo_selector: UtxoSelector,
    selected_utxos: HashMap<OutPoint, Output>,
}

impl MeltSelectUtxos {
    pub fn new(app: Option<&App>) -> Self {
        let mut this = Self {
            inner: None,
            utxo_selector: UtxoSelector::default(),
            selected_utxos: HashMap::default(),
        };
        let Some(app) = app else {
            return this;
        };
        let wallet_updated = {
            let rt_guard = app.runtime.enter();
            let wallet_updated = PromiseStream::from(app.wallet.watch());
            drop(rt_guard);
            wallet_updated
        };
        this.inner = Some('inner: {
            let wallet_rotxn = match app.wallet.env().read_txn() {
                Ok(wallet_rotxn) => wallet_rotxn,
                Err(err) => {
                    let err_msg = format!("{:#}", anyhow::Error::from(err));
                    tracing::error!("{err_msg}");
                    break 'inner Err(err_msg);
                }
            };
            match app.wallet.get_utxos(&wallet_rotxn) {
                Ok(utxos) => Ok(Box::new(MeltSelectUtxosInner {
                    rt_handle: app.runtime.handle().clone(),
                    utxos,
                    wallet: Box::new(app.wallet.clone()),
                    wallet_updated,
                })),
                Err(err) => {
                    let err_msg = format!("{:#}", anyhow::Error::from(err));
                    tracing::error!("{err_msg}");
                    Err(err_msg)
                }
            }
        });
        this
    }

    /// Updates values if the wallet has been updated
    fn update(&mut self) {
        let Some(Ok(inner)) = self.inner.as_mut() else {
            return;
        };
        let rt_guard = inner.rt_handle.enter();
        match inner.wallet_updated.poll_next() {
            Some(Poll::Ready(())) => (),
            Some(Poll::Pending) | None => return,
        }
        let wallet_env = inner.wallet.env().clone();
        let wallet_rotxn = match wallet_env.read_txn() {
            Ok(wallet_rotxn) => wallet_rotxn,
            Err(err) => {
                let err_msg = format!("{:#}", anyhow::Error::from(err));
                tracing::error!("{err_msg}");
                self.inner = Some(Err(err_msg));
                return;
            }
        };
        match inner.wallet.get_utxos(&wallet_rotxn) {
            Ok(utxos) => {
                inner.utxos = utxos;
            }
            Err(err) => {
                let err_msg = format!("{:#}", anyhow::Error::from(err));
                tracing::error!("{err_msg}");
                self.inner = Some(Err(err_msg));
                return;
            }
        }
        drop(rt_guard)
    }

    /// Returns `Some(())` if melt is clicked
    fn show(&mut self, ui: &mut egui::Ui) -> InnerResponse<Option<()>> {
        self.update();
        egui::ScrollArea::horizontal()
            .show(ui, |ui| {
                egui::SidePanel::left("spend_utxo")
                    .exact_width(ui.available_width() / 2.)
                    .resizable(false)
                    .show_inside(ui, |ui| {
                        let utxos = match &self.inner {
                            Some(Ok(inner)) => Some(&inner.utxos),
                            Some(Err(_)) | None => None,
                        };
                        self.utxo_selector.show(
                            utxos,
                            ui,
                            "Melt UTXOs",
                            true,
                            |selected, (outpoint, output)| {
                                if selected {
                                    self.selected_utxos
                                        .insert(outpoint, output);
                                } else {
                                    self.selected_utxos.remove(&outpoint);
                                }
                            },
                        );
                    });
                let resp =
                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        ui.monospace_selectable_singleline(
                            false,
                            format!(
                                "Fee per tx: {}",
                                show_btc_amount(wallet::STANDARD_FEE)
                            ),
                        );
                        ui.vertical_centered(|ui| {
                            if ui
                                .add_enabled(
                                    !self.selected_utxos.is_empty(),
                                    Button::new("Melt"),
                                )
                                .clicked()
                            {
                                Some(())
                            } else {
                                None
                            }
                        })
                    });
                InnerResponse {
                    inner: resp.inner.inner,
                    response: resp.response | resp.inner.response,
                }
            })
            .inner
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
                while let Some(tx_fn) = melt_batch.next_tx().await? {
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
    SelectUtxos(MeltSelectUtxos),
    Melting(Melting),
}

impl MeltInner {
    fn new(app: Option<&App>) -> Self {
        Self::SelectUtxos(MeltSelectUtxos::new(app))
    }

    fn show(&mut self, app: Option<&App>, ui: &mut egui::Ui) {
        match self {
            Self::SelectUtxos(select_utxos) => {
                if let Some(()) = select_utxos.show(ui).inner {
                    let selected_utxos =
                        select_utxos.selected_utxos.drain().collect();
                    let melt_batch = MeltBatch::new(selected_utxos);
                    let melting = Melting::new(app.unwrap(), melt_batch);
                    *self = Self::Melting(melting);
                }
            }
            Self::Melting(melting) => {
                if let Some(true) = melting.show(ui) {
                    *self = Self::SelectUtxos(MeltSelectUtxos::new(app))
                }
            }
        }
    }
}

#[repr(transparent)]
pub struct Melt(MeltInner);

impl Melt {
    pub fn new(app: Option<&App>) -> Self {
        Self(MeltInner::new(app))
    }

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
