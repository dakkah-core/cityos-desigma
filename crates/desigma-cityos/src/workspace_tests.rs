//! Controller tests with owned sessions and an in-memory exchange transport.
//! These exercise the package boundary, not native persistence or owner acceptance.

use crate::bridge::Bridge;
use crate::draft::{binding, draft, revision};
use crate::editor::{BaselineInstall, BaselineRecovery, EditorHost, EditorSession, EditorStatus};
use crate::history::{CityosHistory, CityosHistoryStep};
use crate::workspace::{DraftTab, Workspace};
use desigma_doc::{Command, Document, DocumentCheckpoint, History};
use desigma_library::{SubmissionCommand, SubmissionRecord, SubmissionState};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

#[derive(Debug, PartialEq, Eq)]
enum Call {
    SaveBeforeSwitch,
    ReplaceSession,
    SwapSession,
    EnterDocument,
    OpenFiles,
    Save,
    Undo,
    Redo,
    Presentation,
    Edit,
    History,
    ReviewedBaseline,
    RecoverBaseline,
    RecoverSubmission,
}

// Deliberately not Clone: a successful switch must transfer this exact owner.
struct FakeSession {
    owner: Box<u8>,
    checkpoint: DocumentCheckpoint,
    file_id: String,
    path: Option<PathBuf>,
    dirty: bool,
    selected: String,
    autosave_generation: u64,
}

impl FakeSession {
    fn from_document(file_id: &str, document: Document) -> Self {
        Self {
            owner: Box::new(0),
            checkpoint: DocumentCheckpoint::new(document, History::default()),
            file_id: file_id.into(),
            path: None,
            dirty: true,
            selected: "root".into(),
            autosave_generation: 17,
        }
    }

    fn seeded(file_id: &str, draft_id: &str) -> Self {
        let mut session = Self::from_document(file_id, fixture_document(draft_id, 0, "flat"));
        session.path = Some(PathBuf::from(format!("{file_id}.desigma")));
        for (revision, elevation) in [(1, "resting"), (2, "raised")] {
            session
                .checkpoint
                .history
                .exec(&mut session.checkpoint.document, Command::CityosInstall { document: Box::new(fixture_document(draft_id, revision, elevation)) })
                .unwrap();
        }
        session.checkpoint.history.undo(&mut session.checkpoint.document).unwrap();
        assert!(session.checkpoint.history.can_undo());
        assert!(session.checkpoint.history.can_redo());
        session
    }

    fn snapshot(&self) -> Value {
        json!({
            "owner": (&*self.owner as *const u8) as usize,
            "checkpoint": self.checkpoint.to_json().unwrap(),
            "fileId": self.file_id,
            "path": self.path,
            "dirty": self.dirty,
            "selected": self.selected,
            "autosaveGeneration": self.autosave_generation,
        })
    }
}

impl EditorSession for FakeSession {
    fn name(&self) -> &str {
        &self.checkpoint.document.name
    }
    fn file_id(&self) -> Option<&str> {
        Some(&self.file_id)
    }
}

struct PendingBaseline {
    recovery: BaselineRecovery,
    checkpoint: DocumentCheckpoint,
}

struct FakeHost {
    active: FakeSession,
    calls: Vec<Call>,
    reject_preflight: bool,
    reject_transfer: bool,
    uncertain_baseline: bool,
    pending_baseline: Option<PendingBaseline>,
    readback: Option<BaselineRecovery>,
    recovery_requests: Vec<BaselineRecovery>,
    history_request: Option<CityosHistoryStep>,
    submission_frozen: Option<String>,
    retained_submission: Option<SubmissionRecord>,
}

impl FakeHost {
    fn new(active: FakeSession) -> Self {
        Self {
            active,
            calls: Vec::new(),
            reject_preflight: false,
            reject_transfer: false,
            uncertain_baseline: false,
            pending_baseline: None,
            readback: None,
            recovery_requests: Vec::new(),
            history_request: None,
            submission_frozen: None,
            retained_submission: None,
        }
    }
}

impl EditorHost for FakeHost {
    type Session = FakeSession;
    type Canvas = Document;
    type CanvasState = ();

    fn document(&self) -> &Document {
        &self.active.checkpoint.document
    }
    fn status(&self) -> EditorStatus {
        EditorStatus {
            library_available: true,
            library_file: true,
            external_path: self.active.path.clone(),
            dirty: self.active.dirty,
            frozen: self.submission_frozen.is_some(),
            can_undo: self.active.checkpoint.history.can_undo(),
            can_redo: self.active.checkpoint.history.can_redo(),
            history_pending: self.history_request.is_some(),
            ..Default::default()
        }
    }
    fn save_before_switch(&mut self) -> Result<(), String> {
        self.calls.push(Call::SaveBeforeSwitch);
        if self.reject_preflight {
            Err("preflight save rejected".into())
        } else {
            Ok(())
        }
    }
    fn save_to_draft_library(&mut self) -> Result<(), String> {
        Err("unexpected external library save".into())
    }
    fn replace_session(&mut self, document: Document, path: Option<PathBuf>) -> Result<Self::Session, String> {
        self.calls.push(Call::ReplaceSession);
        if self.reject_transfer {
            return Err("final outgoing save rejected".into());
        }
        let mut incoming = FakeSession::from_document("replacement-file", document);
        incoming.path = path;
        Ok(std::mem::replace(&mut self.active, incoming))
    }
    fn swap_session(&mut self, incoming: &mut Option<Self::Session>) -> Result<Self::Session, String> {
        self.calls.push(Call::SwapSession);
        if self.reject_transfer {
            return Err("final outgoing save rejected".into());
        }
        let incoming = incoming.take().ok_or("missing incoming session")?;
        Ok(std::mem::replace(&mut self.active, incoming))
    }
    fn enter_document(&mut self) {
        self.calls.push(Call::EnterDocument);
    }
    fn open_files(&mut self) -> Result<(), String> {
        self.calls.push(Call::OpenFiles);
        Ok(())
    }
    fn save(&mut self) {
        self.calls.push(Call::Save);
    }
    fn undo(&mut self) {
        self.calls.push(Call::Undo);
    }
    fn redo(&mut self) {
        self.calls.push(Call::Redo);
    }
    fn open_dialog(&mut self) {
        panic!("unexpected external open dialog");
    }
    fn export_portable(&mut self) -> Result<(), String> {
        Err("unexpected portable export".into())
    }
    fn autosave_tick(&mut self, _ctx: &egui::Context) {
        panic!("unexpected native autosave");
    }
    fn flush_autosave(&mut self) -> bool {
        panic!("unexpected native autosave flush");
    }
    fn toast(&mut self, _text: &str, _error: bool) {}
    fn select_source(&mut self, source: &str) {
        self.active.selected = source.into();
    }

    fn install_presentation(&mut self, document: Document) -> Result<(), String> {
        self.calls.push(Call::Presentation);
        self.active.checkpoint.document = document;
        Ok(())
    }
    fn install_edit(&mut self, document: Document) -> Result<(), String> {
        self.calls.push(Call::Edit);
        self.active
            .checkpoint
            .history
            .exec(&mut self.active.checkpoint.document, Command::CityosInstall { document: Box::new(document) })
            .map_err(|error| error.to_string())?;
        self.active.dirty = true;
        Ok(())
    }
    fn take_history_request(&mut self) -> Option<CityosHistoryStep> {
        self.history_request.take()
    }
    fn history_step_current(&self, step: &CityosHistoryStep) -> bool {
        self.active.checkpoint.history.cityos_step_current(self.document(), step)
    }
    fn install_history(&mut self, step: CityosHistoryStep, document: Document) -> Result<(), String> {
        self.calls.push(Call::History);
        self.active.checkpoint.history.commit_cityos(&mut self.active.checkpoint.document, step, document).map_err(|error| error.to_string())
    }
    fn install_reviewed_baseline(&mut self, document: Document) -> Result<BaselineInstall, String> {
        self.calls.push(Call::ReviewedBaseline);
        let value = draft(&document).ok_or("missing baseline draft")?;
        let recovery = BaselineRecovery {
            file_id: self.active.file_id.clone(),
            draft_id: value["draftId"].as_str().ok_or("missing draft identity")?.into(),
            revision: value["draftRevision"].as_u64().ok_or("missing revision")?,
            semantic_digest: value["semanticDigest"].as_str().ok_or("missing semantic digest")?.into(),
            binding_digest: desigma_library::payload_digest(&serde_json::to_vec(value).map_err(|error| error.to_string())?),
        };
        let mut checkpoint = self.active.checkpoint.clone();
        checkpoint.install_baseline(document);
        if self.uncertain_baseline {
            self.pending_baseline = Some(PendingBaseline { recovery: recovery.clone(), checkpoint });
            return Ok(BaselineInstall::Uncertain { recovery, message: "durable write outcome requires exact readback".into() });
        }
        self.active.checkpoint = checkpoint;
        self.active.dirty = false;
        Ok(BaselineInstall::Installed)
    }
    fn recover_reviewed_baseline(&mut self, recovery: &BaselineRecovery) -> Result<bool, String> {
        self.calls.push(Call::RecoverBaseline);
        self.recovery_requests.push(recovery.clone());
        let pending = self.pending_baseline.as_ref().ok_or("missing durable baseline intent")?;
        if &pending.recovery != recovery || self.readback.as_ref() != Some(recovery) {
            return Err("readback does not identify the exact pending generation".into());
        }
        let pending = self.pending_baseline.take().unwrap();
        self.active.checkpoint = pending.checkpoint;
        self.active.dirty = false;
        Ok(true)
    }

    fn dependencies_complete(&self, _document: &Document, _validate_history: bool) -> Result<bool, String> {
        Ok(true)
    }
    fn validate_history_dependencies(&self) -> Result<(), String> {
        Ok(())
    }
    fn retain_dependencies(&self, _cache: &Path, _draft: &Value, _response: &Value) -> Result<(), String> {
        Err("unexpected dependency IO".into())
    }
    fn submission_key(&self) -> Option<(PathBuf, String, String)> {
        None
    }
    fn submissions(&self) -> Result<Vec<SubmissionRecord>, String> {
        Ok(Vec::new())
    }
    fn set_submission_frozen(&mut self, draft_id: Option<String>) {
        self.submission_frozen = draft_id;
    }
    fn recover_submission(&mut self, record: &SubmissionRecord) -> Result<bool, String> {
        self.calls.push(Call::RecoverSubmission);
        if self.retained_submission.as_ref() == Some(record) {
            // The original local record exists but still needs projection readback.
            Ok(false)
        } else {
            Err("unexpected owner recovery".into())
        }
    }
    fn install_accepted(&mut self, _record: &SubmissionRecord, _document: Document) -> Result<(), String> {
        Err("unexpected accepted-submission install".into())
    }
    fn temporary_canvas(document: Document) -> Self::Canvas {
        document
    }
    fn select_temporary_source(_canvas: &mut Self::Canvas, _source: &str) {}
    fn draw_canvas(&mut self, _ui: &mut egui::Ui, _native_scene: bool, _state: &mut ()) -> Option<String> {
        panic!("unexpected native canvas rendering");
    }
    fn draw_temporary_canvas(_canvas: &mut Self::Canvas, _ui: &mut egui::Ui, _native_scene: bool, _state: &mut ()) -> Option<String> {
        panic!("unexpected temporary canvas rendering");
    }
    fn record_widget(_ui: &egui::Ui, _id: &str, _rect: egui::Rect) {}
    fn workspace_tools(_ui: &mut egui::Ui) {}
    fn workspace_tools_feedback(_ui: &mut egui::Ui) {}
}

fn fixture_bundle(draft_id: &str, revision: u64, elevation: &str) -> Value {
    let base = json!({"id":"resource", "root":{"id":"root", "props":{"elevation":"flat"}}});
    let semantic = json!({"id":"resource", "root":{"id":"root", "props":{"elevation":elevation}}});
    let digest = desigma_library::payload_digest(&serde_json::to_vec(&semantic).unwrap());
    json!({
        "historyBoundary": true,
        "resource": {"resourceId":"resource", "label":"Controller fixture", "fixture":"controller-fixture", "editable":true},
        "draft": {
            "schemaVersion":"cityos.design-exchange.v3",
            "documentId":format!("document-{draft_id}"), "draftId":draft_id, "draftRevision":revision,
            "base":{"kind":"canonical-git", "repositoryId":"fixture-repository", "commit":"a".repeat(40)},
            "semanticContract":"cityos.controller-fixture.v1", "semanticBase":base,
            "semanticDraft":semantic, "semanticDigest":digest, "dependencyPins":[],
            "commandReceipts":[{"operationId":format!("fixture-{revision}"), "revision":revision, "requestDigest":"b".repeat(64)}]
        },
        "projection": {
            "schemaVersion":"cityos.desigma-structure.v1", "draftId":draft_id,
            "draftRevision":revision, "semanticDigest":digest,
            "nodes":[{"id":"occurrence-root", "sourceNodeId":"root", "definitionId":"fixture-definition",
                "label":format!("Projection {elevation}"), "x":0, "y":0, "width":320, "height":200, "fill":"#ffffff"}]
        }
    })
}

fn fixture_document(draft_id: &str, revision: u64, elevation: &str) -> Document {
    let bundle = fixture_bundle(draft_id, revision, elevation);
    binding(bundle["draft"].clone(), &bundle["projection"]).unwrap()
}

fn ready_workspace() -> Workspace<FakeHost> {
    Workspace {
        open: true,
        validated: true,
        selected: "root".into(),
        resource: Some(fixture_bundle("active", 0, "flat")["resource"].clone()),
        leave_to_library: false,
        ..Default::default()
    }
}

fn transport(workspace: &mut Workspace<FakeHost>) -> (Receiver<Value>, Sender<Result<Value, String>>) {
    let (bridge, requests, responses) = Bridge::test_transport();
    workspace.bridge = Some(bridge);
    (requests, responses)
}

fn property_field() -> Value {
    json!({"nodeId":"root", "property":"elevation"})
}

#[test]
fn redo_keyboard_chord_is_consumed_before_the_less_specific_undo_chord() {
    use crate::workspace_support::history_commands::{take_shortcut, Shortcut};
    for (modifiers, expected) in [
        (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Some(Shortcut::Redo)),
        (egui::Modifiers::COMMAND, Some(Shortcut::Undo)),
        (egui::Modifiers::SHIFT, None),
        (egui::Modifiers::NONE, None),
    ] {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers }],
            ..Default::default()
        };
        let mut observed = None;
        let _ = ctx.run_ui(input, |ui| {
            let step = ui.ctx().input_mut(take_shortcut);
            if step.is_some() {
                observed = step;
            }
            assert_eq!(ui.ctx().input_mut(take_shortcut), None, "one event cannot trigger both directions");
        });
        assert_eq!(observed, expected);
    }
}

#[test]
fn coverage_is_explicit_coalesced_read_only_and_keeps_draft_and_preview_state() {
    let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
    let before = host.active.snapshot();
    let mut workspace = ready_workspace();
    workspace.current = true;
    workspace.preview = Some(json!({"requestId":"current-preview"}));
    let preview = workspace.preview.clone();
    let resource = workspace.resource.clone();
    let (requests, responses) = transport(&mut workspace);
    assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    workspace.request_coverage(false);
    assert_eq!(requests.try_recv().unwrap(), json!({"op":"SemanticCoverage","summaryOnly":true,"locale":"ar"}));
    workspace.request_coverage(false);
    assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    let report = json!({"installedCoverage":{"schemaVersion":"cityos.design-workspace.installed-coverage.v1",
        "packageIdentity":"test-package","ownerWrites":false,"qualification":"unqualified",
        "levels":(0..10).map(|level|json!({"level":format!("L{level}"),"status":"unscanned"})).collect::<Vec<_>>()}});
    responses.send(Ok(report.clone())).unwrap();
    workspace.poll(&mut host);
    assert_eq!(workspace.coverage.report, Some(report["installedCoverage"].clone()));
    assert!(workspace.coverage.error.is_none());
    workspace.request_coverage(false); // Opening a loaded view is not background polling.
    assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    workspace.request_coverage(true);
    assert_eq!(requests.try_recv().unwrap()["op"], "SemanticCoverage");
    responses.send(Err("Incompatible old helper".into())).unwrap();
    workspace.poll(&mut host);
    assert_eq!(workspace.coverage.error.as_deref(), Some("Incompatible old helper"));
    assert_eq!(workspace.coverage.report, Some(report["installedCoverage"].clone()));
    assert!(workspace.error.is_none());
    assert!(workspace.current);
    assert_eq!(workspace.preview, preview);
    assert_eq!(workspace.resource, resource);
    assert_eq!(host.active.snapshot(), before);
    assert!(host.calls.is_empty());
}

#[test]
fn catalog_coverage_never_rebases_or_grants_capabilities_and_context_removal_clears_it() {
    let host = FakeHost::new(FakeSession::seeded("active-file", "active"));
    let before = host.active.snapshot();
    let mut workspace = ready_workspace();
    workspace.preview = Some(json!({"draftRevision":5,"requestId":"retained-preview"}));
    workspace.current = true;
    let resource = workspace.resource.clone();
    let (requests, _) = transport(&mut workspace);
    let catalog = crate::catalog::CoverageObservation {
        repository_id: "b".repeat(64),
        generation_id: "a".repeat(64),
        canonical_commit: "c".repeat(40),
        reader_identity: "d".repeat(64),
        mode: "local".into(),
        accounting_complete: Some(false),
        stale: true,
        source_occurrences: Some(500),
        library_records: Some(40),
        theme_records: Some(3),
        token_records: Some(31),
        levels: vec![],
    };
    workspace.set_catalog_coverage(Some(catalog.clone()));
    assert_eq!(workspace.coverage.catalog, Some(catalog));
    assert!(workspace.coverage.report.is_none());
    assert!(!workspace.coverage_open());
    assert!(workspace.current);
    assert_eq!(workspace.preview.as_ref().unwrap()["draftRevision"], 5);
    assert_eq!(workspace.resource, resource);
    assert_eq!(host.active.snapshot(), before);
    assert!(host.calls.is_empty());
    assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    workspace.set_catalog_coverage(None);
    assert!(workspace.coverage.catalog.is_none());
    assert_eq!(host.active.snapshot(), before);
}

#[test]
fn coverage_cannot_overlap_commands_or_bypass_recovery_and_is_available_for_temporary_specimens() {
    let mut workspace = ready_workspace();
    let (requests, _) = transport(&mut workspace);
    workspace.busy = Some("Command".into());
    workspace.request_coverage(true);
    assert_eq!(workspace.busy.as_deref(), Some("Command"));
    assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    workspace.busy = None;
    workspace.recovery_pending = true;
    workspace.request_coverage(true);
    assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    workspace.recovery_pending = false;
    workspace.preview_only = true;
    workspace.resource = Some(json!({"kind":"registered-specimen"}));
    workspace.request_coverage(true);
    assert_eq!(requests.try_recv().unwrap()["op"], "SemanticCoverage");
    assert!(workspace.error.is_none());
    assert!(workspace.staged.is_none());
    assert!(workspace.temporary_canvas.is_none());
}

#[test]
fn installed_commands_retire_previous_comparison_and_refresh_changes_for_the_new_revision() {
    for changes_mode in [false, true] {
        let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
        let mut workspace = ready_workspace();
        workspace.changes_mode = changes_mode;
        workspace.compare = Some(json!({"operationCount":1}).to_string());
        workspace.compare_open = true;
        let (requests, responses) = transport(&mut workspace);
        workspace.send_property_command(&host, &property_field(), json!("flat"));
        assert_eq!(requests.try_recv().unwrap()["op"], "Command");
        let next_revision = revision(host.document()).unwrap().1 + 1;
        let next = fixture_bundle("active", next_revision, "flat");
        responses.send(Ok(next.clone())).unwrap();
        workspace.poll(&mut host);
        assert_eq!(draft(host.document()), Some(&next["draft"]));
        assert!(workspace.compare.is_none(), "a prior export cannot describe the installed revision");
        assert!(!workspace.compare_open);
        if changes_mode {
            let request = requests.try_recv().expect("Changes should compare the installed revision automatically");
            assert_eq!(request, json!({"op":"Export","draft":next["draft"]}));
            responses.send(Ok(json!({"operationCount":0}))).unwrap();
            workspace.poll(&mut host);
            assert_eq!(serde_json::from_str::<Value>(workspace.compare.as_ref().unwrap()).unwrap()["operationCount"], 0);
            assert!(!workspace.compare_open);
        }
        assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    }
}

#[test]
fn hidden_prepared_preview_has_no_ack_deadline_even_before_a_child_exists() {
    use std::time::Instant;
    let mut workspace = ready_workspace();
    let started = Instant::now();
    let artifact = json!({"requestId":"prepared-without-child","draftRevision":1});
    workspace.preview = Some(artifact.clone());
    workspace.render_started = Some(started);
    workspace.pending = Some(started);
    assert!(workspace.host.url.is_none());

    workspace.release_inactive_preview();
    workspace.expire_render_acknowledgement(started + crate::repaint::RENDER_ACK_TIMEOUT);

    assert_eq!(workspace.preview, Some(artifact));
    assert_eq!(workspace.pending, Some(started));
    assert!(workspace.render_started.is_none() && workspace.error.is_none());
    assert!(!workspace.current);
}

#[test]
fn changes_mode_defers_preview_work_and_resumes_the_latest_draft_when_visible() {
    use std::time::{Duration, Instant};
    let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
    let mut workspace = ready_workspace();
    let (requests, _responses) = transport(&mut workspace);
    workspace.resource.as_mut().unwrap()["actualWebPreview"] = json!(true);
    workspace.themes_loaded = true;
    workspace.observed = revision(host.document());
    workspace.changes_mode = true;
    let started = Instant::now() - Duration::from_secs(31);
    workspace.pending = Some(started);
    workspace.render_started = Some(started);

    workspace.poll(&mut host);

    assert!(requests.try_recv().is_err(), "Changes must not dispatch a hidden renderer job");
    assert!(workspace.pending.is_some(), "Keep the latest draft pending for Design");
    assert!(workspace.error.is_none() && workspace.render_started.is_none());
    assert!(workspace.progress_repaint_delay().is_none(), "Hidden previews must not poll");

    workspace.changes_mode = false;
    workspace.poll(&mut host);
    let request = requests.try_recv().expect("Visible Design resumes the pending preview");
    assert_eq!(request["op"], "Preview");
    assert_eq!(request["draft"]["draftRevision"], draft(host.document()).unwrap()["draftRevision"]);
    assert!(workspace.pending.is_none());
}

fn inert_config() -> crate::bridge::Config {
    crate::bridge::Config {
        node: "fixture-only/not-a-node-executable".into(),
        bridge: "fixture-only/not-a-bridge.mjs".into(),
        repository: "fixture-only/not-a-repository".into(),
        cache: "fixture-only/not-a-cache".into(),
    }
}

// Synthetic retained DTO only; it is never authenticated, persisted or sent to an owner.
fn accepted_record(document: &Document) -> SubmissionRecord {
    let binding = document.cityos.as_ref().unwrap();
    let command = SubmissionCommand {
        intent_id: "fixture-original-intent".into(),
        operation_id: "fixture-original-operation".into(),
        predecessor: None,
        owner: "fixture-owner".into(),
        resource: "resource".into(),
        scope: [("tenantId".into(), "fixture-tenant".into()), ("actorId".into(), "fixture-actor".into())].into(),
        base: binding.base.clone(),
        expected_revision: "d".repeat(64),
        semantic_base: binding.semantic_base.clone(),
        submitted_snapshot: draft(document).unwrap()["semanticDraft"].clone(),
        dependency_pins: Vec::new(),
    };
    let request_digest = desigma_library::payload_digest(&serde_json::to_vec(&command).unwrap());
    let outcome = json!({
        "operation_id":command.operation_id, "request_digest":request_digest,
        "owner":command.owner, "resource":command.resource, "scope":command.scope,
        "state":"accepted-sync-pending", "receipt_ref":"fixture-original-receipt",
        "owner_revision":"b".repeat(40), "conclusively_ineligible":false,
        "details":{"syntheticFixture":true}
    });
    SubmissionRecord {
        schema_version: "cityos.submission.v1".into(),
        command,
        request_digest,
        state: SubmissionState::AcceptedSyncPending,
        outcome: Some(outcome),
    }
}

#[test]
fn rejected_final_session_switch_retains_active_and_incoming_owners() {
    let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
    host.reject_transfer = true;
    let mut workspace = ready_workspace();
    let active = host.active.snapshot();
    let incoming = FakeSession::seeded("incoming-file", "incoming");
    let incoming_snapshot = incoming.snapshot();
    let mut tab = workspace.retained_tab(incoming);
    tab.locale = "en".into();
    tab.theme = "dark".into();
    tab.theme_id = "incoming-theme".into();
    tab.interactive = true;
    workspace.tabs.insert("incoming".into(), tab);
    let resource = workspace.resource.clone();

    workspace.activate_tab(&mut host, "incoming");

    assert_eq!(host.calls, [Call::SaveBeforeSwitch, Call::SwapSession]);
    assert_eq!(host.active.snapshot(), active);
    assert_eq!(workspace.tabs.len(), 1);
    let retained = &workspace.tabs["incoming"];
    assert_eq!(retained.session.as_ref().unwrap().snapshot(), incoming_snapshot);
    assert_eq!((retained.locale.as_str(), retained.theme.as_str(), retained.theme_id.as_str()), ("en", "dark", "incoming-theme"),);
    assert!(retained.interactive);
    assert_eq!(workspace.resource, resource);
    assert_eq!((&*workspace.locale, &*workspace.theme), ("ar", "light"));
    assert_eq!(workspace.error.as_deref(), Some("final outgoing save rejected"));
}

#[test]
fn failed_preflight_never_passes_the_incoming_slot_to_the_host() {
    let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
    host.reject_preflight = true;
    let mut workspace = ready_workspace();
    let active = host.active.snapshot();
    let incoming = FakeSession::seeded("incoming-file", "incoming");
    let incoming_snapshot = incoming.snapshot();
    workspace.tabs.insert("incoming".into(), workspace.retained_tab(incoming));

    workspace.activate_tab(&mut host, "incoming");

    assert_eq!(host.calls, [Call::SaveBeforeSwitch]);
    assert_eq!(host.active.snapshot(), active);
    assert_eq!(workspace.tabs["incoming"].session.as_ref().unwrap().snapshot(), incoming_snapshot);
    assert_eq!(workspace.error.as_deref(), Some("preflight save rejected"));
}

#[test]
fn successful_session_switch_moves_history_and_restores_tab_metadata() {
    let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
    let mut workspace = ready_workspace();
    workspace.changes_mode = true;
    workspace.incoming = Some(json!({"pending":"outgoing review"}));
    let outgoing_snapshot = host.active.snapshot();
    let outgoing_resource = workspace.resource.clone();
    let incoming = FakeSession::seeded("incoming-file", "incoming");
    let incoming_snapshot = incoming.snapshot();
    let incoming_resource = json!({"resourceId":"incoming-resource"});
    workspace.tabs.insert(
        "incoming".into(),
        DraftTab {
            session: Some(incoming),
            resource: Some(incoming_resource.clone()),
            locale: "en".into(),
            theme: "dark".into(),
            theme_id: "incoming-theme".into(),
            interactive: true,
            selected: "root".into(),
        },
    );

    workspace.activate_tab(&mut host, "incoming");

    assert_eq!(host.calls, [Call::SaveBeforeSwitch, Call::SwapSession]);
    assert_eq!(host.active.snapshot(), incoming_snapshot);
    assert!(!workspace.tabs.contains_key("incoming"));
    assert_eq!(workspace.tabs.len(), 1);
    let outgoing = &workspace.tabs["active"];
    assert_eq!(outgoing.session.as_ref().unwrap().snapshot(), outgoing_snapshot);
    assert_eq!(outgoing.resource, outgoing_resource);
    assert_eq!(workspace.resource, Some(incoming_resource));
    assert_eq!((&*workspace.locale, &*workspace.theme, &*workspace.theme_id), ("en", "dark", "incoming-theme"));
    assert!(workspace.interactive);
    assert_eq!(workspace.selected, "root");
    assert!(!workspace.changes_mode);
    assert!(workspace.incoming.is_none());
    assert!(workspace.error.is_none());
}

#[test]
fn replacing_a_document_retains_the_outgoing_session_only_after_host_success() {
    for reject in [true, false] {
        let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
        host.reject_transfer = reject;
        let mut workspace = ready_workspace();
        let outgoing = host.active.snapshot();
        let path = PathBuf::from("replacement.desigma");

        let result = workspace.replace_active(&mut host, fixture_document("replacement", 0, "flat"), Some(path.clone()));

        assert_eq!(host.calls, [Call::SaveBeforeSwitch, Call::ReplaceSession]);
        if reject {
            assert!(result.is_err());
            assert_eq!(host.active.snapshot(), outgoing);
            assert!(workspace.tabs.is_empty());
        } else {
            result.unwrap();
            assert_eq!(revision(host.document()), Some(("replacement".into(), 0)));
            assert_eq!(host.active.path, Some(path));
            assert_eq!(workspace.tabs["active"].session.as_ref().unwrap().snapshot(), outgoing);
        }
    }
}

fn assert_recovery_fences(workspace: &mut Workspace<FakeHost>, host: &mut FakeHost, requests: &Receiver<Value>) {
    let calls = host.calls.len();
    let active = host.active.snapshot();
    let incoming = workspace.tabs["inactive"].session.as_ref().unwrap().snapshot();
    let recovery = workspace.baseline_recovery.clone();
    let bundle = workspace.reconciliation_bundle.clone();
    let resource = workspace.resource.clone();
    let generation = workspace.preview_generation;
    assert!(workspace.recovery_pending);
    assert!(!workspace.validated);
    // Leave status.frozen false: these checks must exercise controller recovery,
    // rather than accidentally passing through a separate submission freeze.
    assert!(!host.status().frozen);
    workspace.send_property_command(host, &property_field(), json!("raised"));
    host.history_request = host.active.checkpoint.history.prepare_cityos(host.document(), false).unwrap();
    assert!(host.history_request.is_some());
    workspace.dispatch_history_request(host);
    for operation in ["Command", "Reconcile", "AcceptedProjection", "Dependencies"] {
        workspace.send(json!({"op":operation, "draft":draft(host.document())}));
        assert!(workspace.error.is_some());
        assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)), "{operation} escaped the reviewed-baseline fence");
    }
    for boundary in ["begin_request", "begin_harness", "resume"] {
        workspace.error = None;
        match boundary {
            "begin_request" => workspace.begin_request(inert_config(), Some("another-source".into()), true, Some(json!({"resourceId":"another"}))),
            "begin_harness" => workspace.begin_harness(inert_config()),
            "resume" => workspace.resume(inert_config(), host),
            _ => unreachable!(),
        }
        assert!(workspace.error.is_some(), "{boundary} must reject while recovery is pending");
        assert_eq!(workspace.baseline_recovery, recovery, "{boundary} lost the durable identity");
        assert_eq!(workspace.reconciliation_bundle, bundle, "{boundary} lost the retained bundle");
        assert_eq!(workspace.resource, resource);
        assert_eq!(workspace.preview_generation, generation);
        assert!(workspace.config.is_none(), "{boundary} replaced the controller configuration");
        assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)), "{boundary} replaced or dispatched the transport");
    }
    assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    assert!(workspace.busy.is_none());
    assert!(workspace.history_command.is_none());
    assert!(workspace.prepare_switch(host, true).is_err());
    workspace.activate_tab(host, "inactive");
    assert!(workspace.replace_active(host, fixture_document("another", 0, "flat"), None).is_err());
    assert!(workspace.open_desigma_files(host).is_err());
    assert!(workspace.open);
    assert_eq!(host.calls.len(), calls, "blocked paths must not invoke a save, transfer or library exit");
    assert_eq!(host.active.snapshot(), active);
    assert_eq!(workspace.tabs["inactive"].session.as_ref().unwrap().snapshot(), incoming);
}

#[test]
fn original_accepted_recovery_reads_remain_serviceable_while_writes_are_frozen() {
    let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
    let original = accepted_record(host.document());
    host.retained_submission = Some(original.clone());
    host.submission_frozen = Some("active".into());
    let before = host.active.snapshot();
    let mut workspace = ready_workspace();
    workspace.recovery_pending = true;
    workspace.validated = false;
    workspace.submissions.records.push(original.clone());
    let (requests, _responses) = transport(&mut workspace);

    workspace.recover_accepted(&mut host);

    let request = requests.try_recv().expect("the retained original operation must still obtain its accepted projection");
    assert_eq!(request["op"], "AcceptedProjection");
    assert_eq!(request["submission"], serde_json::to_value(&original).unwrap());
    assert_eq!(request["draft"], *draft(host.document()).unwrap());
    assert_eq!(workspace.submissions.requested.as_deref(), Some(original.command.operation_id.as_str()));
    assert_eq!(host.calls, [Call::RecoverSubmission]);
    assert!(workspace.error.is_none());

    // Settle each captured request in this transport-only test; no owner or helper runs.
    workspace.busy = None;
    workspace.send(json!({"op":"Dependencies", "draft":draft(host.document())}));
    assert_eq!(requests.try_recv().unwrap()["op"], "Dependencies");
    workspace.busy = None;
    for operation in ["Command", "Reconcile", "Open", "Project"] {
        workspace.send(json!({"op":operation, "draft":draft(host.document())}));
        assert!(workspace.busy.is_none());
        assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    }
    workspace.send(json!({"op":"ReleaseDependencies", "leaseId":"fixture-lease"}));
    assert_eq!(requests.try_recv().unwrap(), json!({"op":"ReleaseDependencies", "leaseId":"fixture-lease"}));
    assert!(workspace.recovery_pending);
    assert!(!workspace.validated);
    assert!(workspace.baseline_recovery.is_none());
    assert_eq!(workspace.submissions.records, vec![original.clone()]);
    assert_eq!(host.retained_submission.as_ref(), Some(&original));
    assert_eq!(host.active.snapshot(), before);
    assert_eq!(host.submission_frozen.as_deref(), Some("active"));
}

#[test]
fn recovery_reads_require_original_acceptance_and_yield_to_a_reviewed_baseline_fence() {
    for reviewed_baseline in [false, true] {
        let host = FakeHost::new(FakeSession::seeded("active-file", "active"));
        let mut record = accepted_record(host.document());
        let mut workspace = ready_workspace();
        workspace.recovery_pending = true;
        workspace.validated = false;
        if reviewed_baseline {
            workspace.baseline_recovery = Some(BaselineRecovery {
                file_id: "active-file".into(),
                draft_id: "active".into(),
                revision: 4,
                semantic_digest: "c".repeat(64),
                binding_digest: "d".repeat(64),
            });
        } else {
            record.state = SubmissionState::Pending;
            record.outcome = None;
        }
        workspace.submissions.records.push(record.clone());
        let identity = workspace.baseline_recovery.clone();
        let (requests, _responses) = transport(&mut workspace);

        for operation in ["AcceptedProjection", "Dependencies"] {
            workspace.send(json!({"op":operation, "draft":draft(host.document())}));
            assert!(workspace.busy.is_none());
            assert!(workspace.error.is_some());
            assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
        }
        workspace.send(json!({"op":"ReleaseDependencies", "leaseId":"fixture-lease"}));
        assert_eq!(requests.try_recv().unwrap()["op"], "ReleaseDependencies");
        assert!(workspace.recovery_pending);
        assert_eq!(workspace.baseline_recovery, identity);
        assert_eq!(workspace.submissions.records, vec![record]);
    }
}

#[test]
fn uncertain_reviewed_baseline_fences_commands_switches_and_leaving_until_exact_recovery() {
    let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
    host.uncertain_baseline = true;
    let mut workspace = ready_workspace();
    let inactive = FakeSession::seeded("inactive-file", "inactive");
    workspace.tabs.insert("inactive".into(), workspace.retained_tab(inactive));
    let before = host.active.checkpoint.to_json().unwrap();
    let before_history = serde_json::to_value(&host.active.checkpoint.history).unwrap();
    let old_resource = workspace.resource.clone();
    let mut bundle = fixture_bundle("active", revision(host.document()).unwrap().1 + 1, "accepted");
    bundle["resource"]["label"] = json!("Reviewed incoming generation");
    workspace.incoming = Some(json!({"review":"must survive uncertainty"}));

    workspace.install_reconciliation(&mut host, bundle.clone());

    assert_eq!(host.calls, [Call::ReviewedBaseline]);
    assert_eq!(host.active.checkpoint.to_json().unwrap(), before);
    assert_eq!(workspace.resource, old_resource);
    assert!(workspace.incoming.is_some());
    assert_eq!(workspace.reconciliation_bundle.as_ref(), Some(&bundle));
    let expected = workspace.baseline_recovery.clone().unwrap();
    assert_eq!(expected.file_id, "active-file");
    assert_eq!(expected.draft_id, "active");
    assert_eq!(expected.revision, bundle["draft"]["draftRevision"].as_u64().unwrap());
    assert_eq!(expected.semantic_digest, bundle["draft"]["semanticDigest"].as_str().unwrap());
    let persisted_candidate = host.pending_baseline.as_ref().unwrap().checkpoint.to_json().unwrap();
    let (requests, _responses) = transport(&mut workspace);
    assert_recovery_fences(&mut workspace, &mut host, &requests);

    let mut wrong_file = expected.clone();
    wrong_file.file_id = "different-file".into();
    let mut wrong_draft = expected.clone();
    wrong_draft.draft_id = "different-draft".into();
    let mut wrong_revision = expected.clone();
    wrong_revision.revision += 1;
    let mut wrong_digest = expected.clone();
    wrong_digest.semantic_digest = "c".repeat(64);
    let mut wrong_binding = expected.clone();
    wrong_binding.binding_digest = "d".repeat(64);
    let rejected_readbacks = [None, Some(wrong_file), Some(wrong_draft), Some(wrong_revision), Some(wrong_digest), Some(wrong_binding)];
    let rejected_count = rejected_readbacks.len();
    for (attempt, readback) in rejected_readbacks.into_iter().enumerate() {
        host.readback = readback;
        workspace.recover_reconciliation(&mut host);
        assert!(workspace.error.is_some());
        assert_eq!(workspace.baseline_recovery.as_ref(), Some(&expected));
        assert_eq!(workspace.reconciliation_bundle.as_ref(), Some(&bundle));
        assert_eq!(host.recovery_requests.last(), Some(&expected));
        assert_eq!(host.recovery_requests.len(), attempt + 1, "each rejected observation must inquire exactly once");
        assert_eq!(host.active.checkpoint.to_json().unwrap(), before);
        assert_recovery_fences(&mut workspace, &mut host, &requests);
    }

    host.readback = Some(expected.clone());
    workspace.recover_reconciliation(&mut host);

    assert!(!workspace.recovery_pending);
    assert!(workspace.validated);
    assert!(workspace.baseline_recovery.is_none());
    assert!(workspace.reconciliation_bundle.is_none());
    assert!(workspace.incoming.is_none());
    assert!(workspace.error.is_none());
    assert_eq!(workspace.resource, Some(bundle["resource"].clone()));
    assert_eq!(host.active.checkpoint.to_json().unwrap(), persisted_candidate, "recovery must not reseal the candidate");
    assert_eq!(host.calls.iter().filter(|call| **call == Call::ReviewedBaseline).count(), 1);
    assert_eq!(host.recovery_requests, vec![expected; rejected_count + 1], "the final successful readback is also an inquiry");
    assert!(!host.active.checkpoint.history.can_undo());
    assert!(!host.active.checkpoint.history.can_redo());
    assert!(host.active.checkpoint.history.baseline_id().is_some());
    assert_eq!(host.active.checkpoint.retained.len(), 1);
    assert_eq!(serde_json::to_value(&host.active.checkpoint.retained[0].history).unwrap(), before_history);
    let archived = &host.active.checkpoint.retained[0];
    assert_eq!(
        DocumentCheckpoint::new(archived.document.clone(), archived.history.clone()).to_json().unwrap(),
        before,
        "the recovered baseline must retain the complete preceding document and both history stacks",
    );

    workspace.send_property_command(&host, &property_field(), json!("raised"));
    let request = requests.try_recv().expect("exact recovery should re-enable qualified commands");
    assert_eq!(request["op"], "Command");
    assert_eq!(request["command"]["expectedRevision"], bundle["draft"]["draftRevision"]);
    assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    // Settle the synthetic outstanding request before checking the independent exit gate.
    workspace.busy = None;
    workspace.open_desigma_files(&mut host).unwrap();
    assert!(!workspace.open);
    assert_eq!(&host.calls[host.calls.len() - 2..], &[Call::SaveBeforeSwitch, Call::OpenFiles]);
}

#[test]
fn project_reply_installs_presentation_without_semantic_history_or_dirty_changes() {
    for dirty in [false, true] {
        let mut host = FakeHost::new(FakeSession::seeded("active-file", "active"));
        host.active.dirty = dirty;
        host.submission_frozen = Some("active".into());
        let mut workspace = ready_workspace();
        let (requests, responses) = transport(&mut workspace);
        let semantic_before = draft(host.document()).unwrap().clone();
        let history_before = serde_json::to_value(&host.active.checkpoint.history).unwrap();
        let file_before = host.active.file_id.clone();
        let autosave_before = host.active.autosave_generation;
        let mut bundle = fixture_bundle("active", revision(host.document()).unwrap().1, "resting");
        bundle["draft"] = semantic_before.clone();
        bundle["projection"]["semanticDigest"] = semantic_before["semanticDigest"].clone();
        bundle["projection"]["nodes"][0]["label"] = json!("Changed presentation only");
        bundle["projection"]["nodes"][0]["fill"] = json!("#112233");
        let mut expected = binding(bundle["draft"].clone(), &bundle["projection"]).unwrap();
        expected.name = host.document().name.clone();

        workspace.send(json!({"op":"Project", "draft":semantic_before}));
        assert_eq!(requests.try_recv().unwrap()["op"], "Project");
        responses.send(Ok(bundle)).unwrap();
        workspace.poll(&mut host);

        assert_eq!(host.calls, [Call::Presentation, Call::EnterDocument]);
        assert_eq!(host.document().to_json().unwrap(), expected.to_json().unwrap());
        assert_eq!(draft(host.document()), Some(&semantic_before));
        assert_eq!(serde_json::to_value(&host.active.checkpoint.history).unwrap(), history_before);
        assert_eq!(host.active.dirty, dirty);
        assert_eq!(host.active.file_id, file_before);
        assert_eq!(host.active.autosave_generation, autosave_before);
        assert_eq!(host.submission_frozen.as_deref(), Some("active"));
        assert!(workspace.validated);
        assert!(workspace.error.is_none());
        assert!(workspace.busy.is_none());
        assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    }
}
