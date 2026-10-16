//! UI-thread-only child WebView2 adapter. WGPU retains ownership of the editor.
use serde_json::Value;
use std::sync::mpsc::{self, Receiver};
#[derive(Default)]
pub struct Host {
    #[cfg(windows)]
    view: Option<wry::WebView>,
    pub url: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    messages: Option<Receiver<Value>>,
}
impl Host {
    pub fn hide(&self) {
        #[cfg(windows)]
        if let Some(view) = &self.view {
            let _ = view.set_visible(false);
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
    }
    pub fn poll(&mut self) -> Vec<Value> {
        self.messages.as_ref().map(|rx| rx.try_iter().take(16).collect()).unwrap_or_default()
    }
    pub fn show(&mut self, frame: &eframe::Frame, url: &str, rect: egui::Rect, scale: f32) -> Result<(), String> {
        #[cfg(windows)]
        {
            let bounds = wry::Rect {
                position: wry::dpi::PhysicalPosition::new((rect.min.x * scale).round() as i32, (rect.min.y * scale).round() as i32).into(),
                size: wry::dpi::PhysicalSize::new((rect.width() * scale).max(1.).round() as u32, (rect.height() * scale).max(1.).round() as u32).into(),
            };
            let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
            if parsed.scheme() != "http" || parsed.host_str() != Some("127.0.0.1") || parsed.port().is_none() {
                return Err("Preview origin rejected".into());
            }
            if self.url.as_deref() != Some(url) {
                self.reset();
                self.version = Some(wry::webview_version().map_err(|e| format!("WebView2 runtime unavailable: {e}"))?);
                let allowed = url.to_owned();
                let origin = parsed.origin().ascii_serialization();
                let (tx, messages) = mpsc::sync_channel(32);
                let view = wry::WebViewBuilder::new()
                    .with_incognito(true)
                    .with_focused(false)
                    .with_devtools(false)
                    .with_url(url)
                    .with_bounds(bounds)
                    .with_navigation_handler(move |candidate| candidate == allowed)
                    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                    .with_ipc_handler(move |request| {
                        if !request.uri().to_string().starts_with(&format!("{origin}/")) || request.body().len() > 8192 {
                            return;
                        }
                        if let Ok(value) = serde_json::from_str::<Value>(request.body()) {
                            let _ = tx.try_send(value);
                        }
                    })
                    .build_as_child(frame)
                    .map_err(|e| format!("Embedded preview failed: {e}"))?;
                self.view = Some(view);
                self.url = Some(url.into());
                self.messages = Some(messages);
            }
            if let Some(view) = &self.view {
                view.set_bounds(bounds).and_then(|_| view.set_visible(true)).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = (frame, url, rect, scale);
            Err("Embedded host is currently available on Windows only".into())
        }
    }
}
