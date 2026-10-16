//! Temporary catalog views live outside App.doc and never enter editor save/history.
use desigma_cityos_catalog::{Client, Entry, Filters, Page, ReaderConfig};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
mod canvas;
mod preview;
#[cfg(test)]
pub(crate) mod tests;
mod ui;

#[derive(Clone, Copy, Default, PartialEq)]
enum DetailTab {
    #[default]
    Preview,
    Source,
    Inspector,
}

enum Job {
    Scan(u64, ReaderConfig, String, bool),
    Query(u64, Filters, usize),
    Materialize(u64, String),
}
enum Reply {
    Scan(u64, Result<Value, String>),
    Query(u64, Filters, usize, Result<Page, String>),
    Materialize(u64, String, Result<Value, String>),
    SourceChanged,
}
pub struct Browser {
    pub open: bool,
    pub workspace_request: Option<WorkspaceRequest>,
    pub workspace_theme: Option<String>,
    pub has_bound_draft: bool,
    pub host_diagnostic_request: bool,
    repository: String,
    reader: String,
    node: String,
    cache: Option<PathBuf>,
    canonical_ref: Option<String>,
    mode: String,
    filters: Filters,
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
    epoch: u64,
    busy: bool,
    summary: Option<Value>,
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
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        std::thread::spawn(move || {
            let mut client: Option<Client> = None;
            let mut previous_config: Option<ReaderConfig> = None;
            let mut previous_mode = String::new();
            loop {
                let job = match requests.recv_timeout(Duration::from_millis(100)) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if client.as_mut().is_some_and(|c| c.take_source_changed()) {
                            if responses.send(Reply::SourceChanged).is_err() {
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
                        let result = if previous_config.as_ref() == Some(&config) && previous_mode == mode && client.is_some() {
                            client.as_mut().unwrap().scan(&config, &mode)
                        } else {
                            client = None;
                            Client::connect_with_cancel(&config, worker_cancel.clone()).and_then(|mut c| {
                                let result = c.scan_with_cache(&config, &mode, prefer_cache);
                                client = Some(c);
                                result
                            })
                        };
                        previous_config = Some(config);
                        previous_mode = mode;
                        if result.is_err() {
                            client = None;
                            previous_config = None;
                        }
                        Reply::Scan(epoch, result)
                    }
                    Job::Query(epoch, filters, offset) => {
                        let result = client.as_mut().ok_or_else(|| "Refresh the repository first".to_string()).and_then(|c| c.query(&filters, offset));
                        Reply::Query(epoch, filters, offset, result)
                    }
                    Job::Materialize(epoch, id) => {
                        let result = client.as_mut().ok_or_else(|| "Refresh the repository first".to_string()).and_then(|c| c.materialize(&id));
                        Reply::Materialize(epoch, id, result)
                    }
                };
                if responses.send(reply).is_err() {
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
            has_bound_draft: false,
            host_diagnostic_request: false,
            repository: repository.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            reader: reader.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            node: node.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "node".into()),
            cache,
            canonical_ref,
            mode: "committed".into(),
            filters: Filters { collection: "design".into(), ..Default::default() },
            jobs,
            replies,
            epoch: 0,
            busy: false,
            summary: None,
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
            auto_refresh: false,
            specimen_canvas: None,
            theme_mode: "light".into(),
        };
        if open {
            browser.refresh();
        }
        browser
    }
    fn refresh(&mut self) {
        if self.busy {
            return;
        }
        self.cancelled.store(false, Ordering::Release);
        self.epoch += 1;
        self.busy = true;
        self.activity = if self.summary.is_some() { "Refreshing repository" } else { "Reading repository" };
        self.error = None;
        self.last_refresh = Instant::now();
        let config = ReaderConfig {
            node: self.node.clone().into(),
            reader: self.reader.clone().into(),
            repository: self.repository.clone().into(),
            cache: self.cache.clone(),
            canonical_ref: self.canonical_ref.clone(),
        };
        let _ = self.jobs.send(Job::Scan(self.epoch, config, self.mode.clone(), self.summary.is_none()));
    }
    fn query(&mut self, offset: usize) {
        if self.busy {
            return;
        }
        self.cancelled.store(false, Ordering::Release);
        self.epoch += 1;
        self.busy = true;
        self.activity = "Finding items";
        let _ = self.jobs.send(Job::Query(self.epoch, self.filters.clone(), offset));
    }
    fn poll(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            match reply {
                Reply::Scan(epoch, result) if epoch == self.epoch => {
                    self.busy = false;
                    self.last_refresh = Instant::now();
                    self.refresh_failures = if result.as_ref().is_ok_and(|summary| summary["stale"] != true || summary["restoredCache"] == true) {
                        0
                    } else {
                        self.refresh_failures.saturating_add(1)
                    };
                    match result {
                        Ok(summary) => {
                            if summary["snapshot"]["mode"].as_str() != Some(self.mode.as_str()) {
                                self.error = Some("The reader returned a different source mode. The previous catalog is still shown; refresh to retry.".into());
                            } else {
                                let changed = self.summary.as_ref().is_none_or(|old| old["generationId"] != summary["generationId"]);
                                self.summary = Some(summary);
                                if changed {
                                    self.specimen_canvas = None;
                                    self.specimen = None;
                                    self.source_text.clear();
                                    self.pending_selection = self.selected.is_some();
                                    self.query(0);
                                }
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Reply::Query(epoch, filters, offset, result) if epoch == self.epoch => {
                    self.busy = false;
                    match result {
                        Ok(page) if filters == self.filters => {
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
                        Err(e) => self.error = Some(e),
                    }
                }
                Reply::Materialize(epoch, id, result) if epoch == self.epoch => {
                    self.busy = false;
                    if self.selected.as_ref().is_none_or(|entry| entry.id != id) {
                        continue;
                    }
                    match result {
                        Ok(value) => {
                            // A refresh may have changed metadata for the same occurrence.
                            if let Ok(entry) = serde_json::from_value(value["entry"].clone()) {
                                self.selected = Some(entry);
                            }
                            self.source_text = preview::source_text(&value);
                            self.specimen = Some(value);
                            self.preview_error = None;
                        }
                        Err(e) => self.preview_error = Some(e),
                    }
                }
                Reply::SourceChanged => self.source_changed = true,
                _ => {}
            }
        }
    }
    fn filters_changed(&mut self) {
        self.last_interaction = Instant::now();
        self.filter_changed = Some(Instant::now());
        self.selected = None;
        self.specimen_canvas = None;
        self.specimen = None;
        self.source_text.clear();
        self.preview_error = None;
        self.pending_selection = false;
    }
    fn select(&mut self, entry: Entry) {
        self.last_interaction = Instant::now();
        self.selected = Some(entry);
        self.specimen_canvas = None;
        self.specimen = None;
        self.source_text.clear();
        self.preview_error = None;
        self.pending_selection = true;
        self.detail_tab = DetailTab::Preview;
    }
    fn tick(&mut self) {
        self.poll();
        if self.busy {
            return;
        }
        if self.filter_changed.is_some_and(|t| t.elapsed() >= Duration::from_millis(250)) {
            self.filter_changed = None;
            if self.summary.is_some() {
                self.query(0);
            }
        } else if self.pending_selection {
            self.pending_selection = false;
            if let Some(entry) = &self.selected {
                self.cancelled.store(false, Ordering::Release);
                self.epoch += 1;
                self.busy = true;
                self.activity = "Loading preview";
                let _ = self.jobs.send(Job::Materialize(self.epoch, entry.id.clone()));
            }
        } else if self.auto_refresh && self.source_changed && self.mode == "local" {
            self.source_changed = false;
            self.refresh();
        } else if self.auto_refresh
            && self.summary.is_some()
            && self.last_refresh.elapsed() >= Duration::from_secs((60_u64 << self.refresh_failures.min(4)).min(900))
            && self.last_interaction.elapsed() >= Duration::from_secs(10)
        {
            self.refresh();
        }
    }
}
