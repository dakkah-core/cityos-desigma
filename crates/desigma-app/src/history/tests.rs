//! Version history logic without a window: activity timing, saving, viewing, restoring.

use super::*;
use desigma_doc::{Node, Rect};

struct Tmp(std::path::PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("desigma_history_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app with a library file open, holding `n` rectangles.
fn app_with_file(t: &Tmp, rects: usize) -> App {
    let mut app = App::new(Document::new("t"));
    app.attach_library(t.0.clone());
    app.new_library_file(None);
    assert!(app.lib_file.is_some());
    for _ in 0..rects {
        add_rect(&mut app);
    }
    app
}

fn add_rect(app: &mut App) {
    let id = app.doc.alloc_id();
    let page = app.page;
    let cmd = desigma_doc::Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(id, Rect::new(0.0, 0.0, 10.0, 10.0))] };
    app.hist.exec(&mut app.doc, cmd).unwrap();
    app.changed();
}

fn count(app: &App) -> usize {
    app.doc.get(app.page).map(|p| p.children.len()).unwrap_or(0)
}

fn sealed_app_with_file(t: &Tmp, portable: bool) -> App {
    let mut app = app_with_file(t, 1);
    app.save_library_file().unwrap();
    crate::home::thumb::finish_library_jobs(&t.0);
    let library = app.lib.clone().unwrap();
    let file = app.lib_file.as_ref().unwrap().clone();
    let mut checkpoint = library.load_checkpoint(&file.id).unwrap();
    let mut next = checkpoint.document.clone();
    let id = next.alloc_id();
    next.insert(app.page, Node::rectangle(id, Rect::new(20., 0., 10., 10.))).unwrap();
    checkpoint.install_baseline(next);
    library.save_checkpoint(&file.id, &checkpoint).unwrap();
    let id = if portable {
        let text = library.export_checkpoint(&file.id).unwrap();
        let verified = desigma_library::VerifiedPortableCheckpoint::parse(&text).unwrap();
        library.import_checkpoint_as_new(&file.project_id, None, verified).unwrap().id
    } else { file.id };
    app.open_library_file(&id);
    crate::home::thumb::finish_library_jobs(&t.0);
    assert_eq!(count(&app), 2);
    assert!(app.hist.baseline_id().is_some());
    assert!(!app.hist.can_undo());
    app
}

fn retained_states(checkpoint: &desigma_doc::DocumentCheckpoint) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(&checkpoint.to_json().unwrap()).unwrap()["retained"].clone()
}

fn save_and_reopen_sealed_state(app: &mut App, rects: usize, baseline: &str, retained: &serde_json::Value) {
    app.save_library_file().unwrap();
    let root = app.lib.as_ref().unwrap().root().to_path_buf();
    crate::home::thumb::finish_library_jobs(&root);
    let file = app.lib_file.as_ref().unwrap().id.clone();
    let library = desigma_library::Library::open(&root).unwrap();
    let checkpoint = library.load_checkpoint(&file).unwrap();
    assert_eq!(checkpoint.history.baseline_id(), Some(baseline));
    assert_eq!(&retained_states(&checkpoint), retained, "retained baselines remain exact inactive archives");
    assert_eq!(serde_json::to_value(&checkpoint.history).unwrap(), serde_json::to_value(&app.hist).unwrap());
    let fingerprint = desigma_library::checkpoint_fingerprint(&checkpoint).unwrap();
    assert_eq!(app.autosave.saved_checkpoint_fingerprint.as_deref(), Some(fingerprint.as_str()));
    // Replace the editor itself, so the next Undo/Redo must come from disk.
    let mut reopened = App::new(Document::new("Reopen"));
    reopened.lib = Some(library);
    reopened.open_library_file(&file);
    assert_eq!(count(&reopened), rects);
    assert_eq!(reopened.doc.to_json().unwrap(), app.doc.to_json().unwrap());
    assert_eq!(serde_json::to_value(&reopened.hist).unwrap(), serde_json::to_value(&app.hist).unwrap());
    assert_eq!(reopened.hist.baseline_id(), Some(baseline));
    assert!(reopened.flush_autosave());
    crate::home::thumb::finish_library_jobs(&root);
    *app = reopened;
}

#[test]
fn sealed_native_and_portable_version_restore_undo_redo_save_and_reopen_keep_baseline_and_archives() {
    for portable in [false, true] {
        let t = Tmp::new(if portable { "sealed_portable_restore" } else { "sealed_native_restore" });
        let mut app = sealed_app_with_file(&t, portable);
        let library = app.lib.clone().unwrap();
        let file = app.lib_file.as_ref().unwrap().id.clone();
        let checkpoint = library.load_checkpoint(&file).unwrap();
        let retained = retained_states(&checkpoint);
        assert_eq!(checkpoint.retained.len(), 1);
        assert!(checkpoint.retained[0].history.can_undo());
        let baseline = app.hist.baseline_id().unwrap().to_owned();
        assert!(app.save_named_version("Sealed two rectangles", ""));
        let version = library.list_versions(&file)[0].n;
        add_rect(&mut app);
        app.save_library_file().unwrap();
        crate::home::thumb::finish_library_jobs(&t.0);
        let fence = app.hist.change_identity();
        let transition = app.hist.prepare_transition(false).unwrap();

        app.restore_version(version);
        assert_eq!(count(&app), 2, "ordinary sealed/portable restore must succeed");
        assert!(app.hist.next_is_document_restore(false));
        assert_eq!(app.hist.baseline_id(), Some(baseline.as_str()));
        assert!(!std::sync::Arc::ptr_eq(&fence, &app.hist.change_identity()));
        assert!(!app.hist.transition_current(&transition));
        assert!(app.hist.can_undo()); assert!(!app.hist.can_redo());
        save_and_reopen_sealed_state(&mut app, 2, &baseline, &retained);

        app.undo(); assert_eq!(count(&app), 3);
        assert!(app.hist.can_undo(), "the same-baseline pre-restore active history returns");
        save_and_reopen_sealed_state(&mut app, 3, &baseline, &retained);
        app.redo(); assert_eq!(count(&app), 2);
        save_and_reopen_sealed_state(&mut app, 2, &baseline, &retained);
        app.undo(); assert_eq!(count(&app), 3);
        app.undo(); assert_eq!(count(&app), 2);
        assert!(app.hist.can_redo()); assert!(!app.hist.can_undo(), "inactive archived Undo never becomes active");
        save_and_reopen_sealed_state(&mut app, 2, &baseline, &retained);
        app.redo(); assert_eq!(count(&app), 3);
        save_and_reopen_sealed_state(&mut app, 3, &baseline, &retained);
    }
}

fn bound_version(mut document: Document) -> Document {
    document.cityos = Some(desigma_doc::CityosBinding {
        schema_version: "cityos.design-exchange.v1".into(), document_id: "document".into(), draft_id: "draft".into(),
        base: desigma_doc::CityosBase::CanonicalGit { repository_id: "fixture".into(), commit: "a".repeat(40) },
        semantic_base: serde_json::json!({"owner":"retained"}), dependency_pins: vec![], occurrences: Default::default(),
        exchange_v2: None, exchange_v3: None,
    });
    document
}

#[test]
fn ordinary_restore_undo_redo_cannot_cross_a_sealed_or_bound_baseline() {
    let t = Tmp::new("sealed_restore_fence");
    let mut app = sealed_app_with_file(&t, false);
    let library = app.lib.clone().unwrap();
    let file = app.lib_file.as_ref().unwrap().id.clone();
    assert!(app.save_named_version("Before restore", ""));
    let version = library.list_versions(&file)[0].n;
    add_rect(&mut app);
    app.restore_version(version);
    assert!(app.hist.next_is_document_restore(false));
    for protected in [false, true] {
        let mut checkpoint = library.load_checkpoint(&file).unwrap();
        let next = if protected { bound_version(checkpoint.document.clone()) } else { checkpoint.document.clone() };
        checkpoint.install_baseline(next);
        library.save_checkpoint(&file, &checkpoint).unwrap();
        app.open_library_file(&file);
        let before = library.load_checkpoint(&file).unwrap().to_json().unwrap();
        let document = app.doc.to_json().unwrap();
        assert!(!app.hist.can_undo()); assert!(!app.hist.can_redo());
        assert!(!app.try_undo_restore()); assert!(!app.try_redo_restore());
        assert_eq!(app.doc.to_json().unwrap(), document);
        assert_eq!(library.load_checkpoint(&file).unwrap().to_json().unwrap(), before);
    }
}

#[test]
fn bound_versions_and_parked_owner_drafts_require_reviewed_baseline_recovery() {
    let t = Tmp::new("bound_restore_protected");
    let mut app = sealed_app_with_file(&t, false);
    let library = app.lib.clone().unwrap();
    let file = app.lib_file.as_ref().unwrap().id.clone();
    let ordinary = app.doc.clone();
    let bound = bound_version(ordinary.clone());
    let ordinary_version = library.save_version(&file, &ordinary, Some("Ordinary".into()), false).unwrap();
    let bound_version = library.save_version(&file, &bound, Some("Bound".into()), false).unwrap();
    let versions = library.list_versions(&file).len();
    let before = library.load_checkpoint(&file).unwrap().to_json().unwrap();
    app.restore_version(bound_version.n);
    assert_eq!(app.doc.to_json().unwrap(), ordinary.to_json().unwrap());
    assert!(!app.hist.next_is_document_restore(false));
    app.doc = bound.clone();
    app.restore_version(ordinary_version.n);
    assert_eq!(app.doc.to_json().unwrap(), bound.to_json().unwrap());
    app.doc = ordinary.clone();
    app.history.viewing = Some(Viewing { n: ordinary_version.n, title: "Ordinary".into(),
        saved: Snapshot { doc: bound, hist: app.hist.clone(), page: app.page, selection: vec![] } });
    app.restore_version(ordinary_version.n);
    assert!(app.history.viewing.is_some(), "a bound live state stays parked and protected");
    assert_eq!(library.load_checkpoint(&file).unwrap().to_json().unwrap(), before);
    assert_eq!(library.list_versions(&file).len(), versions, "denial creates no automatic version");
}

#[test]
fn ten_minutes_of_activity_make_an_auto_version() {
    let mut a = Activity::default();
    assert!(!a.due());
    // an edit every 30 seconds for ten minutes
    for i in 0..=20 {
        a.note_edit(i as f64 * 30.0);
    }
    assert!(a.due(), "{a:?}");
    a.reset();
    assert!(!a.due());
}

#[test]
fn pauses_do_not_count_as_activity() {
    let mut a = Activity::default();
    a.note_edit(0.0);
    a.note_edit(3600.0); // an hour away from the keyboard counts for at most a minute
    a.note_edit(7200.0);
    assert!(!a.due());
}

#[test]
fn labels_for_the_panel() {
    let day = 86_400_000u64;
    let now = 10 * day + 5_000;
    assert_eq!(day_label(now, now - 1000), "Today");
    assert_eq!(day_label(now, now - day - 1), "Yesterday");
    assert_eq!(day_label(now, now - 3 * day), "3 days ago");
    assert_eq!(day_label(now, 0), "1970-01-01");
    assert_eq!(stamp(1_000_000_000_123), "2001-09-09 01:46 UTC");
}

#[test]
fn files_outside_the_library_have_no_history() {
    let mut app = App::new(Document::new("t"));
    assert!(!app.has_history());
    app.begin_save_version();
    assert!(app.history.save_modal.is_none());
    assert!(app.toast.as_ref().is_some_and(|t| t.text == "Save to library to keep version history" && t.error));
    assert!(app.save_auto_version().is_none());
    assert!(!app.save_named_version("x", ""));
}

#[test]
fn auto_versions_only_when_something_changed() {
    let t = Tmp::new("auto");
    let mut app = app_with_file(&t, 0);
    assert!(app.save_auto_version().is_none(), "nothing changed since opening");
    add_rect(&mut app);
    let v = app.save_auto_version().expect("a version");
    assert!(v.auto && v.label.is_none());
    assert!(app.save_auto_version().is_none(), "unchanged since the last version");
    add_rect(&mut app);
    app.save_close_version();
    let list = app.lib.as_ref().unwrap().list_versions(&app.lib_file.as_ref().unwrap().id);
    assert_eq!(list.len(), 2);
}

#[test]
fn the_tick_saves_after_ten_minutes_of_edits() {
    let t = Tmp::new("tick");
    let mut app = app_with_file(&t, 1);
    for i in 0..=21 {
        app.now = i as f64 * 30.0;
        app.autosave.changed_at = Some(app.now);
        app.autosave.confirmed = true;
        app.history_tick();
        app.autosave.changed_at = None;
        app.autosave.confirmed = false;
        app.history_tick();
        add_rect(&mut app);
    }
    let id = app.lib_file.as_ref().unwrap().id.clone();
    assert_eq!(app.lib.as_ref().unwrap().list_versions(&id).len(), 1);
}

#[test]
fn save_view_and_restore_round_trip_with_one_undo() {
    let t = Tmp::new("restore");
    let mut app = app_with_file(&t, 2);
    assert!(app.save_named_version("Two rects", "before the redesign"));
    let id = app.lib_file.as_ref().unwrap().id.clone();
    let lib = app.lib.clone().unwrap();
    let named = lib.list_versions(&id);
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].label.as_deref(), Some("Two rects"));
    assert_eq!(named[0].description.as_deref(), Some("before the redesign"));
    assert!(!named[0].auto);

    add_rect(&mut app);
    add_rect(&mut app);
    assert_eq!(count(&app), 4);
    app.save_library_file().unwrap();

    // viewing shows the old document and leaves the live one alone
    app.view_version(named[0].n);
    assert_eq!(count(&app), 2);
    assert!(app.history.viewing.is_some());
    add_rect_via_tx(&mut app);
    assert_eq!(count(&app), 2, "the view is read-only");
    app.flush_autosave();
    assert_eq!(lib.load_file(&id).unwrap().get(app.page).unwrap().children.len(), 4, "the file on disk keeps the live document");
    app.exit_version_view();
    assert_eq!(count(&app), 4);
    assert!(app.hist.can_undo(), "the live undo stack survived the view");

    // restore: a new auto version of the 4-rect state, then the 2-rect state is current
    let before = lib.list_versions(&id).len();
    app.view_version(named[0].n);
    app.restore_version(named[0].n);
    assert!(app.history.viewing.is_none());
    assert_eq!(count(&app), 2);
    let after = lib.list_versions(&id);
    assert_eq!(after.len(), before + 1);
    let kept = after.iter().find(|v| v.auto).expect("pre-restore auto version");
    assert_eq!(lib.load_version(&id, kept.n).unwrap().get(app.page).unwrap().children.len(), 4);
    assert_eq!(lib.load_file(&id).unwrap().get(app.page).unwrap().children.len(), 2, "the file on disk was restored too");

    // one undo returns to the pre-restore state, redo restores again
    app.undo();
    assert_eq!(count(&app), 4);
    assert_eq!(lib.load_file(&id).unwrap().get(app.page).unwrap().children.len(), 4);
    app.redo();
    assert_eq!(count(&app), 2);
    app.undo();
    assert_eq!(count(&app), 4);
    // ... and the older undo history is there again
    app.undo();
    assert_eq!(count(&app), 3);
}

fn add_rect_via_tx(app: &mut App) {
    let mut tx = crate::edit::Tx::new();
    let id = app.doc.alloc_id();
    let page = app.page;
    tx.cmd(&mut app.doc, desigma_doc::Command::Insert { parent: page, index: 0, nodes: vec![Node::rectangle(id, Rect::new(0.0, 0.0, 5.0, 5.0))] }).unwrap();
    app.run_tx(tx);
}

#[test]
fn an_edit_after_a_restore_keeps_the_restore_undoable_last() {
    let t = Tmp::new("restore_edit");
    let mut app = app_with_file(&t, 1);
    assert!(app.save_named_version("one", ""));
    let n = app.lib.as_ref().unwrap().list_versions(&app.lib_file.as_ref().unwrap().id)[0].n;
    add_rect(&mut app);
    add_rect(&mut app);
    app.restore_version(n);
    assert_eq!(count(&app), 1);
    add_rect(&mut app);
    assert_eq!(count(&app), 2);
    app.undo();
    assert_eq!(count(&app), 1, "first the edit");
    app.undo();
    assert_eq!(count(&app), 3, "then the restore");
}

#[test]
fn a_stale_redo_after_undoing_a_restore_is_dropped() {
    let t = Tmp::new("stale_redo");
    let mut app = app_with_file(&t, 1);
    assert!(app.save_named_version("one", ""));
    let n = app.lib.as_ref().unwrap().list_versions(&app.lib_file.as_ref().unwrap().id)[0].n;
    add_rect(&mut app);
    app.restore_version(n);
    app.undo();
    assert_eq!(count(&app), 2);
    add_rect(&mut app); // a new edit invalidates the redo of the restore
    app.redo();
    assert_eq!(count(&app), 3, "redo did not jump back to the restored document");
}

#[test]
fn naming_duplicating_and_deleting_versions() {
    let t = Tmp::new("manage");
    let mut app = app_with_file(&t, 1);
    let lib = app.lib.clone().unwrap();
    let id = app.lib_file.as_ref().unwrap().id.clone();
    add_rect(&mut app);
    let v = app.save_auto_version().unwrap();
    app.name_version(v.n, " Milestone ");
    let named = lib.list_versions(&id);
    assert_eq!(named[0].label.as_deref(), Some("Milestone"));
    assert!(named[0].is_named());

    let files_before = lib.all_files().len();
    app.duplicate_version(v.n);
    let files = lib.all_files();
    assert_eq!(files.len(), files_before + 1);
    let copy = files.iter().find(|f| f.name.contains("Milestone")).expect("duplicate named after the version");
    assert_eq!(lib.load_file(&copy.id).unwrap().pages.len(), 1);
    assert_eq!(copy.project, app.lib_file.as_ref().unwrap().project_id);

    app.delete_version(v.n);
    assert!(lib.list_versions(&id).is_empty());
    assert!(t.0.join("trash").read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("-v")), "moved to the trash");
}

#[test]
fn leaving_the_file_saves_a_closing_version_and_drops_the_view() {
    let t = Tmp::new("leave");
    let mut app = app_with_file(&t, 1);
    let id = app.lib_file.as_ref().unwrap().id.clone();
    let lib = app.lib.clone().unwrap();
    assert!(app.save_named_version("v", ""));
    add_rect(&mut app);
    let n = lib.list_versions(&id)[0].n;
    app.view_version(n);
    app.go_home();
    assert!(app.history.viewing.is_none());
    let list = lib.list_versions(&id);
    assert_eq!(list.len(), 2, "a closing auto version was added");
    assert_eq!(lib.load_file(&id).unwrap().get(app.page).map(|p| p.children.len()), Some(2), "the live document, not the viewed one, is on disk");
}
