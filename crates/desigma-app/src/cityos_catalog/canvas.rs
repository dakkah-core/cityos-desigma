//! Thin private App/tile host for the package's detached inspection document.
use crate::app::App;
use desigma_cityos::catalog::SpecimenRenderer;
use desigma_doc::Document;

pub(super) struct SpecimenCanvas { app: App }

impl SpecimenCanvas {
    pub(super) fn new(document: Document) -> Self {
        let mut app = App::new(document);
        app.first_fit_done = true;
        app.view.zoom = 0.85;
        app.view.offset = desigma_doc::Vec2::new(-20., -20.);
        Self { app }
    }
}

impl SpecimenRenderer for SpecimenCanvas {
    fn show(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Inspection canvas");
            if ui.button("Fit").clicked() {
                self.app.first_fit_done = false;
            }
            if ui.button("−").clicked() {
                self.app.view.zoom *= 0.8;
            }
            if ui.button("+").clicked() {
                self.app.view.zoom *= 1.25;
            }
            ui.small("Native frames, text and swatches · temporary");
        });
        let rect = ui.available_rect_before_wrap();
        self.app.canvas_rect = rect;
        self.app.now = ui.ctx().input(|i| i.time);
        if !self.app.first_fit_done {
            self.app.first_fit_done = true;
            self.app.fit_all();
        }
        crate::canvas::tiles::paint(&mut self.app, ui, rect);
        let response = ui.interact(rect, ui.id().with("specimen-canvas"), egui::Sense::drag());
        if response.dragged() {
            let delta = ui.ctx().input(|i| i.pointer.delta());
            self.app.view.offset.x -= delta.x / self.app.view.zoom;
            self.app.view.offset.y -= delta.y / self.app.view.zoom;
        }
        if response.hovered() {
            let zoom = ui.ctx().input(|i| i.zoom_delta());
            if zoom != 1. {
                let at = ui.ctx().input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
                self.app.view.zoom_at(rect.min, at, self.app.view.zoom * zoom);
            } else {
                let delta = ui.ctx().input(|i| i.smooth_scroll_delta);
                self.app.view.offset.x -= delta.x / self.app.view.zoom;
                self.app.view.offset.y -= delta.y / self.app.view.zoom;
            }
        }
        ui.allocate_rect(rect, egui::Sense::hover());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_renderer_keeps_save_delete_and_history_keys_out_of_editor_state() {
        let mut document = Document::new("Detached specimen");
        let page = document.add_page("Inspection");
        let id = document.alloc_id();
        let mut node = desigma_doc::Node::rectangle(id, desigma_doc::Rect::new(0., 0., 100., 80.));
        node.locked = true;
        document.insert(page, node).unwrap();
        let original = document.to_json().unwrap();
        let mut surface = SpecimenCanvas::new(document);
        let ctx = egui::Context::default();
        for key in [egui::Key::S, egui::Key::Z, egui::Key::D, egui::Key::Delete] {
            let modifiers = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
            let _ = ctx.run_ui(egui::RawInput { modifiers, events: vec![egui::Event::Key {
                key, physical_key: None, pressed: true, repeat: false, modifiers,
            }], ..Default::default() }, |ui| surface.show(ui));
        }
        assert_eq!(surface.app.doc.to_json().unwrap(), original);
        assert!(surface.app.doc.cityos.is_none());
        assert!(surface.app.lib.is_none() && surface.app.lib_file.is_none() && surface.app.file_path.is_none());
        assert!(!surface.app.hist.can_undo() && !surface.app.hist.can_redo());
        assert!(surface.app.autosave.changed_at.is_none());
    }
}
