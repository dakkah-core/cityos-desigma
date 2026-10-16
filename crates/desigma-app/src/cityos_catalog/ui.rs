use super::{preview, Browser, DetailTab};
use crate::theme;
use desigma_cityos_catalog::{Entry, Filters};
use egui::{Align, Align2, Context, FontId, Frame, Layout, RichText, Sense, Stroke, StrokeKind, Ui};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const COLLECTIONS: [(&str, &str); 8] = [
    ("design", "Design library"),
    ("themes", "Themes"),
    ("components", "Components"),
    ("tokens", "Design tokens"),
    ("compositions", "Compositions"),
    ("products", "Products & surfaces"),
    ("sources", "All source records"),
    ("issues", "Needs attention"),
];

pub(super) fn count(value: usize) -> String {
    let digits = value.to_string();
    digits.chars().enumerate().fold(String::new(), |mut out, (i, ch)| {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
        out
    })
}
pub(super) fn chip(ui: &mut Ui, text: &str) {
    Frame::new().fill(theme::BG_TERTIARY).corner_radius(4).inner_margin(egui::Margin::symmetric(7, 4)).show(ui, |ui| {
        ui.label(RichText::new(text).size(theme::CATALOG_SMALL).color(theme::TEXT_SECONDARY));
    });
}
fn nav_row(ui: &mut Ui, selected: bool, text: &str, suffix: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::CATALOG_NAV_ROW), Sense::click());
    crate::widgets::record(ui, &format!("catalog-nav-{text}"), rect);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), selected, text));
    let painter = ui.painter().with_clip_rect(rect);
    if selected || response.hovered() {
        painter.rect_filled(rect, theme::CATALOG_RADIUS, if selected { theme::BG_SELECTED_SECONDARY } else { theme::BG_HOVER });
    }
    if selected {
        painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())), 2, theme::BORDER_SELECTED);
    }
    if response.has_focus() {
        painter.rect_stroke(rect.shrink(1.0), theme::CATALOG_RADIUS, Stroke::new(1.0, theme::BORDER_SELECTED), StrokeKind::Inside);
    }
    let right = if suffix.is_empty() { rect.right() - 8.0 } else { rect.right() - 48.0 };
    painter.with_clip_rect(egui::Rect::from_min_max(rect.min, egui::pos2(right, rect.bottom()))).text(
        egui::pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(theme::CATALOG_BODY),
        if selected { theme::TEXT } else { theme::TEXT_SECONDARY },
    );
    painter.text(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        Align2::RIGHT_CENTER,
        suffix,
        FontId::proportional(theme::CATALOG_SMALL),
        theme::TEXT_TERTIARY,
    );
    response.on_hover_text(text)
}
fn item_row(ui: &mut Ui, entry: &Entry, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::CATALOG_ROW), Sense::click());
    crate::widgets::record(ui, &format!("catalog-item-{}", entry.id), rect);
    let title = preview::display_name(entry);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), selected, &title));
    let painter = ui.painter().with_clip_rect(rect);
    if selected || response.hovered() {
        painter.rect_filled(rect, theme::CATALOG_RADIUS, if selected { theme::BG_SELECTED_SECONDARY } else { theme::BG_HOVER });
    }
    if response.has_focus() {
        painter.rect_stroke(rect.shrink(1.0), theme::CATALOG_RADIUS, Stroke::new(1.0, theme::BORDER_SELECTED), StrokeKind::Inside);
    }
    let tile = egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 16.0), egui::vec2(36.0, 36.0));
    painter.rect_filled(tile, 6, theme::BG_TERTIARY);
    painter.text(
        tile.center(),
        Align2::CENTER_CENTER,
        entry.level.as_deref().unwrap_or("·"),
        FontId::proportional(theme::CATALOG_SMALL),
        theme::TEXT_SECONDARY,
    );
    let text_rect = egui::Rect::from_min_max(rect.min + egui::vec2(57.0, 0.0), rect.max - egui::vec2(10.0, 0.0));
    let p = painter.with_clip_rect(text_rect);
    p.text(egui::pos2(text_rect.left(), rect.top() + 23.0), Align2::LEFT_CENTER, title, FontId::proportional(theme::CATALOG_BODY), theme::TEXT);
    let meta = format!("{} · {}", preview::kind_name(&entry.kind), entry.package.as_deref().map(|p| p.trim_start_matches("@cityos/")).unwrap_or(&entry.family));
    p.text(egui::pos2(text_rect.left(), rect.top() + 45.0), Align2::LEFT_CENTER, meta, FontId::proportional(theme::CATALOG_SMALL), theme::TEXT_SECONDARY);
    response.on_hover_text(format!("{}\n{}\n{}", entry.label, entry.source_path, entry.classification_status))
}

impl Browser {
    pub fn show(&mut self, ui: &mut Ui, ctx: &Context) {
        self.inline_preview=None;
        if ctx.input(|input| {
            input.pointer.any_pressed()
                || input.smooth_scroll_delta != egui::Vec2::ZERO
                || input.events.iter().any(|event| matches!(event, egui::Event::Key { .. } | egui::Event::Text(_)))
        }) {
            self.last_interaction = Instant::now();
        }
        self.tick();
        ctx.request_repaint_after(Duration::from_millis(if self.busy || self.filter_changed.is_some() || self.pending_selection { 100 } else { 1000 }));
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.spacing_mut().button_padding = egui::vec2(10.0, 7.0);
        for (style, size) in [
            (egui::TextStyle::Body, theme::CATALOG_BODY),
            (egui::TextStyle::Button, theme::CATALOG_BODY),
            (egui::TextStyle::Small, theme::CATALOG_SMALL),
            (egui::TextStyle::Monospace, theme::CATALOG_SMALL),
            (egui::TextStyle::Heading, theme::CATALOG_TITLE),
        ] {
            let font = FontId::new(size, if style == egui::TextStyle::Monospace { egui::FontFamily::Monospace } else { egui::FontFamily::Proportional });
            ui.style_mut().text_styles.insert(style, font);
        }
        let narrow = ui.available_width() < theme::CATALOG_NARROW;
        let stacked = ui.available_width() < theme::CATALOG_STACK;
        Frame::new().inner_margin(egui::Margin::symmetric(theme::CATALOG_PAD, 14)).show(ui, |ui| self.header(ui));
        ui.separator();
        Frame::new().inner_margin(egui::Margin::symmetric(theme::CATALOG_PAD, 8)).show(ui, |ui| self.search_bar(ui, ctx));
        if narrow {
            ui.collapsing("Browse collections, levels & products", |ui| {
                egui::ScrollArea::vertical().id_salt("catalog-compact-nav").max_height(240.0).show(ui, |ui| self.navigation(ui));
            });
        }
        egui::Panel::bottom("catalog-status")
            .frame(Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(theme::CATALOG_PAD, 8)))
            .show(ui, |ui| self.status(ui));
        if !narrow {
            egui::Panel::left("catalog-navigation")
                .resizable(false)
                .exact_size(theme::CATALOG_NAV_WIDTH)
                .frame(Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(12, 12)))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("catalog-nav-scroll").show(ui, |ui| self.navigation(ui));
                });
        }
        if stacked {
            egui::CentralPanel::default().frame(Frame::new().inner_margin(12)).show(ui, |ui| {
                if self.selected.is_some() {
                    if ui.button("← Results").clicked() {
                        self.selected = None;
                        self.pending_selection = false;
                    }
                    self.detail(ui);
                } else {
                    self.results(ui);
                }
            });
        } else {
            egui::Panel::left("catalog-results-panel")
                .default_size(theme::CATALOG_LIST_WIDTH)
                .size_range(270.0..=480.0)
                .resizable(true)
                .frame(Frame::new().fill(theme::BG).inner_margin(12))
                .show(ui, |ui| self.results(ui));
            egui::CentralPanel::default().frame(Frame::new().fill(theme::BG_SECONDARY).inner_margin(theme::CATALOG_PAD)).show(ui, |ui| self.detail(ui));
        }
        self.settings(ctx);
        self.coverage(ctx);
        if self.settings_open||self.coverage_open {self.inline_preview=None;}
    }
    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let back = ui.button("← Files");
            crate::widgets::record(ui, "catalog-back", back.rect);
            if back.clicked() {
                self.open = false;
            }
            ui.add_space(theme::CATALOG_GAP);
            ui.label(RichText::new("CityOS").size(theme::CATALOG_TITLE).strong());
            ui.label(RichText::new("Design library").size(theme::CATALOG_TITLE).color(theme::TEXT_SECONDARY));
            if ui.button("Design files…").clicked() {
                self.workspace_request = Some(super::WorkspaceRequest::browse());
            }
            if self.has_bound_draft && ui.button("Resume canvas draft").clicked() {
                self.workspace_request = Some(super::WorkspaceRequest { source_path: None, preview_only: false, resume: true });
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                }
                ui.checkbox(&mut self.auto_refresh, "Live updates")
                    .on_hover_text("Automatic incremental refresh. Low-impact scheduling yields to selected designs; unchanged snapshots are reused.");
                if self.busy {
                    if ui.button("Cancel").clicked() {
                        self.cancelled.store(true, Ordering::Release);
                    }
                    ui.spinner();
                } else if ui.button("Refresh").clicked() {
                    self.refresh();
                }
                let old = self.mode.clone();
                ui.add_enabled_ui(!self.busy, |ui| {
                    egui::ComboBox::from_id_salt("catalog-mode")
                        .width(155.0)
                        .selected_text(if self.mode == "local" { "Local preview" } else { "GitLab main" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.mode, "committed".into(), "GitLab main");
                            ui.selectable_value(&mut self.mode, "local".into(), "Local preview");
                        });
                });
                if old != self.mode {
                    self.last_interaction = Instant::now();
                    self.refresh();
                }
            });
        });
    }
    fn search_bar(&mut self, ui: &mut Ui, ctx: &Context) {
        let before = self.filters.clone();
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 190.0).max(140.0);
            let search = ui.add_sized(
                [width, 34.0],
                egui::TextEdit::singleline(&mut self.filters.search).id_salt("catalog-search").hint_text("Search names, tokens, shells, or source paths…"),
            );
            crate::widgets::record(ui, "catalog-search", search.rect);
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
                search.request_focus();
            }
            if search.changed() {
                self.last_interaction = Instant::now();
            }
            if ui.button("Clear").on_hover_text("Clear search and filters in this collection").clicked() {
                self.filters = Filters { collection: self.filters.collection.clone(), ..Default::default() };
            }
            ui.toggle_value(&mut self.filters_open, "Filters");
        });
        if self.filters_open {
            ui.horizontal_wrapped(|ui| {
                for (label, value) in [
                    ("Owner", &mut self.filters.owner),
                    ("Package", &mut self.filters.package),
                    ("Target", &mut self.filters.target),
                    ("Locale", &mut self.filters.locale),
                ] {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(value).desired_width(120.0));
                }
                egui::ComboBox::from_id_salt("catalog-status-filter")
                    .selected_text(if self.filters.status.is_empty() { "Any classification" } else { &self.filters.status })
                    .show_ui(ui, |ui| {
                        for (value, label) in
                            [("", "Any classification"), ("classified", "Classified"), ("unresolved", "Unclassified"), ("conflicting", "Conflicting")]
                        {
                            ui.selectable_value(&mut self.filters.status, value.into(), label);
                        }
                    });
            });
        }
        if self.filters != before {
            self.filters_changed();
        }
    }
    fn navigation(&mut self, ui: &mut Ui) {
        let before = self.filters.clone();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.products_open, false, "Design system");
            ui.selectable_value(&mut self.products_open, true, "Products");
        });
        ui.add_space(theme::CATALOG_GAP);
        if self.products_open {
            self.product_navigation(ui);
        } else {
            for (id, label) in COLLECTIONS {
                if id == "sources" {
                    ui.add_space(theme::CATALOG_GAP);
                    ui.separator();
                }
                let n = self
                    .summary
                    .as_ref()
                    .and_then(|s| s["navigation"]["collections"].as_array())
                    .and_then(|rows| rows.iter().find(|row| row["id"] == id))
                    .and_then(|row| row["count"].as_u64());
                let suffix = n.filter(|n| *n < 10000).map(|n| count(n as usize)).unwrap_or_default();
                if nav_row(ui, self.filters.collection == id && self.filters.level.is_empty() && self.filters.product.is_empty(), label, &suffix).clicked() {
                    self.filters = Filters { collection: id.into(), ..Default::default() };
                }
                if id == "compositions" {
                    ui.add_space(theme::CATALOG_GAP);
                    ui.label(RichText::new("L0–L9 · generated from repository").small().color(theme::TEXT_SECONDARY));
                    let levels = self.summary.as_ref().and_then(|s| s["navigation"]["levels"].as_array()).cloned().unwrap_or_default();
                    for level in levels {
                        let id = level["id"].as_str().unwrap_or("");
                        let name = level["name"].as_str().unwrap_or("");
                        let total = level["count"].as_u64().unwrap_or(0) as usize;
                        if nav_row(ui, self.filters.level == id, &format!("{id}  {name}"), &count(total)).clicked() {
                            self.filters = Filters { collection: "design".into(), level: id.into(), ..Default::default() };
                        }
                    }
                    ui.add_space(theme::CATALOG_GAP);
                }
            }
        }
        if before != self.filters {
            self.filters_changed();
        }
    }
    fn product_navigation(&mut self, ui: &mut Ui) {
        let products = self.summary.as_ref().and_then(|s| s["navigation"]["products"].as_array()).cloned().unwrap_or_default();
        if products.is_empty() {
            ui.label("Products appear after the repository loads.");
        }
        for product in products {
            let id = product["id"].as_str().unwrap_or("");
            let name = product["name"].as_str().unwrap_or(id);
            if nav_row(ui, self.filters.product == id, name, "").clicked() {
                self.filters = Filters { collection: "sources".into(), product: id.into(), ..Default::default() };
            }
            if self.filters.product == id {
                ui.indent(id, |ui| {
                    if let Some(targets) = product["targets"].as_array() {
                        for target in targets {
                            let target = target.as_str().unwrap_or("");
                            if ui.selectable_label(self.filters.target == target, target).clicked() {
                                self.filters.target = target.into();
                                self.filters.search.clear();
                            }
                        }
                    }
                    if let Some(shells) = product["shells"].as_array() {
                        for shell in shells {
                            ui.collapsing(format!("Shell · {}", shell["id"].as_str().unwrap_or("Unassigned")), |ui| {
                                if let Some(routes) = shell["routes"].as_array() {
                                    for route in routes {
                                        let path = route["path"].as_str().unwrap_or("");
                                        let label = path.split("/app/").last().unwrap_or(path);
                                        if ui.button(label).on_hover_text(path).clicked() {
                                            self.filters.search = path.into();
                                            self.filters.target.clear();
                                        }
                                    }
                                }
                            });
                        }
                    }
                });
            }
        }
    }
    fn results(&mut self, ui: &mut Ui) {
        let title = if !self.filters.level.is_empty() {
            format!("{} · Design items", self.filters.level)
        } else if !self.filters.product.is_empty() {
            preview::humanize(&self.filters.product)
        } else {
            COLLECTIONS.iter().find(|(id, _)| *id == self.filters.collection).map(|(_, label)| label.to_string()).unwrap_or_else(|| "Design library".into())
        };
        ui.strong(title);
        let waiting = self.displayed_filters != self.filters;
        ui.label(RichText::new(if waiting { "Updating results…".into() } else { format!("{} items", count(self.total)) }).small().color(theme::TEXT_SECONDARY));
        ui.add_space(8.0);
        if self.summary.is_none() {
            ui.add_space(24.0);
            ui.spinner();
            ui.label("Reading your repository…");
            ui.label(RichText::new("The library will appear here when the first capture is ready.").color(theme::TEXT_SECONDARY));
            return;
        }
        if waiting {
            ui.spinner();
            return;
        }
        if self.entries.is_empty() {
            ui.add_space(24.0);
            ui.strong("No matching items");
            ui.label("Try a shorter name or clear the filters.");
            if ui.button("Reset filters").clicked() {
                self.filters = Filters { collection: self.filters.collection.clone(), ..Default::default() };
                self.filters_changed();
            }
            return;
        }
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt(("catalog-results-scroll", serde_json::to_string(&self.displayed_filters).unwrap_or_default()))
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 48.0).max(80.0))
            .show_rows(ui, theme::CATALOG_ROW, self.entries.len(), |ui, range| {
                for index in range {
                    let entry = &self.entries[index];
                    if item_row(ui, entry, self.selected.as_ref().is_some_and(|s| s.id == entry.id)).clicked() {
                        chosen = Some(entry.clone());
                    }
                }
            });
        if let Some(entry) = chosen {
            self.select(entry);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} of {}", count(self.entries.len()), count(self.total))).small().color(theme::TEXT_SECONDARY));
            if let Some(offset) = self.next {
                if ui.add_enabled(!self.busy, egui::Button::new("Load more")).clicked() {
                    self.query(offset);
                }
            }
        });
    }
    fn detail(&mut self, ui: &mut Ui) {
        let Some(entry) = self.selected.clone() else {
            egui::ScrollArea::vertical().id_salt("catalog-welcome").show(ui, |ui| self.welcome(ui));
            return;
        };
        ui.horizontal_wrapped(|ui| {
            chip(ui, preview::kind_name(&entry.kind));
            if let Some(level) = &entry.level {
                chip(ui, level);
            }
            chip(ui, if self.summary.as_ref().is_some_and(|s| s["snapshot"]["mode"] == "local") { "Local preview" } else { "Committed source" });
        });
        ui.add_space(theme::CATALOG_GAP);
        ui.label(RichText::new(preview::display_name(&entry)).size(theme::CATALOG_TITLE).strong());
        ui.label(RichText::new(&entry.label).small().color(theme::TEXT_SECONDARY));
        let native_tokens = matches!(entry.kind.as_str(), "theme" | "design-token" | "token");
        if native_tokens {
            let committed = self.summary.as_ref().is_some_and(|s| s["snapshot"]["mode"] == "committed");
            if ui
                .add_enabled(
                    committed && self.specimen.is_some(),
                    egui::Button::new(if entry.kind == "theme" { "Create editable theme library" } else { "Create editable token" }),
                )
                .clicked()
            {
                self.local_design_error = None;
                let snapshot = &self.summary.as_ref().unwrap()["snapshot"];
                match super::local_library::create(&entry, self.specimen.as_ref().unwrap(), snapshot, &self.theme_mode) {
                    Ok(document) => self.local_design_request = Some(document),
                    Err(error) => self.local_design_error = Some(error),
                }
            }
            if let Some(error) = &self.local_design_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ui.button("Return to current design").clicked() {
                    self.open = false;
                }
            }
            ui.small(if committed {
                "Creates a local Desigma design with editable variables. Save it normally; source submission is not enabled."
            } else {
                "Switch to GitLab main to create a local design copy. Local changes remain previews."
            });
        }
        if entry.kind == "theme" {
            let selectable = self.specimen.as_ref().is_some_and(|s| s["data"]["metadata"]["selectable"] != false);
            if ui
                .add_enabled(
                    selectable,
                    egui::Button::new(if self.mode == "local" { "Preview canonical theme on web frame" } else { "Preview theme on web frame" }),
                )
                .clicked()
            {
                self.workspace_theme = Some(entry.semantic_id.clone());
                self.workspace_request = Some(super::WorkspaceRequest::selected("config/experience/living-canvas-frame/cityos-web.input.json", true));
            }
        }
        if self.specimen.is_some() {
            ui.horizontal_wrapped(|ui| {
                if ui.button(if self.specimen_canvas.is_some() { "Reload inspection canvas" } else { "Load inspection canvas" }).clicked() {
                    self.specimen_canvas = Some(super::canvas::SpecimenCanvas::new(&entry, self.specimen.as_ref().unwrap(), &self.theme_mode));
                    self.detail_tab = DetailTab::Preview;
                }
                if self.specimen_canvas.is_some() && ui.button("Back to specimen").clicked() {
                    self.specimen_canvas = None;
                }
                ui.small("Temporary · does not create a saved design");
            });
            if let Some(modes) = self.specimen.as_ref().and_then(|s| s["presentation"]["modes"].as_object()) {
                let old = self.theme_mode.clone();
                ui.horizontal_wrapped(|ui| {
                    for key in modes.keys() {
                        ui.selectable_value(&mut self.theme_mode, key.clone(), preview::humanize(key));
                    }
                });
                if old != self.theme_mode && self.specimen_canvas.is_some() {
                    self.specimen_canvas = Some(super::canvas::SpecimenCanvas::new(&entry, self.specimen.as_ref().unwrap(), &self.theme_mode));
                }
            }
        }
        let composition = self.specimen.as_ref().is_some_and(|value| value["data"]["composition"]["schemaVersion"] == "cityos.experience-composition.v1");
        let profile=self.specimen.as_ref().map(|s|s["workspaceProfile"].clone()).filter(|p|p["kind"]=="component-specimen");
        if let Some(profile)=profile {
            if let Some(source)=profile["sourcePath"].as_str(){ui.horizontal_wrapped(|ui|{
                if ui.button("Preview actual component").clicked(){self.workspace_request=Some(super::WorkspaceRequest::selected(source,true));}
                if ui.button(if self.mode=="local" {"Open canonical draft"}else{"Open on canvas"}).clicked(){self.workspace_request=Some(super::WorkspaceRequest::selected(source,false));}
            });ui.small("Canonical component fixture · typed local edits · source submission mapping unavailable");}
        }
        if composition || entry.source_path.ends_with(".input.json") {
            ui.horizontal_wrapped(|ui| {
                let actual = entry.source_path == "config/experience/living-canvas-frame/cityos-web.input.json";
                if ui
                    .button(if actual {
                        "Open actual web preview"
                    } else if self.mode == "local" {
                        "Open canonical structure"
                    } else {
                        "Open structure workspace"
                    })
                    .clicked()
                {
                    self.workspace_request = Some(super::WorkspaceRequest::selected(&entry.source_path, true));
                }
                if entry.source_path == "config/experience/living-canvas-frame/cityos-web.input.json"
                    && ui.button(if self.mode == "local" { "Open canonical draft" } else { "Open on canvas" }).clicked()
                {
                    self.workspace_request = Some(super::WorkspaceRequest::selected(&entry.source_path, false));
                }
            });
            ui.small("Opens this source from canonical GitLab. Local overlay changes remain previews. Renderer availability is shown per file.");
        }
        ui.add_space(theme::CATALOG_GAP);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.detail_tab, DetailTab::Preview, "Specimen");
            ui.selectable_value(&mut self.detail_tab, DetailTab::Inspector, "Source & support");
            ui.selectable_value(&mut self.detail_tab, DetailTab::Source, "Raw data");
        });
        ui.separator();
        if self.detail_tab == DetailTab::Preview {
            if let Some(canvas) = &mut self.specimen_canvas {
                canvas.show(ui);
                return;
            }
            if self.mode=="committed" {if let Some(profile)=self.specimen.as_ref().map(|s|s["workspaceProfile"].clone()).filter(|p|p["sourceDigest"].is_string()) {
                let commit=self.summary.as_ref().map(|s|s["snapshot"]["canonicalCommit"].clone()).unwrap_or(Value::Null);
                ui.label("Actual renderer · pinned captured source · temporary preview");
                if !self.inline_status.is_empty(){ui.small(&self.inline_status);}
                let (_,rect)=ui.allocate_space(egui::vec2(ui.available_width().max(1.),ui.available_height().max(1.)));
                self.inline_preview=Some((serde_json::json!({"sourcePath":profile["sourcePath"],"sourceDigest":profile["sourceDigest"],"commit":commit}),rect));
                return;
            }}
        }
        egui::ScrollArea::vertical().id_salt(("catalog-detail", &entry.id, self.detail_tab as u8)).auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(theme::CATALOG_GAP);
            match self.detail_tab {
                DetailTab::Inspector => preview::inspector(ui, &entry),
                DetailTab::Source => {
                    if self.source_text.is_empty() {
                        ui.label("Captured source is loading.");
                    } else {
                        ui.horizontal(|ui| {
                            ui.label("Captured data · read-only");
                            if ui.button("Copy").clicked() {
                                ui.ctx().copy_text(self.source_text.clone());
                            }
                        });
                        ui.add(egui::TextEdit::multiline(&mut self.source_text.as_str()).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                    }
                }
                DetailTab::Preview => {
                    if let Some(error) = &self.preview_error {
                        ui.label("Preview could not load.");
                        ui.small(error);
                        if ui.button("Retry preview").clicked() {
                            self.pending_selection = true;
                        }
                    } else if let Some(specimen) = &self.specimen {
                        if !preview::theme_sheet(ui, &specimen["presentation"], &mut self.theme_mode) {
                            preview::show(ui, &entry, &specimen["data"]);
                        }
                    } else {
                        ui.spinner();
                        ui.label(if self.activity == "Refreshing repository" {
                            "Preview queued while the repository refreshes…"
                        } else {
                            "Loading captured data…"
                        });
                    }
                }
            }
        });
    }
    fn welcome(&mut self, ui: &mut Ui) {
        ui.add_space(24.0);
        ui.label(RichText::new("Your generated design system").size(30.0).strong());
        ui.add_space(theme::CATALOG_GAP);
        ui.label(
            RichText::new("Loaded from repository registries, exports and captured theme files. Select a level, inspect a specimen, or load its native inspection canvas.")
                .size(theme::CATALOG_BODY)
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(24.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Browse themes →").clicked() {
                self.filters = Filters { collection: "themes".into(), ..Default::default() };
                self.filters_changed();
            }
            if ui.button("Choose a design file →").clicked() {
                self.workspace_request = Some(super::WorkspaceRequest::browse());
            }
        });
        ui.add_space(16.);
        let levels = self.summary.as_ref().and_then(|s| s["navigation"]["levels"].as_array()).cloned().unwrap_or_default();
        if levels.is_empty() {
            ui.label("The L0–L9 overview appears when the repository capture is ready.");
        }
        egui::Grid::new("catalog-level-overview").num_columns(3).spacing(egui::vec2(16., 10.)).striped(true).show(ui, |ui| {
            ui.strong("Level");
            ui.strong("Records");
            ui.strong("Loading support");
            ui.end_row();
            for level in levels {
                let id = level["id"].as_str().unwrap_or("");
                let name = level["name"].as_str().unwrap_or("");
                let total = level["count"].as_u64().unwrap_or(0);
                if ui.button(format!("{id} · {name}")).clicked() {
                    self.filters = Filters { collection: "design".into(), level: id.into(), ..Default::default() };
                    self.filters_changed();
                }
                ui.label(count(total as usize));
                ui.small(if total == 0 { "No classified records" } else { "Specimen + inspection canvas" });
                ui.end_row();
            }
        });
        ui.add_space(theme::CATALOG_GAP);
        ui.label(
            RichText::new("Inspection canvases show source data as native objects. Actual web rendering and editable drafts are currently qualified for the canonical web frame. Other renderer and editing adapters remain unavailable.")
                .small()
                .color(theme::TEXT_SECONDARY),
        );
    }
    fn status(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.busy {
                ui.spinner();
                ui.label(crate::cityos_jobs::resource_status().unwrap_or(self.activity));
            } else if self.error.is_some() {
                ui.label("Refresh unavailable · previous catalog retained").on_hover_text(self.error.as_deref().unwrap_or(""));
            } else if self.summary.as_ref().is_some_and(|s| s["stale"] == true) {
                ui.label("Offline · browsing saved snapshot").on_hover_text(self.summary.as_ref().unwrap()["refreshError"].as_str().unwrap_or(""));
            } else if self.summary.as_ref().is_some_and(|s| s["restoredCache"] == true) {
                ui.label("Saved snapshot · automatic updates enabled");
            } else {
                ui.label("Repository connected");
            }
            if let Some(summary) = &self.summary {
                let sha = summary["snapshot"]["canonicalCommit"].as_str().unwrap_or("");
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        if summary["snapshot"]["mode"] == "local" {
                            "Local preview"
                        } else if summary["snapshot"]["resolution"] == "explicit-pinned" {
                            "Pinned commit"
                        } else {
                            "GitLab main"
                        },
                        sha.get(..10).unwrap_or(sha)
                    ))
                    .small()
                    .color(theme::TEXT_SECONDARY),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Source accounting").clicked() {
                    self.coverage_open = true;
                }
                ui.label(RichText::new("Read-only").small().color(theme::TEXT_SECONDARY));
            });
        });
    }
    fn settings(&mut self, ctx: &Context) {
        let mut open = self.settings_open;
        egui::Window::new("Catalog connection").id(egui::Id::new("catalog-settings")).open(&mut open).default_width(540.0).resizable(true).show(ctx, |ui| {
            ui.label("CityOS repository");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.repository).desired_width(400.0));
                if ui.button("Browse…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.repository = path.to_string_lossy().into_owned();
                    }
                }
            });
            ui.collapsing("Reader settings", |ui| {
                ui.label("Packaged reader");
                ui.text_edit_singleline(&mut self.reader);
                if ui.button("Choose reader…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("CityOS reader", &["mjs"]).pick_file() {
                        self.reader = path.to_string_lossy().into_owned();
                    }
                }
                ui.label("Node executable");
                ui.text_edit_singleline(&mut self.node);
            });
            ui.add_space(theme::CATALOG_GAP);
            if ui.add_enabled(!self.busy && !self.repository.trim().is_empty(), egui::Button::new("Connect & refresh")).clicked() {
                self.refresh();
                self.settings_open = false;
            }
        });
        self.settings_open &= open;
    }
    fn coverage(&mut self, ctx: &Context) {
        egui::Window::new("Source accounting").open(&mut self.coverage_open).default_width(680.0).vscroll(true).show(ctx,|ui|{
            let Some(summary)=&self.summary else {ui.label("Connect a repository to inspect its census.");return;};
            ui.label(format!("{} source occurrences",count(summary["entryCount"].as_u64().unwrap_or(0) as usize)));
            ui.label("The design library is a browsing selection. The census includes every captured source occurrence, including unresolved and conflicting records.");
            if let Some(rows)=summary["census"].as_array(){egui::Grid::new("catalog-census").striped(true).show(ui,|ui|{
                for label in ["Source family","Expected","Accounted","Health"]{ui.strong(label);}ui.end_row();
                for row in rows {ui.label(row["family"].as_str().unwrap_or(""));ui.label(row["expected"].to_string());ui.label(row["accounted"].to_string());ui.label(if row["healthy"]==true{"Available"}else{"Incomplete"});ui.end_row();}
            });}
            ui.separator();ui.label("Native discovery is unavailable until its owner is qualified.");
            ui.collapsing("Snapshot & diagnostics",|ui|{ui.monospace(serde_json::to_string_pretty(&serde_json::json!({"snapshot":summary["snapshot"],"diagnostics":summary["diagnostics"]})).unwrap_or_default());});
        });
    }
}
