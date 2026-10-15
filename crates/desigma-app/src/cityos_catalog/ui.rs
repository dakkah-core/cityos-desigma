use super::{preview, Browser, DetailTab};
use crate::theme;
use desigma_cityos_catalog::{Entry, Filters};
use egui::{Align, Align2, Context, FontId, Frame, Layout, RichText, Sense, Stroke, StrokeKind, Ui};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const COLLECTIONS: [(&str, &str); 7] = [
    ("design", "Design library"),
    ("components", "Components & foundations"),
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
        if ctx.input(|input| {
            input.pointer.any_pressed()
                || input.smooth_scroll_delta != egui::Vec2::ZERO
                || input.events.iter().any(|event| matches!(event, egui::Event::Key { .. } | egui::Event::Text(_)))
        }) {
            self.last_interaction = Instant::now();
        }
        self.tick();
        ctx.request_repaint_after(Duration::from_millis(100));
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
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                }
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
                    ui.label(RichText::new("Composition levels").small().color(theme::TEXT_SECONDARY));
                    let levels = self.summary.as_ref().and_then(|s| s["navigation"]["levels"].as_array()).cloned().unwrap_or_default();
                    for level in levels {
                        let id = level["id"].as_str().unwrap_or("");
                        let name = level["name"].as_str().unwrap_or("");
                        if nav_row(ui, self.filters.level == id, &format!("{id}  {name}"), "").clicked() {
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
        ui.add_space(theme::CATALOG_GAP);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.detail_tab, DetailTab::Preview, "Preview");
            ui.selectable_value(&mut self.detail_tab, DetailTab::Inspector, "Source & support");
            ui.selectable_value(&mut self.detail_tab, DetailTab::Source, "Raw data");
        });
        ui.separator();
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
                        preview::show(ui, &entry, &specimen["data"]);
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
        ui.label(RichText::new("Explore the CityOS\ndesign system.").size(32.0).strong());
        ui.add_space(theme::CATALOG_GAP);
        ui.label(
            RichText::new("Browse foundations, components, compositions and the products built from them.")
                .size(theme::CATALOG_BODY)
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(24.0);
        for (id, title, detail) in [
            ("components", "Components & foundations", "Registered building blocks, organized from L0 to L9."),
            ("tokens", "Design tokens", "Color values, aliases and token contracts."),
            ("compositions", "Compositions", "Authored structure, slots and properties."),
        ] {
            Frame::new().fill(theme::BG).corner_radius(theme::CATALOG_RADIUS).inner_margin(16).show(ui, |ui| {
                ui.set_width((ui.available_width()).max(120.0));
                if ui.button(RichText::new(format!("{title}  →")).strong()).clicked() {
                    self.filters = Filters { collection: id.into(), ..Default::default() };
                    self.filters_changed();
                }
                ui.label(RichText::new(detail).color(theme::TEXT_SECONDARY));
            });
            ui.add_space(8.0);
        }
        ui.add_space(theme::CATALOG_GAP);
        ui.label(
            RichText::new("Select an item to see its data preview. Source records and unresolved classifications remain available in the sidebar.")
                .small()
                .color(theme::TEXT_SECONDARY),
        );
    }
    fn status(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.busy {
                ui.spinner();
                ui.label(self.activity);
            } else if self.error.is_some() {
                ui.label("Refresh unavailable · previous catalog retained").on_hover_text(self.error.as_deref().unwrap_or(""));
            } else if self.summary.as_ref().is_some_and(|s| s["restoredCache"] == true) {
                ui.label("Saved snapshot · updates checked on refresh");
            } else if self.summary.as_ref().is_some_and(|s| s["stale"] == true) {
                ui.label("Offline · browsing saved snapshot").on_hover_text(self.summary.as_ref().unwrap()["refreshError"].as_str().unwrap_or(""));
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
