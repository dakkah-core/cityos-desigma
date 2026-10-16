//! UI-thread-only child WebView2 adapter. WGPU retains ownership of the editor.
use serde_json::Value;
use std::cell::Cell;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
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
            { self.context = None; }
            self.storage = Some(storage);
        }
    }
    pub fn hide(&self) {
        #[cfg(windows)]
        if let Some(view) = &self.view {
            if self.visible.replace(false) {
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
    pub fn poll(&mut self) -> Vec<Value> {
        self.messages.as_ref().map(|rx| rx.try_iter().take(16).collect()).unwrap_or_default()
    }
    pub fn show(&mut self, frame: &eframe::Frame, url: &str, rect: egui::Rect, scale: f32, ctx: &egui::Context) -> Result<(), String> {
        #[cfg(windows)]
        {
            let bounds = wry::Rect {
                position: wry::dpi::PhysicalPosition::new((rect.min.x * scale).round() as i32, (rect.min.y * scale).round() as i32).into(),
                size: wry::dpi::PhysicalSize::new((rect.width() * scale).max(1.).round() as u32, (rect.height() * scale).max(1.).round() as u32).into(),
            };
            let bounds_key = [
                (rect.min.x * scale).round() as i32,
                (rect.min.y * scale).round() as i32,
                (rect.width() * scale).max(1.).round() as i32,
                (rect.height() * scale).max(1.).round() as i32,
            ];
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
                            if tx.try_send(value).is_ok() { repaint.request_repaint(); }
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
