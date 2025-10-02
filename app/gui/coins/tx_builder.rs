use std::{
    collections::{HashMap, HashSet},
    task::Poll,
};

use eframe::egui;

use thunder_orchard::{
    types::{GetValue, OutPoint, Output, Transaction, orchard},
    util::Watchable,
    wallet::Wallet,
};

use super::{
    tx_creator::TxCreator,
    utxo_creator::UtxoCreator,
    utxo_selector::{UtxoSelector, show_utxo},
};
use crate::{app::App, gui::util::UiExt, util::PromiseStream};

struct TxBuilderInner {
    rt_handle: tokio::runtime::Handle,
    wallet: Wallet,
    utxos: HashMap<OutPoint, Output>,
    wallet_updated: PromiseStream<<Wallet as Watchable<()>>::WatchStream>,
}

pub struct TxBuilder {
    inner: Option<Result<TxBuilderInner, String>>,
    /// regular tx without extra data or special inputs/outputs
    base_tx: Transaction<
        orchard::InProgress<orchard::Unproven, orchard::Unauthorized>,
    >,
    tx_creator: TxCreator,
    utxo_creator: UtxoCreator,
    utxo_selector: UtxoSelector,
}

impl TxBuilder {
    pub fn new(app: Option<&App>) -> Self {
        let mut this = Self {
            inner: None,
            base_tx: Transaction::default(),
            tx_creator: TxCreator::default(),
            utxo_creator: UtxoCreator::default(),
            utxo_selector: UtxoSelector::default(),
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
                Ok(utxos) => Ok(TxBuilderInner {
                    rt_handle: app.runtime.handle().clone(),
                    utxos,
                    wallet: app.wallet.clone(),
                    wallet_updated,
                }),
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

    pub fn show_value_in(&mut self, ui: &mut egui::Ui) {
        ui.heading("Value In");
        let inner = match &self.inner {
            Some(Ok(inner)) => inner,
            Some(Err(err_msg)) => {
                ui.monospace_selectable_multiline(err_msg.as_str());
                return;
            }
            None => return,
        };
        let selected: HashSet<_> = self
            .base_tx
            .inputs
            .iter()
            .map(|(outpoint, _)| *outpoint)
            .collect();
        let mut spent_utxos: Vec<_> = inner
            .utxos
            .iter()
            .filter(|(outpoint, _)| selected.contains(outpoint))
            .collect();
        let value_in: bitcoin::Amount = spent_utxos
            .iter()
            .map(|(_, output)| output.get_value())
            .sum();
        self.tx_creator.value_in = value_in;
        spent_utxos.sort_by_key(|(outpoint, _)| format!("{outpoint}"));
        ui.separator();
        ui.monospace(format!("Total: {value_in}"));
        ui.separator();
        egui::Grid::new("utxos").striped(true).show(ui, |ui| {
            ui.monospace("kind");
            ui.monospace("outpoint");
            ui.monospace("value");
            ui.end_row();
            let mut remove = None;
            for (vout, (outpoint, _)) in self.base_tx.inputs.iter().enumerate()
            {
                let output = &inner.utxos[outpoint];
                show_utxo(ui, outpoint, output);
                if ui.button("remove").clicked() {
                    remove = Some(vout);
                }
                ui.end_row();
            }
            if let Some(vout) = remove {
                self.base_tx.inputs.remove(vout);
            }
        });
    }

    pub fn show_value_out(&mut self, ui: &mut egui::Ui) {
        ui.heading("Value Out");
        ui.separator();
        let value_out: bitcoin::Amount =
            self.base_tx.outputs.iter().map(GetValue::get_value).sum();
        self.tx_creator.value_out = value_out;
        ui.monospace(format!("Total: {value_out}"));
        ui.separator();
        egui::Grid::new("outputs").striped(true).show(ui, |ui| {
            let mut remove = None;
            ui.monospace("vout");
            ui.monospace("address");
            ui.monospace("value");
            ui.end_row();
            for (vout, output) in self.base_tx.outputs.iter().enumerate() {
                let address = &format!("{}", output.address)[0..8];
                let value = output.get_value();
                ui.monospace(format!("{vout}"));
                ui.monospace(address.to_string());
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Max),
                    |ui| {
                        ui.monospace(format!("{value}"));
                    },
                );
                if ui.button("remove").clicked() {
                    remove = Some(vout);
                }
                ui.end_row();
            }
            if let Some(vout) = remove {
                self.base_tx.outputs.remove(vout);
            }
        });
    }

    pub fn show(
        &mut self,
        app: Option<&App>,
        ui: &mut egui::Ui,
    ) -> anyhow::Result<()> {
        self.update();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            egui::SidePanel::left("spend_utxo")
                .exact_width(250.)
                .resizable(false)
                .show_inside(ui, |ui| {
                    let utxos = match &self.inner {
                        Some(Ok(inner)) => Some(&inner.utxos),
                        Some(Err(_)) | None => None,
                    };
                    self.utxo_selector.show(utxos, ui, &mut self.base_tx);
                });
            egui::SidePanel::left("value_in")
                .exact_width(250.)
                .resizable(false)
                .show_inside(ui, |ui| {
                    let () = self.show_value_in(ui);
                });
            egui::SidePanel::left("value_out")
                .exact_width(250.)
                .resizable(false)
                .show_inside(ui, |ui| {
                    let () = self.show_value_out(ui);
                });
            egui::SidePanel::left("create_utxo")
                .exact_width(450.)
                .resizable(false)
                .show_separator_line(false)
                .show_inside(ui, |ui| {
                    self.utxo_creator.show(app, ui, &mut self.base_tx);
                    ui.separator();
                    self.tx_creator.show(app, ui, &mut self.base_tx).unwrap();
                });
        });
        Ok(())
    }
}
