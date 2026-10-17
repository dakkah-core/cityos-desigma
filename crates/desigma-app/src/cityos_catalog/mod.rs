//! Thin desktop host for the extracted CityOS catalog.
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
pub use desigma_cityos::catalog::WorkspaceRequest;

mod canvas;
mod style;
#[cfg(test)]
pub(crate) mod tests;

pub struct Browser(desigma_cityos::catalog::Browser);

impl Browser {
    pub fn new(repository: Option<PathBuf>, reader: Option<PathBuf>, node: Option<PathBuf>, cache: Option<PathBuf>, canonical_ref: Option<String>) -> Self {
        Self(desigma_cityos::catalog::Browser::new(repository, reader, node, cache, canonical_ref, style::services()))
    }
    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) { self.0.show(ui, ctx); }
    pub(crate) fn atlas_workspace_menu(ui: &mut egui::Ui) {
        desigma_cityos::catalog::Browser::atlas_workspace_menu(ui, style::services());
    }
    pub(crate) fn atlas_workspace_feedback(ui: &mut egui::Ui) {
        desigma_cityos::catalog::Browser::atlas_workspace_feedback(ui, style::services());
    }
}
impl Deref for Browser {
    type Target = desigma_cityos::catalog::Browser;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl DerefMut for Browser {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}
