use std::collections::{HashMap, HashSet};

use eframe::egui;
use thunder_orchard::types::{GetValue, OutPoint, Output};

pub fn show_utxo(ui: &mut egui::Ui, outpoint: &OutPoint, output: &Output) {
    let (kind, hash, vout) = match outpoint {
        OutPoint::Regular { txid, vout } => {
            ("regular", format!("{txid}"), *vout)
        }
        OutPoint::Deposit(outpoint) => {
            ("deposit", format!("{}", outpoint.txid), outpoint.vout)
        }
        OutPoint::Coinbase { merkle_root, vout } => {
            ("coinbase", format!("{merkle_root}"), *vout)
        }
    };
    let hash = &hash[0..8];
    let value = output.get_value();
    ui.monospace(kind.to_string());
    ui.monospace(format!("{hash}:{vout}",));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
        ui.monospace(format!("{value}"));
    });
}

#[derive(Debug, Default)]
pub struct UtxoSelector {
    pub selected: HashSet<OutPoint>,
}

impl UtxoSelector {
    /// `on_select` is called when a utxo is selected or deselected.
    /// The bool arg is `true` if the utxo was selected, and `false` if it was
    /// deselected.
    pub fn show<OnSelect>(
        &mut self,
        utxos: Option<&HashMap<OutPoint, Output>>,
        ui: &mut egui::Ui,
        heading: &str,
        show_selected: bool,
        mut on_select: OnSelect,
    ) where
        OnSelect: FnMut(bool, (OutPoint, Output)),
    {
        ui.heading(heading);
        let Some(utxos) = utxos else { return };
        let (total, utxos): (bitcoin::Amount, Vec<_>) = {
            let total: bitcoin::Amount = utxos
                .iter()
                .filter(|(outpoint, _)| !self.selected.contains(outpoint))
                .map(|(_, output)| output.get_value())
                .sum();
            let mut utxos: Vec<_> = utxos.iter().collect();
            utxos.sort_by_key(|(outpoint, _)| format!("{outpoint}"));
            (total, utxos)
        };
        ui.separator();
        ui.monospace(format!("Total: {total}"));
        ui.separator();
        egui::Grid::new("utxos").striped(true).show(ui, |ui| {
            ui.monospace("kind");
            ui.monospace("outpoint");
            ui.monospace("value");
            ui.end_row();
            for (outpoint, output) in utxos {
                if !show_selected && self.selected.contains(outpoint) {
                    continue;
                }
                //ui.horizontal(|ui| {});
                show_utxo(ui, outpoint, output);

                if show_selected {
                    let mut selected_checked =
                        self.selected.contains(&outpoint);
                    if ui
                        .checkbox(&mut selected_checked, "select UTXO")
                        .clicked()
                    {
                        if selected_checked {
                            self.selected.insert(*outpoint);
                            on_select(true, (*outpoint, output.clone()));
                        } else {
                            self.selected.remove(&outpoint);
                            on_select(false, (*outpoint, output.clone()));
                        }
                    };
                } else if ui
                    .add_enabled(
                        !self.selected.contains(&outpoint),
                        egui::Button::new("spend"),
                    )
                    .clicked()
                {
                    on_select(true, (*outpoint, output.clone()))
                }
                ui.end_row();
            }
        });
    }
}
