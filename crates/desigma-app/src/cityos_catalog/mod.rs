//! Temporary catalog views live outside App.doc and never enter editor save/history.
use desigma_cityos_catalog::{Client, Entry, Filters, Page, ReaderConfig};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
mod canvas;
mod local_library;
mod preview;
mod support;
#[cfg(test)]
pub(crate) mod tests;
mod ui;

const ACTIVE_REPAINT: Duration = Duration::from_millis(100);
const IDLE_REPAINT: Duration = Duration::from_secs(5);
const LOCAL_EVENT_POLL: Duration = Duration::from_millis(750);
const FILTER_DEBOUNCE: Duration = Duration::from_millis(75);

#[derive(Clone, Copy, Default, PartialEq)]
enum DetailTab {
    #[default]
    Preview,
    Source,
    Inspector,
}

#[derive(Clone)]
struct Location {
    filters: Filters,
    selected: Option<Entry>,
    detail_tab: DetailTab,
    theme_mode: String,
    results_visible: bool,
}

enum Job {
    Scan(u64, ReaderConfig, String, bool),
    Query(u64, Filters, usize, CatalogBinding),
    Materialize(u64, String, CatalogBinding),
}

#[derive(Clone, Debug, PartialEq)]
struct CatalogBinding {
    config: ReaderConfig,
    mode: String,
    generation: String,
}

fn scan_cancelled(error: &str) -> bool {
    matches!(error, "Catalog request cancelled; previous generation retained"
        | "CityOS background work cancelled; previous generation retained")
}

fn retain_cancelled_refresh(result: &Result<Value, String>, cancelled: bool, same_session: bool, had_generation: bool) -> bool {
    cancelled && same_session && had_generation && result.as_ref().is_err_and(|error| scan_cancelled(error))
}

fn read_catalog<T>(client: &mut Option<Client>, session: &mut Option<CatalogBinding>, fence: &mut bool,
    binding: &CatalogBinding, read: impl FnOnce(&mut Client) -> Result<T, String>) -> Result<T, String> {
    let result = (|| {
        let current = client.as_mut().ok_or("Refresh the repository first")?;
        if session.as_ref() != Some(binding) || current.generation.as_deref() != Some(binding.generation.as_str()) {
            return Err("Catalog session changed; refresh the repository before reading".into());
        }
        if *fence {
            // Client's cancellation error does not distinguish an ack from a killed
            // helper. This no-op Cancel is ordered after the original Cancel and
            // Client validates its reply against the retained generation. The
            // reader fences retired scan callbacks before acknowledging Cancel;
            // worker termination/cleanup may still be finishing asynchronously.
            current.request("Cancel", serde_json::json!({"targetRequestId":"settled-refresh"}), Duration::from_secs(2))?;
            *fence = false;
        }
        read(current)
    })();
    if let Err(error) = result {
        *client = None;
        *session = None;
        *fence = false;
        return Err(format!("{error}. Previous catalog retained. Use Refresh to reconnect from cache."));
    }
    result
}
enum Reply {
    Scan(u64, Result<Value, String>),
    Query(u64, Filters, usize, Result<Page, String>),
    Materialize(u64, String, Result<Value, String>),
    SourceChanged,
}

fn send_reply(responses: &Sender<Reply>, repaint: &Mutex<Option<egui::Context>>, reply: Reply) -> bool {
    if responses.send(reply).is_err() { return false; }
    // Notify only after queueing a result/event. Never wake egui on an empty poll.
    let context = repaint.lock().ok().and_then(|context| context.clone());
    if let Some(context) = context { context.request_repaint(); }
    true
}
pub struct Browser {
    pub open: bool,
    pub workspace_request: Option<WorkspaceRequest>,
    pub workspace_theme: Option<String>,
    pub local_design_request: Option<desigma_doc::Document>,
    pub local_design_error: Option<String>,
    pub has_bound_draft: bool,
    pub host_diagnostic_request: bool,
    pub inline_preview:Option<(Value,egui::Rect)>,
    pub inline_status:String,
    repository: String,
    reader: String,
    node: String,
    cache: Option<PathBuf>,
    canonical_ref: Option<String>,
    mode: String,
    filters: Filters,
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
    repaint: Arc<Mutex<Option<egui::Context>>>,
    epoch: u64,
    busy: bool,
    summary: Option<Value>,
    catalog_binding: Option<CatalogBinding>,
    scanning: Option<(ReaderConfig, String)>,
    entries: Vec<Entry>,
    total: usize,
    next: Option<usize>,
    selected: Option<Entry>,
    specimen: Option<Value>,
    error: Option<String>,
    last_refresh: Instant,
    filter_changed: Option<Instant>,
    cancelled: Arc<AtomicBool>,
    source_changed: bool,
    activity: &'static str,
    displayed_filters: Filters,
    pending_selection: bool,
    detail_tab: DetailTab,
    settings_open: bool,
    coverage_open: bool,
    filters_open: bool,
    products_open: bool,
    source_text: String,
    last_interaction: Instant,
    preview_error: Option<String>,
    refresh_failures: u32,
    auto_refresh: bool,
    specimen_canvas: Option<canvas::SpecimenCanvas>,
    theme_mode: String,
    results_visible: bool,
    visual_results: bool,
    location_filters: Filters,
    navigation_back: Vec<Location>,
    navigation_forward: Vec<Location>,
}

pub struct WorkspaceRequest {
    pub source_path: Option<String>,
    pub preview_only: bool,
    pub resume: bool,
}
impl WorkspaceRequest {
    pub fn browse() -> Self {
        Self { source_path: None, preview_only: true, resume: false }
    }
    pub fn selected(source: &str, preview_only: bool) -> Self {
        Self { source_path: Some(source.into()), preview_only, resume: false }
    }
}

impl Browser {
    pub fn workspace_config(&self) -> crate::design_workspace::Config {
        crate::design_workspace::Config {
            node: PathBuf::from(&self.node),
            bridge: PathBuf::from(&self.reader).with_file_name("workspace.mjs"),
            repository: PathBuf::from(&self.repository),
            cache: self.cache.clone().unwrap_or_else(|| std::env::temp_dir().join("desigma-cityos-cache")),
        }
    }
    pub fn new(repository: Option<PathBuf>, reader: Option<PathBuf>, node: Option<PathBuf>, cache: Option<PathBuf>, canonical_ref: Option<String>) -> Self {
        let (jobs, requests) = mpsc::channel();
        let (responses, replies) = mpsc::channel();
        let repaint = Arc::new(Mutex::new(None));
        let worker_repaint = repaint.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        std::thread::spawn(move || {
            let mut client: Option<Client> = None;
            let mut session: Option<CatalogBinding> = None;
            let mut fence = false;
            loop {
                // Job sends interrupt this wait immediately. Only local source
                // notifications require the bounded 750 ms drain interval.
                let interval = if session.as_ref().is_some_and(|binding| binding.mode == "local") { LOCAL_EVENT_POLL } else { IDLE_REPAINT };
                let job = match requests.recv_timeout(interval) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if client.as_mut().is_some_and(|c| c.take_source_changed()) {
                            if !send_reply(&responses, &worker_repaint, Reply::SourceChanged) {
                                break;
                            }
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                let reply = match job {
                    Job::Scan(epoch, config, mode, prefer_cache) => {
                        // Preserve a healthy session for incremental refresh. Configuration
                        // changes replace the child and cannot reuse its old generations.
                        let same_session = session.as_ref().is_some_and(|binding| binding.config == config && binding.mode == mode)
                            && client.is_some();
                        let had_generation = client.as_ref().is_some_and(|c| session.as_ref().is_some_and(|binding|
                            c.generation.as_deref() == Some(binding.generation.as_str())));
                        let result = crate::cityos_jobs::run(false,Some(&worker_cancel),|| if same_session {
                            client.as_mut().unwrap().scan(&config, &mode)
                        } else {
                            client = None;
                            Client::connect_with_cancel(&config, worker_cancel.clone()).and_then(|mut c| {
                                let result = c.scan_with_cache(&config, &mode, prefer_cache);
                                client = Some(c);
                                result
                            })
                        });
                        let result = result.and_then(|summary| {
                            if summary["snapshot"]["mode"].as_str() != Some(mode.as_str()) {
                                return Err("The reader returned a different source mode".into());
                            }
                            let generation = summary["generationId"].as_str().filter(|id| !id.is_empty()).ok_or("Missing generation identity")?;
                            session = Some(CatalogBinding { config, mode, generation: generation.into() });
                            fence = false;
                            Ok(summary)
                        });
                        if retain_cancelled_refresh(&result, worker_cancel.load(Ordering::Acquire), same_session, had_generation) {
                            fence = true;
                        } else if result.is_err() {
                            client = None;
                            session = None;
                            fence = false;
                        }
                        Reply::Scan(epoch, result)
                    }
                    Job::Query(epoch, filters, offset, binding) => {
                        let result = read_catalog(&mut client, &mut session, &mut fence, &binding, |c| c.query(&filters, offset));
                        Reply::Query(epoch, filters, offset, result)
                    }
                    Job::Materialize(epoch, id, binding) => {
                        let result = read_catalog(&mut client, &mut session, &mut fence, &binding, |c| c.materialize(&id));
                        Reply::Materialize(epoch, id, result)
                    }
                };
                if !send_reply(&responses, &worker_repaint, reply) {
                    break;
                }
            }
        });
        let open = repository.is_some();
        let reader = reader.or_else(|| std::env::current_exe().ok().map(|p| p.with_file_name("cityos-reader").join("reader.mjs")));
        let mut browser = Self {
            open,
            workspace_request: None,
            workspace_theme: None,
            local_design_request: None,
            local_design_error: None,
            has_bound_draft: false,
            host_diagnostic_request: false,
            inline_preview:None,
            inline_status:String::new(),
            repository: repository.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            reader: reader.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            node: node.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "node".into()),
            cache,
            canonical_ref,
            mode: "committed".into(),
            filters: Filters { collection: "design".into(), ..Default::default() },
            jobs,
            replies,
            repaint,
            epoch: 0,
            busy: false,
            summary: None,
            catalog_binding: None,
            scanning: None,
            entries: vec![],
            total: 0,
            next: None,
            selected: None,
            specimen: None,
            error: None,
            last_refresh: Instant::now(),
            filter_changed: None,
            cancelled,
            source_changed: false,
            activity: "",
            displayed_filters: Filters::default(),
            pending_selection: false,
            detail_tab: DetailTab::default(),
            settings_open: !open,
            coverage_open: false,
            filters_open: false,
            products_open: false,
            source_text: String::new(),
            last_interaction: Instant::now(),
            preview_error: None,
            refresh_failures: 0,
            auto_refresh: true,
            specimen_canvas: None,
            theme_mode: "light".into(),
            results_visible: true,
            visual_results: false,
            location_filters: Filters { collection: "design".into(), ..Default::default() },
            navigation_back: vec![],
            navigation_forward: vec![],
        };
        if open {
            browser.refresh();
        }
        browser
    }
    fn reader_config(&self) -> ReaderConfig {
        ReaderConfig { node: self.node.clone().into(), reader: self.reader.clone().into(), repository: self.repository.clone().into(),
            cache: self.cache.clone(), canonical_ref: self.canonical_ref.clone() }
    }
    fn usable_generation(&self) -> Option<&CatalogBinding> {
        self.catalog_binding.as_ref().filter(|binding| binding.config == self.reader_config() && binding.mode == self.mode
            && self.summary.as_ref().is_some_and(|summary| summary["generationId"].as_str() == Some(binding.generation.as_str())
                && summary["snapshot"]["mode"].as_str() == Some(binding.mode.as_str())))
    }
    fn yield_to_catalog_read(&self) {
        if self.busy && self.activity == "Refreshing repository" && self.usable_generation().is_some() {
            // Keep the epoch and busy flag until Scan settles. Clearing cancellation
            // or queueing a read here could revive the scan while it is cancelling.
            self.cancelled.store(true, Ordering::Release);
        }
    }
    fn refresh(&mut self) {
        if self.busy {
            return;
        }
        let prefer_cache = self.usable_generation().is_none();
        self.cancelled.store(false, Ordering::Release);
        self.epoch += 1;
        self.busy = true;
        self.activity = if prefer_cache { "Reading repository" } else { "Refreshing repository" };
        self.error = None;
        self.last_refresh = Instant::now();
        let config = self.reader_config();
        self.scanning = Some((config.clone(), self.mode.clone()));
        let _ = self.jobs.send(Job::Scan(self.epoch, config, self.mode.clone(), prefer_cache));
    }
    pub fn yield_to_design(&self) {
        if self.busy && matches!(self.activity,"Reading repository"|"Refreshing repository") {self.cancelled.store(true,Ordering::Release);}
    }
    fn query(&mut self, offset: usize) {
        if self.busy {
            self.yield_to_catalog_read();
            return;
        }
        let Some(binding) = self.usable_generation().cloned() else {
            self.error = Some("Refresh the repository before reading this source configuration".into());
            return;
        };
        self.cancelled.store(false, Ordering::Release);
        self.epoch += 1;
        self.busy = true;
        self.activity = "Finding items";
        let _ = self.jobs.send(Job::Query(self.epoch, self.filters.clone(), offset, binding));
    }
    fn poll(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            match reply {
                Reply::Scan(epoch, result) if epoch == self.epoch => {
                    self.busy = false;
                    self.last_refresh = Instant::now();
                    let scan = self.scanning.take();
                    let cancelled_refresh = self.cancelled.load(Ordering::Acquire) && self.usable_generation().is_some()
                        && scan.as_ref().is_some_and(|(config, mode)| *config == self.reader_config() && *mode == self.mode)
                        && result.as_ref().is_err_and(|error| scan_cancelled(error));
                    // The worker has returned from Scan (including its cancellation
                    // handshake). Only now may an interactive read reset this flag.
                    self.cancelled.store(false, Ordering::Release);
                    if cancelled_refresh {
                        self.error = None;
                        continue;
                    }
                    self.refresh_failures = if result.as_ref().is_ok_and(|summary| summary["stale"] != true || summary["restoredCache"] == true) {
                        0
                    } else {
                        self.refresh_failures.saturating_add(1)
                    };
                    match result {
                        Ok(summary) => {
                            if summary["snapshot"]["mode"].as_str() != Some(self.mode.as_str())
                                || scan.as_ref().is_some_and(|(config, mode)| *config != self.reader_config() || *mode != self.mode) {
                                self.catalog_binding = None;
                                self.error = Some("The reader returned a different source mode. The previous catalog is still shown; refresh to retry.".into());
                            } else {
                                let Some(generation) = summary["generationId"].as_str().filter(|id| !id.is_empty()) else {
                                    self.catalog_binding = None;
                                    self.error = Some("Missing generation identity; refresh to retry".into());
                                    continue;
                                };
                                self.catalog_binding = Some(CatalogBinding { config: self.reader_config(), mode: self.mode.clone(), generation: generation.into() });
                                let changed = self.summary.as_ref().is_none_or(|old| old["generationId"] != summary["generationId"]);
                                let needs_query = changed || self.displayed_filters != self.filters || self.filter_changed.is_some();
                                self.summary = Some(summary);
                                if changed {
                                    self.clear_preview();
                                    self.pending_selection = self.selected.is_some();
                                }
                                if needs_query {
                                    self.filter_changed = None;
                                    self.query(0);
                                }
                            }
                        }
                        Err(e) => { self.catalog_binding = None; self.error = Some(e); },
                    }
                }
                Reply::Query(epoch, filters, offset, result) if epoch == self.epoch => {
                    self.busy = false;
                    match result {
                        Ok(page) if filters == self.filters && self.usable_generation().is_some() => {
                            if offset == 0 {
                                self.entries = page.entries;
                            } else {
                                self.entries.extend(page.entries);
                            }
                            self.displayed_filters = filters;
                            self.total = page.total;
                            self.next = page.next_offset;
                        }
                        Ok(_) => {} // The user changed filters while the request was in flight.
                        Err(e) => { self.catalog_binding = None; self.error = Some(e); },
                    }
                }
                Reply::Materialize(epoch, id, result) if epoch == self.epoch => {
                    self.busy = false;
                    if result.is_err() { self.catalog_binding = None; }
                    if let Err(error) = &result { self.error = Some(error.clone()); }
                    if self.usable_generation().is_none() || self.selected.as_ref().is_none_or(|entry| entry.id != id) {
                        if let Err(error) = result { self.preview_error = Some(error); }
                        continue;
                    }
                    match result {
                        Ok(value) => {
                            // A refresh may have changed metadata for the same occurrence.
                            let Ok(entry) = serde_json::from_value::<Entry>(value["entry"].clone()) else {
                                self.preview_error = Some("The preview did not identify its selected source. Select it again to retry.".into());
                                continue;
                            };
                            if entry.id != id || !support::matches(&entry, &value) {
                                self.preview_error = Some("The preview returned a different source identity. Select it again to retry.".into());
                                continue;
                            }
                            self.selected = Some(entry);
                            self.source_text = preview::source_text(&value);
                            self.specimen = Some(value);
                            self.preview_error = None;
                        }
                        Err(e) => { self.catalog_binding = None; self.preview_error = Some(e); },
                    }
                }
                Reply::SourceChanged => self.source_changed = true,
                _ => {}
            }
        }
    }
    fn filters_changed(&mut self) {
        if self.filters != self.location_filters {
            // Coalesce a search typed during the debounce interval into one location.
            let mut previous = self.location_filters.clone();
            previous.search = self.filters.search.clone();
            if self.filter_changed.is_none() || previous != self.filters {
                self.navigation_back.push(self.location());
                if self.navigation_back.len() > 40 { self.navigation_back.remove(0); }
            }
            self.navigation_forward.clear();
            self.location_filters = self.filters.clone();
        }
        self.last_interaction = Instant::now();
        self.filter_changed = Some(Instant::now());
        self.selected = None;
        self.results_visible = true;
        self.clear_preview();
        self.pending_selection = false;
        self.yield_to_catalog_read();
    }
    fn clear_preview(&mut self) {
        self.specimen_canvas = None;
        self.specimen = None;
        self.source_text.clear();
        self.preview_error = None;
        self.local_design_error = None;
        self.inline_status.clear();
        self.inline_preview = None;
    }
    fn location(&self) -> Location {
        Location { filters: self.location_filters.clone(), selected: self.selected.clone(), detail_tab: self.detail_tab,
            theme_mode: self.theme_mode.clone(), results_visible: self.results_visible }
    }
    fn navigate_history(&mut self, forward: bool) {
        let location = if forward { self.navigation_forward.pop() } else { self.navigation_back.pop() };
        let Some(location) = location else { return; };
        let current = self.location();
        if forward { self.navigation_back.push(current); } else { self.navigation_forward.push(current); }
        self.clear_preview();
        self.filters = location.filters.clone();
        self.location_filters = location.filters;
        self.selected = location.selected;
        self.detail_tab = location.detail_tab;
        self.theme_mode = location.theme_mode;
        self.results_visible = location.results_visible;
        self.products_open = !self.filters.product.is_empty();
        self.pending_selection = self.selected.is_some();
        self.filter_changed = Some(Instant::now());
        self.last_interaction = Instant::now();
        self.yield_to_catalog_read();
    }
    fn select(&mut self, entry: Entry) {
        self.last_interaction = Instant::now();
        self.selected = Some(entry);
        self.results_visible = false;
        self.clear_preview();
        self.pending_selection = true;
        self.detail_tab = DetailTab::Preview;
        self.yield_to_catalog_read();
    }
    fn refresh_delay(&self, now: Instant) -> Duration {
        let interval = Duration::from_secs((60_u64 << self.refresh_failures.min(4)).min(900));
        interval.saturating_sub(now.saturating_duration_since(self.last_refresh))
            .max(Duration::from_secs(10).saturating_sub(now.saturating_duration_since(self.last_interaction)))
    }
    fn next_repaint_after(&self, now: Instant) -> Option<Duration> {
        if !self.open { return None; }
        if self.busy || self.pending_selection { return Some(ACTIVE_REPAINT); }
        if let Some(changed)=self.filter_changed { return Some(FILTER_DEBOUNCE.saturating_sub(now.saturating_duration_since(changed))); }
        if !self.auto_refresh || self.summary.is_none() { return None; }
        // Source notifications wake the UI through send_reply. This guard also
        // preserves the absolute refresh/backoff deadline rather than adding 5 s.
        Some(IDLE_REPAINT.min(self.refresh_delay(now)))
    }
    fn tick(&mut self) {
        self.poll();
        if self.busy {
            if self.filter_changed.is_some() || self.pending_selection { self.yield_to_catalog_read(); }
            return;
        }
        if let Some(changed) = self.filter_changed {
            if changed.elapsed() < FILTER_DEBOUNCE { return; }
            self.filter_changed = None;
            if self.summary.is_some() {
                self.query(0);
            }
        } else if self.pending_selection {
            self.pending_selection = false;
            if let Some(entry) = &self.selected {
                let Some(binding) = self.usable_generation().cloned() else {
                    self.preview_error = Some("Refresh the repository before loading this source configuration".into());
                    return;
                };
                self.cancelled.store(false, Ordering::Release);
                self.epoch += 1;
                self.busy = true;
                self.activity = "Loading preview";
                let _ = self.jobs.send(Job::Materialize(self.epoch, entry.id.clone(), binding));
            }
        } else if self.auto_refresh && self.source_changed && self.mode == "local" {
            self.source_changed = false;
            self.refresh();
        } else if self.auto_refresh
            && self.summary.is_some()
            && self.refresh_delay(Instant::now()).is_zero()
        {
            self.refresh();
        }
    }
}
