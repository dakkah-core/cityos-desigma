//! Source-only app regressions; these exercise open_path without a native window.
use super::*;
use desigma_library::{Library, SubmissionCommand};
use serde_json::json;
use std::collections::BTreeMap;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("desigma-portable-app-{}", desigma_library::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn library(&self) -> Library {
        Library::open(self.0.join("target")).unwrap()
    }

    fn portable(&self) -> (PathBuf, DocumentCheckpoint) {
        let source = Library::open(self.0.join("source")).unwrap();
        let project = source.create_project("Source").unwrap();
        let payloads = ["inactive", "undo-only", "active", "redo-only"];
        let pins: Vec<_> = payloads.iter().map(|bytes| source.retain_cityos_payload(bytes.as_bytes()).unwrap()).collect();
        let mut checkpoint = DocumentCheckpoint::new(bound(&pins[0], "inactive"), History::default());
        checkpoint.install_baseline(bound(&pins[1], "undo-only"));
        for index in [2, 3] {
            checkpoint.history.exec(&mut checkpoint.document, Command::CityosInstall {
                document: Box::new(bound(&pins[index], payloads[index])),
            }).unwrap();
        }
        checkpoint.history.undo(&mut checkpoint.document).unwrap();
        let file = source.create_file_with_history(&project.id, None, "Portable", &checkpoint.document, &History::default()).unwrap();
        source.save_checkpoint(&file.id, &checkpoint).unwrap();
        // Export precedes prepare: an unresolved matching submission must gate
        // later exports. Its local record cannot travel in this earlier copy.
        let portable = source.export_checkpoint_with_policy(&desigma_cityos::storage_policy::CITYOS_STORAGE_POLICY, &file.id).unwrap();
        let binding = checkpoint.document.cityos.as_ref().unwrap();
        source.prepare_submission(SubmissionCommand {
            intent_id: "portable-intent".into(), operation_id: "portable-operation".into(), predecessor: None,
            owner: "experience".into(), resource: "frame".into(),
            scope: BTreeMap::from([("tenantId".into(), "fixture".into()), ("actorId".into(), "fixture".into())]),
            base: binding.base.clone(), expected_revision: "a".repeat(40), semantic_base: binding.semantic_base.clone(),
            submitted_snapshot: binding.exchange().unwrap()["semanticDraft"].clone(), dependency_pins: pins,
        }).unwrap();
        (self.write("portable.json", &portable), checkpoint)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn ordinary(name: &str) -> Document {
    let mut doc = Document::new(name);
    let page = doc.add_page("Page 1");
    let id = doc.alloc_id();
    doc.insert(page, Node::rectangle(id, Rect::new(0., 0., 40., 20.))).unwrap();
    doc
}

fn bound(pin: &str, state: &str) -> Document {
    let mut doc = ordinary("Portable");
    let id = doc.get(doc.pages[0]).unwrap().children[0];
    doc.images.insert("embedded".into(), vec![2, 4, 6]);
    let semantic_base = json!({"id":"frame","state":"base"});
    doc.cityos = Some(CityosBinding {
        schema_version: "cityos.design-exchange.v3".into(), document_id: "document".into(), draft_id: "draft".into(),
        base: CityosBase::CanonicalGit { repository_id: "repo".into(), commit: "a".repeat(40) },
        semantic_base: semantic_base.clone(), dependency_pins: vec![pin.into()],
        occurrences: BTreeMap::from([(id.0.to_string(), CityosOccurrence {
            definition_id: "fixture".into(), source_node_id: "material".into(), instance_id: Some("instance".into()),
            occurrence_id: "occurrence".into(), view_id: "view".into(),
        })]),
        exchange_v2: None,
        exchange_v3: Some(json!({
            "schemaVersion":"cityos.design-exchange.v3", "documentId":"document", "draftId":"draft", "draftRevision":0,
            "semanticContract":"fixture", "commandReceipts":[],
            "base":{"kind":"canonical-git","repositoryId":"repo","commit":"a".repeat(40)},
            "semanticBase":semantic_base, "semanticDraft":{"id":"frame","state":state}, "dependencyPins":[{"digest":pin}],
        })),
    });
    doc
}

fn at_home(lib: &Library) -> App {
    let mut app = App::new(ordinary("Home placeholder"));
    app.lib = Some(lib.clone());
    app.home = Some(crate::home::Home::new());
    app
}

fn with_file(lib: &Library, doc: &Document) -> (App, desigma_library::FileEntry) {
    let project = lib.create_project("Current").unwrap();
    let folder = lib.create_folder(&project.id, None, "Folder").unwrap();
    let file = lib.create_file_with_history(&project.id, Some(&folder.id), &doc.name, doc, &History::default()).unwrap();
    let mut app = at_home(lib);
    app.open_library_file(&file.id);
    assert_eq!(app.lib_file.as_ref().unwrap().id, file.id);
    (app, file)
}

fn assert_error(app: &App) {
    assert!(app.toast.as_ref().is_some_and(|toast| toast.error));
}

#[test]
fn portable_open_restores_exact_binding_histories_and_every_payload_without_submission_authority() {
    let temp = Temp::new();
    let (path, expected) = temp.portable();
    let lib = temp.library();
    let mut app = at_home(&lib);
    app.open_path(path);
    let imported = app.lib_file.as_ref().expect("new library file").clone();
    assert_eq!(lib.list_projects().len(), 1);
    assert_eq!(lib.files_in(&imported.project_id, None).unwrap().len(), 1, "no empty placeholder file");
    assert_eq!(app.doc.to_json().unwrap(), expected.document.to_json().unwrap());
    assert_eq!(serde_json::to_value(&app.hist).unwrap(), serde_json::to_value(&expected.history).unwrap());
    assert!(app.hist.can_undo() && app.hist.can_redo());
    assert!(app.hist.baseline_id().is_some());
    let reopened = Library::open(lib.root()).unwrap();
    assert_eq!(reopened.load_checkpoint(&imported.id).unwrap().to_json().unwrap(), expected.to_json().unwrap());
    for bytes in ["inactive", "undo-only", "active", "redo-only"] {
        assert_eq!(reopened.load_cityos_payload(&desigma_library::payload_digest(bytes.as_bytes())).unwrap(), bytes.as_bytes());
    }
    assert!(reopened.document_submissions(&imported.id).unwrap().is_empty());
    assert!(reopened.load_submission("portable-operation").is_err());
    assert!(!reopened.path("cityos/submissions").exists());
    assert!(!reopened.path("cityos/submission-sync").exists());
    assert!(app.file_path.is_none() && app.home.is_none());
    assert_eq!(app.autosave.saved_hash, crate::home::doc_hash(&app.doc));
    assert!(!app.toast.as_ref().unwrap().error);
    // Imported undo is still a request for trusted semantic validation.
    let before = app.doc.to_json().unwrap();
    app.undo();
    assert!(app.ui.cityos_history_request.is_some());
    assert_eq!(app.doc.to_json().unwrap(), before);
}

#[test]
fn portable_open_saves_current_library_file_and_creates_a_sibling_each_time() {
    let temp = Temp::new();
    let (path, _) = temp.portable();
    let lib = temp.library();
    lib.create_project("First project must not win").unwrap();
    let (mut app, original) = with_file(&lib, &ordinary("Current"));
    app.add_page();
    let saved = app.doc.to_json().unwrap();
    let history = serde_json::to_value(&app.hist).unwrap();
    app.open_path(path.clone());
    let first = app.lib_file.as_ref().unwrap().id.clone();
    assert_ne!(first, original.id);
    assert_eq!(lib.load_file(&original.id).unwrap().to_json().unwrap(), saved);
    assert_eq!(serde_json::to_value(lib.load_checkpoint(&original.id).unwrap().history).unwrap(), history);
    assert_eq!(lib.file(&first).unwrap().folder, original.folder);
    assert_eq!(lib.file(&first).unwrap().project, original.project);
    app.open_path(path);
    assert_ne!(app.lib_file.as_ref().unwrap().id, first);
    assert_eq!(lib.files_in(&original.project, None).unwrap().len(), 3);
    assert_eq!(lib.load_file(&original.id).unwrap().to_json().unwrap(), saved);
}

#[test]
fn portable_open_prefers_home_folder_and_reuses_an_existing_project_from_recents() {
    for selected in [false, true] {
        let temp = Temp::new();
        let (path, _) = temp.portable();
        let lib = temp.library();
        let first = lib.create_project("First").unwrap();
        let second = lib.create_project("Selected").unwrap();
        let folder = lib.create_folder(&second.id, None, "Nested").unwrap();
        let mut app = at_home(&lib);
        if selected {
            app.home.as_mut().unwrap().section = crate::home::Section::Project(second.id.clone(), Some(folder.id.clone()));
        }
        app.open_path(path);
        let entry = lib.file(&app.lib_file.as_ref().unwrap().id).unwrap();
        assert_eq!(entry.project, if selected { second.id } else { first.id });
        assert_eq!(entry.folder, selected.then_some(folder.id));
        assert_eq!(lib.list_projects().len(), 2);
    }
}

#[test]
fn failed_current_library_save_keeps_document_history_target_and_creates_no_import() {
    let temp = Temp::new();
    let (path, _) = temp.portable();
    let lib = temp.library();
    let (mut app, original) = with_file(&lib, &ordinary("Current"));
    let disk = lib.load_file(&original.id).unwrap().to_json().unwrap();
    app.add_page();
    let doc = app.doc.to_json().unwrap();
    let history = serde_json::to_value(&app.hist).unwrap();
    // A directory at the journal pathname fails before any new durable intent.
    let journal = lib.path(format!("projects/{}/.save.pending.json", original.project));
    std::fs::create_dir(&journal).unwrap();
    app.open_path(path);
    assert_error(&app);
    assert_eq!(app.doc.to_json().unwrap(), doc);
    assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
    assert_eq!(app.lib_file.as_ref().unwrap().id, original.id);
    std::fs::remove_dir(journal).unwrap();
    assert_eq!(lib.load_file(&original.id).unwrap().to_json().unwrap(), disk);
    assert_eq!(lib.files_in(&original.project, None).unwrap().len(), 1);
    assert!(!lib.path("cityos/payloads").exists());
}

#[test]
fn portable_open_saves_ordinary_native_edits_and_aborts_on_native_save_failure() {
    for fail in [false, true] {
        let temp = Temp::new();
        let (path, _) = temp.portable();
        let lib = temp.library();
        let mut app = App::new(ordinary("External"));
        app.lib = Some(lib.clone());
        let native = temp.0.join("current.desigma");
        io::save_path(&app.doc, &native).unwrap();
        app.autosave.external_fingerprint = Some(io::file_fingerprint(&native).unwrap());
        if fail { std::fs::remove_file(&native).unwrap(); std::fs::create_dir(&native).unwrap(); }
        app.file_path = Some(native.clone());
        app.autosave.saved_hash = crate::home::doc_hash(&app.doc);
        app.add_page();
        let doc = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        app.open_path(path);
        if fail {
            assert_error(&app);
            assert_eq!(app.doc.to_json().unwrap(), doc);
            assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
            assert_eq!(app.file_path.as_ref(), Some(&native));
            assert!(app.lib_file.is_none() && lib.list_projects().is_empty());
        } else {
            assert_eq!(io::load_path(&native).unwrap().to_json().unwrap(), doc);
            assert!(app.lib_file.is_some());
        }
    }
}

#[test]
fn unsaved_ordinary_documents_and_changed_imports_are_never_discarded_for_a_checkpoint() {
    for extension in [None, Some("json"), Some("svg")] {
        let temp = Temp::new();
        let (path, _) = temp.portable();
        let lib = temp.library();
        let mut app = App::new(ordinary("Unsaved"));
        app.lib = Some(lib.clone());
        app.file_path = extension.map(|ext| temp.write(&format!("original.{ext}"), "keep original import"));
        app.add_page();
        let doc = app.doc.to_json().unwrap();
        let history = serde_json::to_value(&app.hist).unwrap();
        let original_path = app.file_path.clone();
        app.open_path(path);
        assert_error(&app);
        assert_eq!(app.doc.to_json().unwrap(), doc);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
        assert_eq!(app.file_path, original_path);
        if let Some(path) = original_path { assert_eq!(std::fs::read_to_string(path).unwrap(), "keep original import"); }
        assert!(app.lib_file.is_none() && lib.list_projects().is_empty());
        assert!(!lib.path("cityos/payloads").exists());
    }
}

#[test]
fn returning_home_does_not_turn_unsaved_edits_into_a_disposable_placeholder() {
    let temp = Temp::new();
    let (path, _) = temp.portable();
    let lib = temp.library();
    let mut app = App::new(ordinary("Unsaved"));
    app.lib = Some(lib.clone());
    app.add_page();
    app.go_home();
    assert!(app.home.is_some());
    let doc = app.doc.to_json().unwrap();
    let history = serde_json::to_value(&app.hist).unwrap();
    app.open_path(path);
    assert_error(&app);
    assert_eq!(app.doc.to_json().unwrap(), doc);
    assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
    assert!(app.home.is_some() && app.lib_file.is_none());
    assert!(lib.list_projects().is_empty());
}

#[test]
fn missing_library_keeps_the_current_document_and_unflushed_notes() {
    let temp = Temp::new();
    let (path, _) = temp.portable();
    let mut app = App::new(crate::decks::new_document("Current"));
    app.decks.notes_for = Some(app.deck_slides()[0]);
    app.decks.notes_draft = "pending notes".into();
    let doc = app.doc.to_json().unwrap();
    app.open_path(path);
    assert_error(&app);
    assert_eq!(app.doc.to_json().unwrap(), doc);
    assert!(app.decks.notes_saved.is_empty());
    assert_eq!(app.decks.notes_draft, "pending notes");
    assert!(!app.hist.can_undo());
    assert!(app.lib.is_none() && app.lib_file.is_none());
}

#[test]
fn invalid_portable_and_failed_import_leave_the_current_document_and_empty_library_intact() {
    for invalid in [false, true] {
        let temp = Temp::new();
        let (path, _) = temp.portable();
        let lib = temp.library();
        let mut app = at_home(&lib);
        if invalid {
            let mut value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            value["checkpoint"] = json!("{broken");
            std::fs::write(&path, value.to_string()).unwrap();
        } else {
            // Project creation fails after validation and flush, before import.
            std::fs::write(lib.path("projects"), b"blocked").unwrap();
        }
        let doc = app.doc.to_json().unwrap();
        app.open_path(path);
        assert_error(&app);
        assert_eq!(app.doc.to_json().unwrap(), doc);
        assert!(app.home.is_some() && app.lib_file.is_none());
        assert!(lib.list_projects().is_empty());
        assert!(!lib.path("cityos/payloads").exists());
        assert!(!lib.path("checkpoints").exists());
    }
}

#[test]
fn malformed_portables_never_flush_pending_notes_save_current_file_or_merge_comments() {
    let temp = Temp::new();
    let (path, _) = temp.portable();
    let valid: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let lib = temp.library();
    let (mut app, original) = with_file(&lib, &crate::decks::new_document("Current"));
    let disk = lib.load_file(&original.id).unwrap().to_json().unwrap();
    app.decks.notes_for = Some(app.deck_slides()[0]);
    app.decks.notes_draft = "pending notes".into();
    app.doc.name = "Unsaved current".into();
    let doc = app.doc.to_json().unwrap();
    let history = serde_json::to_value(&app.hist).unwrap();
    let saved_hash = app.autosave.saved_hash;
    for fault in ["truncated", "future", "checkpoint", "missing", "corrupt", "comments"] {
        let mut value = valid.clone();
        match fault {
            "future" => value["schemaVersion"] = json!("desigma.portable-checkpoint.v999"),
            "checkpoint" => value["checkpoint"] = json!("{truncated"),
            "missing" => {
                let pin = value["payloads"].as_object().unwrap().keys().next().unwrap().clone();
                value["payloads"].as_object_mut().unwrap().remove(&pin);
            }
            "corrupt" => {
                let pin = value["payloads"].as_object().unwrap().keys().next().unwrap().clone();
                value["payloads"][pin] = json!([0]);
            }
            "comments" => { value["comments"] = json!([{"id":"unexpected", "message":"do not merge"}]); }
            _ => {}
        }
        let text = if fault == "truncated" { r#"{"schemaVersion":"desigma.portable-checkpoint.v1","checkpoint":"#.to_string() } else { value.to_string() };
        app.open_path(temp.write("invalid.json", &text));
        assert_error(&app);
        assert_eq!(app.doc.to_json().unwrap(), doc, "{fault}");
        assert_eq!(serde_json::to_value(&app.hist).unwrap(), history, "{fault}");
        assert_eq!(app.autosave.saved_hash, saved_hash);
        assert!(app.decks.notes_saved.is_empty());
        assert_eq!(app.decks.notes_draft, "pending notes");
        assert_eq!(app.lib_file.as_ref().unwrap().id, original.id);
        assert_eq!(lib.load_file(&original.id).unwrap().to_json().unwrap(), disk);
        assert_eq!(lib.files_in(&original.project, None).unwrap().len(), 1);
        assert!(!lib.path("cityos/payloads").exists());
    }
}

#[test]
fn ordinary_native_json_and_svg_open_paths_still_use_the_original_loader() {
    let temp = Temp::new();
    let expected = ordinary("Ordinary");
    for extension in ["desigma", "json", "svg"] {
        let path = temp.write(&format!("ordinary.{extension}"), &if extension == "svg" {
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8"/></svg>"#.into()
        } else { expected.to_json().unwrap() });
        let loaded = io::load_path(&path).unwrap();
        let mut app = App::new(ordinary("Previous"));
        app.open_path(path.clone());
        assert_eq!(app.doc.to_json().unwrap(), loaded.to_json().unwrap());
        assert_eq!(app.file_path, (extension != "svg").then_some(path));
        assert!(app.lib_file.is_none() && !app.hist.can_undo());
    }
}

#[test]
fn ordinary_comments_and_variables_json_still_merge_into_the_current_document() {
    let temp = Temp::new();
    let mut source = ordinary("Variables");
    let mut variables = VariableStore::default();
    let collection = variables.add_collection("Theme");
    variables.add_variable(&collection, "Gap", VariableKind::Number).unwrap();
    variables.command().apply(&mut source).unwrap();
    let variable_json = desigma_fig::export_variables_json(&source).to_string();
    let mut app = App::new(ordinary("Current"));
    app.open_path(temp.write("variables.json", &variable_json));
    assert_eq!(app.doc.name, "Current");
    assert_eq!(app.doc.variables.len(), 1);
    app.open_path(temp.write("comments.json", r#"{"comments":[{"id":"1","message":"Keep this comment","user":{"handle":"Fixture"},"created_at":"2026-01-01T00:00:00Z","client_meta":{"x":4,"y":8}}]}"#));
    assert_eq!(app.doc.name, "Current");
    assert_eq!(app.doc.comments.len(), 1);
    app.open_path(temp.write("comments-array.json", r#"[{"id":"2","message":"Array payload","user":{"handle":"Fixture"},"created_at":"2026-01-01T00:00:00Z","client_meta":{"x":4,"y":8}}]"#));
    assert_eq!(app.doc.comments.len(), 2, "ordinary JSON arrays must pass through portable recognition");
    assert!(app.lib_file.is_none() && app.file_path.is_none());
    assert!(app.hist.can_undo());
}
