use super::super::WorkspaceRequest;
use super::preview_choices_menu;
use egui::{Context, Event, FullOutput, Id, Key, Modifiers, PointerButton, Rect, Vec2};
use serde_json::{json, Value};

fn pin(version: usize) -> Value {
    let source = format!("packages/design-system/src/components/{}/registered-preview.tsx", "long-source-segment/".repeat(6));
    let profile = format!("cityos.authoring.{}profile-{version}", "long-profile-segment.".repeat(6));
    json!({
        "kind":"registered-specimen", "resourceId":format!("fixture:{version}"),
        "componentId":"cityos.fixture", "componentVersion":version.to_string(),
        "publicExport":"RegisteredComponentWithALongUnbrokenSourceExportName",
        "profileRef":profile, "targetProfileRef":"cityos.target.web.arabic-accessible-preview",
        "sourcePath":source, "sourcePaths":[source, "packages/shared.tsx"],
        "sourceDigest":"b".repeat(64), "implementationDigest":"c".repeat(64), "commit":"a".repeat(40),
        "locator":{"componentId":"cityos.fixture", "componentVersion":version.to_string(),
            "profileRef":profile, "targetProfileRef":"cityos.target.web.arabic-accessible-preview"},
        "fixture":{"variant":version}, "opaqueCapturedField":{"preserve":[version, "exact"]}
    })
}

struct Menu {
    ctx: Context,
    size: Vec2,
    panel_width: f32,
    choices: Vec<Value>,
    request: Option<WorkspaceRequest>,
}

impl Menu {
    fn new(width: f32, panel_width: f32, count: usize) -> Self {
        let ctx = Context::default();
        ctx.all_styles_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
            style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(crate::theme::CATALOG_BODY));
        });
        Self { ctx, size: egui::vec2(width, 700.0), panel_width, choices: (1..=count).map(pin).collect(), request: None }
    }

    fn frame(&mut self, events: Vec<Event>) -> FullOutput {
        let ctx = self.ctx.clone();
        ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, self.size)), events, ..Default::default() }, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                // A narrow detail panel at the right edge of a potentially wide viewport.
                let panel =
                    Rect::from_min_max(egui::pos2(ui.max_rect().right() - self.panel_width.min(ui.available_width()), ui.max_rect().top()), ui.max_rect().max);
                ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
                    ui.set_clip_rect(panel);
                    crate::widgets::record(ui, "catalog-preview-panel", panel);
                    ui.horizontal_wrapped(|ui| {
                        if let Some(request) = preview_choices_menu(ui, &self.choices) {
                            self.request = Some(request);
                        }
                    });
                });
            });
        })
    }

    fn rect(&self, name: &str) -> Rect {
        self.ctx.data(|data| data.get_temp(Id::new(("test-rect", name.to_owned())))).unwrap_or_else(|| panic!("Missing {name}"))
    }

    fn choice_id(&self, index: usize) -> Id {
        self.ctx.data(|data| data.get_temp(Id::new(("catalog-preview-choice-id", index)))).expect("Choice must be focusable")
    }

    fn click(&mut self, name: &str) {
        let pos = self.rect(name).center();
        for pressed in [true, false] {
            self.frame(vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
    }

    fn key(&mut self, key: Key) {
        for pressed in [true, false] {
            self.frame(vec![Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: Modifiers::NONE }]);
        }
    }

    fn settle(&mut self) -> FullOutput {
        self.frame(vec![]);
        self.frame(vec![])
    }

    fn open(&mut self) -> FullOutput {
        self.settle();
        self.click("catalog-choose-preview");
        self.settle()
    }

    fn assert_bounds(&self) {
        let popup = self.rect("catalog-preview-popup");
        let panel = self.rect("catalog-preview-panel");
        let viewport = Rect::from_min_size(egui::Pos2::ZERO, self.size);
        assert!(viewport.expand(1.0).contains_rect(popup), "Popup escaped viewport: {popup:?} in {viewport:?}");
        assert!(popup.width() <= panel.width().min(560.0) + 1.0, "Popup exceeds panel width: {popup:?} in {panel:?}");
        let scroll = self.rect("catalog-preview-scroll");
        let content: Vec2 = self.ctx.data(|data| data.get_temp(Id::new("catalog-preview-content-size"))).unwrap();
        assert!(content.x <= scroll.width() + 1.0, "Choices require horizontal scrolling: {content:?} in {scroll:?}");
        let mut previous_bottom = None;
        for index in 0..self.choices.len() {
            let choice = self.rect(&format!("catalog-preview-choice-{index}"));
            assert!(choice.left() >= scroll.left() - 1.0 && choice.right() <= scroll.right() + 1.0, "Choice overflows: {choice:?} in {scroll:?}");
            if let Some(bottom) = previous_bottom {
                assert!(choice.top() >= bottom, "Wrapped choices overlap");
            }
            previous_bottom = Some(choice.bottom());
        }
    }
}

fn choice_text<'a>(shape: &'a egui::epaint::Shape, export: &str) -> Option<&'a egui::epaint::TextShape> {
    match shape {
        egui::epaint::Shape::Text(text) if text.galley.job.text.starts_with(export) => Some(text),
        egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(|shape| choice_text(shape, export)),
        _ => None,
    }
}

#[test]
fn preview_menu_narrow_viewports_and_panels_wrap_without_overflow_or_overlap() {
    for (width, panel_width) in [(240.0, 240.0), (320.0, 320.0), (390.0, 390.0), (1600.0, 260.0)] {
        let mut menu = Menu::new(width, panel_width, 2);
        let output = menu.open();
        menu.assert_bounds();
        let choice = menu.rect("catalog-preview-choice-0");
        assert!(choice.height() > 72.0, "Long labels must grow beyond the old fixed row height");
        let text =
            output.shapes.iter().find_map(|shape| choice_text(&shape.shape, menu.choices[0]["publicExport"].as_str().unwrap())).expect("Real egui button text");
        assert!(text.galley.rows.len() > 3 && !text.galley.elided, "Source and profile labels must wrap fully");
        assert!(choice.expand(1.0).contains_rect(text.galley.rect.translate(text.pos.to_vec2())), "Button clips its wrapped label");
        for field in ["sourcePath", "profileRef", "targetProfileRef"] {
            assert!(text.galley.job.text.contains(menu.choices[0][field].as_str().unwrap()), "Missing captured {field}");
        }
        assert!(menu.request.is_none(), "Opening a menu must not choose a fallback");
    }
}

#[test]
fn preview_menu_wide_layout_is_bounded_and_uses_natural_row_heights() {
    let mut wide = Menu::new(1600.0, 900.0, 2);
    wide.open();
    wide.assert_bounds();
    let mut narrow = Menu::new(320.0, 320.0, 2);
    narrow.open();
    assert!(wide.rect("catalog-preview-choice-0").width() > narrow.rect("catalog-preview-choice-0").width());
    assert!(wide.rect("catalog-preview-choice-0").height() < narrow.rect("catalog-preview-choice-0").height());
}

#[test]
fn preview_menu_resize_preserves_focus_and_keyboard_activation_keeps_exact_locator() {
    for key in [Key::Enter, Key::Space] {
        let mut menu = Menu::new(1600.0, 900.0, 2);
        menu.open();
        let first = menu.choice_id(0);
        menu.ctx.memory_mut(|memory| memory.request_focus(first));
        menu.settle();
        menu.size.x = 320.0;
        menu.panel_width = 320.0;
        menu.settle();
        menu.assert_bounds();
        assert_eq!(menu.choice_id(0), first, "Reflow must preserve widget identity");
        assert!(menu.ctx.memory(|memory| memory.has_focus(first)), "Reflow lost keyboard focus");
        menu.key(Key::Tab);
        menu.settle();
        let second = menu.choice_id(1);
        assert!(menu.ctx.memory(|memory| memory.has_focus(second)), "Tab must reach the next wrapped choice");
        let choice = menu.rect("catalog-preview-choice-1");
        assert!(menu.rect("catalog-preview-scroll").intersects(choice), "Focused choice must scroll into view");
        assert!(menu.request.is_none(), "Focus must not implicitly select");
        menu.key(key);
        let request = menu.request.as_ref().expect("Keyboard activation must open the focused preview");
        assert_eq!(request.source_path.as_deref(), menu.choices[1]["sourcePath"].as_str());
        assert_eq!(request.resource_pin.as_ref(), Some(&menu.choices[1]), "Every captured field and locator must survive unchanged");
        assert!(request.preview_only && !request.resume);
    }
}

#[test]
fn preview_menu_keyboard_reaches_offscreen_choices_and_escape_does_not_select() {
    let mut menu = Menu::new(320.0, 320.0, 8);
    menu.open();
    let first = menu.choice_id(0);
    menu.ctx.memory_mut(|memory| memory.request_focus(first));
    menu.settle();
    for index in 1..menu.choices.len() {
        menu.key(Key::Tab);
        menu.settle();
        let id = menu.choice_id(index);
        assert!(menu.ctx.memory(|memory| memory.has_focus(id)), "Offscreen choice {index} must remain in keyboard order");
        assert!(
            menu.rect("catalog-preview-scroll").intersects(menu.rect(&format!("catalog-preview-choice-{index}"))),
            "Focused choice {index} remains offscreen"
        );
    }
    assert!(menu.request.is_none());
    menu.key(Key::Escape);
    let output = menu.settle();
    assert!(menu.request.is_none());
    assert!(
        !output.shapes.iter().any(|shape| choice_text(&shape.shape, menu.choices[0]["publicExport"].as_str().unwrap()).is_some()),
        "Escape must close the popup"
    );
}

#[test]
fn preview_menu_pointer_activation_keeps_the_exact_nondefault_locator() {
    for width in [320.0, 1600.0] {
        let mut menu = Menu::new(width, width.min(900.0), 2);
        for pin in &mut menu.choices {
            pin["publicExport"] = json!("SharedExport");
            pin["profileRef"] = json!("authoring");
            pin["targetProfileRef"] = json!("web");
            pin["sourcePath"] = json!("packages/shared.tsx");
            pin["locator"]["profileRef"] = json!("authoring");
            pin["locator"]["targetProfileRef"] = json!("web");
        }
        menu.open();
        menu.assert_bounds();
        menu.click("catalog-preview-choice-1");
        let request = menu.request.as_ref().expect("Click must open the chosen preview");
        assert_eq!(request.source_path.as_deref(), Some("packages/shared.tsx"));
        assert_eq!(request.resource_pin.as_ref(), Some(&menu.choices[1]));
        assert!(request.preview_only && !request.resume);
    }
}
