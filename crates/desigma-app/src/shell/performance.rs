//! Exploratory, opt-in native diagnostics. Never requests a repaint or starts a timer.
//! Live files can lag behind the last UI pass; only the drop flush is complete.

use egui::{Context, RepaintCause};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WRITE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_CAUSES: usize = 64;
const MAX_CAUSES_PER_FRAME: usize = 128;
const MAX_REPORT_BYTES: usize = 32 * 1024;
static NEXT_REPORT: AtomicU64 = AtomicU64::new(0);

/// Optional details are deliberately unknown until a parent-owned workspace getter supplies them.
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceSnapshot {
    pub open: bool,
    /// Existing inline_pending(): error-free busy OR pending OR waiting for renderer.
    pub combined_pending: bool,
    pub busy: Option<bool>,
    pub pending: Option<bool>,
    pub render_waiting: Option<bool>,
    pub current: Option<bool>,
    pub progress_repaint: Option<bool>,
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct State {
    pub workspace: WorkspaceSnapshot,
    pub fonts_loading: bool,
    pub autosave_waiting: bool,
    pub autosave_confirmed: bool,
    pub autosave_retry_scheduled: bool,
    pub pointer_down: bool,
    pub text_editing: bool,
    pub minimized: bool,
    pub catalog_open: bool,
    pub home_visible: bool,
    pub screenshot_active: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CauseCount {
    source: &'static str,
    line: u32,
    frames: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    schema_version: &'static str,
    evidence_class: &'static str,
    acceptance_benchmark: bool,
    root_process_id: u32,
    process_started_unix_ms: Option<u64>,
    report_started_unix_ms: u64,
    written_elapsed_ms: u64,
    finalized: bool,
    frame_count: u64,
    ui_pass_count: u64,
    first_frame_elapsed_ms: Option<u64>,
    last_frame_elapsed_ms: Option<u64>,
    last_frame_unix_ms: Option<u64>,
    max_frame_gap_ms: u64,
    /// <=16, <=100, <=250, <=1000, <=5000, >5000 ms, excluding the first frame.
    frame_gap_buckets: [u64; 6],
    root_cpu_seconds_at_start: Option<f64>,
    root_cpu_seconds_when_written: Option<f64>,
    root_cpu_delta_seconds: Option<f64>,
    active_state_frames: BTreeMap<&'static str, u64>,
    latest_frame_start_state: Option<State>,
    /// egui reports causes from its previous UI pass, not OS window-message attribution.
    repaint_causes_from_previous_pass: Vec<CauseCount>,
    omitted_cause_occurrences: u64,
    write_attempts: u64,
    write_failures: u64,
    flush_policy: &'static str,
    live_snapshot_limitation: &'static str,
}

pub(super) struct Reporter {
    path: PathBuf,
    temporary: PathBuf,
    started: Instant,
    last_attempt: Duration,
    last_frame_number: Option<u64>,
    report: Report,
}

impl Reporter {
    pub(super) fn start(path: PathBuf) -> std::io::Result<Self> {
        let mut name = path.file_name().ok_or_else(|| std::io::Error::other("report needs a filename"))?.to_os_string();
        name.push(format!(".desigma-perf-{}-{}.tmp", std::process::id(), NEXT_REPORT.fetch_add(1, Ordering::Relaxed)));
        let temporary = path.with_file_name(name);
        let (process_started_unix_ms, cpu) = root_sample();
        let mut reporter = Self {
            path, temporary, started: Instant::now(), last_attempt: Duration::ZERO, last_frame_number: None,
            report: Report {
                schema_version: "cityos.desigma.native-performance.v1",
                evidence_class: "exploratory-instrumented", acceptance_benchmark: false,
                root_process_id: std::process::id(), process_started_unix_ms,
                report_started_unix_ms: unix_ms(), written_elapsed_ms: 0, finalized: false,
                frame_count: 0, ui_pass_count: 0, first_frame_elapsed_ms: None,
                last_frame_elapsed_ms: None, last_frame_unix_ms: None, max_frame_gap_ms: 0,
                frame_gap_buckets: [0; 6], root_cpu_seconds_at_start: cpu,
                root_cpu_seconds_when_written: cpu, root_cpu_delta_seconds: cpu.map(|_| 0.0),
                active_state_frames: BTreeMap::new(), latest_frame_start_state: None,
                repaint_causes_from_previous_pass: Vec::with_capacity(MAX_CAUSES), omitted_cause_occurrences: 0,
                write_attempts: 0, write_failures: 0,
                flush_policy: "startup, first frame, then every 5s when a frame occurs, and drop; no wakeup timer",
                live_snapshot_limitation: "Unflushed frames may follow this live snapshot. Correlate root PID/start with the process sample; only finalized=true covers the complete diagnostic lifetime.",
            },
        };
        reporter.flush(Duration::ZERO, false)?;
        Ok(reporter)
    }

    pub(super) fn record(&mut self, ctx: &Context, state: State) {
        let frame = ctx.cumulative_frame_nr();
        // Do not clone egui's causes again during a layout's additional passes.
        let causes = if self.last_frame_number != Some(frame) { ctx.repaint_causes() } else { Vec::new() };
        self.observe(frame, state, &causes, self.started.elapsed(), unix_ms());
    }

    fn observe(&mut self, frame: u64, state: State, causes: &[RepaintCause], elapsed: Duration, now_unix_ms: u64) {
        self.report.ui_pass_count = self.report.ui_pass_count.saturating_add(1);
        if self.last_frame_number == Some(frame) {
            return;
        }
        self.last_frame_number = Some(frame);
        self.report.frame_count = self.report.frame_count.saturating_add(1);
        let at = millis(elapsed);
        if let Some(previous) = self.report.last_frame_elapsed_ms {
            let gap = at.saturating_sub(previous);
            self.report.max_frame_gap_ms = self.report.max_frame_gap_ms.max(gap);
            let bucket = [16, 100, 250, 1000, 5000].iter().position(|limit| gap <= *limit).unwrap_or(5);
            self.report.frame_gap_buckets[bucket] = self.report.frame_gap_buckets[bucket].saturating_add(1);
        }
        self.report.first_frame_elapsed_ms.get_or_insert(at);
        self.report.last_frame_elapsed_ms = Some(at);
        self.report.last_frame_unix_ms = Some(now_unix_ms);
        self.report.latest_frame_start_state = Some(state);
        for (name, active) in [
            ("workspaceOpen", Some(state.workspace.open)),
            ("workspaceCombinedPending", Some(state.workspace.combined_pending)),
            ("workspaceBusy", state.workspace.busy),
            ("workspacePending", state.workspace.pending),
            ("workspaceRenderWaiting", state.workspace.render_waiting),
            ("workspaceCurrent", state.workspace.current),
            ("workspaceProgressRepaint", state.workspace.progress_repaint),
            ("fontsLoading", Some(state.fonts_loading)),
            ("autosaveWaiting", Some(state.autosave_waiting)),
            ("autosaveConfirmed", Some(state.autosave_confirmed)),
            ("autosaveRetryScheduled", Some(state.autosave_retry_scheduled)),
            ("pointerDown", Some(state.pointer_down)),
            ("textEditing", Some(state.text_editing)),
            ("minimized", Some(state.minimized)),
            ("catalogOpen", Some(state.catalog_open)),
            ("homeVisible", Some(state.home_visible)),
            ("screenshotActive", Some(state.screenshot_active)),
        ] {
            if let Some(active) = active {
                let count = self.report.active_state_frames.entry(name).or_default();
                *count = count.saturating_add(u64::from(active));
            }
        }
        // Never retain reason strings or raw source paths: either can include identities/content.
        let mut seen = Vec::with_capacity(MAX_CAUSES_PER_FRAME);
        for cause in causes.iter().take(MAX_CAUSES_PER_FRAME) {
            let key = (source(cause.file), cause.line);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            if let Some(count) = self.report.repaint_causes_from_previous_pass.iter_mut().find(|c| (c.source, c.line) == key) {
                count.frames = count.frames.saturating_add(1);
            } else if self.report.repaint_causes_from_previous_pass.len() < MAX_CAUSES {
                self.report.repaint_causes_from_previous_pass.push(CauseCount { source: key.0, line: key.1, frames: 1 });
            } else {
                self.report.omitted_cause_occurrences = self.report.omitted_cause_occurrences.saturating_add(1);
            }
        }
        self.report.omitted_cause_occurrences = self.report.omitted_cause_occurrences.saturating_add(causes.len().saturating_sub(MAX_CAUSES_PER_FRAME) as u64);
        if self.report.write_failures < 3
            && (self.report.frame_count == 1 || elapsed.saturating_sub(self.last_attempt) >= WRITE_INTERVAL)
            && self.flush(elapsed, false).is_err()
            && self.report.write_failures == 1
        {
            eprintln!("desigma: performance report write failed; diagnostic writes are bounded");
        }
    }

    fn flush(&mut self, elapsed: Duration, finalized: bool) -> std::io::Result<()> {
        self.last_attempt = elapsed;
        self.report.written_elapsed_ms = millis(elapsed);
        self.report.finalized = finalized;
        self.report.write_attempts = self.report.write_attempts.saturating_add(1);
        self.report.root_cpu_seconds_when_written = root_sample().1;
        self.report.root_cpu_delta_seconds =
            self.report.root_cpu_seconds_when_written.zip(self.report.root_cpu_seconds_at_start).map(|(now, start)| (now - start).max(0.0));
        let result = self.write_atomic();
        if result.is_err() {
            self.report.write_failures = self.report.write_failures.saturating_add(1);
        }
        result
    }

    fn write_atomic(&self) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&self.report).map_err(std::io::Error::other)?;
        if bytes.len() > MAX_REPORT_BYTES {
            return Err(std::io::Error::other("performance report exceeds bound"));
        }
        // Create-new protects unrelated files. Only remove a temporary file we actually created.
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&self.temporary)?;
        let result = file.write_all(&bytes).and_then(|_| file.flush());
        drop(file);
        let result = result.and_then(|_| std::fs::rename(&self.temporary, &self.path));
        if result.is_err() {
            let _ = std::fs::remove_file(&self.temporary);
        }
        result
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        let _ = self.flush(self.started.elapsed(), true);
    }
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}
fn unix_ms() -> u64 {
    millis(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default())
}

fn source(file: &str) -> &'static str {
    // Fixed labels only; even an unknown absolute source path cannot escape into the report.
    let suffix = |unix: &str, windows: &str| file.ends_with(unix) || file.ends_with(windows);
    if suffix("design_workspace/mod.rs", "design_workspace\\mod.rs") {
        "workspace"
    } else if suffix("design_workspace/host.rs", "design_workspace\\host.rs") {
        "webview-ipc"
    } else if suffix("home/session.rs", "home\\session.rs") {
        "autosave"
    } else if suffix("canvas/tiles.rs", "canvas\\tiles.rs") {
        "canvas-tiles"
    } else if suffix("canvas/draw.rs", "canvas\\draw.rs") {
        "canvas-overlay"
    } else if suffix("canvas/gpu.rs", "canvas\\gpu.rs") {
        "canvas-gpu"
    } else if suffix("cityos_catalog/ui.rs", "cityos_catalog\\ui.rs") {
        "catalog"
    } else if suffix("widgets/spinner.rs", "widgets\\spinner.rs") {
        "egui-spinner"
    } else if file.ends_with("shell.rs") {
        "shell"
    } else if file.ends_with("present.rs") {
        "presentation"
    } else if file.ends_with("fonts.rs") {
        "fonts"
    } else if file.contains("egui-") || file.contains("egui/") || file.contains("egui\\") {
        "egui"
    } else if file.contains("eframe-") {
        "eframe"
    } else {
        "other"
    }
}

#[cfg(windows)]
fn root_sample() -> (Option<u64>, Option<f64>) {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    impl FileTime {
        fn ticks(&self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessTimes(process: *mut std::ffi::c_void, created: *mut FileTime, exited: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }
    let (mut created, mut exited, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    // SAFETY: the current-process pseudo-handle is valid and all output structs live for the call.
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return (None, None);
    }
    let epoch_ticks = 116_444_736_000_000_000;
    (Some(created.ticks().saturating_sub(epoch_ticks) / 10_000), Some((kernel.ticks().saturating_add(user.ticks())) as f64 / 10_000_000.0))
}
#[cfg(not(windows))]
fn root_sample() -> (Option<u64>, Option<f64>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Output(PathBuf);
    impl Output {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("desigma-native-performance-{}-{}.json", std::process::id(), NEXT_REPORT.fetch_add(1, Ordering::Relaxed))))
        }
        fn read(&self) -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(&self.0).unwrap()).unwrap()
        }
    }
    impl Drop for Output {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn performance_startup_and_drop_report_zero_frames_with_root_identity() {
        let output = Output::new();
        let reporter = Reporter::start(output.0.clone()).unwrap();
        let initial = output.read();
        assert_eq!(initial["rootProcessId"], std::process::id());
        assert_eq!(initial["frameCount"], 0);
        assert!(initial["lastFrameUnixMs"].is_null());
        assert_eq!(initial["finalized"], false);
        drop(reporter);
        let final_report = output.read();
        assert_eq!(final_report["frameCount"], 0);
        assert_eq!(final_report["finalized"], true);
        assert_eq!(final_report["acceptanceBenchmark"], false);
        #[cfg(windows)]
        assert!(final_report["processStartedUnixMs"].as_u64().is_some());
    }

    #[test]
    fn performance_counts_frames_separately_from_passes_and_throttles_writes() {
        let output = Output::new();
        let mut reporter = Reporter::start(output.0.clone()).unwrap();
        let state = State {
            fonts_loading: true,
            autosave_waiting: true,
            workspace: WorkspaceSnapshot { combined_pending: true, ..Default::default() },
            ..Default::default()
        };
        reporter.observe(1, state, &[], Duration::ZERO, 1000);
        reporter.observe(1, state, &[], Duration::from_millis(1), 1001);
        reporter.observe(2, state, &[], Duration::from_secs(4), 5000);
        assert_eq!(output.read()["frameCount"], 1, "live snapshots deliberately lag; no wakeup timer flushes them");
        reporter.observe(3, state, &[], Duration::from_secs(5), 6000);
        let report = output.read();
        assert_eq!(report["frameCount"], 3);
        assert_eq!(report["uiPassCount"], 4);
        assert_eq!(report["writeAttempts"], 3);
        assert_eq!(report["activeStateFrames"]["fontsLoading"], 3);
        assert_eq!(report["activeStateFrames"]["autosaveWaiting"], 3);
        assert_eq!(report["activeStateFrames"]["workspaceCombinedPending"], 3);
        assert!(report["latestFrameStartState"]["workspace"]["renderWaiting"].is_null());
    }

    #[test]
    fn performance_bounds_causes_and_omits_paths_reasons_and_document_data() {
        let output = Output::new();
        let mut reporter = Reporter::start(output.0.clone()).unwrap();
        let causes: Vec<_> = (0..1000)
            .map(|line| RepaintCause { file: "C:/Users/private-owner/private-document.rs", line, reason: "private document contents".into() })
            .collect();
        reporter.observe(1, State::default(), &causes, Duration::ZERO, 1);
        let text = std::fs::read_to_string(&output.0).unwrap();
        assert!(text.len() <= MAX_REPORT_BYTES);
        assert!(!text.contains("private-owner") && !text.contains("private-document") && !text.contains("private document contents"));
        assert_eq!(reporter.report.repaint_causes_from_previous_pass.len(), MAX_CAUSES);
        assert_eq!(reporter.report.omitted_cause_occurrences, 1000 - MAX_CAUSES as u64);
        let repeated = vec![RepaintCause { file: "crates/desigma-app/src/home/session.rs", line: 303, reason: "".into() }; 4];
        assert_eq!(source(repeated[0].file), "autosave");
        assert_eq!(source("crates\\desigma-app\\src\\design_workspace\\mod.rs"), "workspace");
    }

    #[test]
    fn performance_observer_never_requests_a_repaint_or_replaces_the_callback() {
        let output = Output::new();
        let mut reporter = Reporter::start(output.0.clone()).unwrap();
        let ctx = Context::default();
        for _ in 0..4 {
            let _ = ctx.run_ui(Default::default(), |_| {});
        }
        let wakes = std::sync::Arc::new(AtomicU64::new(0));
        let observed = wakes.clone();
        ctx.set_request_repaint_callback(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        let before = ctx.has_requested_repaint();
        reporter.record(&ctx, State::default());
        assert_eq!(ctx.has_requested_repaint(), before);
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
        ctx.request_repaint();
        assert!(wakes.load(Ordering::Relaxed) > 0, "the original integration callback is retained");
    }
}
