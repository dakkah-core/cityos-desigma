use super::{preview, support, Browser, DetailTab};
use crate::theme;
use desigma_cityos_catalog::{Entry, Filters};
use egui::{Align, Align2, Context, FontId, Frame, Layout, RichText, Sense, Stroke, StrokeKind, Ui};
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::time::Instant;

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

const TILE_HEIGHT: f32 = 148.0;
fn tile_columns(width: f32) -> usize { ((width + 8.0) / 168.0).floor().clamp(1.0, 4.0) as usize }
fn item_tile(ui: &mut Ui, entry: &Entry, selected: bool, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width.max(1.0), TILE_HEIGHT), Sense::click());
    crate::widgets::record(ui, &format!("catalog-item-{}", entry.id), rect);
    let title = preview::display_name(entry);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), selected, &title));
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect.shrink(1.0), theme::CATALOG_RADIUS, if selected { theme::BG_SELECTED_SECONDARY } else if response.hovered() { theme::BG_HOVER } else { theme::BG_TERTIARY });
    if selected || response.has_focus() {
        painter.rect_stroke(rect.shrink(1.0), theme::CATALOG_RADIUS, Stroke::new(1.0, theme::BORDER_SELECTED), StrokeKind::Inside);
    }
    let content = rect.shrink(12.0);
    let painter = painter.with_clip_rect(content);
    painter.text(content.min, Align2::LEFT_TOP, entry.level.as_deref().unwrap_or_else(|| preview::kind_name(&entry.kind)), FontId::proportional(theme::CATALOG_TITLE), theme::TEXT_SECONDARY);
    painter.text(content.min + egui::vec2(0.0, 48.0), Align2::LEFT_TOP, title, FontId::proportional(theme::CATALOG_BODY), theme::TEXT);
    painter.text(content.min + egui::vec2(0.0, 73.0), Align2::LEFT_TOP, preview::kind_name(&entry.kind), FontId::proportional(theme::CATALOG_SMALL), theme::TEXT_SECONDARY);
    painter.text(content.min + egui::vec2(0.0, 100.0), Align2::LEFT_TOP, "Select to preview", FontId::proportional(theme::CATALOG_SMALL), theme::TEXT_TERTIARY);
    response.on_hover_text(format!("{}\n{}\n{}", entry.label, entry.source_path, entry.classification_status))
}

fn action_button(ui: &mut Ui, id: &str, label: &str, reason: Option<&str>) -> bool {
    let response = ui.add_enabled(reason.is_none(), egui::Button::new(label));
    crate::widgets::record(ui, id, response.rect);
    let clicked = response.clicked();
    if let Some(reason) = reason { response.on_disabled_hover_text(reason); }
    clicked
}

impl Browser {
    pub fn show(&mut self, ui: &mut Ui, ctx: &Context) {
        if let Ok(mut repaint) = self.repaint.lock() { *repaint = Some(ctx.clone()); }
        self.inline_preview=None;
        if ctx.input(|input| {
            input.pointer.any_pressed()
                || input.smooth_scroll_delta != egui::Vec2::ZERO
                || input.events.iter().any(|event| matches!(event, egui::Event::Key { .. } | egui::Event::Text(_)))
        }) {
            self.last_interaction = Instant::now();
        }
        self.tick();
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
        self.atlas_tools.feedback(ui);
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
                if self.selected.is_some() && !self.results_visible {
                    let results = ui.button("← Results");
                    crate::widgets::record(ui, "catalog-results-back", results.rect);
                    if results.clicked() {
                        self.results_visible = true;
                    }
                    if !self.results_visible { self.detail(ui); } else { self.results(ui); }
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
        if let Some(delay) = self.next_repaint_after(Instant::now()) { ctx.request_repaint_after(delay); }
        if !self.open {
            if let Ok(mut repaint) = self.repaint.lock() { *repaint = None; }
        }
    }
    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let back = ui.button("← Desigma");
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
            self.atlas_tools.menu(ui);
            ui.with_layout(Layout::right_to_left(Align::Center).with_main_wrap(true), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                }
                ui.checkbox(&mut self.auto_refresh, "Live updates")
                    .on_hover_text("Automatic incremental refresh. Low-impact scheduling yields to selected designs; unchanged snapshots are reused.");
                if self.busy {
                    if ui.button("Cancel").clicked() {
                        self.cancelled.store(true, Ordering::Release);
                    }
                    crate::cityos_jobs::progress_marker(ui);
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
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!self.navigation_back.is_empty(), egui::Button::new("← Back")).clicked() { self.navigate_history(false); }
            if ui.add_enabled(!self.navigation_forward.is_empty(), egui::Button::new("Forward →")).clicked() { self.navigate_history(true); }
            if self.results_visible {
                if let Some(entry) = self.selected.as_ref() {
                    if ui.button("Return to selection").on_hover_text(preview::display_name(entry)).clicked() { self.results_visible = false; }
                }
            }
        });
        let before = self.filters.clone();
        ui.horizontal_wrapped(|ui| {
            let width = (ui.available_width() - 190.0).max(80.0);
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
                    for level in support::levels(self.summary.as_ref()) {
                        let suffix = level.count.map(|n| count(n as usize)).unwrap_or_else(|| "?".into());
                        if nav_row(ui, self.filters.level == level.id, &format!("{}  {}", level.id, level.name), &suffix).clicked() {
                            self.filters = Filters { collection: "design".into(), level: level.id, ..Default::default() };
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
        let diagnostics = self.summary.as_ref().and_then(|s| s["navigation"]["diagnostics"].as_array()).map_or(0, Vec::len);
        if diagnostics > 0 { ui.small(format!("{diagnostics} unresolved hierarchy relationships · see Source accounting")); }
        if products.is_empty() {
            ui.label(if self.summary.is_some() { "No products are registered in this snapshot." } else { "Products appear after the repository loads." });
        }
        for product in products {
            let id = product["id"].as_str().unwrap_or("");
            if id.is_empty() { continue; }
            let name = product["name"].as_str().unwrap_or(id);
            if nav_row(ui, self.filters.product == id, name, "").clicked() {
                self.filters = Filters { collection: "sources".into(), product: id.into(), ..Default::default() };
            }
            if self.filters.product == id {
                ui.indent(id, |ui| {
                    if let Some(targets) = product["targets"].as_array() {
                        if !targets.is_empty() { ui.small("Declared targets"); }
                        for target in targets {
                            let target = target.as_str().or_else(|| target["id"].as_str()).unwrap_or("");
                            if target.is_empty() { continue; }
                            if ui.selectable_label(self.filters.target == target, preview::humanize(target)).clicked() {
                                self.filters.target = target.into();
                                self.filters.search.clear();
                            }
                        }
                    }
                    self.product_children(ui, &product, id, 0);
                    let has_shells = product["shells"].as_array().is_some_and(|shells| !shells.is_empty())
                        || product["targets"].as_array().is_some_and(|targets| targets.iter().any(|target| target["shells"].as_array().is_some_and(|shells| !shells.is_empty())));
                    if !has_shells {
                        ui.small("No shell hierarchy is registered for this product in this capture.");
                    }
                });
            }
        }
    }
    fn product_children(&mut self, ui: &mut Ui, parent: &Value, scope: &str, depth: usize) {
        if depth >= 6 { ui.small("Further registered hierarchy is available in source records."); return; }
        for (key, kind) in [("targets", "Target"), ("shells", "Shell"), ("routes", "Route"), ("pages", "Page"), ("screens", "Screen"), ("compositions", "Composition")] {
            let Some(children) = parent[key].as_array() else { continue; };
            for (index, child) in children.iter().enumerate() {
                if key == "targets" && !child.is_object() { continue; }
                let id = child["id"].as_str().or_else(|| child["path"].as_str()).or_else(|| child.as_str()).unwrap_or("Unnamed registration");
                let name = child["name"].as_str().unwrap_or(id);
                let path = child["sourcePath"].as_str().or_else(|| child["path"].as_str());
                let salt = format!("{scope}/{key}/{index}");
                let max_chars = ((ui.available_width() - 70.0) / 8.0).max(6.0) as usize;
                let short = name.chars().take(max_chars).collect::<String>();
                let short = if name.chars().count() > max_chars { format!("{short}…") } else { short };
                let response = egui::CollapsingHeader::new(format!("{kind} · {short}")).id_salt(&salt).show(ui, |ui| {
                    if key == "targets" && ui.selectable_label(self.filters.target == id, "Browse this target").clicked() {
                        self.filters.target = id.into();
                        self.filters.search.clear();
                    }
                    if let Some(path) = path {
                        ui.label(path);
                        if key == "compositions" && child["kind"] == "composition" {
                            let open = ui.button("Open selected composition workspace");
                            crate::widgets::record(ui, &format!("catalog-hierarchy-open-{salt}"), open.rect);
                            if open.clicked() { self.workspace_request = Some(super::WorkspaceRequest::selected(path, true)); }
                        }
                        if ui.button("Browse source records").on_hover_text("Find this captured source path, including compositions registered outside the product directory.").clicked() {
                            self.filters.collection = "sources".into();
                            self.filters.product.clear();
                            self.filters.search = path.into();
                            self.filters.target.clear();
                        }
                    } else { ui.small("No source path is registered for this item."); }
                    self.product_children(ui, child, &salt, depth + 1);
                });
                crate::widgets::record(ui, &format!("catalog-hierarchy-{salt}"), response.header_response.rect);
                response.header_response.on_hover_text(format!("{kind} · {name}"));
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
        ui.horizontal_wrapped(|ui| {
            ui.strong(title);
            ui.selectable_value(&mut self.visual_results, false, "List");
            ui.selectable_value(&mut self.visual_results, true, "Tiles");
        });
        let waiting = self.displayed_filters != self.filters;
        ui.label(RichText::new(if waiting { "Updating results…".into() } else { format!("{} items", count(self.total)) }).small().color(theme::TEXT_SECONDARY));
        ui.add_space(8.0);
        if self.summary.is_none() {
            ui.add_space(24.0);
            if let Some(error) = &self.error {
                ui.strong("Repository capture is unavailable");
                ui.label(error);
                if ui.add_enabled(!self.busy, egui::Button::new("Retry capture")).clicked() { self.refresh(); }
                if ui.button("Connection settings").clicked() { self.settings_open = true; }
                return;
            }
            crate::cityos_jobs::progress_marker(ui);
            ui.label("Reading your repository…");
            ui.label(RichText::new("The library will appear here when the first capture is ready.").color(theme::TEXT_SECONDARY));
            return;
        }
        if waiting {
            crate::cityos_jobs::progress_marker(ui);
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
        let columns = if self.visual_results { tile_columns(ui.available_width()) } else { 1 };
        let row_height = if self.visual_results { TILE_HEIGHT } else { theme::CATALOG_ROW };
        egui::ScrollArea::vertical()
            .id_salt(("catalog-results-scroll", self.visual_results, serde_json::to_string(&self.displayed_filters).unwrap_or_default()))
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 48.0).max(80.0))
            .show_rows(ui, row_height, self.entries.len().div_ceil(columns), |ui, range| {
                let width = ((ui.available_width() - ui.spacing().item_spacing.x * (columns - 1) as f32) / columns as f32).max(1.0);
                for row in range {
                    ui.horizontal(|ui| {
                        for index in row * columns..((row + 1) * columns).min(self.entries.len()) {
                            let entry = &self.entries[index];
                            let selected = self.selected.as_ref().is_some_and(|s| s.id == entry.id);
                            let response = if self.visual_results { item_tile(ui, entry, selected, width) } else { item_row(ui, entry, selected) };
                            if response.clicked() { chosen = Some(entry.clone()); }
                        }
                    });
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
        let id = self.selected.as_ref().map(|entry| entry.id.clone());
        egui::ScrollArea::vertical().id_salt(("catalog-detail-scroll", id)).auto_shrink([false, false]).show(ui, |ui| self.detail_body(ui));
    }
    fn detail_body(&mut self, ui: &mut Ui) {
        let Some(entry) = self.selected.clone() else {
            self.welcome(ui);
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
        ui.label(RichText::new(&entry.source_path).small().color(theme::TEXT_SECONDARY));
        let actions = support::actions(&entry, self.specimen.as_ref(), self.summary.as_ref(), &self.mode);
        ui.horizontal_wrapped(|ui| {
            let unavailable = actions.source.is_none().then_some(actions.preview_reason.as_str());
            let label = if actions.source.is_some() && !actions.actual_preview { "Open structure workspace" } else { "Open actual preview" };
            if action_button(ui, "catalog-preview-action", label, unavailable) {
                if let Some(source) = &actions.source {
                    self.workspace_request = Some(super::WorkspaceRequest::selected(source, true));
                }
            }
            let unavailable = (!actions.editable).then_some(actions.edit_reason.as_str());
            if action_button(ui, "catalog-edit-action", if self.mode == "local" { "Open canonical draft" } else { "Open on canvas" }, unavailable) {
                if let Some(source) = &actions.source {
                    self.workspace_request = Some(super::WorkspaceRequest::selected(source, false));
                }
            }
        });
        ui.label(RichText::new(&actions.preview_reason).small().color(theme::TEXT_SECONDARY));
        if !actions.editable { ui.small(&actions.edit_reason); }
        for diagnostic in entry.diagnostics.iter().take(3) {
            if let Some(message) = diagnostic["message"].as_str() { ui.small(message); }
        }
        if actions.source.is_some() && self.mode == "local" {
            ui.small("Workspace actions open canonical source. The local overlay remains a data preview.");
        }
        if matches!(entry.kind.as_str(), "theme" | "design-token" | "token") {
            if action_button(ui, "catalog-local-copy-action", if entry.kind == "theme" { "Create editable theme library" } else { "Create editable token" }, actions.copy_reason.as_deref()) {
                if let (Some(summary), Some(specimen)) = (&self.summary, &self.specimen) {
                    self.local_design_error = None;
                    match super::local_library::create(&entry, specimen, &summary["snapshot"], &self.theme_mode) {
                        Ok(document) => self.local_design_request = Some(document),
                        Err(error) => self.local_design_error = Some(error),
                    }
                }
            }
            ui.small(actions.copy_reason.as_deref().unwrap_or("Creates an ordinary local design with editable variables. Save it normally; source submission is not enabled."));
        }
        if entry.kind == "theme" {
            if action_button(ui, "catalog-theme-preview-action", "Preview theme on canonical web frame", actions.theme_reason.as_deref()) {
                self.workspace_theme = Some(entry.semantic_id.clone());
                self.workspace_request = Some(super::WorkspaceRequest::selected("config/experience/living-canvas-frame/cityos-web.input.json", true));
            }
            ui.small(actions.theme_reason.as_deref().unwrap_or("Applies this canonical theme to the registered web-frame fixture. The token sheet below shows the captured values."));
        }
        if let Some(error) = &self.local_design_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Return to current design").clicked() { self.open = false; }
        }
        let specimen = self.specimen.as_ref().filter(|s| support::matches(&entry, s));
        ui.horizontal_wrapped(|ui| {
            if action_button(ui, "catalog-inspection-action", if self.specimen_canvas.is_some() { "Reload inspection canvas" } else { "Load inspection canvas" }, specimen.is_none().then_some("Load this resource before opening its inspection canvas.")) {
                if let Some(specimen) = specimen {
                    self.specimen_canvas = Some(super::canvas::SpecimenCanvas::new(&entry, specimen, &self.theme_mode));
                    self.detail_tab = DetailTab::Preview;
                }
            }
            if self.specimen_canvas.is_some() && ui.button("Back to specimen").clicked() { self.specimen_canvas = None; }
        });
        if let Some(modes) = specimen.and_then(|s| s["presentation"]["modes"].as_object()) {
            if !modes.contains_key(&self.theme_mode) { self.theme_mode = modes.keys().next().cloned().unwrap_or_default(); }
            let old = self.theme_mode.clone();
            ui.horizontal_wrapped(|ui| {
                ui.label("Token sheet mode");
                for key in modes.keys() { ui.selectable_value(&mut self.theme_mode, key.clone(), preview::humanize(key)); }
            });
            if old != self.theme_mode && self.specimen_canvas.is_some() {
                if let Some(specimen) = specimen { self.specimen_canvas = Some(super::canvas::SpecimenCanvas::new(&entry, specimen, &self.theme_mode)); }
            }
        }
        ui.add_space(theme::CATALOG_GAP);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.detail_tab, DetailTab::Preview, "Specimen");
            ui.selectable_value(&mut self.detail_tab, DetailTab::Inspector, "Source & support");
            ui.selectable_value(&mut self.detail_tab, DetailTab::Source, "Raw data");
        });
        ui.separator();
        if self.detail_tab == DetailTab::Preview {
            if let Some(canvas) = &mut self.specimen_canvas {
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 360.0), Layout::top_down(Align::Min), |ui| canvas.show(ui));
                return;
            }
            if let Some(pin) = actions.inline_pin {
                ui.label("Actual renderer · pinned captured source · temporary preview");
                if !self.inline_status.is_empty(){ui.small(&self.inline_status);}
                let (_,rect)=ui.allocate_space(egui::vec2(ui.available_width().max(1.),360.0));
                let rect = rect.intersect(ui.clip_rect());
                if rect.width() > 1.0 && rect.height() > 1.0 { self.inline_preview=Some((pin,rect)); }
                return;
            }
        }
        ui.push_id(("catalog-detail", &entry.id, self.detail_tab as u8), |ui| {
            ui.add_space(theme::CATALOG_GAP);
            match self.detail_tab {
                DetailTab::Inspector => preview::inspector(ui, &entry),
                DetailTab::Source => {
                    if let Some(error) = &self.preview_error {
                        ui.label("Captured source could not load.");
                        ui.small(error);
                        if ui.button("Retry captured source").clicked() { self.pending_selection = true; }
                    } else if self.source_text.is_empty() {
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
                        crate::cityos_jobs::progress_marker(ui);
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
        self.level_overview(ui);
        ui.add_space(theme::CATALOG_GAP);
        ui.label(
            RichText::new("Inspection canvases show captured data as native objects. Actual rendering and local drafts depend on the selected resource's registered adapter. Support is shown before opening; owner qualification and submission remain separate.")
                .small()
                .color(theme::TEXT_SECONDARY),
        );
    }
    fn level_overview(&mut self, ui: &mut Ui) {
        ui.strong("L0–L9 capability coverage");
        ui.small("Captured record flags · unknown counts are not treated as zero. Selected workspace adapters and editable local token copies are reported separately.");
        for level in support::levels(self.summary.as_ref()) {
            Frame::new().inner_margin(egui::Margin::symmetric(0, 6)).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button(format!("{} · {}", level.id, level.name)).clicked() {
                        self.filters = Filters { collection: "design".into(), level: level.id.clone(), ..Default::default() };
                        self.filters_changed();
                    }
                    let total = level.count.map(|n| count(n as usize)).unwrap_or_else(|| "Unknown".into());
                    ui.small(format!("{total} records"));
                });
                ui.horizontal_wrapped(|ui| {
                    for (label, number) in [("Data specimens", level.specimens), ("Renderer flags", level.renderers), ("Editing flags", level.editable)] {
                        let number = number.map(|n| count(n as usize)).unwrap_or_else(|| "Unknown".into());
                        chip(ui, &format!("{label} · {number}"));
                    }
                });
                if level.count == Some(0) { ui.small("No classified records are reported at this level."); }
                else if level.count.is_none() { ui.small("The capture does not report this level; refresh or inspect source accounting."); }
            });
        }
    }
    fn status(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.busy {
                crate::cityos_jobs::progress_marker(ui);
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
        let mut open = self.coverage_open;
        egui::Window::new("Source accounting").open(&mut open).default_width(680.0).vscroll(true).show(ctx,|ui|{
            let Some(summary)=self.summary.clone() else {ui.label("Connect a repository to inspect its census.");return;};
            ui.label(format!("{} source occurrences",count(summary["entryCount"].as_u64().unwrap_or(0) as usize)));
            ui.label("The design library is a browsing selection. The census includes every captured source occurrence, including unresolved and conflicting records.");
            if let Some(rows)=summary["census"].as_array(){egui::Grid::new("catalog-census").striped(true).show(ui,|ui|{
                for label in ["Source family","Expected","Accounted","Health"]{ui.strong(label);}ui.end_row();
                for row in rows {ui.label(row["family"].as_str().unwrap_or(""));ui.label(row["expected"].to_string());ui.label(row["accounted"].to_string());ui.label(if row["healthy"]==true{"Available"}else{"Incomplete"});ui.end_row();}
            });}
            ui.separator();
            self.level_overview(ui);
            ui.separator();
            ui.label(summary["native"]["reason"].as_str().unwrap_or("Native owner discovery qualification is not reported by this capture."));
            ui.collapsing("Snapshot & diagnostics",|ui|{ui.monospace(serde_json::to_string_pretty(&serde_json::json!({"snapshot":summary["snapshot"],"diagnostics":summary["diagnostics"],"navigationDiagnostics":summary["navigation"]["diagnostics"]})).unwrap_or_default());});
        });
        self.coverage_open = open;
    }
}
