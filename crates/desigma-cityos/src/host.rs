//! UI-thread-only child WebView2 adapter. WGPU retains ownership of the editor.
use serde_json::Value;
use std::cell::Cell;
#[cfg(any(windows, test))]
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

/// Convert one layout observation consistently at every supported Windows DPI.
/// Reject invalid geometry before touching a live child window.
#[cfg(any(windows, test))]
fn physical_bounds(rect: egui::Rect, scale: f32) -> Result<[i32; 4], String> {
    if !scale.is_finite() || scale <= 0. || !rect.is_finite() || rect.width() <= 0. || rect.height() <= 0. {
        return Err("Preview panel geometry is unavailable".into());
    }
    let values = [rect.min.x * scale, rect.min.y * scale, rect.width() * scale, rect.height() * scale];
    if values.iter().any(|value| !value.is_finite() || *value >= i32::MAX as f32 || *value <= i32::MIN as f32) {
        return Err("Preview panel geometry exceeds platform bounds".into());
    }
    Ok([values[0].round() as i32, values[1].round() as i32, values[2].round().max(1.) as i32, values[3].round().max(1.) as i32])
}
/// A delayed presentation event cannot focus a hidden or replaced child view.
#[cfg(any(windows, test))]
fn focus_current_preview(current_url: Option<&str>, visible: bool, expected_url: &str, focus: impl FnOnce() -> Result<(), String>) -> Result<bool, String> {
    if !visible || current_url != Some(expected_url) {
        return Ok(false);
    }
    focus()?;
    Ok(true)
}
#[derive(Default)]
pub struct Host {
    #[cfg(windows)]
    view: Option<wry::WebView>,
    #[cfg(windows)]
    context: Option<wry::WebContext>,
    storage: Option<std::path::PathBuf>,
    pub url: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    messages: Option<Receiver<Value>>,
    bounds: Option<[i32; 4]>,
    visible: Cell<bool>,
    allowed: Option<Arc<Mutex<String>>>,
    origin: Option<String>,
}
impl Host {
    pub fn configure_storage(&mut self, cache: &std::path::Path) {
        let storage = cache.join("webview2-preview");
        if self.storage.as_ref() != Some(&storage) {
            self.reset();
            #[cfg(windows)]
            {
                self.context = None;
            }
            self.storage = Some(storage);
        }
    }
    pub fn hide(&self) {
        #[cfg(windows)]
        if let Some(view) = &self.view {
            if self.visible.replace(false) {
                let _ = view.evaluate_script("window.dispatchEvent(new CustomEvent('cityos-preview-visibility',{detail:false}))");
                let _ = view.set_visible(false);
            }
        }
    }
    pub fn reset(&mut self) {
        #[cfg(windows)]
        {
            self.view = None;
        }
        self.url = None;
        self.messages = None;
        self.error = None;
        self.bounds = None;
        self.visible.set(false);
        self.allowed = None;
        self.origin = None;
    }
    /// Release a renderer that no longer has a visible workspace. Transient
    /// dialogs still use hide(), preserving their live interaction state.
    pub fn release(&mut self) -> bool {
        let active = self.url.is_some();
        #[cfg(windows)]
        let active = active || self.view.is_some() || self.context.is_some();
        self.reset();
        #[cfg(windows)]
        {
            self.context = None;
        }
        self.version = None;
        active
    }
    pub fn poll(&mut self) -> Vec<Value> {
        self.messages.as_ref().map(|rx| rx.try_iter().take(16).collect()).unwrap_or_default()
    }
    /// Return keyboard focus without hiding the renderer or changing draft state.
    /// The workspace must validate the complete event identity before calling.
    pub fn focus_parent(&self, expected_url: &str) -> Result<bool, String> {
        #[cfg(windows)]
        if let Some(view) = &self.view {
            return focus_current_preview(self.url.as_deref(), self.visible.get(), expected_url, || {
                view.focus_parent().map_err(|error| format!("Cannot return focus to the editor: {error}"))
            });
        }
        let _ = expected_url;
        Ok(false)
    }
    pub fn show(&mut self, frame: &eframe::Frame, url: &str, rect: egui::Rect, scale: f32, ctx: &egui::Context) -> Result<(), String> {
        #[cfg(windows)]
        {
            let bounds_key = physical_bounds(rect, scale)?;
            let bounds = wry::Rect {
                position: wry::dpi::PhysicalPosition::new(bounds_key[0], bounds_key[1]).into(),
                size: wry::dpi::PhysicalSize::new(bounds_key[2] as u32, bounds_key[3] as u32).into(),
            };
            let parsed = url::Url::parse(url).map_err(|e: url::ParseError| e.to_string())?;
            if parsed.scheme() != "http" || parsed.host_str() != Some("127.0.0.1") || parsed.port().is_none() {
                return Err("Preview origin rejected".into());
            }
            let next_origin = parsed.origin().ascii_serialization();
            if self.view.is_some() && self.origin.as_deref() == Some(next_origin.as_str()) && self.url.as_deref() != Some(url) {
                if let Some(allowed) = &self.allowed {
                    *allowed.lock().map_err(|_| "Preview navigation lock failed")? = url.to_owned();
                }
                self.view.as_ref().unwrap().load_url(url).map_err(|e| e.to_string())?;
                self.url = Some(url.into());
            }
            if self.view.is_none() || self.origin.as_deref() != Some(next_origin.as_str()) {
                self.reset();
                self.version = Some(wry::webview_version().map_err(|e| format!("WebView2 runtime unavailable: {e}"))?);
                // Even InPrivate WebView2 creates an environment directory. Keep
                // mutable browser data outside the verified executable package.
                if self.context.is_none() {
                    let storage = self.storage.as_ref().filter(|path| path.is_absolute()).ok_or("Application preview cache is unavailable")?;
                    std::fs::create_dir_all(storage).map_err(|e| format!("Preview cache unavailable: {e}"))?;
                    self.context = Some(wry::WebContext::new(Some(storage.clone())));
                }
                let allowed = Arc::new(Mutex::new(url.to_owned()));
                let navigation = allowed.clone();
                let origin = parsed.origin().ascii_serialization();
                let (tx, messages) = mpsc::sync_channel(32);
                let repaint = ctx.clone();
                let view = wry::WebViewBuilder::new_with_web_context(self.context.as_mut().unwrap())
                    .with_incognito(true)
                    .with_focused(false)
                    .with_devtools(false)
                    .with_url(url)
                    .with_bounds(bounds)
                    .with_navigation_handler(move |candidate| navigation.lock().is_ok_and(|allowed| candidate == *allowed))
                    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                    .with_ipc_handler(move |request| {
                        if !request.uri().to_string().starts_with(&format!("{origin}/")) || request.body().len() > 8192 {
                            return;
                        }
                        if let Ok(value) = serde_json::from_str::<Value>(request.body()) {
                            if tx.try_send(value).is_ok() {
                                repaint.request_repaint();
                            }
                        }
                    })
                    .build_as_child(frame)
                    .map_err(|e| format!("Embedded preview failed: {e}"))?;
                self.view = Some(view);
                self.url = Some(url.into());
                self.messages = Some(messages);
                self.allowed = Some(allowed);
                self.origin = Some(next_origin);
                self.bounds = Some(bounds_key);
                self.visible.set(true);
            }
            if let Some(view) = &self.view {
                if self.bounds != Some(bounds_key) {
                    view.set_bounds(bounds).map_err(|e| e.to_string())?;
                    self.bounds = Some(bounds_key);
                }
                if !self.visible.replace(true) {
                    view.set_visible(true).map_err(|e| e.to_string())?;
                    view.evaluate_script("window.dispatchEvent(new CustomEvent('cityos-preview-visibility',{detail:true}))").map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = (frame, url, rect, scale, ctx);
            Err("Embedded host is currently available on Windows only".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_return_requires_the_same_visible_child_and_propagates_platform_failure() {
        let previous = "http://127.0.0.1:1/previous";
        let current = "http://127.0.0.1:1/current";
        for (url, visible, expected) in [(None, true, current), (Some(current), false, current), (Some(current), true, previous)] {
            assert!(!focus_current_preview(url, visible, expected, || panic!("Obsolete or hidden focus request")).unwrap());
        }
        let called = Cell::new(false);
        assert!(focus_current_preview(Some(current), true, current, || {
            called.set(true);
            Ok(())
        })
        .unwrap());
        assert!(called.get());
        assert!(focus_current_preview(Some(current), true, current, || Err("platform focus failure".into())).is_err());
    }
    #[test]
    fn releasing_an_inactive_renderer_fences_old_ipc_and_retains_its_cache_location() {
        let mut host = Host::default();
        let cache = std::env::temp_dir().join("desigma-inactive-host-test");
        host.configure_storage(&cache);
        let (sender, receiver) = mpsc::sync_channel(1);
        host.messages = Some(receiver);
        host.url = Some("http://127.0.0.1:1/fixture".into());
        host.version = Some("fixture-runtime".into());
        host.visible.set(true);
        assert!(host.release());
        assert!(host.url.is_none() && host.version.is_none() && host.messages.is_none());
        assert!(!host.visible.get());
        assert_eq!(host.storage.as_deref(), Some(cache.join("webview2-preview").as_path()));
        assert!(sender.try_send(serde_json::json!({"kind":"rendered"})).is_err(), "Old renderer responses cannot enter a recreated host");
        assert!(!host.release(), "Release is idempotent");
    }
    #[test]
    fn child_bounds_use_the_same_rounding_at_each_required_dpi() {
        let rect = egui::Rect::from_min_size(egui::pos2(80., 120.), egui::vec2(640., 480.));
        for (scale, expected) in [(1., [80, 120, 640, 480]), (1.25, [100, 150, 800, 600]), (1.5, [120, 180, 960, 720]), (2., [160, 240, 1280, 960])] {
            assert_eq!(physical_bounds(rect, scale).unwrap(), expected);
        }
        assert_eq!(physical_bounds(egui::Rect::from_min_size(egui::pos2(0.25, 0.75), egui::vec2(0.1, 0.1)), 1.5).unwrap(), [0, 1, 1, 1]);
    }
    #[test]
    fn invalid_layout_never_reaches_the_native_child_window() {
        let valid = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640., 480.));
        for scale in [0., -1., f32::NAN, f32::INFINITY, f32::MAX] {
            assert!(physical_bounds(valid, scale).is_err());
        }
        for rect in [egui::Rect::NOTHING, egui::Rect::EVERYTHING, egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(0., 480.))] {
            assert!(physical_bounds(rect, 1.).is_err());
        }
    }
    #[test]
    fn preview_storage_uses_the_application_cache_and_survives_view_reset() {
        let mut host = Host::default();
        let cache = std::env::temp_dir().join("desigma-host-storage-test");
        host.configure_storage(&cache);
        assert_eq!(host.storage.as_deref(), Some(cache.join("webview2-preview").as_path()));
        host.url = Some("previous view".into());
        host.reset();
        assert_eq!(host.storage.as_deref(), Some(cache.join("webview2-preview").as_path()));
        let other = cache.join("other-installation");
        host.configure_storage(&other);
        assert_eq!(host.storage.as_deref(), Some(other.join("webview2-preview").as_path()));
        assert!(host.url.is_none());
    }
}
