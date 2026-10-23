//! Application preferences only; these never enter a CityOS semantic draft.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum View {
    Canvas,
    #[default]
    Split,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    pub version: u32,
    pub view: View,
    pub inspector: bool,
    pub preview_width: f32,
}
impl Default for Layout {
    fn default() -> Self {
        Self { version: 1, view: View::Split, inspector: true, preview_width: 560. }
    }
}
impl Layout {
    pub fn load(cache: &Path) -> Result<Self, String> {
        let path = cache.join("workspace-layout.json");
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("Cannot read workspace layout: {error}")),
        };
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take(65537).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("Workspace layout exceeds its size limit".into());
        }
        let value: Self = serde_json::from_slice(&bytes).map_err(|e| format!("Invalid workspace layout: {e}"))?;
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || !self.preview_width.is_finite() || !(280. ..=1400.).contains(&self.preview_width) {
            return Err("Unsupported workspace layout; reset panels to recover".into());
        }
        Ok(())
    }
    pub fn save(&self, cache: &Path) -> Result<(), String> {
        self.validate()?;
        std::fs::create_dir_all(cache).map_err(|e| e.to_string())?;
        desigma_library::write_atomic(&cache.join("workspace-layout.json"), &serde_json::to_vec(self).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Cannot save workspace layout: {e}"))
    }
    pub fn effective_view(&self, width: f32, narrow_preview: bool) -> View {
        if self.view == View::Split && width < 1100. {
            if narrow_preview {
                View::Preview
            } else {
                View::Canvas
            }
        } else {
            self.view
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn narrow_workspace_uses_one_surface_without_changing_saved_preference() {
        let layout = Layout::default();
        assert_eq!(layout.effective_view(900., false), View::Canvas);
        assert_eq!(layout.effective_view(900., true), View::Preview);
        assert_eq!(layout.effective_view(1500., false), View::Split);
        assert_eq!(layout.view, View::Split);
    }
    #[test]
    fn layout_roundtrip_is_separate_from_documents_and_rejects_future_versions() {
        let root = std::env::temp_dir().join(format!("desigma-layout-{}", uuid::Uuid::new_v4()));
        let layout = Layout { view: View::Canvas, inspector: false, preview_width: 480., ..Default::default() };
        layout.save(&root).unwrap();
        assert_eq!(Layout::load(&root).unwrap(), layout);
        std::fs::write(root.join("workspace-layout.json"), br#"{"version":2}"#).unwrap();
        assert!(Layout::load(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
