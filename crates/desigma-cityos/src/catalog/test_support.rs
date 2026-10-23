//! Explicit catalog scenarios for host tests. No editor state is exported.
use super::*;
use serde_json::json;

pub fn idle_browser(services: CatalogUiServices) -> Browser {
    let mut browser = Browser::new(None, None, None, None, None, services);
    browser.open = true;
    browser.settings_open = false;
    browser.auto_refresh = false;
    browser
}

pub fn populated_browser(services: CatalogUiServices) -> Browser {
    let (mut browser, _, _) = captured_browser(services);
    browser.selected = Some(entry("one"));
    browser.results_visible = false;
    browser.specimen = Some(json!({"entry":entry("one"),"data":{"$type":"color","$value":"#f8f8f3"}}));
    browser
}

pub(super) fn entry(id: &str) -> Entry {
    serde_json::from_value(json!({"id":id,"semanticId":"token:/colors/surface/page","label":"token:/colors/surface/page","level":"L0","family":"foundations","kind":"token","owner":"design-system","package":"@cityos/design-tokens","product":null,"targets":["web"],"locales":["ar","en"],"sourcePath":"tokens.json","sourcePointer":"/colors/page","sourceDigest":"digest","observationId":"git:fixture","dependencies":[],"generationStatus":"not-applicable","classificationStatus":"classified","temporaryView":"token-sheet","persistentDraft":false,"editable":false,"rendererQualified":false,"diagnostics":[]})).unwrap()
}
pub(super) fn captured_browser(services: CatalogUiServices) -> (Browser, Sender<Reply>, Receiver<Job>) {
    let mut browser = Browser::new(None, None, None, None, None, services);
    let (jobs, requests) = mpsc::channel();
    let (responses, replies) = mpsc::channel();
    browser.jobs = jobs;
    browser.replies = replies;
    browser.open = true;
    browser.settings_open = false;
    browser.summary = Some(
        json!({"generationId":"first","snapshot":{"mode":"committed","canonicalCommit":"1234567890"},"entryCount":1,"navigation":{"levels":[{"id":"L0","name":"Foundation","count":1}],"products":[]}}),
    );
    browser.catalog_binding = Some(CatalogBinding { config: browser.reader_config(), mode: browser.mode.clone(), generation: "first".into() });
    browser.entries = vec![entry("one")];
    browser.total = 1;
    browser.displayed_filters = browser.filters.clone();
    (browser, responses, requests)
}

#[cfg(test)]
pub(super) fn services() -> CatalogUiServices {
    CatalogUiServices {
        style: test_style,
        record: |ui, name, rect| {
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("test-rect", name.to_string())), rect));
        },
        specimen_renderer: |_| Box::new(TestRenderer),
        pick_repository: || None,
        pick_reader: || None,
    }
}
#[cfg(test)]
struct TestRenderer;
#[cfg(test)]
impl SpecimenRenderer for TestRenderer {
    fn show(&mut self, _ui: &mut egui::Ui) {}
}

// Snapshot of chrome inputs for package UI tests; production styles always
// come from the host. Preserve the desktop's premultiplied color channels.
#[cfg(test)]
fn test_style() -> CatalogStyle {
    use egui::Color32;
    const fn rgb(c: u32) -> Color32 {
        Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
    }
    const fn rgba(c: u32, a: u8) -> Color32 {
        let r = ((c >> 16) & 0xFF) * a as u32 / 255;
        let g = ((c >> 8) & 0xFF) * a as u32 / 255;
        let b = (c & 0xFF) * a as u32 / 255;
        Color32::from_rgba_premultiplied(r as u8, g as u8, b as u8, a)
    }
    CatalogStyle {
        bg: rgb(0x2C2C2C),
        bg_hover: rgb(0x383838),
        bg_secondary: rgb(0x383838),
        bg_selected_secondary: rgb(0x32394D),
        bg_tertiary: rgb(0x444444),
        border_selected: rgb(0x0C8CE9),
        catalog_body: 14.0,
        catalog_gap: 12.0,
        catalog_list_width: 340.0,
        catalog_narrow: 1080.0,
        catalog_nav_row: 34.0,
        catalog_nav_width: 224.0,
        catalog_pad: 20,
        catalog_radius: 8,
        catalog_row: 68.0,
        catalog_small: 12.0,
        catalog_stack: 760.0,
        catalog_title: 26.0,
        danger: rgb(0xF24822),
        text: rgb(0xFFFFFF),
        text_secondary: rgba(0xFFFFFF, 0xB2),
        text_tertiary: rgba(0xFFFFFF, 0x66),
    }
}
