//! Native plugin bundles and browser panels with scoped document access.
mod bundled;
pub mod runtime;
pub mod store;
mod svg;
pub mod ui;

use crate::app::App;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

#[derive(Default)]
pub struct PluginsState {
    pub open: bool,
    pub install_form: bool,
    pub url: String,
    pub error: Option<String>,
    pub installed: Vec<store::InstalledPlugin>,
    pub pending: bool,
    pub fresh: bool,
    pub root: Option<PathBuf>,
    pub runtime: Option<runtime::Runtime>,
    pub running_id: Option<String>,
    receiver: Option<Receiver<Result<store::InstalledPlugin, String>>>,
}

impl App {
    fn plugin_root(&self) -> PathBuf {
        self.plugins.root.clone().or_else(|| self.lib.as_ref().map(|lib| lib.root().to_path_buf())).unwrap_or_else(desigma_library::Library::default_root)
    }

    pub fn open_plugins(&mut self) {
        crate::canvas::interact::finish_text(self, true);
        self.ui.menu = None;
        self.ui.ctx_menu = None;
        self.plugins.open = true;
        if !self.plugins.pending {
            self.plugins.install_form = false;
            self.plugins.error = None;
            match store::list(&self.plugin_root()) {
                Ok(plugins) => self.plugins.installed = plugins,
                Err(error) => self.plugins.error = Some(error),
            }
        }
    }

    pub fn install_plugin(&mut self, ctx: &egui::Context) {
        if self.plugins.pending {
            return;
        }
        let url = self.plugins.url.trim().to_string();
        if let Err(error) = store::validate_url(&url) {
            self.plugins.error = Some(error);
            return;
        }
        self.begin_plugin_install(ctx, move |root| store::install(root, &url));
    }

    pub fn install_included_plugin(&mut self, ctx: &egui::Context) {
        self.begin_plugin_install(ctx, store::install_icon_browser);
    }

    pub fn install_local_plugin(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.begin_plugin_install(ctx, move |root| store::install_local(root, &path));
    }

    fn begin_plugin_install(&mut self, ctx: &egui::Context, install: impl FnOnce(&std::path::Path) -> Result<store::InstalledPlugin, String> + Send + 'static) {
        if self.plugins.pending {
            return;
        }
        let root = self.plugin_root();
        let (sender, receiver) = mpsc::channel();
        self.plugins.receiver = Some(receiver);
        self.plugins.pending = true;
        self.plugins.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(install(&root));
            ctx.request_repaint();
        });
    }

    pub fn poll_plugins(&mut self) {
        self.poll_plugin_runtime();
        let Some(receiver) = &self.plugins.receiver else { return };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Plugin installation stopped. Try installing again.".into()),
        };
        self.plugins.receiver = None;
        self.plugins.pending = false;
        match result {
            Ok(plugin) => {
                self.plugins.installed.retain(|p| p.id != plugin.id || p.version != plugin.version);
                self.plugins.installed.push(plugin);
                self.plugins.installed.sort_by(|a, b| a.name.cmp(&b.name));
                self.plugins.install_form = false;
                self.plugins.url.clear();
                self.plugins.error = None;
                self.show_toast("Plugin installed. Open it from the Plugins library.", false);
            }
            Err(error) => self.plugins.error = Some(error),
        }
    }

    pub fn stop_plugin(&mut self) {
        self.plugins.runtime = None;
        self.plugins.running_id = None;
    }

    pub fn start_plugin(&mut self, plugin: &store::InstalledPlugin, ctx: &egui::Context) -> Result<(), String> {
        if self.home.is_some() || self.history.viewing.is_some() || self.proto.present.is_some() {
            return Err("Open an editable document before running a plugin.".into());
        }
        crate::canvas::interact::finish_text(self, true);
        let bundle = store::load_bundle(&self.plugin_root(), plugin)?;
        let runtime = runtime::Runtime::start(bundle, ctx.clone())?;
        self.stop_plugin();
        self.plugins.running_id = Some(plugin.id.clone());
        self.plugins.runtime = Some(runtime);
        Ok(())
    }

    pub fn open_plugin(&mut self, plugin: &store::InstalledPlugin, ctx: &egui::Context) {
        #[cfg(feature = "open-url")]
        let result = self.start_plugin(plugin, ctx).and_then(|()| {
            open::that(self.plugins.runtime.as_ref().expect("started runtime").url()).map_err(|_| "Cannot open the plugin panel in your browser.".to_string())
        });
        #[cfg(not(feature = "open-url"))]
        let result: Result<(), String> = {
            let _ = (plugin, ctx);
            Err("This build does not support opening browser plugin panels.".into())
        };
        match result {
            Ok(()) => {
                self.plugins.error = None;
                self.plugins.open = false;
            }
            Err(error) => {
                self.stop_plugin();
                self.plugins.error = Some(error);
            }
        }
    }

    fn poll_plugin_runtime(&mut self) {
        if self.home.is_some() || self.history.viewing.is_some() {
            self.stop_plugin();
            return;
        }
        // At most one operation per frame keeps a busy plugin from monopolizing the editor.
        let request = self.plugins.runtime.as_ref().and_then(runtime::Runtime::try_recv);
        if let Some(request) = request {
            if request.cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            let result = self.insert_plugin_svg(&request.svg, &request.name);
            let _ = request.reply.send(result);
        }
    }

    pub(crate) fn insert_plugin_svg(&mut self, source: &str, name: &str) -> Result<String, String> {
        if self.home.is_some() || self.history.viewing.is_some() || self.proto.present.is_some() {
            return Err("Return to an editable document before inserting an icon.".into());
        }
        if self.scrub.is_some()
            || self.dialog.is_some()
            || !matches!(self.canvas.inter, crate::canvas::Interaction::None)
            || self.canvas.pen.is_some()
            || self.canvas.builder.is_some()
            || self.canvas.vedit.is_some()
        {
            return Err("Finish the current edit or close the dialog before inserting an icon.".into());
        }
        svg::validate(source)?;
        if name.trim().is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
            return Err("The icon name must contain 1–200 printable characters.".into());
        }
        crate::canvas::interact::finish_text(self, true);
        let rect = self.canvas_rect;
        let center = self.view.to_page(rect.min, rect.center());
        let parent = self.container_for_insert();
        let id = crate::clipboard::place_svg(self, source, name.trim(), center, parent)?;
        self.show_toast(format!("Inserted {name}"), false);
        Ok(id.0.to_string())
    }
}
