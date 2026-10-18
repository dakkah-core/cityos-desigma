//! On-demand, read-only installed support. This view never grants edit authority.
use serde_json::Value;

#[derive(Default)]
pub struct Coverage {
    pub open: bool,
    pub report: Option<Value>,
    pub error: Option<String>,
}

impl Coverage {
    pub fn receive(&mut self, result: Result<Value, String>) {
        match result.and_then(validate) {
            Ok(report) => { self.report = Some(report); self.error = None; }
            Err(error) => self.error = Some(error), // Keep the previous observation, visibly stale.
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> bool {
        let mut refresh = false;
        egui::Window::new("CityOS coverage · L0–L9").open(&mut self.open)
            .default_size([940., 540.]).show(ctx, |ui| {
                // Floating windows do not inherit the workspace panel's local
                // typography. Keep support readable at the same native scale.
                for (style, size) in [(egui::TextStyle::Body, 15.), (egui::TextStyle::Button, 15.),
                    (egui::TextStyle::Small, 13.), (egui::TextStyle::Monospace, 13.)] {
                    ui.style_mut().text_styles.insert(style, egui::FontId::proportional(size));
                }
                ui.spacing_mut().button_padding = egui::vec2(10., 6.);
                ui.spacing_mut().item_spacing.y = 8.;
                ui.label("Installed preview, canvas and editing support");
                ui.small("Scoped registrations and canonical resources. Whole-estate census and qualification remain separate.");
                if ui.add_enabled(!busy, egui::Button::new("Refresh coverage")).clicked() { refresh = true; }
                if busy { ui.label("Reading installed support…"); }
                if let Some(error) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                    if self.report.is_some() { ui.small("Previous observation retained · refresh failed"); }
                }
                let Some(report) = &self.report else {
                    ui.label("No coverage observation loaded."); return;
                };
                if let Some(commit) = report["observation"]["commit"].as_str() {
                    ui.monospace(format!("Canonical observation {commit}"));
                }
                ui.small("Preview: ready / needs selection. Canvas: protected inspection. Available edits still require their own qualification.");
                egui::ScrollArea::both().id_salt("cityos-installed-levels").show(ui, |ui| {
                    egui::Grid::new("cityos-installed-coverage").striped(true).spacing([18., 8.]).show(ui, |ui| {
                        for heading in ["Level", "Definitions", "Canonical", "Specimens", "Preview", "Canvas", "Editable", "Field ops"] {
                            ui.strong(heading);
                        }
                        ui.end_row();
                        for row in report["levels"].as_array().into_iter().flatten() {
                            ui.label(row["level"].as_str().unwrap_or("Unknown"));
                            ui.label(if row["status"] == "unscanned" { "Unscanned".into() } else { number(&row["registeredDefinitions"]) });
                            ui.label(number(&row["canonicalResources"]["records"]));
                            ui.label(number(&row["temporarySpecimens"]["records"]));
                            ui.label(preview_text(row));
                            ui.label(number(&row["canonicalResources"]["protectedCanvas"]["available"]));
                            ui.label(number(&row["canonicalResources"]["edit"]["available"]));
                            ui.label(number(&row["editOperations"]["available"]));
                            ui.end_row();
                        }
                    });
                    ui.separator();
                    ui.label(format!("Unclassified: {} registrations · {} canonical resources · {} specimens · {} field operations",
                        number(&report["unknown"]["registrations"]), number(&report["unknown"]["canonicalResources"]),
                        number(&report["unknown"]["temporarySpecimens"]), number(&report["unknown"]["editOperations"])));
                    ui.label(format!("Source findings: {} · Failed reads: {} · Unadapted inputs: {}",
                        number(&report["unknown"]["diagnostics"]), number(&report["unknown"]["failedSourceReads"]),
                        number(&report["unknown"]["unadaptedInputs"])));
                    ui.label("Whole-estate totals: unknown · Installed capabilities: unqualified · Owner writes: unavailable");
                    ui.collapsing("Scope and limitations", |ui| {
                        for limitation in report["limitations"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                            ui.label(limitation);
                        }
                        ui.small(format!("Package identity: {}", report["packageIdentity"].as_str().unwrap_or("unknown")));
                    });
                });
            });
        refresh
    }
}

fn number(value: &Value) -> String {
    value.as_u64().map(|value| value.to_string()).unwrap_or_else(|| "Unknown".into())
}

fn preview_text(row: &Value) -> String {
    let sum = |field: &str| {
        let canonical = row["canonicalResources"]["preview"][field].as_u64()?;
        let specimens = row["temporarySpecimens"]["preview"][field].as_u64()?;
        canonical.checked_add(specimens)
    };
    match (sum("available"), sum("selectable")) {
        (Some(ready), Some(selectable)) => format!("{ready} / {selectable}"),
        _ => "Unknown".into(),
    }
}

fn validate(value: Value) -> Result<Value, String> {
    let report = &value["installedCoverage"];
    let valid = report["schemaVersion"] == "cityos.design-workspace.installed-coverage.v1"
        && report["packageIdentity"].as_str().is_some_and(|value| !value.is_empty())
        && report["ownerWrites"] == false && report["qualification"] == "unqualified"
        && report["levels"].as_array().is_some_and(|rows| rows.len() == 10
            && rows.iter().enumerate().all(|(index, row)| row["level"] == format!("L{index}")));
    if !valid { return Err("Installed coverage is unavailable or incompatible. Install a matching workspace package and retry.".into()); }
    Ok(report.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn response() -> Value {
        json!({"installedCoverage":{"schemaVersion":"cityos.design-workspace.installed-coverage.v1",
            "packageIdentity":"package-a","ownerWrites":false,"qualification":"unqualified",
            "levels":(0..10).map(|level|json!({"level":format!("L{level}"),"status":"unscanned"})).collect::<Vec<_>>()}})
    }

    #[test]
    fn unknown_is_not_zero_and_incompatible_refresh_keeps_previous_observation() {
        let mut coverage = Coverage::default();
        coverage.receive(Ok(response()));
        let previous = coverage.report.clone();
        assert!(coverage.error.is_none());
        assert_eq!(preview_text(&coverage.report.as_ref().unwrap()["levels"][0]), "Unknown");
        assert_eq!(number(&Value::Null), "Unknown");
        for value in [json!({}), {
            let mut value = response(); value["installedCoverage"]["levels"][9]["level"] = json!("L8"); value
        }, {
            let mut value = response(); value["installedCoverage"]["ownerWrites"] = json!(true); value
        }] {
            coverage.receive(Ok(value));
            assert!(coverage.error.is_some());
            assert_eq!(coverage.report, previous);
        }
        coverage.receive(Err("Helper unavailable".into()));
        assert_eq!(coverage.report, previous);
        assert_eq!(coverage.error.as_deref(), Some("Helper unavailable"));
    }
}
