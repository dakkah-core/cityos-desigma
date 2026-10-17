//! Host-owned catalog chrome and interactions. The package never imports editor UI.
use desigma_doc::Document;
use egui::{Color32, Rect, Ui};
use std::path::PathBuf;

/// Renderer for an already built temporary inspection document.
/// Implementations keep it outside editor persistence and history.
pub trait SpecimenRenderer {
    fn show(&mut self, ui: &mut Ui);
}

/// Only the chrome values consumed by the catalog, supplied by its current host.
#[derive(Clone, Copy)]
pub struct CatalogStyle {
    pub bg: Color32,
    pub bg_hover: Color32,
    pub bg_secondary: Color32,
    pub bg_selected_secondary: Color32,
    pub bg_tertiary: Color32,
    pub border_selected: Color32,
    pub catalog_body: f32,
    pub catalog_gap: f32,
    pub catalog_list_width: f32,
    pub catalog_narrow: f32,
    pub catalog_nav_row: f32,
    pub catalog_nav_width: f32,
    pub catalog_pad: i8,
    pub catalog_radius: u8,
    pub catalog_row: f32,
    pub catalog_small: f32,
    pub catalog_stack: f32,
    pub catalog_title: f32,
    pub danger: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
}

/// Narrow host hooks. Recording remains a host callback even when the package
/// itself is built without cfg(test), so desktop test rectangles keep working.
#[derive(Clone, Copy)]
pub struct CatalogUiServices {
    pub style: fn() -> CatalogStyle,
    pub record: fn(&Ui, &str, Rect),
    pub specimen_renderer: fn(Document) -> Box<dyn SpecimenRenderer>,
    pub pick_repository: fn() -> Option<PathBuf>,
    pub pick_reader: fn() -> Option<PathBuf>,
}
impl CatalogUiServices {
    pub(super) fn theme(self) -> CatalogStyle { (self.style)() }
    pub(super) fn record(self, ui: &Ui, name: &str, rect: Rect) { (self.record)(ui, name, rect); }
    pub(super) fn renderer(self, document: Document) -> Box<dyn SpecimenRenderer> { (self.specimen_renderer)(document) }
}
