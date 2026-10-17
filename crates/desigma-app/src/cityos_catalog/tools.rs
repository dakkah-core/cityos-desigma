//! Private Atlas tools open through the existing native browser launcher.
//! Only these fixed routes leave Desigma; catalog data and credentials do not.
use crate::theme;
use egui::{RichText, Ui};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, TryRecvError}};
use std::time::{Duration, Instant};

const HANDOFF_TIMEOUT: Duration = Duration::from_secs(10);
const HANDOFF_POLL: Duration = Duration::from_millis(25);
const UI_POLL: Duration = Duration::from_millis(50);
static HANDOFF_WORKER: AtomicBool = AtomicBool::new(false);
const SESSION_ID: &str = "cityos-atlas-tools-session";

const ATLAS_SITE: &str = "https://cityos-atlas.qahtani1979.chatgpt.site/atlas-v4.1/";
const PRIVATE_ACCESS: &str = "Private Site · sign in with your approved account in your browser.";
const BROWSER_DISABLED: &str = "Browser opening is disabled in this build. Copy a link and open it in your browser.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    Overview,
    Graph,
    Findings,
    Reports,
    Runs,
}

const DESTINATIONS: [Destination; 5] = [Destination::Overview, Destination::Graph, Destination::Findings, Destination::Reports, Destination::Runs];

impl Destination {
    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Graph => "Architecture graph",
            Self::Findings => "Findings",
            Self::Reports => "Reports & Codex exports",
            Self::Runs => "Analysis runs",
        }
    }

    fn page(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Graph => "workbench",
            Self::Findings => "findings",
            Self::Reports => "reports",
            Self::Runs => "runs",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Overview => "Review the latest accepted analysis and its coverage.",
            Self::Graph => "Trace systems, services, packages and dependencies.",
            Self::Findings => "Inspect issues, retained evidence and source locations.",
            Self::Reports => "Prepare scoped reports and download Codex batches for fixes.",
            Self::Runs => "Inspect analysis history, source revisions and execution status.",
        }
    }

    fn url(self) -> String {
        // Atlas investigation.js parseRoute/serializeRoute: v3 with an empty
        // state follows its latest accepted analysis. Graph is `workbench`.
        format!("{ATLAS_SITE}#v=3&page={}&state=%7B%7D", self.page())
    }
}

fn atlas_url_allowed(url: &str) -> bool {
    // An exact route allowlist also rejects credentials, alternate ports,
    // arbitrary hashes, redirect queries and deceptive host suffixes.
    DESTINATIONS.iter().any(|destination| destination.url() == url)
}

fn validate_atlas_url(url: &str) -> Result<(), String> {
    if !atlas_url_allowed(url) {
        return Err("Blocked Atlas link. Use one of the registered Atlas tools.".into());
    }
    #[cfg(feature = "open-url")]
    {
        Ok(())
    }
    #[cfg(not(feature = "open-url"))]
    {
        Err(BROWSER_DISABLED.into())
    }
}

// Only the worker calls the OS launcher. All commands come from the existing
// pinned `open` dependency for an exact allowlisted URL, never catalog content.
fn open_atlas_url(url: &str, cancelled: &AtomicBool, deadline: Instant) -> Result<(), String> {
    validate_atlas_url(url)?;
    #[cfg(feature = "open-url")]
    {
        let mut spawn_error = "No system browser launcher is available.".to_string();
        for mut command in open::commands(url) {
            handoff_active(cancelled, deadline)?;
            command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            match command.spawn() {
                Ok(mut child) => return wait_for_helper(&mut child, cancelled, deadline),
                Err(error) => spawn_error = format!("Could not start the system browser launcher ({:?}).", error.kind()),
            }
        }
        Err(spawn_error)
    }
    #[cfg(not(feature = "open-url"))]
    { let _ = (cancelled, deadline); Err(BROWSER_DISABLED.into()) }
}

fn handoff_active(cancelled: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err("Browser handoff canceled. Your browser may already have opened.".into())
    } else if Instant::now() >= deadline {
        Err("Browser handoff timed out. Check your browser before retrying.".into())
    } else { Ok(()) }
}

#[cfg(any(feature = "open-url", test))]
fn wait_for_helper(child: &mut std::process::Child, cancelled: &AtomicBool, deadline: Instant) -> Result<(), String> {
    struct Helper<'a> { child: &'a mut std::process::Child, reaped: bool }
    impl Drop for Helper<'_> {
        fn drop(&mut self) {
            if !self.reaped {
                // Own only the launcher child. Never search for/kill a browser,
                // its descendants, or another application's PID. Off the UI.
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
    let mut helper = Helper { child, reaped: false };
    loop {
        handoff_active(cancelled, deadline)?;
        match helper.child.try_wait() {
            Ok(Some(status)) => {
                helper.reaped = true;
                return if status.success() { Ok(()) } else {
                    Err(format!("The browser launcher failed (exit {}).", status.code().map_or("unknown".into(), |code| code.to_string())))
                };
            }
            Ok(None) => std::thread::park_timeout(HANDOFF_POLL),
            Err(error) => return Err(format!("Could not observe the browser launcher ({:?}).", error.kind())),
        }
    }
}

struct WorkerPermit;
impl Drop for WorkerPermit {
    fn drop(&mut self) { HANDOFF_WORKER.store(false, Ordering::Release); }
}

struct PendingLaunch {
    destination: Destination,
    cancelled: Arc<AtomicBool>,
    result: Receiver<Result<(), String>>,
    worker: std::thread::Thread,
}
impl PendingLaunch {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.worker.unpark();
    }
}
impl Drop for PendingLaunch {
    fn drop(&mut self) { self.cancel(); } // No join or OS wait on the UI thread.
}

fn spawn_handoff(destination: Destination, work: impl FnOnce(&AtomicBool, Instant) -> Result<(), String> + Send + 'static) -> Result<PendingLaunch, String> {
    HANDOFF_WORKER.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "A previous browser handoff is still finishing. Check your browser or try again shortly.".to_string())?;
    let permit = WorkerPermit;
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let (send, result) = mpsc::sync_channel(1);
    let deadline = Instant::now() + HANDOFF_TIMEOUT;
    let worker = std::thread::Builder::new().name("atlas-browser-handoff".into()).spawn(move || {
        let outcome = handoff_active(&signal, deadline).and_then(|()| work(&signal, deadline));
        // A new launch is permitted only after the owned helper has been reaped.
        drop(permit);
        let _ = send.send(outcome);
    }).map_err(|_| "Could not start browser handoff. Copy the link and open it manually.".to_string())?;
    let handle = worker.thread().clone();
    drop(worker); // Worker owns cleanup; disposal only signals cancellation.
    Ok(PendingLaunch { destination, cancelled, result, worker: handle })
}

struct LaunchFailure {
    destination: Destination,
    message: String,
}

#[derive(Default)]
struct AtlasSession {
    failure: Option<LaunchFailure>,
    pending: Option<PendingLaunch>,
    dispatched: Option<Destination>,
}
impl AtlasSession {
    fn open(&mut self, destination: Destination) {
        if self.pending.is_some() { return; }
        self.failure = None;
        self.dispatched = None;
        let launch = validate_atlas_url(&destination.url()).and_then(|()|
            spawn_handoff(destination, move |cancelled, deadline| open_atlas_url(&destination.url(), cancelled, deadline)));
        match launch {
            Ok(pending) => self.pending = Some(pending),
            Err(message) => self.failure = Some(LaunchFailure { destination, message }),
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let outcome = self.pending.as_ref().map(|pending| (pending.destination, pending.result.try_recv()));
        match outcome {
            Some((_, Err(TryRecvError::Empty))) => ctx.request_repaint_after(UI_POLL),
            Some((destination, result)) => {
                self.pending = None;
                match result.unwrap_or_else(|_| Err("Browser handoff stopped. Check your browser before retrying.".into())) {
                    Ok(()) => self.dispatched = Some(destination),
                    Err(message) => self.failure = Some(LaunchFailure { destination, message }),
                }
                ctx.request_repaint();
            }
            None => {}
        }
    }
}

// One context-owned session survives catalog/canvas navigation. Workers capture
// no Context/session Arc: closing the context drops PendingLaunch and cancels it.
fn session(ctx: &egui::Context) -> Arc<Mutex<AtlasSession>> {
    ctx.data_mut(|data| data.get_temp_mut_or_default::<Arc<Mutex<AtlasSession>>>(egui::Id::new(SESSION_ID)).clone())
}

#[derive(Default)]
pub(super) struct AtlasTools { menu_open: bool }

// Expose only UI entry points on the already-visible Browser type; the existing
// catalog module and its shared ownership/visibility need no changes.
impl super::Browser {
    pub(crate) fn atlas_workspace_menu(ui: &mut Ui) { AtlasTools::default().menu(ui); }
    pub(crate) fn atlas_workspace_feedback(ui: &mut Ui) { AtlasTools::default().feedback(ui); }
}

impl AtlasTools {

    pub(super) fn menu(&mut self, ui: &mut Ui) {
        let state = session(ui.ctx());
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.poll(ui.ctx());
        let menu_response = ui.button("Atlas ↗");
        // Expanding the guide and copying links must leave the menu readable.
        // Destinations close explicitly after starting their browser handoff.
        let menu = egui::Popup::menu(&menu_response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_max_width(320.0);
            ui.strong("CityOS Atlas");
            ui.add(egui::Label::new(RichText::new("Investigate architecture · prepare fixes").small().color(theme::TEXT_SECONDARY)).wrap());
            ui.separator();
            if !cfg!(feature = "open-url") {
                ui.add(egui::Label::new(RichText::new(BROWSER_DISABLED).small().color(theme::TEXT_SECONDARY)).wrap());
            }
            for destination in DESTINATIONS {
                ui.push_id(destination.page(), |ui| {
                    ui.horizontal(|ui| {
                        let disabled = if cfg!(feature = "open-url") { "A browser handoff is in progress. Check its status below." } else { BROWSER_DISABLED };
                        let response = ui.add_enabled(cfg!(feature = "open-url") && state.pending.is_none(), egui::Button::new(format!("{} ↗", destination.label())));
                        crate::widgets::record(ui, &format!("catalog-atlas-{}", destination.page()), response.rect);
                        if response.on_hover_text(destination.hint()).on_disabled_hover_text(disabled).clicked() {
                            state.open(destination);
                            ui.ctx().request_repaint();
                            ui.close();
                        }
                        if !cfg!(feature = "open-url") {
                            let copy = ui.small_button("Copy link");
                            crate::widgets::record(ui, &format!("catalog-atlas-copy-{}", destination.page()), copy.rect);
                            if copy.clicked() {
                                ui.ctx().copy_text(destination.url());
                            }
                        }
                    });
                });
            }
            ui.separator();
            ui.add(egui::Label::new(RichText::new(PRIVATE_ACCESS).small().color(theme::TEXT_SECONDARY)).wrap());
            let guide = ui.collapsing("Prepare fixes for Codex", |ui| {
                ui.add(egui::Label::new("In Atlas Reports, use a completed report's Download Codex batches action. The ZIP contains README.md, numbered batches of up to 25 tasks, and the original full report.").wrap());
                ui.add(egui::Label::new("Open Codex in C:/Dakkah-CityOS/dakkah-cityos-cms. Load README.md plus one downloaded .codex.md batch at a time. Run batches serially by default.").wrap());
                ui.add(egui::Label::new("Before changing code, match the report's exact source pin to the CMS checkout. These links follow the latest accepted analysis; choose and pin the intended run in Atlas.").wrap());
                ui.add(egui::Label::new("Optional CLI fallback: scripts/atlas-codex-pack.mjs in the Atlas repository.").wrap());
            });
            crate::widgets::record(ui, "catalog-atlas-guide", guide.header_response.rect);
        });
        self.menu_open = menu.is_some();
        crate::widgets::record(ui, "catalog-atlas-menu", menu_response.rect);
        menu_response.on_hover_text("Open private Atlas tools in your default browser");
    }

    pub(super) fn is_open(&self) -> bool {
        self.menu_open
    }

    pub(super) fn feedback(&mut self, ui: &mut Ui) {
        let state = session(ui.ctx());
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.poll(ui.ctx());
        let pending = state.pending.is_some();
        let (destination, message, failed) = if let Some(job) = &state.pending {
            (job.destination, if job.cancelled.load(Ordering::Acquire) { "Canceling browser handoff… Your browser may already have opened." } else { "Sending to your browser…" }.to_string(), false)
        } else if let Some(failure) = &state.failure {
            (failure.destination, format!("Atlas could not open: {}", failure.message), true)
        } else if let Some(destination) = state.dispatched {
            (destination, "Browser handoff finished. Check your browser and sign in with your approved account. Atlas access has not been verified.".into(), false)
        } else { return; };
        let mut retry = false;
        let mut dismiss = false;
        egui::Frame::new().inner_margin(egui::Margin::symmetric(theme::CATALOG_PAD, 6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if pending { ui.spinner(); }
                ui.add(egui::Label::new(RichText::new(message).color(if failed { theme::DANGER } else { theme::TEXT_SECONDARY })).wrap());
                if pending {
                    if ui.button("Cancel handoff").clicked() { if let Some(job) = &state.pending { job.cancel(); } }
                } else if failed {
                    retry = ui.add_enabled(cfg!(feature = "open-url"), egui::Button::new("Retry"))
                        .on_disabled_hover_text(BROWSER_DISABLED).clicked();
                }
                let copy = ui.button("Copy Atlas link");
                crate::widgets::record(ui, "catalog-atlas-copy-failure", copy.rect);
                if copy.clicked() { ui.ctx().copy_text(destination.url()); }
                if !pending { dismiss = ui.button("Dismiss").clicked(); }
            });
        });
        if dismiss { state.failure = None; state.dispatched = None; }
        else if retry { state.open(destination); ui.ctx().request_repaint(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_destinations_use_supported_v3_routes_without_credentials_or_catalog_data() {
        let expected = ["overview", "workbench", "findings", "reports", "runs"];
        for (destination, page) in DESTINATIONS.into_iter().zip(expected) {
            let raw = destination.url();
            assert!(atlas_url_allowed(&raw));
            let url = url::Url::parse(&raw).unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("cityos-atlas.qahtani1979.chatgpt.site"));
            assert_eq!(url.path(), "/atlas-v4.1/");
            assert_eq!(url.username(), "");
            assert_eq!(url.password(), None);
            assert_eq!(url.query(), None);
            let params: std::collections::BTreeMap<_, _> = url::form_urlencoded::parse(url.fragment().unwrap().as_bytes()).into_owned().collect();
            assert_eq!(params.len(), 3);
            assert_eq!(params["v"], "3");
            assert_eq!(params["page"], page);
            assert_eq!(serde_json::from_str::<serde_json::Value>(&params["state"]).unwrap(), serde_json::json!({}));
        }
    }

    #[test]
    fn atlas_allowlist_blocks_deceptive_urls_and_arbitrary_routes_before_os_handoff() {
        let allowed = Destination::Overview.url();
        let invalid = [
            allowed.replacen("https:", "http:", 1),
            allowed.replacen("https://", "https://owner@", 1),
            allowed.replacen(".site/", ".site.evil.example/", 1),
            allowed.replacen(".site/", ".site@evil.example/", 1),
            allowed.replacen(".site/", ".site:8443/", 1),
            allowed.replacen("/atlas-v4.1/", "/atlas-v4.1/../admin/", 1),
            allowed.replacen("#", "?redirect=https://evil.example#", 1),
            allowed.replace("page=overview", "page=graph"),
            allowed.replace("state=%7B%7D", "state=%7B%22runId%22%3A%22invented%22%7D"),
            format!("{allowed}&token=private"),
            format!(" {allowed}"),
            format!("{allowed}\n"),
            "javascript:alert(1)".into(),
            "file:///C:/Windows/System32/cmd.exe".into(),
            "//cityos-atlas.qahtani1979.chatgpt.site/atlas-v4.1/".into(),
        ];
        for url in invalid {
            assert!(!atlas_url_allowed(&url), "Unexpected allowlisted URL: {url}");
            assert!(validate_atlas_url(&url).unwrap_err().starts_with("Blocked Atlas link."));
        }
    }

    fn painted_text(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(text) => { out.push_str(&text.galley.job.text); out.push('\n'); }
            egui::epaint::Shape::Vec(shapes) => { for shape in shapes { painted_text(shape, out); } }
            _ => {}
        }
    }

    fn frame(tools: &mut AtlasTools, ctx: &egui::Context, width: f32, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 720.0))),
            events, ..Default::default()
        }, |ui| { tools.menu(ui); tools.feedback(ui); })
    }

    fn click(tools: &mut AtlasTools, ctx: &egui::Context, width: f32, id: &str) -> egui::FullOutput {
        let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", id.to_string())))).unwrap();
        let pos = rect.center();
        let mut output = None;
        for pressed in [true, false] {
            output = Some(frame(tools, ctx, width, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]));
        }
        output.unwrap()
    }

    #[test]
    fn atlas_menu_is_compact_and_exposes_all_five_tools_only_on_request() {
        for width in [390.0, 1200.0] {
            let mut tools = AtlasTools::default();
            let ctx = egui::Context::default();
            ctx.all_styles_mut(|style| style.animation_time = 0.0);
            frame(&mut tools, &ctx, width, vec![]);
            let closed = frame(&mut tools, &ctx, width, vec![]);
            let mut text = String::new();
            for shape in &closed.shapes { painted_text(&shape.shape, &mut text); }
            assert!(!tools.is_open());
            assert!(text.contains("Atlas ↗"));
            assert!(!text.contains("CityOS Atlas") && !text.contains("Codex Markdown"));
            click(&mut tools, &ctx, width, "catalog-atlas-menu");
            frame(&mut tools, &ctx, width, vec![]);
            let open = frame(&mut tools, &ctx, width, vec![]);
            assert!(tools.is_open());
            text.clear();
            for shape in &open.shapes { painted_text(&shape.shape, &mut text); }
            assert!(text.contains("CityOS Atlas"));
            assert!(text.contains(PRIVATE_ACCESS));
            assert!(text.contains("Prepare fixes for Codex"));
            assert!(!text.contains("downloaded .codex.md"), "The workflow guide starts collapsed");
            for destination in DESTINATIONS {
                assert!(text.contains(destination.label()));
                let id = format!("catalog-atlas-{}", destination.page());
                let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", id)))).unwrap();
                assert!(rect.left() >= 0.0 && rect.right() <= width && rect.bottom() <= 720.0, "Tool outside viewport: {rect:?}");
            }
        }
    }

    #[test]
    fn atlas_access_is_available_without_a_catalog_capture() {
        for width in [390.0, 1200.0] {
            let ctx = egui::Context::default();
            let mut browser = super::super::Browser::new(None, None, None, None, None);
            browser.open = true;
            browser.settings_open = false;
            browser.auto_refresh = false;
            for _ in 0..2 {
                let _ = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 900.0))),
                    ..Default::default()
                }, |ui| browser.show(ui, &ctx));
            }
            let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", "catalog-atlas-menu".to_string())))).unwrap();
            assert!(rect.left() >= 0.0 && rect.right() <= width && rect.bottom() <= 900.0);
            assert!(browser.summary.is_none());
            assert!(!browser.busy, "Opening Atlas does not require a repository scan");
        }
    }

    #[test]
    fn browser_failure_offers_the_exact_destination_for_manual_recovery() {
        let ctx = egui::Context::default();
        let mut tools = AtlasTools::default();
        session(&ctx).lock().unwrap().failure = Some(LaunchFailure { destination: Destination::Reports, message: "No default browser is configured".into() });
        frame(&mut tools, &ctx, 390.0, vec![]);
        let output = frame(&mut tools, &ctx, 390.0, vec![]);
        let mut text = String::new();
        for shape in &output.shapes { painted_text(&shape.shape, &mut text); }
        assert!(text.contains("No default browser is configured"));
        assert!(text.contains("Copy Atlas link") && text.contains("Retry") && text.contains("Dismiss"));
        let copied = click(&mut tools, &ctx, 390.0, "catalog-atlas-copy-failure");
        assert!(copied.platform_output.commands.iter().any(|command| matches!(command,
            egui::OutputCommand::CopyText(text) if text == &Destination::Reports.url())));
        assert!(!text.contains("connected") && !text.contains("authenticated"));
    }

    #[cfg(not(feature = "open-url"))]
    #[test]
    fn build_without_browser_support_offers_real_copy_links_and_never_claims_launch() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut tools = AtlasTools::default();
        assert_eq!(validate_atlas_url(&Destination::Reports.url()), Err(BROWSER_DISABLED.into()));
        frame(&mut tools, &ctx, 390.0, vec![]);
        frame(&mut tools, &ctx, 390.0, vec![]);
        click(&mut tools, &ctx, 390.0, "catalog-atlas-menu");
        frame(&mut tools, &ctx, 390.0, vec![]);
        let output = frame(&mut tools, &ctx, 390.0, vec![]);
        let mut text = String::new();
        for shape in &output.shapes { painted_text(&shape.shape, &mut text); }
        assert!(text.contains(BROWSER_DISABLED));
        let copied = click(&mut tools, &ctx, 390.0, "catalog-atlas-copy-reports");
        assert!(copied.platform_output.commands.iter().any(|command| matches!(command,
            egui::OutputCommand::CopyText(text) if text == &Destination::Reports.url())));
        assert!(session(&ctx).lock().unwrap().failure.is_none());
    }
}

#[cfg(test)]
#[path = "tools/launch_tests.rs"]
mod launch_tests;
