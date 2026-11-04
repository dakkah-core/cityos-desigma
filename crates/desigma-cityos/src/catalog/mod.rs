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
pub mod canvas;
mod style;
pub use style::{CatalogStyle, CatalogUiServices, SpecimenRenderer};
pub mod local_library;
mod observation;
mod preview;
mod support;
pub use observation::CoverageObservation;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;
mod tools;
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
    Restore(u64, ReaderConfig, String),
    Scan(u64, ReaderConfig, String, bool),
    Switch(u64, ReaderConfig, String),
    Query(u64, Filters, usize, CatalogBinding),
    Materialize(u64, String, CatalogBinding),
}

#[derive(Clone, Debug, PartialEq)]
struct CatalogBinding {
    config: ReaderConfig,
    mode: String,
    generation: String,
}

// Encode the complete canonical path, rather than using a collision-prone hash.
// Chunking also avoids a single path component exceeding filesystem limits.
fn repository_cache(root: Option<&std::path::Path>, repository: &std::path::Path, mode: &str) -> Result<Option<PathBuf>, String> {
    let Some(root) = root else { return Ok(None) };
    if !matches!(mode, "committed" | "local") {
        return Err("Unsupported catalog source mode".into());
    }
    let path = std::fs::canonicalize(repository).map_err(|e| format!("Repository is unavailable: {e}"))?;
    let path = path.to_str().ok_or("Repository path cannot be represented safely")?;
    let encoded: String = path.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
    let mut cache = root.join("repository-connections-v1").join(mode);
    for chunk in encoded.as_bytes().chunks(64) {
        cache.push(std::str::from_utf8(chunk).map_err(|_| "Invalid repository path encoding")?);
    }
    Ok(Some(cache))
}

fn scan_cancelled(error: &str) -> bool {
    matches!(error, "Catalog request cancelled; previous generation retained" | "CityOS background work cancelled; previous generation retained")
}

fn retain_cancelled_refresh(result: &Result<Value, String>, cancelled: bool, same_session: bool, had_generation: bool) -> bool {
    cancelled && same_session && had_generation && result.as_ref().is_err_and(|error| scan_cancelled(error))
}

// A reader response can establish an observed identity, but it does not prove
// the identity of the selected repository unless a trusted expectation exists.
fn switch_generation(config: &ReaderConfig, mode: &str, summary: &Value) -> Result<String, String> {
    let pin = config.canonical_ref.as_deref().map(|value| {
        if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("The canonical pin must be a full commit identity".to_string());
        }
        Ok(value.to_ascii_lowercase())
    }).transpose()?;
    desigma_cityos_catalog::federation::Binding::from_summary(
        "switch", "reader", mode, summary, None, pin.as_deref(),
    ).map(|binding| binding.generation_id)
      .map_err(|_| "The reader returned an invalid or mismatched source identity".into())
}

fn recover_previous_catalog(binding: &CatalogBinding, cancelled: Arc<AtomicBool>, refresh_error: &str) -> Result<(Client, Value), String> {
    if binding.config.cache.is_none() {
        return Err("Saved-library recovery is unavailable without a cache".into());
    }
    let mut client = Client::connect_with_cancel(&binding.config, cancelled)?;
    let mut summary = client.restore_cache(&binding.config, &binding.mode)?;
    if summary["generationId"].as_str() != Some(binding.generation.as_str()) || summary["snapshot"]["mode"].as_str() != Some(binding.mode.as_str()) {
        return Err("The saved library changed; refresh before reading a different generation".into());
    }
    summary["stale"] = Value::Bool(true);
    summary["refreshError"] = Value::String(refresh_error.into());
    summary["restoredAfterRefreshFailure"] = Value::Bool(true);
    Ok((client, summary))
}

fn read_catalog<T>(
    client: &mut Option<Client>,
    session: &mut Option<CatalogBinding>,
    fence: &mut bool,
    binding: &CatalogBinding,
    read: impl FnOnce(&mut Client) -> Result<T, String>,
) -> Result<T, String> {
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
    Switch(u64, Result<Value, (String, bool)>),
    Query(u64, Box<Filters>, usize, Result<Page, String>),
    Materialize(u64, String, Result<Value, String>),
    SourceChanged,
}

fn send_reply(responses: &Sender<Reply>, repaint: &Mutex<Option<egui::Context>>, reply: Reply) -> bool {
    if responses.send(reply).is_err() {
        return false;
    }
    // Notify only after queueing a result/event. Never wake egui on an empty poll.
    let context = repaint.lock().ok().and_then(|context| context.clone());
    if let Some(context) = context {
        context.request_repaint();
    }
    true
}
pub struct Browser {
    services: CatalogUiServices,
    pub open: bool,
    pub workspace_request: Option<WorkspaceRequest>,
    pub workspace_theme: Option<String>,
    pub local_design_request: Option<desigma_doc::Document>,
    pub local_design_error: Option<String>,
    pub has_bound_draft: bool,
    pub host_diagnostic_request: bool,
    pub inline_preview: Option<(WorkspaceRequest, egui::Rect)>,
    pub inline_status: String,
    inline_generation: u64,
    inline_display_key: Option<(WorkspaceIdentity, Value)>,
    repository: String,
    reader: String,
    node: String,
    cache: Option<PathBuf>,
    cache_root: Option<PathBuf>,
    draft_repository: String,
    draft_reader: String,
    draft_node: String,
    switching: Option<(ReaderConfig, String)>,
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
    specimen_canvas: Option<Box<dyn SpecimenRenderer>>,
    theme_mode: String,
    results_visible: bool,
    visual_results: bool,
    location_filters: Filters,
    navigation_back: Vec<Location>,
    navigation_forward: Vec<Location>,
    atlas_tools: tools::AtlasTools,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceIdentity {
    config: ReaderConfig,
    mode: String,
    binding: Option<CatalogBinding>,
}

#[cfg(test)]
impl WorkspaceIdentity {
    pub(crate) fn inline_test_identity() -> Self {
        Self { config: ReaderConfig { node: PathBuf::new(), reader: PathBuf::new(), repository: PathBuf::new(), cache: None, canonical_ref: None },
            mode: "committed".into(), binding: None }
    }
}

pub struct WorkspaceRequest {
    config: crate::bridge::Config,
    identity: WorkspaceIdentity,
    pub source_path: Option<String>,
    pub source_identity: Option<crate::workspace::CanonicalSourceIdentity>,
    pub authored_preview_identity: Option<Value>,
    inline_generation: u64,
    pub resource_pin: Option<Value>,
    pub preview_only: bool,
    pub resume: bool,
}
impl WorkspaceRequest {
    pub(crate) fn inline_key(&self) -> Option<(WorkspaceIdentity, Value)> {
        let selection = match (&self.resource_pin, &self.source_identity, &self.authored_preview_identity) {
            (Some(pin), None, None) => serde_json::json!({"pin":pin}),
            (None, Some(source), Some(authored)) => serde_json::json!({"sourcePath":source.source_path,"commit":source.commit,
                "sourceDigest":source.source_digest,"resourceId":source.resource_id,"authoredPreviewIdentity":authored}),
            _ => return None,
        };
        Some((self.identity.clone(), serde_json::json!({"selection":selection,"generation":self.inline_generation})))
    }
    pub fn config(&self) -> crate::bridge::Config {
        self.config.clone()
    }
    pub fn identity(&self) -> WorkspaceIdentity {
        self.identity.clone()
    }
    fn browse(config: crate::bridge::Config, identity: WorkspaceIdentity) -> Self {
        Self { config, identity, source_path: None, source_identity: None, authored_preview_identity: None, inline_generation: 0, resource_pin: None, preview_only: true, resume: false }
    }
    fn selected(config: crate::bridge::Config, identity: WorkspaceIdentity, source: &str, preview_only: bool) -> Self {
        Self { config, identity, source_path: Some(source.into()), source_identity: None, authored_preview_identity: None, inline_generation: 0, resource_pin: None, preview_only, resume: false }
    }
    fn pinned_preview(config: crate::bridge::Config, identity: WorkspaceIdentity, resource: Value) -> Self {
        Self { config, identity, source_path: resource["sourcePath"].as_str().map(str::to_owned), source_identity: None, authored_preview_identity: None, inline_generation: 0, resource_pin: Some(resource), preview_only: true, resume: false }
    }
}

impl Browser {
    fn workspace_identity(&self) -> WorkspaceIdentity {
        WorkspaceIdentity { config: self.reader_config(), mode: self.mode.clone(), binding: self.usable_generation().cloned() }
    }
    fn browse_request(&self, resume: bool) -> WorkspaceRequest {
        let mut request = WorkspaceRequest::browse(self.workspace_config(), self.workspace_identity());
        request.resume = resume;
        request.preview_only = !resume;
        request
    }
    fn selected_request(&self, source: &str, preview_only: bool) -> Option<WorkspaceRequest> {
        self.usable_generation()?;
        Some(WorkspaceRequest::selected(self.workspace_config(), self.workspace_identity(), source, preview_only))
    }
    fn composition_request(&self, source: &str, digest: &str, observation: &str, resource_id: Option<&str>, preview_only: bool) -> Option<WorkspaceRequest> {
        self.usable_generation()?;
        let snapshot = &self.summary.as_ref()?["snapshot"];
        if self.mode != "committed" || snapshot["mode"] != "committed" {
            return None;
        }
        let commit = snapshot["canonicalCommit"].as_str()?;
        if !snapshot["observations"].as_array()?.iter().any(|item| {
            item["id"] == observation && item["kind"] == "git" && item["revision"] == commit
        }) {
            return None;
        }
        let source_identity = crate::workspace::CanonicalSourceIdentity::new(source, commit, digest, resource_id)?;
        let mut request = WorkspaceRequest::selected(self.workspace_config(), self.workspace_identity(), source, preview_only);
        request.source_identity = Some(source_identity);
        Some(request)
    }
    // The materialized entry retains fields not represented by the typed list
    // row. This is a candidate for bridge verification, never a capability.
    fn authored_preview_request(&self, entry: &Entry) -> Option<WorkspaceRequest> {
        if entry.kind != "authored-composition" || entry.classification_status != "classified" {
            return None;
        }
        let specimen = self.specimen.as_ref().filter(|specimen| support::matches(entry, specimen))?;
        let selection = specimen.get("entry")?.get("previewAvailability")?.get("selection")?;
        let object = selection.as_object()?;
        for key in ["repositoryId", "commit", "sourcePath", "sourceDigest", "resourceId", "revision", "profileRef", "targetProfileRef", "componentId", "componentVersion", "adapterId", "adapterDigest", "rendererDigest", "packageIdentity", "referenceClosureDigest"] {
            object.get(key)?.as_str().filter(|value| !value.is_empty())?;
        }
        let snapshot = &self.summary.as_ref()?["snapshot"];
        if selection["repositoryId"] != snapshot["repositoryId"] || selection["commit"] != snapshot["canonicalCommit"]
            || selection["sourcePath"] != entry.source_path || selection["sourceDigest"] != entry.source_digest
            || selection["resourceId"] != entry.semantic_id {
            return None;
        }
        let mut request = self.composition_request(&entry.source_path, &entry.source_digest, &entry.observation_id, Some(&entry.semantic_id), true)?;
        request.authored_preview_identity = Some(selection.clone());
        Some(request)
    }
    fn navigation_composition_request(&self, child: &Value) -> Option<WorkspaceRequest> {
        let source = child["sourcePath"].as_str()?;
        let digest = child["sourceDigest"].as_str()?;
        let observation = child["observationId"].as_str()?;
        if child["classificationStatus"] != "classified" {
            return None;
        }
        let sources = child["sources"].as_array()?;
        if sources.is_empty() || !sources.iter().all(|item| {
            item["sourcePath"] == source && item["sourceDigest"] == digest && item["observationId"] == observation && item["classificationStatus"] == "classified"
        }) {
            return None;
        }
        self.composition_request(source, digest, observation, Some(child["id"].as_str()?), true)
    }
    fn pinned_request(&self, pin: Value) -> Option<WorkspaceRequest> {
        self.usable_generation()?;
        Some(WorkspaceRequest::pinned_preview(self.workspace_config(), self.workspace_identity(), pin))
    }
    fn request_is_current(&self, request: &WorkspaceRequest) -> bool {
        request.identity == self.workspace_identity()
    }
    pub fn take_workspace_request(&mut self) -> Option<WorkspaceRequest> {
        if self.switching.is_some() {
            self.workspace_request = None;
            self.workspace_theme = None;
            return None;
        }
        let request = self.workspace_request.take()?;
        if self.request_is_current(&request) {
            self.retry_inline_preview();
            Some(request)
        } else {
            self.workspace_theme = None;
            self.error = Some("Catalog connection or generation changed; select the resource again.".into());
            None
        }
    }
    pub fn take_inline_preview(&mut self) -> Option<(WorkspaceRequest, egui::Rect)> {
        if self.switching.is_some() {
            self.inline_preview = None;
            return None;
        }
        let (request, rect) = self.inline_preview.take()?;
        (self.request_is_current(&request) && request.inline_generation == self.inline_generation).then_some((request, rect))
    }
    fn prepare_inline_preview(&mut self, request: &mut WorkspaceRequest) -> bool {
        request.inline_generation = self.inline_generation;
        let Some(mut key) = request.inline_key() else { return false };
        if self.inline_display_key.as_ref() != Some(&key) {
            if self.inline_display_key.is_some() {
                self.inline_generation = self.inline_generation.wrapping_add(1);
                request.inline_generation = self.inline_generation;
                let Some(rekeyed) = request.inline_key() else { return false };
                key = rekeyed;
            }
            self.inline_status.clear();
            self.inline_display_key = Some(key);
        }
        true
    }
    fn queue_inline_preview(&mut self, mut request: WorkspaceRequest, rect: egui::Rect) {
        if !self.prepare_inline_preview(&mut request) { return; }
        self.inline_preview = Some((request, rect));
    }
    fn retry_inline_preview(&mut self) {
        self.inline_generation = self.inline_generation.wrapping_add(1);
        self.inline_display_key = None;
        self.inline_status.clear();
        self.inline_preview = None;
    }
    /// Reuse only the currently bound loaded generation. No query, scan or source read.
    pub fn coverage_observation(&self) -> Option<CoverageObservation> {
        self.usable_generation()?;
        CoverageObservation::from_summary(self.summary.as_ref()?, self.error.is_some())
    }
    pub fn workspace_config(&self) -> crate::bridge::Config {
        crate::bridge::Config::from_reader_options(
            Some(self.repository.clone().into()),
            Some(self.reader.clone().into()),
            Some(self.node.clone().into()),
            self.cache.clone(),
        )
    }
    pub fn new(
        repository: Option<PathBuf>,
        reader: Option<PathBuf>,
        node: Option<PathBuf>,
        cache: Option<PathBuf>,
        canonical_ref: Option<String>,
        services: CatalogUiServices,
    ) -> Self {
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
                        if client.as_mut().is_some_and(|c| c.take_source_changed()) && !send_reply(&responses, &worker_repaint, Reply::SourceChanged) {
                            break;
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                let reply = match job {
                    Job::Restore(epoch, config, mode) => {
                        let result = crate::jobs::run(true, Some(&worker_cancel), || {
                            client = None;
                            Client::connect_with_cancel(&config, worker_cancel.clone()).and_then(|mut current| {
                                let result = current.restore_cache(&config, &mode);
                                client = Some(current);
                                result
                            })
                        })
                        .and_then(|summary| {
                            if summary["snapshot"]["mode"].as_str() != Some(mode.as_str()) {
                                return Err("The reader returned a different source mode".into());
                            }
                            let generation = summary["generationId"].as_str().filter(|id| !id.is_empty()).ok_or("Missing generation identity")?;
                            session = Some(CatalogBinding { config, mode, generation: generation.into() });
                            fence = false;
                            Ok(summary)
                        });
                        if result.is_err() {
                            client = None;
                            session = None;
                            fence = false;
                        }
                        Reply::Scan(epoch, result)
                    }
                    Job::Scan(epoch, config, mode, prefer_cache) => {
                        // Preserve a healthy session for incremental refresh. Configuration
                        // changes replace the child and cannot reuse its old generations.
                        let same_session = session.as_ref().is_some_and(|binding| binding.config == config && binding.mode == mode) && client.is_some();
                        let had_generation = client
                            .as_ref()
                            .is_some_and(|c| session.as_ref().is_some_and(|binding| c.generation.as_deref() == Some(binding.generation.as_str())));
                        let previous = session.clone();
                        let result = crate::jobs::run(false, Some(&worker_cancel), || {
                            if same_session {
                                client.as_mut().unwrap().scan(&config, &mode)
                            } else {
                                client = None;
                                Client::connect_with_cancel(&config, worker_cancel.clone()).and_then(|mut c| {
                                    let result = c.scan_with_cache(&config, &mode, prefer_cache);
                                    client = Some(c);
                                    result
                                })
                            }
                        });
                        let mut result = result.and_then(|summary| {
                            if summary["snapshot"]["mode"].as_str() != Some(mode.as_str()) {
                                return Err("The reader returned a different source mode".into());
                            }
                            let generation = summary["generationId"].as_str().filter(|id| !id.is_empty()).ok_or("Missing generation identity")?;
                            session = Some(CatalogBinding { config, mode, generation: generation.into() });
                            fence = false;
                            Ok(summary)
                        });
                        if let Some(error) = result.as_ref().err().cloned() {
                            if same_session && had_generation && !worker_cancel.load(Ordering::Acquire) {
                                if let Some(binding) = previous.filter(|binding| binding.config.cache.is_some()) {
                                    // A failed remote refresh must not strand browsing. Reconnect
                                    // only to the exact validated saved generation; never rescan,
                                    // adopt a replacement pointer, or consult the remote here.
                                    client = None;
                                    match crate::jobs::run(true, Some(&worker_cancel), || recover_previous_catalog(&binding, worker_cancel.clone(), &error)) {
                                        Ok((recovered, summary)) => {
                                            client = Some(recovered);
                                            session = Some(binding);
                                            fence = false;
                                            result = Ok(summary);
                                        }
                                        Err(recovery_error) => result = Err(format!("{error}. Saved-library recovery failed: {recovery_error}")),
                                    }
                                }
                            }
                        }
                        if retain_cancelled_refresh(&result, worker_cancel.load(Ordering::Acquire), same_session, had_generation) {
                            fence = true;
                        } else if result.is_err() {
                            client = None;
                            session = None;
                            fence = false;
                        }
                        Reply::Scan(epoch, result)
                    }
                    Job::Switch(epoch, config, mode) => {
                        // Keep only one reader alive. A failed connection can recover
                        // the old generation only from its exact saved cache.
                        let previous = session.take();
                        client = None;
                        fence = false;
                        let result = crate::jobs::run(false, Some(&worker_cancel), || {
                            let mut candidate = Client::connect_with_cancel(&config, worker_cancel.clone())?;
                            let summary = candidate.scan_with_cache(&config, &mode, true)?;
                            let generation = switch_generation(&config, &mode, &summary)?;
                            session = Some(CatalogBinding { config, mode, generation });
                            client = Some(candidate);
                            Ok(summary)
                        });
                        let result = result.map_err(|error| {
                            let mut recovered = false;
                            if !worker_cancel.load(Ordering::Acquire) {
                                if let Some(binding) = previous.filter(|binding| binding.config.cache.is_some()) {
                                    if let Ok((restored, _)) = crate::jobs::run(true, Some(&worker_cancel), || recover_previous_catalog(&binding, worker_cancel.clone(), &error)) {
                                        client = Some(restored);
                                        session = Some(binding);
                                        recovered = true;
                                    }
                                }
                            }
                            (error, recovered)
                        });
                        Reply::Switch(epoch, result)
                    }
                    Job::Query(epoch, filters, offset, binding) => {
                        let result = read_catalog(&mut client, &mut session, &mut fence, &binding, |c| c.query(&filters, offset));
                        Reply::Query(epoch, Box::new(filters), offset, result)
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
        let reader = Some(crate::bridge::Config::reader_path(reader));
        let repository = repository.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let reader = reader.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let node = node.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "node".into());
        let scoped_cache = repository_cache(cache.as_deref(), std::path::Path::new(&repository), "committed").ok().flatten();
        let mut browser = Self {
            services,
            open,
            workspace_request: None,
            workspace_theme: None,
            local_design_request: None,
            local_design_error: None,
            has_bound_draft: false,
            host_diagnostic_request: false,
            inline_preview: None,
            inline_status: String::new(),
            inline_generation: 0,
            inline_display_key: None,
            draft_repository: repository.clone(),
            draft_reader: reader.clone(),
            draft_node: node.clone(),
            repository,
            reader,
            node,
            cache: scoped_cache,
            cache_root: cache,
            switching: None,
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
            atlas_tools: tools::AtlasTools::default(),
        };
        if open {
            browser.refresh();
        }
        browser
    }
    fn reader_config(&self) -> ReaderConfig {
        ReaderConfig {
            node: self.node.clone().into(),
            reader: self.reader.clone().into(),
            repository: self.repository.clone().into(),
            cache: self.cache.clone(),
            canonical_ref: self.canonical_ref.clone(),
        }
    }
    fn connect_draft(&mut self, mode: &str) {
        if self.busy { return; }
        let repository = match std::fs::canonicalize(self.draft_repository.trim()) {
            Ok(path) if path.is_dir() => path,
            Ok(_) => { self.error = Some("The selected repository is not a directory".into()); return; }
            Err(error) => { self.error = Some(format!("Repository is unavailable: {error}")); return; }
        };
        let cache = match repository_cache(self.cache_root.as_deref(), &repository, mode) {
            Ok(cache) => cache,
            Err(error) => { self.error = Some(error); return; }
        };
        let same_repository = std::fs::canonicalize(&self.repository).is_ok_and(|old| old == repository);
        let config = ReaderConfig {
            node: self.draft_node.trim().into(),
            reader: self.draft_reader.trim().into(),
            repository,
            cache,
            canonical_ref: if same_repository { self.canonical_ref.clone() } else { None },
        };
        if config == self.reader_config() && mode == self.mode {
            self.refresh();
            return;
        }
        // The host resumes a bound canvas using the active catalog config, but
        // does not expose its repository identity to this module. Do not retarget it.
        if self.has_bound_draft {
            self.error = Some("Connection switching is unavailable while a CityOS canvas draft is bound. Close the bound draft first.".into());
            return;
        }
        self.cancelled.store(false, Ordering::Release);
        self.epoch += 1;
        self.busy = true;
        self.activity = "Connecting repository";
        self.error = None;
        self.last_interaction = Instant::now();
        self.workspace_request = None;
        self.workspace_theme = None;
        self.inline_preview = None;
        self.switching = Some((config.clone(), mode.into()));
        if self.jobs.send(Job::Switch(self.epoch, config, mode.into())).is_err() {
            self.busy = false;
            self.switching = None;
            self.error = Some("Catalog worker is unavailable; previous view retained".into());
        }
    }
    fn usable_generation(&self) -> Option<&CatalogBinding> {
        self.catalog_binding.as_ref().filter(|binding| {
            binding.config == self.reader_config()
                && binding.mode == self.mode
                && self.summary.as_ref().is_some_and(|summary| {
                    summary["generationId"].as_str() == Some(binding.generation.as_str()) && summary["snapshot"]["mode"].as_str() == Some(binding.mode.as_str())
                })
        })
    }
    fn yield_to_catalog_read(&self) {
        if self.busy && self.activity == "Refreshing repository" && self.usable_generation().is_some() {
            // Keep the epoch and busy flag until Scan settles. Clearing cancellation
            // or queueing a read here could revive the scan while it is cancelling.
            self.cancelled.store(true, Ordering::Release);
        }
    }
    fn refresh(&mut self) {
        self.start_refresh(self.usable_generation().is_none() && self.cache.is_some());
    }
    fn start_refresh(&mut self, cache_only: bool) {
        if self.busy {
            return;
        }
        let prefer_cache = self.usable_generation().is_none();
        self.cancelled.store(false, Ordering::Release);
        self.epoch += 1;
        self.busy = true;
        self.activity = if cache_only {
            "Restoring saved library"
        } else if prefer_cache {
            "Reading repository"
        } else {
            "Refreshing repository"
        };
        self.error = None;
        self.last_refresh = Instant::now();
        let config = self.reader_config();
        self.scanning = Some((config.clone(), self.mode.clone()));
        let _ = self.jobs.send(if cache_only {
            Job::Restore(self.epoch, config, self.mode.clone())
        } else {
            Job::Scan(self.epoch, config, self.mode.clone(), prefer_cache)
        });
    }
    pub fn yield_to_design(&self) {
        if self.busy && matches!(self.activity, "Reading repository" | "Refreshing repository" | "Restoring saved library" | "Connecting repository") {
            self.cancelled.store(true, Ordering::Release);
        }
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
                Reply::Switch(epoch, result) if epoch == self.epoch => {
                    self.busy = false;
                    self.cancelled.store(false, Ordering::Release);
                    let Some((config, mode)) = self.switching.take() else { continue };
                    // Handoffs queued during the switch belong to an uncertain session,
                    // including when the old session was successfully recovered.
                    self.workspace_request = None;
                    self.workspace_theme = None;
                    self.inline_preview = None;
                    match result {
                        Ok(summary) => {
                            if self.has_bound_draft {
                                // The worker has already replaced its old session. Keep the
                                // old configuration but do not claim its binding is usable.
                                self.catalog_binding = None;
                                self.error = Some("Connection switch stopped because a CityOS canvas draft became bound. Close the bound draft, then reconnect to browse.".into());
                                continue;
                            }
                            let Some(generation) = summary["generationId"].as_str().filter(|id| !id.is_empty()) else {
                                self.catalog_binding = None;
                                self.error = Some("Connection returned no generation; previous view retained, reconnect to browse".into());
                                continue;
                            };
                            if summary["snapshot"]["mode"].as_str() != Some(mode.as_str()) {
                                self.catalog_binding = None;
                                self.error = Some("Connection returned a different source mode; previous view retained, reconnect to browse".into());
                                continue;
                            }
                            self.repository = config.repository.to_string_lossy().into_owned();
                            self.reader = config.reader.to_string_lossy().into_owned();
                            self.node = config.node.to_string_lossy().into_owned();
                            self.cache = config.cache.clone();
                            self.canonical_ref = config.canonical_ref.clone();
                            self.mode = mode.clone();
                            self.catalog_binding = Some(CatalogBinding { config, mode, generation: generation.into() });
                            self.summary = Some(summary);
                            self.entries.clear();
                            self.total = 0;
                            self.next = None;
                            self.selected = None;
                            self.clear_preview();
                            self.workspace_request = None;
                            self.workspace_theme = None;
                            self.local_design_request = None;
                            self.navigation_back.clear();
                            self.navigation_forward.clear();
                            self.pending_selection = false;
                            self.source_changed = false;
                            self.filter_changed = None;
                            self.results_visible = true;
                            self.refresh_failures = 0;
                            self.last_refresh = Instant::now();
                            self.error = None;
                            self.query(0);
                        }
                        Err((error, recovered)) => {
                            if !recovered { self.catalog_binding = None; }
                            self.error = Some(if recovered {
                                format!("Connection failed: {error}. Previous saved catalog restored.")
                            } else {
                                format!("Connection failed: {error}. Previous view retained; reconnect to browse it.")
                            });
                        }
                    }
                }
                Reply::Scan(epoch, result) if epoch == self.epoch => {
                    self.busy = false;
                    self.last_refresh = Instant::now();
                    let scan = self.scanning.take();
                    if self.activity == "Restoring saved library"
                        && !self.cancelled.load(Ordering::Acquire)
                        && scan.as_ref().is_some_and(|(config, mode)| *config == self.reader_config() && *mode == self.mode)
                        && result.as_ref().is_err_and(|error| error.starts_with("CACHE_RESTORE_UNAVAILABLE"))
                    {
                        // A cache miss/incompatible capture can only enter the
                        // pressure-aware background path, never this recovery job.
                        self.start_refresh(false);
                        continue;
                    }
                    let cancelled_refresh = self.cancelled.load(Ordering::Acquire)
                        && self.usable_generation().is_some()
                        && scan.as_ref().is_some_and(|(config, mode)| *config == self.reader_config() && *mode == self.mode)
                        && result.as_ref().is_err_and(|error| scan_cancelled(error));
                    // The worker has returned from Scan (including its cancellation
                    // handshake). Only now may an interactive read reset this flag.
                    self.cancelled.store(false, Ordering::Release);
                    if cancelled_refresh {
                        self.error = None;
                        continue;
                    }
                    self.refresh_failures = if result
                        .as_ref()
                        .is_ok_and(|summary| summary["refreshError"].is_null() && (summary["stale"] != true || summary["restoredCache"] == true))
                    {
                        0
                    } else {
                        self.refresh_failures.saturating_add(1)
                    };
                    match result {
                        Ok(summary) => {
                            if summary["snapshot"]["mode"].as_str() != Some(self.mode.as_str())
                                || scan.as_ref().is_some_and(|(config, mode)| *config != self.reader_config() || *mode != self.mode)
                            {
                                self.catalog_binding = None;
                                self.error = Some("The reader returned a different source mode. The previous catalog is still shown; refresh to retry.".into());
                            } else {
                                let Some(generation) = summary["generationId"].as_str().filter(|id| !id.is_empty()) else {
                                    self.catalog_binding = None;
                                    self.error = Some("Missing generation identity; refresh to retry".into());
                                    continue;
                                };
                                self.catalog_binding =
                                    Some(CatalogBinding { config: self.reader_config(), mode: self.mode.clone(), generation: generation.into() });
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
                        Err(e) => {
                            self.catalog_binding = None;
                            self.error = Some(e);
                        }
                    }
                }
                Reply::Query(epoch, filters, offset, result) if epoch == self.epoch => {
                    let filters = *filters;
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
                        Err(e) => {
                            self.catalog_binding = None;
                            self.error = Some(e);
                        }
                    }
                }
                Reply::Materialize(epoch, id, result) if epoch == self.epoch => {
                    self.busy = false;
                    if result.is_err() {
                        self.catalog_binding = None;
                    }
                    if let Err(error) = &result {
                        self.error = Some(error.clone());
                    }
                    if self.usable_generation().is_none() || self.selected.as_ref().is_none_or(|entry| entry.id != id) {
                        if let Err(error) = result {
                            self.preview_error = Some(error);
                        }
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
                        Err(e) => {
                            self.catalog_binding = None;
                            self.preview_error = Some(e);
                        }
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
                if self.navigation_back.len() > 40 {
                    self.navigation_back.remove(0);
                }
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
        self.inline_generation = self.inline_generation.wrapping_add(1);
        self.inline_display_key = None;
        self.specimen_canvas = None;
        self.specimen = None;
        self.source_text.clear();
        self.preview_error = None;
        self.local_design_error = None;
        self.inline_status.clear();
        self.inline_preview = None;
    }
    fn location(&self) -> Location {
        Location {
            filters: self.location_filters.clone(),
            selected: self.selected.clone(),
            detail_tab: self.detail_tab,
            theme_mode: self.theme_mode.clone(),
            results_visible: self.results_visible,
        }
    }
    fn navigate_history(&mut self, forward: bool) {
        let location = if forward { self.navigation_forward.pop() } else { self.navigation_back.pop() };
        let Some(location) = location else {
            return;
        };
        let current = self.location();
        if forward {
            self.navigation_back.push(current);
        } else {
            self.navigation_forward.push(current);
        }
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
        interval
            .saturating_sub(now.saturating_duration_since(self.last_refresh))
            .max(Duration::from_secs(10).saturating_sub(now.saturating_duration_since(self.last_interaction)))
    }
    fn next_repaint_after(&self, now: Instant) -> Option<Duration> {
        if !self.open {
            return None;
        }
        if self.busy || self.pending_selection {
            return Some(ACTIVE_REPAINT);
        }
        if let Some(changed) = self.filter_changed {
            return Some(FILTER_DEBOUNCE.saturating_sub(now.saturating_duration_since(changed)));
        }
        if !self.auto_refresh || self.summary.is_none() {
            return None;
        }
        // Source notifications wake the UI through send_reply. This guard also
        // preserves the absolute refresh/backoff deadline rather than adding 5 s.
        Some(IDLE_REPAINT.min(self.refresh_delay(now)))
    }
    fn tick(&mut self) {
        self.poll();
        if self.busy {
            if self.filter_changed.is_some() || self.pending_selection {
                self.yield_to_catalog_read();
            }
            return;
        }
        if let Some(changed) = self.filter_changed {
            if changed.elapsed() < FILTER_DEBOUNCE {
                return;
            }
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
        } else if self.auto_refresh && self.summary.is_some() && self.refresh_delay(Instant::now()).is_zero() {
            self.refresh();
        }
    }
}
