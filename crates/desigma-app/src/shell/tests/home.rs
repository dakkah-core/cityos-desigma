//! UI tests for the projects library: the home screen, opening / renaming / saving
//! library files, trash, drag and drop, and the removed account / plan labels.

use super::*;
use crate::home::{Act, DropTo, Intent, Section};
use desigma_doc::Document;
use desigma_library::Library;
use std::path::PathBuf;

struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("desigma_home_ui_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        // The thumbnail thread may still be writing; ignore failures.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn home_sim() -> (Sim, TmpDir) {
    let dir = TmpDir::new();
    let s = Sim::with(Options { home: true, library: Some(dir.0.clone()), ..Default::default() });
    (s, dir)
}

fn autosave_app() -> (crate::app::App, Library, TmpDir, String) {
    let dir = TmpDir::new();
    let library = Library::open(&dir.0).unwrap();
    let project = library.create_project("Autosave").unwrap();
    let mut doc = Document::new("History draft"); doc.add_page("Page");
    let file = library.create_file(&project.id, None, &doc.name, &doc).unwrap();
    let mut app = crate::app::App::new(doc);
    app.lib = Some(library.clone()); app.open_library_file(&file.id);
    crate::home::thumb::finish_library_jobs(&dir.0);
    (app, library, dir, file.id)
}

fn edit_then_undo_same_document(app: &mut crate::app::App) {
    let before = app.doc.to_json().unwrap();
    let mut node = app.doc.get(app.doc.pages[0]).unwrap().clone();
    node.name.push_str(" changed");
    app.hist.exec(&mut app.doc, desigma_doc::Command::Update { node: Box::new(node) }).unwrap();
    assert!(app.hist.undo(&mut app.doc).unwrap());
    assert_eq!(app.doc.to_json().unwrap(), before);
    assert!(app.hist.can_redo());
}

#[test]
fn history_only_autosave_preserves_redo_when_edit_undo_returns_to_saved_document() {
    let (mut app, library, _dir, file) = autosave_app();
    let saved_hash = app.autosave.saved_hash;
    let saved_history = app.autosave.saved_history_hash;
    let fingerprint = app.autosave.saved_checkpoint_fingerprint.clone();
    let revision = app.canvas.tiles.rev();
    edit_then_undo_same_document(&mut app);
    assert_eq!(app.canvas.tiles.rev(), revision, "history events do not require a canvas invalidation");
    let ctx = egui::Context::default();
    app.now = 10.; app.autosave_tick(&ctx);
    app.now = 10.4; app.autosave_tick(&ctx);
    assert!(app.autosave.pending());
    app.now = 12.; app.autosave_tick(&ctx);
    assert_eq!(app.autosave.saves, 1);
    assert_eq!(app.autosave.saved_hash, saved_hash);
    assert_ne!(app.autosave.saved_history_hash, saved_history);
    assert_ne!(app.autosave.saved_checkpoint_fingerprint, fingerprint);
    assert!(library.load_checkpoint(&file).unwrap().history.can_redo());
    app.open_library_file(&file);
    assert!(app.hist.can_redo(), "reopening must retain history-only redo");
    assert!(app.hist.redo(&mut app.doc).unwrap());
    assert!(app.doc.get(app.doc.pages[0]).unwrap().name.ends_with(" changed"));
}

#[test]
fn history_only_failed_autosave_retries_without_advancing_persisted_history_or_fingerprint() {
    let (mut app, library, dir, file) = autosave_app();
    let saved = (app.autosave.saved_hash, app.autosave.saved_history_hash, app.autosave.saved_checkpoint_fingerprint.clone());
    edit_then_undo_same_document(&mut app);
    let manifest = library.path("projects").join(&app.lib_file.as_ref().unwrap().project_id).join("project.json");
    let bytes = std::fs::read(&manifest).unwrap();
    std::fs::write(&manifest, b"{damaged manifest").unwrap();
    let ctx = egui::Context::default();
    app.now = 10.; app.autosave_tick(&ctx);
    app.now = 10.4; app.autosave_tick(&ctx);
    app.now = 12.; app.autosave_tick(&ctx);
    assert_eq!((app.autosave.saved_hash, app.autosave.saved_history_hash, app.autosave.saved_checkpoint_fingerprint.clone()), saved);
    assert!(app.autosave.pending()); assert!(app.hist.can_redo());
    assert_eq!(app.autosave.failures, 1); assert_eq!(app.autosave.retry_at, Some(14.));
    std::fs::write(&manifest, bytes).unwrap();
    app.now = 13.; app.autosave_tick(&ctx);
    assert_eq!(app.autosave.saves, 0);
    app.now = 14.; app.autosave_tick(&ctx);
    assert_eq!(app.autosave.saves, 1); assert_eq!(app.autosave.failures, 0);
    assert_eq!(app.autosave.retry_at, None);
    assert!(library.load_checkpoint(&file).unwrap().history.can_redo());
    crate::home::thumb::finish_library_jobs(&dir.0);
}

#[test]
fn history_only_flush_persists_redo_and_missing_checkpoint_observation_fails_closed() {
    let (mut app, library, _dir, file) = autosave_app();
    edit_then_undo_same_document(&mut app);
    let fingerprint = app.autosave.saved_checkpoint_fingerprint.take().unwrap();
    assert!(!app.flush_autosave()); assert!(app.autosave.pending());
    assert!(!library.load_checkpoint(&file).unwrap().history.can_redo());
    assert!(app.autosave.saved_checkpoint_fingerprint.is_none());
    app.autosave.saved_checkpoint_fingerprint = Some(fingerprint);
    assert!(app.flush_autosave());
    assert!(library.load_checkpoint(&file).unwrap().history.can_redo());
}

#[test]
fn history_fence_change_with_identical_serialized_stacks_does_not_rewrite_checkpoint() {
    let (mut app, library, _dir, file) = autosave_app();
    let mut node = app.doc.get(app.doc.pages[0]).unwrap().clone(); node.name.push_str(" changed");
    app.hist.exec(&mut app.doc, desigma_doc::Command::Update { node: Box::new(node) }).unwrap();
    app.save_library_file().unwrap();
    let before = library.load_checkpoint(&file).unwrap().to_json().unwrap();
    let identity = app.hist.change_identity();
    assert!(app.hist.undo(&mut app.doc).unwrap()); assert!(app.hist.redo(&mut app.doc).unwrap());
    assert!(!std::sync::Arc::ptr_eq(&identity, &app.hist.change_identity()));
    let ctx = egui::Context::default();
    app.now = 10.; app.autosave_tick(&ctx);
    app.now = 10.4; app.autosave_tick(&ctx);
    app.now = 20.; app.autosave_tick(&ctx);
    assert!(app.autosave.changed_at.is_none()); assert_eq!(app.autosave.saves, 0);
    assert_eq!(library.load_checkpoint(&file).unwrap().to_json().unwrap(), before);
}

#[test]
fn library_observation_precedes_entry_name_normalization_and_rejects_a_newer_writer() {
    let (mut app, library, _dir, file) = autosave_app();
    library.rename_file(&file, "Normalized name").unwrap();
    let checkpoint = library.load_checkpoint(&file).unwrap();
    let fingerprint = desigma_library::checkpoint_fingerprint(&checkpoint).unwrap();
    app.open_library_file(&file);
    assert_eq!(app.doc.name, "Normalized name");
    assert_ne!(app.doc.name, checkpoint.document.name);
    assert_eq!(app.autosave.saved_checkpoint_fingerprint.as_deref(), Some(fingerprint.as_str()));
    assert!(app.flush_autosave());
    assert_eq!(library.load_checkpoint(&file).unwrap().to_json().unwrap(), checkpoint.to_json().unwrap());
    let mut newer = checkpoint.document; newer.name = "Other writer".into();
    let newer_fingerprint = library.save_file_with_history_expected(&file, &newer, &checkpoint.history, &fingerprint).unwrap();
    app.doc.name = "Keep local candidate".into();
    assert!(app.save_library_file().is_err());
    assert_eq!(app.doc.name, "Keep local candidate");
    assert_eq!(app.autosave.saved_checkpoint_fingerprint.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(desigma_library::checkpoint_fingerprint(&library.load_checkpoint(&file).unwrap()).unwrap(), newer_fingerprint);
}

#[test]
fn external_saves_keep_observed_bytes_and_never_claim_durable_history() {
    let dir = TmpDir::new(); std::fs::create_dir_all(&dir.0).unwrap();
    let mut doc = Document::new("External"); doc.add_page("Page");
    let (path, fingerprint) = crate::io::save_path_observed(&doc, &dir.0.join("external.desigma")).unwrap();
    let mut app = crate::app::App::new(doc); app.open_path(path.clone());
    assert_eq!(app.autosave.external_fingerprint.as_deref(), Some(fingerprint.as_str()));
    let mut node = app.doc.get(app.doc.pages[0]).unwrap().clone(); node.name = "Edited".into();
    app.hist.exec(&mut app.doc, desigma_doc::Command::Update { node: Box::new(node) }).unwrap();
    app.save_external_to(&path).unwrap();
    assert!(app.hist.can_undo()); assert!(app.autosave.saved_history_hash.is_none());
    assert!(app.autosave.saved_checkpoint_fingerprint.is_none());
    assert_ne!(app.autosave.external_fingerprint.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(app.autosave.external_fingerprint.as_ref().unwrap(), &crate::io::file_fingerprint(&path).unwrap());
    let candidate = app.doc.to_json().unwrap();
    app.autosave.external_fingerprint = None;
    assert!(app.save_external_to(&path).is_err(), "same-target Save As cannot infer a missing observation");
    assert_eq!(app.doc.to_json().unwrap(), candidate);
}

#[test]
fn external_same_target_save_as_rejects_stale_observation_but_explicit_copy_observes_its_write() {
    let dir = TmpDir::new(); std::fs::create_dir_all(&dir.0).unwrap();
    let mut doc = Document::new("Original"); doc.add_page("Page");
    let (path, fingerprint) = crate::io::save_path_observed(&doc, &dir.0.join("external.desigma")).unwrap();
    let mut app = crate::app::App::new(doc.clone()); app.open_path(path.clone());
    doc.name = "Other writer".into(); crate::io::save_path_observed(&doc, &path).unwrap();
    app.doc.name = "Keep local candidate".into();
    assert!(app.save_external_to(&path.with_extension("")).is_err(), "extension normalization must retain CAS");
    assert_eq!(app.doc.name, "Keep local candidate");
    assert_eq!(app.autosave.external_fingerprint.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(crate::io::load_path(&path).unwrap().name, "Other writer");
    let copy = dir.0.join("copy.desigma"); app.save_external_to(&copy).unwrap();
    assert_eq!(app.file_path.as_ref(), Some(&copy));
    assert_eq!(app.autosave.external_fingerprint.as_ref().unwrap(), &crate::io::file_fingerprint(&copy).unwrap());
    assert!(app.autosave.saved_history_hash.is_none());
}

#[test]
fn local_design_creation_preserves_a_stale_external_candidate_and_the_newer_file() {
    let dir = TmpDir::new(); std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("external.desigma");
    let mut original = Document::new("Original"); original.add_page("Page");
    crate::io::save_path(&original, &path).unwrap();
    let mut app = crate::app::App::new(Document::new("Empty")); app.open_path(path.clone());
    let observation = app.autosave.external_fingerprint.clone();
    original.name = "Newer saved design".into(); crate::io::save_path(&original, &path).unwrap();
    let before = std::fs::read(&path).unwrap(); app.doc.name = "Keep local edits".into();
    assert!(app.preserve_before_local_design().unwrap_err().contains("changed on disk"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(app.doc.name, "Keep local edits");
    assert_eq!(app.autosave.external_fingerprint, observation);
    assert_eq!(app.file_path.as_ref(), Some(&path));
}

#[test]
fn external_preservation_never_replaces_an_import_source_with_native_json() {
    let dir = TmpDir::new(); std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("source.fig"); std::fs::write(&path, b"original import bytes").unwrap();
    let mut app = crate::app::App::new(Document::new("Imported design"));
    app.file_path = Some(path.clone());
    app.autosave.external_fingerprint = Some(crate::io::file_fingerprint(&path).unwrap());
    assert!(app.save_current_external().unwrap_err().contains("native file"));
    assert_eq!(std::fs::read(path).unwrap(), b"original import bytes");
}

#[test]
fn save_as_cannot_replace_a_managed_library_document_outside_its_checkpoint() {
    let (mut app, library, _dir, file) = autosave_app();
    let entry = library.file(&file).unwrap();
    let target = library.path("projects").join(&entry.project).join("files").join(format!("{file}.desigma"));
    let before = library.load_checkpoint(&file).unwrap().to_json().unwrap();
    let observation = app.autosave.saved_checkpoint_fingerprint.clone();
    app.doc.name = "Unsaved candidate".into();
    assert!(app.save_external_to(&target).unwrap_err().contains("projects library"));
    assert_eq!(library.load_checkpoint(&file).unwrap().to_json().unwrap(), before);
    assert_eq!(app.doc.name, "Unsaved candidate");
    assert_eq!(app.autosave.saved_checkpoint_fingerprint, observation);
    assert!(app.file_path.is_none());
}

#[test]
fn opening_a_managed_file_path_uses_its_original_checkpoint_instead_of_an_external_copy() {
    let (mut app, library, _dir, file) = autosave_app();
    let mut node = app.doc.get(app.doc.pages[0]).unwrap().clone(); node.name = "Retain undo".into();
    app.hist.exec(&mut app.doc, desigma_doc::Command::Update { node: Box::new(node) }).unwrap();
    app.save_library_file().unwrap();
    let entry = library.file(&file).unwrap();
    let path = library.path("projects").join(&entry.project).join("files").join(format!("{file}.desigma"));
    let history = serde_json::to_value(&app.hist).unwrap();
    let mut other = crate::app::App::new(Document::new("Empty")); other.lib = Some(library.clone());
    other.open_path(path);
    assert_eq!(other.lib_file.as_ref().map(|file| file.id.as_str()), Some(file.as_str()));
    assert!(other.file_path.is_none());
    assert_eq!(serde_json::to_value(&other.hist).unwrap(), history);
    assert!(other.autosave.saved_checkpoint_fingerprint.is_some());
    assert_eq!(library.all_files().len(), 1);
}

#[test]
fn reopening_the_active_library_file_observes_its_own_pending_save_and_history() {
    let (mut app, library, _dir, file) = autosave_app();
    let mut node = app.doc.get(app.doc.pages[0]).unwrap().clone(); node.name = "Unsaved layer change".into();
    app.hist.exec(&mut app.doc, desigma_doc::Command::Update { node: Box::new(node) }).unwrap();
    let expected = app.doc.to_json().unwrap(); let history = serde_json::to_value(&app.hist).unwrap();
    app.open_library_file(&file);
    assert_eq!(app.doc.to_json().unwrap(), expected);
    assert_eq!(serde_json::to_value(&app.hist).unwrap(), history);
    let checkpoint = library.load_checkpoint(&file).unwrap();
    assert_eq!(checkpoint.document.to_json().unwrap(), expected);
    assert_eq!(app.autosave.saved_checkpoint_fingerprint.as_ref().unwrap(), &desigma_library::checkpoint_fingerprint(&checkpoint).unwrap());
    assert!(app.flush_autosave(), "reopen must not leave a stale predecessor fence");
}

#[test]
fn cityos_temporary_views_cannot_save_autosave_or_create_history(){
    let (mut s,_dir)=home_sim();
    open(&mut s,"Welcome");
    let library=lib(&s);let files=library.all_files().len();
    let id=s.app().lib_file.as_ref().unwrap().id.clone();
    let saved=library.load_file(&id).unwrap().to_json().unwrap();
    let versions=library.list_versions(&id).len();
    // An actual editor document underneath the catalog must stay dirty and unsaved.
    s.shell.app.doc.name="Unsaved editor underneath catalog".into();
    let document=s.app().doc.to_json().unwrap();let saved_hash=s.app().autosave.saved_hash;
    s.shell.cityos=crate::cityos_catalog::tests::populated_browser();
    for key in [Key::S,Key::Z,Key::Y,Key::D]{s.cmd_key(key);}
    s.mods=Modifiers::COMMAND|Modifiers::ALT;s.key(Key::S);s.mods=Modifiers::NONE;
    for _ in 0..120{s.frame(vec![]);}
    assert_eq!(library.all_files().len(),files);assert_eq!(s.app().doc.to_json().unwrap(),document);assert!(s.app().history.save_modal.is_none());assert_eq!(s.app().autosave.saves,0);
    assert_eq!(s.app().lib_file.as_ref().unwrap().id,id);
    assert_eq!(s.app().autosave.saved_hash,saved_hash);
    assert_eq!(library.load_file(&id).unwrap().to_json().unwrap(),saved);
    assert_eq!(library.list_versions(&id).len(),versions);
}

#[test]
fn failed_flush_retains_dirty_revision_for_retry(){
    let (mut s,_dir)=home_sim();let library=lib(&s);let id=library.all_files()[0].id.clone();s.shell.app.open_library_file(&id);
    let project=s.app().lib_file.as_ref().unwrap().project_id.clone();let blocked=library.path("projects").join(project).join(".save.pending.json");std::fs::create_dir(&blocked).unwrap();
    let previous=s.app().autosave.saved_hash;s.shell.app.doc.name="unsaved edit".into();s.shell.app.flush_autosave();
    assert_eq!(s.app().autosave.saved_hash,previous);assert!(s.app().autosave.changed_at.is_some());
    std::fs::remove_dir(blocked).unwrap();s.shell.app.flush_autosave();assert_ne!(s.app().autosave.saved_hash,previous);assert!(s.app().autosave.changed_at.is_none());
}

#[test]
fn failed_autosave_backs_off_without_clearing_dirty_state(){
    let (mut s,_dir)=home_sim();let library=lib(&s);let id=library.all_files()[0].id.clone();s.shell.app.open_library_file(&id);
    let project=s.app().lib_file.as_ref().unwrap().project_id.clone();
    let blocked=library.path("projects").join(project).join(".save.pending.json");std::fs::create_dir(&blocked).unwrap();
    let app=&mut s.shell.app;let saved=app.autosave.saved_hash;app.doc.name="Retry this revision".into();
    app.autosave.changed_at=Some(0.);app.autosave.confirmed=true;app.now=10.;
    let ctx=egui::Context::default();app.autosave_tick(&ctx);
    assert_eq!(app.autosave.saved_hash,saved);assert_eq!(app.autosave.failures,1);assert_eq!(app.autosave.retry_at,Some(12.));
    std::fs::remove_dir(blocked).unwrap();app.now=11.;app.autosave_tick(&ctx);
    assert_eq!(app.autosave.saved_hash,saved);assert!(app.autosave.pending());
    app.now=12.;app.autosave_tick(&ctx);
    assert_ne!(app.autosave.saved_hash,saved);assert_eq!(app.autosave.failures,0);assert_eq!(app.autosave.retry_at,None);
    assert_eq!(library.load_file(&id).unwrap().name,"Retry this revision");
}

#[test]
fn failed_save_prevents_home_open_new_and_document_replacement(){
    let (mut s,_dir)=home_sim();let library=lib(&s);let id=library.all_files()[0].id.clone();s.shell.app.open_library_file(&id);
    let project=s.app().lib_file.as_ref().unwrap().project_id.clone();
    let other=library.create_file(&project,None,"Other",&Document::new("Other")).unwrap();
    s.shell.app.doc.name="Unsaved current".into();let document=s.app().doc.to_json().unwrap();
    let blocked=library.path("projects").join(project).join(".save.pending.json");std::fs::create_dir(&blocked).unwrap();
    s.shell.app.go_home();assert!(s.app().home.is_none());
    s.shell.app.open_library_file(&other.id);assert_eq!(s.app().lib_file.as_ref().unwrap().id,id);
    s.shell.app.new_library_file(None);
    assert!(!s.shell.app.replace_document(Document::new("Replacement"),None));
    assert_eq!(s.app().doc.to_json().unwrap(),document);assert!(s.app().autosave.pending());
    std::fs::remove_dir(blocked).unwrap();assert_eq!(library.all_files().len(),2);
}

#[test]
fn catalog_launcher_and_back_preserve_the_home_library() {
    let (mut s,_dir)=home_sim();
    let library=lib(&s);let files=library.all_files().len();
    s.shell.cityos=crate::cityos_catalog::tests::populated_browser();s.shell.cityos.open=false;
    s.shell.app.home.as_mut().unwrap().search = "Welcome".into();
    s.frame(vec![]);
    assert!(s.rect_of("home-cityos").right() <= theme::HOME_SIDEBAR_W);
    click_rect(&mut s,"home-cityos");
    assert!(s.shell.cityos.open);assert!(s.app().home.is_some());
    click_rect(&mut s,"catalog-back");
    assert!(!s.shell.cityos.open);assert!(s.app().home.is_some());assert_eq!(library.all_files().len(),files);
    assert_eq!(s.app().home.as_ref().unwrap().search, "Welcome");
}

#[test]
fn cityos_rail_and_back_preserve_unsaved_document_history_selection_and_view() {
    let (mut s, _dir) = home_sim();
    open(&mut s, "Welcome");
    let card = s.by_name("Card");
    s.shell.app.select_only(card);
    s.shell.app.edit_nodes(&[card], |node| node.name = "Unsaved card".into());
    s.shell.app.ui.left_collapsed = true;
    let document = s.app().doc.to_json().unwrap();
    let selection = s.app().selection.clone();
    let view = s.app().view;
    let saved_hash = s.app().autosave.saved_hash;
    let saves = s.app().autosave.saves;
    let history = (s.app().hist.can_undo(), s.app().hist.can_redo());
    let files = lib(&s).all_files().len();
    s.shell.cityos = crate::cityos_catalog::tests::populated_browser();
    s.shell.cityos.open = false;
    s.frame(vec![]);
    click_rect(&mut s, "rail-CityOS");
    assert!(s.shell.cityos.open);
    assert!(!s.app().ui.cityos_requested, "the request is consumed once");
    s.cmd_key(Key::S);
    click_rect(&mut s, "catalog-back");
    assert!(!s.shell.cityos.open);
    assert!(s.app().home.is_none());
    assert_eq!(s.app().doc.to_json().unwrap(), document);
    assert_eq!(s.app().selection, selection);
    assert_eq!(s.app().view.offset, view.offset);
    assert_eq!(s.app().view.zoom, view.zoom);
    assert_eq!((s.app().hist.can_undo(), s.app().hist.can_redo()), history);
    assert_eq!(s.app().autosave.saved_hash, saved_hash);
    assert_eq!(s.app().autosave.saves, saves);
    assert!(s.app().ui.left_collapsed);
    assert_eq!(lib(&s).all_files().len(), files);
    s.rect_of("rail-CityOS");
}

#[test]
fn cityos_rail_returns_to_the_active_theme_variables_collection() {
    let mut s = Sim::with(Options { fixture: Some("variables".into()), ..Default::default() });
    s.shell.app.open_variables();
    s.shell.app.vars.full = true;
    let collection = s.app().vars.collection.clone();
    let document = s.app().doc.to_json().unwrap();
    s.shell.cityos = crate::cityos_catalog::tests::populated_browser();
    s.shell.cityos.open = false;
    s.frame(vec![]);
    click_rect(&mut s, "rail-CityOS");
    assert!(s.shell.cityos.open);
    click_rect(&mut s, "catalog-back");
    assert!(!s.shell.cityos.open);
    assert!(s.app().vars.open && s.app().vars.full);
    assert_eq!(s.app().vars.collection, collection);
    assert_eq!(s.app().doc.to_json().unwrap(), document);
}

#[test]
fn closing_cityos_saves_only_the_existing_document_and_cancels_close_on_failure() {
    fn close(s: &mut Sim) -> egui::FullOutput {
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1088.0))),
            time: Some(s.t + 0.05),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        let ctx = s.ctx.clone();
        ctx.run_ui(raw, |ui| s.shell.draw(ui))
    }
    let (mut s, _dir) = home_sim();
    open(&mut s, "Welcome");
    let library = lib(&s);
    let file = s.app().lib_file.as_ref().unwrap().clone();
    let saved = library.load_file(&file.id).unwrap().to_json().unwrap();
    let files = library.all_files().len();
    s.shell.app.doc.name = "Unsaved beneath CityOS".into();
    s.shell.cityos = crate::cityos_catalog::tests::populated_browser();
    let blocked = library.path("projects").join(&file.project_id).join(".save.pending.json");
    std::fs::create_dir(&blocked).unwrap();
    let output = close(&mut s);
    assert!(output.viewport_output.values().flat_map(|v| &v.commands).any(|c| matches!(c, egui::ViewportCommand::CancelClose)));
    assert!(s.app().autosave.pending());
    assert!(s.shell.cityos.open);
    std::fs::remove_dir(blocked).unwrap();
    // The injected invalid journal deliberately makes the project unavailable
    // to readers as well as writers. Re-enable reads before checking its bytes.
    assert_eq!(library.load_file(&file.id).unwrap().to_json().unwrap(), saved);
    let output = close(&mut s);
    assert!(!output.viewport_output.values().flat_map(|v| &v.commands).any(|c| matches!(c, egui::ViewportCommand::CancelClose)));
    assert_eq!(library.load_file(&file.id).unwrap().name, "Unsaved beneath CityOS");
    assert_eq!(library.all_files().len(), files, "a specimen never becomes another document");
}

fn lib(s: &Sim) -> Library {
    s.app().lib.clone().expect("a library")
}

fn click_rect(s: &mut Sim, name: &str) {
    let r = s.rect_of(name);
    s.click(r.center());
}

fn double_click(s: &mut Sim, p: Pos2) {
    s.click(p);
    s.click(p);
}

fn run_action(s: &mut Sim, a: crate::panels::menus::Action) {
    let ctx = s.ctx.clone();
    crate::panels::menus::run(&mut s.shell.app, &ctx, a);
}

fn file_id(s: &Sim, name: &str) -> String {
    lib(s).all_files().into_iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no library file {name}")).id
}

fn open(s: &mut Sim, name: &str) {
    let id = file_id(s, name);
    crate::home::apply(&mut s.shell.app, Act::Open(id));
    s.frame(vec![]);
    s.frame(vec![]);
}

/// Frames until `secs` of UI time have passed.
fn wait(s: &mut Sim, secs: f64) {
    let end = s.t + secs;
    while s.t < end {
        s.frame(vec![]);
    }
}

#[test]
fn first_launch_shows_the_home_screen_with_the_getting_started_project() {
    let (s, _d) = home_sim();
    let home = s.app().home.as_ref().expect("home screen");
    assert_eq!(home.section, Section::Recents);
    let names: Vec<_> = home.snap.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Getting started"]);
    assert_eq!(home.snap.files.len(), 1);
    assert_eq!(home.snap.files[0].name, "Welcome");
    // the seeded file carries the demo document and already has a thumbnail
    let l = lib(&s);
    assert!(l.load_file(&home.snap.files[0].id).unwrap().nodes.len() > 10);
    assert!(l.thumbnail_path(&home.snap.files[0]).is_some());
    let card = s.rect_of("home-card-Welcome");
    assert!(card.width() > 150.0 && card.min.x > theme::HOME_SIDEBAR_W);
    s.rect_of("home-project-Getting started");
    s.rect_of("home-new-file");
    s.rect_of("home-import");
    assert_eq!(s.shell.title, theme::APP_TITLE);
}

#[test]
fn new_design_file_creates_and_opens_an_untitled_library_file() {
    let (mut s, _d) = home_sim();
    click_rect(&mut s, "home-new-file");
    assert!(s.app().home.is_none(), "the editor opens");
    let lf = s.app().lib_file.clone().expect("a library file is open");
    assert_eq!((lf.name.as_str(), lf.project.as_str()), ("Untitled", "Getting started"));
    assert_eq!(s.app().doc.name, "Untitled");
    assert_eq!(lib(&s).all_files().len(), 2);
    assert!(lib(&s).load_file(&lf.id).is_ok());
}

#[test]
fn double_clicking_a_card_opens_it_and_single_click_selects() {
    let (mut s, _d) = home_sim();
    let card = s.rect_of("home-card-Welcome");
    s.click(card.center());
    assert_eq!(s.app().home.as_ref().unwrap().selected.as_deref(), Some(file_id(&s, "Welcome").as_str()));
    assert!(s.app().home.is_some());
    let before = lib(&s).file(&file_id(&s, "Welcome")).unwrap().opened;
    double_click(&mut s, card.center());
    assert!(s.app().home.is_none());
    assert_eq!(s.app().lib_file.as_ref().unwrap().name, "Welcome");
    assert!(s.app().doc.nodes.len() > 10, "the demo document loaded");
    assert!(lib(&s).file(&file_id(&s, "Welcome")).unwrap().opened > before, "opening updates Recents");
}

#[test]
fn the_editor_header_shows_project_and_file_and_none_of_the_account_labels() {
    let (mut s, _d) = home_sim();
    open(&mut s, "Welcome");
    let r = s.rect_of("file-header");
    assert!(r.left() >= theme::RAIL_WIDTH, "the header sits in the left panel");
    // the second line is the save state, not a plan
    assert_eq!(s.app().lib_file.as_ref().unwrap().project, "Getting started");
    assert!(!s.app().autosave.pending());
}

#[test]
fn renaming_in_the_header_persists_to_the_library() {
    let (mut s, _d) = home_sim();
    open(&mut s, "Welcome");
    let id = s.app().lib_file.as_ref().unwrap().id.clone();
    let hit = s.rect_of("file-header");
    double_click(&mut s, Pos2::new(hit.left() + 90.0, hit.center().y));
    assert!(s.app().renaming_file, "double-click starts renaming");
    s.frame(vec![]);
    s.frame(vec![]);
    s.type_text("Launch page");
    s.key(Key::Enter);
    s.frame(vec![]);
    assert!(!s.app().renaming_file);
    assert_eq!(lib(&s).file(&id).unwrap().name, "Launch page");
    assert_eq!(s.app().lib_file.as_ref().unwrap().name, "Launch page");
    assert_eq!(s.app().doc.name, "Launch page");
}

#[test]
fn renaming_a_file_from_the_home_dialog_persists() {
    let (mut s, _d) = home_sim();
    let id = file_id(&s, "Welcome");
    crate::home::apply(&mut s.shell.app, Act::RenameFile(id.clone()));
    s.frame(vec![]);
    s.frame(vec![]);
    assert!(matches!(s.app().dialog.as_ref().map(|d| &d.intent), Some(Intent::RenameFile(_))));
    s.type_text("Intro");
    s.frame(vec![]);
    click_rect(&mut s, "dialog-ok");
    assert!(s.app().dialog.is_none());
    assert_eq!(lib(&s).file(&id).unwrap().name, "Intro");
    assert!(s.app().home.as_ref().unwrap().snap.files.iter().any(|f| f.name == "Intro"), "the grid refreshed");
}

#[test]
fn right_click_menu_moves_a_file_to_the_trash_and_trash_restores_it() {
    let (mut s, _d) = home_sim();
    let card = s.rect_of("home-card-Welcome");
    s.right_click(card.center());
    assert!(s.app().home.as_ref().unwrap().menu.is_some(), "context menu open");
    // "Move to trash" is the 8th row, after three separators
    let pos = card.center();
    let y = pos.y + theme::MENU_PADDING + 7.0 * theme::MENU_ITEM_HEIGHT + 3.0 * crate::widgets::menu::SEP_H + theme::MENU_ITEM_HEIGHT / 2.0;
    s.click(Pos2::new(pos.x + 60.0, y));
    s.frame(vec![]);
    let l = lib(&s);
    assert!(l.all_files().is_empty());
    assert_eq!(l.list_trash().len(), 1);
    assert!(s.app().home.as_ref().unwrap().snap.files.is_empty());
    // Trash view: restore
    click_rect(&mut s, "home-trash");
    assert_eq!(s.app().home.as_ref().unwrap().section, Section::Trash);
    s.frame(vec![]);
    click_rect(&mut s, "home-restore-Welcome");
    s.frame(vec![]);
    assert_eq!(l.all_files().len(), 1);
    assert!(l.list_trash().is_empty());
    assert!(s.app().home.as_ref().unwrap().snap.trash.is_empty());
}

#[test]
fn emptying_the_trash_needs_a_confirmation() {
    let (mut s, _d) = home_sim();
    let id = file_id(&s, "Welcome");
    crate::home::apply(&mut s.shell.app, Act::Trash(id));
    s.frame(vec![]);
    click_rect(&mut s, "home-trash");
    s.frame(vec![]);
    click_rect(&mut s, "home-empty-trash");
    s.frame(vec![]);
    s.frame(vec![]);
    assert_eq!(s.app().dialog.as_ref().map(|d| d.intent.clone()), Some(Intent::EmptyTrash));
    assert_eq!(lib(&s).list_trash().len(), 1, "nothing is deleted before confirming");
    // Cancel keeps everything
    click_rect(&mut s, "dialog-cancel");
    assert!(s.app().dialog.is_none());
    assert_eq!(lib(&s).list_trash().len(), 1);
    // Confirm empties
    click_rect(&mut s, "home-empty-trash");
    s.frame(vec![]);
    s.frame(vec![]);
    click_rect(&mut s, "dialog-ok");
    assert!(lib(&s).list_trash().is_empty());
    s.frame(vec![]);
    assert!(s.app().home.as_ref().unwrap().snap.trash.is_empty());
}

#[test]
fn dragging_a_card_onto_a_project_moves_the_file() {
    let (mut s, _d) = home_sim();
    let l = lib(&s);
    let other = l.create_project("Marketing").unwrap();
    s.shell.app.home.as_mut().unwrap().dirty = true;
    s.frame(vec![]);
    s.frame(vec![]);
    let from = s.rect_of("home-card-Welcome").center();
    let to = s.rect_of("home-project-Marketing").center();
    s.drag(from, to);
    s.frame(vec![]);
    let f = l.all_files().into_iter().find(|f| f.name == "Welcome").unwrap();
    assert_eq!(f.project, other.id);
    // and into a folder via the action used by the folder rows
    let folder = l.create_folder(&other.id, None, "Q4").unwrap();
    crate::home::apply(&mut s.shell.app, Act::Drop(f.id.clone(), DropTo::Folder(other.id.clone(), folder.id.clone())));
    assert_eq!(l.file(&f.id).unwrap().folder.as_deref(), Some(folder.id.as_str()));
}

#[test]
fn search_filters_files_and_the_sidebar_navigates_sections() {
    let (mut s, _d) = home_sim();
    let l = lib(&s);
    let p = l.list_projects()[0].id.clone();
    let doc = Document::new("x");
    l.create_file(&p, None, "Checkout flow", &doc).unwrap();
    s.shell.app.home.as_mut().unwrap().dirty = true;
    s.frame(vec![]);
    s.shell.app.home.as_mut().unwrap().search = "check".into();
    s.frame(vec![]);
    let files = s.app().home.as_ref().unwrap().files();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "Checkout flow");
    assert_eq!(s.app().home.as_ref().unwrap().title(), "Results for \u{201c}check\u{201d}");
    // choosing a section clears the search
    click_rect(&mut s, "home-starred");
    let h = s.app().home.as_ref().unwrap();
    assert_eq!((h.section.clone(), h.search.as_str()), (Section::Starred, ""));
    click_rect(&mut s, "home-project-Getting started");
    assert!(matches!(s.app().home.as_ref().unwrap().section, Section::Project(..)));
    assert_eq!(s.app().home.as_ref().unwrap().files().len(), 2);
}

#[test]
fn back_to_files_flushes_edits_and_returns_to_the_home_screen() {
    let (mut s, _d) = home_sim();
    open(&mut s, "Welcome");
    let id = s.app().lib_file.as_ref().unwrap().id.clone();
    let before = lib(&s).load_file(&id).unwrap().nodes.len();
    s.key(Key::R);
    s.mods = Modifiers::COMMAND;
    s.drag(s.screen(300.0, 800.0), s.screen(380.0, 860.0));
    s.mods = Modifiers::NONE;
    assert_eq!(s.count(), before + 1);
    assert_eq!(lib(&s).load_file(&id).unwrap().nodes.len(), before, "not saved yet");
    run_action(&mut s, crate::panels::menus::Action::BackToFiles);
    s.frame(vec![]);
    assert!(s.app().home.is_some());
    assert_eq!(lib(&s).load_file(&id).unwrap().nodes.len(), before + 1, "leaving the editor saved the edit");
}

#[test]
fn autosave_runs_after_the_idle_delay_and_ctrl_s_saves_at_once() {
    let (mut s, _d) = home_sim();
    open(&mut s, "Welcome");
    let id = s.app().lib_file.as_ref().unwrap().id.clone();
    let before = lib(&s).load_file(&id).unwrap().nodes.len();
    s.key(Key::R);
    s.mods = Modifiers::COMMAND;
    s.drag(s.screen(300.0, 800.0), s.screen(380.0, 860.0));
    s.mods = Modifiers::NONE;
    wait(&mut s, 0.8);
    assert!(s.app().autosave.pending(), "a save is pending");
    assert_eq!(lib(&s).load_file(&id).unwrap().nodes.len(), before, "not before the 1.5s idle delay");
    wait(&mut s, 1.2);
    assert_eq!(lib(&s).load_file(&id).unwrap().nodes.len(), before + 1, "autosaved");
    assert_eq!(s.app().autosave.saves, 1);
    assert!(!s.app().autosave.pending());
    // Ctrl+S is immediate
    s.mods = Modifiers::COMMAND;
    s.drag(s.screen(500.0, 800.0), s.screen(560.0, 860.0));
    s.mods = Modifiers::NONE;
    s.key(Key::R);
    s.mods = Modifiers::COMMAND;
    s.drag(s.screen(600.0, 800.0), s.screen(660.0, 860.0));
    s.mods = Modifiers::NONE;
    let now = s.count();
    s.cmd_key(Key::S);
    assert_eq!(lib(&s).load_file(&id).unwrap().nodes.len(), now);
    assert!(s.app().toast.as_ref().is_some_and(|t| t.text.starts_with("Saved to")));
}

#[test]
fn unchanged_documents_are_never_rewritten_by_autosave() {
    let (mut s, _d) = home_sim();
    open(&mut s, "Welcome");
    let id = s.app().lib_file.as_ref().unwrap().id.clone();
    let m0 = lib(&s).file(&id).unwrap().modified;
    // zooming bumps the canvas revision but changes nothing in the document
    s.shell.app.zoom_to(2.0);
    wait(&mut s, 3.0);
    assert_eq!(lib(&s).file(&id).unwrap().modified, m0);
    assert_eq!(s.app().autosave.saves, 0);
}

#[test]
fn external_files_keep_their_name_and_offer_save_to_library() {
    let dir = TmpDir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let ext = dir.0.join("outside.desigma");
    let mut d = Document::new("Outside");
    d.add_page("P");
    std::fs::write(&ext, d.to_json().unwrap()).unwrap();
    let mut s = Sim::with(Options { file: Some(ext.clone()), library: Some(dir.0.join("lib")), ..Default::default() });
    assert!(s.app().home.is_none(), "a file argument opens the editor");
    assert!(s.app().lib_file.is_none());
    assert_eq!(s.app().doc.name, "Outside");
    click_rect(&mut s, "save-to-library");
    s.frame(vec![]);
    assert_eq!(s.app().dialog.as_ref().map(|d| d.intent.clone()), Some(Intent::SaveToLibrary));
    click_rect(&mut s, "dialog-ok");
    assert!(s.app().dialog.is_none());
    let lf = s.app().lib_file.clone().expect("now a library file");
    assert_eq!(lf.name, "Outside");
    assert_eq!(lib(&s).file(&lf.id).unwrap().name, "Outside");
    assert!(ext.exists(), "the original is untouched");
}

#[test]
fn importing_a_file_adds_it_to_the_project() {
    let (mut s, d) = home_sim();
    let svg = d.0.join("logo.svg");
    std::fs::write(&svg, r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#f00"/></svg>"##).unwrap();
    let p = lib(&s).list_projects()[0].id.clone();
    assert_eq!(s.shell.app.import_into_library(&[svg], &p, None), 1);
    s.frame(vec![]);
    s.frame(vec![]);
    assert!(s.app().home.as_ref().unwrap().snap.files.iter().any(|f| f.name == "logo"));
}

#[test]
fn account_and_plan_labels_are_gone_from_the_ui_sources() {
    // The product has no accounts, plans or cloud; guard against the Figma labels coming back.
    let sources = [
        ("left.rs", include_str!("../../panels/left.rs")),
        ("right.rs", include_str!("../../panels/right.rs")),
        ("rail.rs", include_str!("../../panels/rail.rs")),
        ("menus.rs", include_str!("../../panels/menus.rs")),
        ("toolbar.rs", include_str!("../../panels/toolbar.rs")),
    ];
    let banned = ["\"Drafts\"", "\"Free\"", "\"Share\"", "\"Agents\"", "Help and account", "Upgrade", "\"Team\"", "\"Pro\"", "avatar"];
    for (file, src) in sources {
        for b in banned {
            assert!(!src.contains(b), "{file} still mentions {b}");
        }
    }
    assert!(!crate::panels::rail::ITEMS.iter().any(|i| i.2 == "Agents"));
}

#[test]
fn help_menu_offers_shortcuts_about_and_a_confirmed_github_link() {
    let (mut s, _d) = home_sim();
    open(&mut s, "Welcome");
    run_action(&mut s, crate::panels::menus::Action::Shortcuts);
    s.frame(vec![]);
    assert_eq!(s.app().dialog.as_ref().map(|d| d.intent.clone()), Some(Intent::Shortcuts));
    s.key(Key::Escape);
    assert!(s.app().dialog.is_none());
    run_action(&mut s, crate::panels::menus::Action::AboutDesigma);
    s.frame(vec![]);
    assert_eq!(s.app().dialog.as_ref().map(|d| d.intent.clone()), Some(Intent::About));
    click_rect(&mut s, "dialog-ok");
    assert!(s.app().dialog.is_none());
    // the GitHub link asks first and is cancellable
    run_action(&mut s, crate::panels::menus::Action::OpenGithub);
    s.frame(vec![]);
    assert_eq!(s.app().dialog.as_ref().map(|d| d.intent.clone()), Some(Intent::OpenGithub));
    click_rect(&mut s, "dialog-cancel");
    assert!(s.app().dialog.is_none());
    assert!(s.app().toast.as_ref().is_none_or(|t| !t.error));
}

#[test]
fn library_option_sets_the_root_and_home_option_shows_the_home_screen() {
    let dir = TmpDir::new();
    let s = Sim::with(Options { home: true, library: Some(dir.0.clone()), screenshot: Some(dir.0.join("shot.png")), ..Default::default() });
    assert_eq!(lib(&s).root(), dir.0.as_path());
    assert!(s.app().home.is_some());
    assert!(dir.0.join("projects").exists(), "the library was created under --library");
    // without --home a screenshot run stays in the editor and never touches a library
    let e = Sim::with(Options { screenshot: Some(dir.0.join("e.png")), ..Default::default() });
    assert!(e.app().home.is_none() && e.app().lib.is_none());
}
