//! GUI-independent editing, layout, text measurement, and derived updates.
//!
//! Upstream engine integrated from bb09ba87b1b7e7246b5dfbf916d75e3f12abe870.
//! Local adaptations fence managed documents without interpreting their editing policy.
pub mod components;
pub mod connect;
pub mod edit;
pub mod layout;
mod resize;
pub mod table;
pub mod textmeasure;
mod util;
pub mod variables;

pub use resize::apply_resize;

#[cfg(test)]
mod managed_document_regressions {
    use super::*;
    use desigma_doc::*;
    use edit::Tx;
    use std::collections::HashSet;

    // A valid saved binding is enough to fence native editing; it confers no authority.
    // Parse through the core API so the engine needs no extension or JSON dependency.
    fn manage(doc: &mut Document) {
        let saved = r#"{
            "schemaVersion": "desigma.document.v3",
            "document": {
                "name": "managed fixture", "nodes": {}, "pages": [],
                "styles": {}, "images": {}, "next_id": 0,
                "cityos": {
                    "schema_version": "cityos.design-exchange.v2",
                    "document_id": "document", "draft_id": "draft",
                    "base": {"kind": "canonical-git", "repository_id": "repo",
                        "commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                    "semantic_base": {}, "dependency_pins": [], "occurrences": {},
                    "exchange_v2": {
                        "schemaVersion": "cityos.design-exchange.v2",
                        "documentId": "document", "draftId": "draft", "draftRevision": 0,
                        "base": {"kind": "canonical-git", "repositoryId": "repo",
                            "commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                        "semanticBase": {}, "semanticDraft": {}
                    }
                }
            }
        }"#;
        doc.cityos = Document::from_json(saved).unwrap().cityos;
        assert!(doc.is_managed());
    }

    fn frame_doc() -> (Document, NodeId, NodeId, NodeId) {
        let mut doc = Document::new("native");
        let page = doc.add_page("Page");
        let frame = doc.alloc_id();
        doc.insert(page, Node::frame(frame, Rect::new(0.0, 0.0, 100.0, 100.0))).unwrap();
        let child = doc.alloc_id();
        let mut node = Node::rectangle(child, Rect::new(80.0, 10.0, 10.0, 10.0));
        node.constraints.horizontal = Constraint::Max;
        doc.insert(frame, node).unwrap();
        (doc, page, frame, child)
    }

    fn variable_doc() -> (Document, NodeId, HashSet<String>) {
        let (mut doc, _, _, child) = frame_doc();
        let mut store = VariableStore::default();
        let collection = store.add_collection("Sizes");
        let variable = store.add_variable(&collection, "Width", VariableKind::Number).unwrap();
        let mode = store.collections[&collection].default_mode.clone();
        assert!(store.set_value(&variable, &mode, VariableValue::Number(42.0)));
        store.command().apply(&mut doc).unwrap();
        doc.get_mut(child).unwrap().variables.bindings.insert("width".into(), variable.clone());
        (doc, child, HashSet::from([variable]))
    }

    fn table_doc() -> (Document, NodeId) {
        let mut doc = Document::new("table");
        doc.kind = DocumentKind::Board;
        let page = doc.add_page("Board");
        let nodes = table_nodes(&mut doc, Vec2::ZERO, 2, 2);
        let table = nodes[0].id;
        Command::Insert { parent: page, index: 0, nodes }.apply(&mut doc).unwrap();
        (doc, table)
    }

    #[test]
    fn managed_edit_resize_and_layout_leave_document_and_history_untouched() {
        let (mut doc, _, frame, child) = frame_doc();
        manage(&mut doc);
        let before = doc.to_json().unwrap();
        let mut tx = Tx::new();
        assert!(tx.ed.node(&mut doc, child).is_none());
        apply_resize(&mut doc, &mut tx.ed, frame, Vec2::new(150.0, 120.0), Affine::translate(5.0, 6.0), true);
        layout::apply_constraints(&mut doc, &mut tx.ed, frame, Vec2::new(100.0, 100.0), Vec2::new(200.0, 200.0));
        layout::relayout_around(&mut doc, &mut tx.ed, child);
        assert!(tx.ed.touched().is_empty());
        let mut history = History::default();
        assert!(!tx.commit(&mut doc, &mut history).unwrap());
        assert_eq!(doc.to_json().unwrap(), before);
        assert!(!history.can_undo() && !history.can_redo());
    }

    #[test]
    fn managed_direct_variable_reflow_does_not_write_resolved_properties() {
        let (mut doc, child, variables) = variable_doc();
        manage(&mut doc);
        let before = doc.to_json().unwrap();
        variables::reflow_bound(&mut doc, &variables, &[child]);
        assert_eq!(doc.to_json().unwrap(), before);
        assert_eq!(doc.get(child).unwrap().size.x, 10.0);
    }

    #[test]
    fn stale_ordinary_edit_cancellation_cannot_replace_a_managed_projection() {
        let (mut doc, _, _, child) = frame_doc();
        let mut edit = edit::Edit::new();
        edit.touch(&doc, child);
        // A new projection arrives after an ordinary gesture captured this ID.
        // Cancellation must not restore that gesture's unrelated old bytes.
        doc.get_mut(child).unwrap().opacity = 0.4;
        manage(&mut doc);
        let projected = doc.to_json().unwrap();
        edit.revert(&mut doc);
        assert_eq!(doc.to_json().unwrap(), projected);
    }

    #[test]
    fn ordinary_direct_variable_reflow_still_applies_resolved_properties() {
        let (mut doc, child, variables) = variable_doc();
        assert!(!doc.is_managed());
        assert_eq!(variables::bound_nodes(&doc, &variables), vec![child]);
        variables::reflow_bound(&mut doc, &variables, &[child]);
        assert_eq!(doc.get(child).unwrap().size.x, 42.0);
    }

    #[test]
    fn managed_row_and_column_insertions_preserve_the_allocator() {
        for column in [false, true] {
            let (mut doc, table) = table_doc();
            manage(&mut doc);
            let before = doc.to_json().unwrap();
            let expected_next = doc.clone().alloc_id();
            let mut tx = Tx::new();
            let inserted = if column { table::insert_col(&mut doc, &mut tx, table, 1) } else { table::insert_row(&mut doc, &mut tx, table, 1) };
            assert!(!inserted);
            let mut history = History::default();
            assert!(!tx.commit(&mut doc, &mut history).unwrap());
            // Serialization includes next_id, not just the visible node tree.
            assert_eq!(doc.to_json().unwrap(), before);
            assert_eq!(doc.alloc_id(), expected_next);
            assert!(!history.can_undo() && !history.can_redo());
        }
    }

    #[test]
    fn managed_table_removal_grid_edits_and_resize_are_inert() {
        let (mut doc, table) = table_doc();
        manage(&mut doc);
        let before = doc.to_json().unwrap();
        let mut tx = Tx::new();
        assert!(!table::remove_row(&mut doc, &mut tx, table, 0));
        assert!(!table::remove_col(&mut doc, &mut tx, table, 0));
        table::set_row_height(&mut doc, &mut tx, table, 0, 90.0);
        table::set_col_width(&mut doc, &mut tx, table, 0, 250.0);
        apply_resize(&mut doc, &mut tx.ed, table, Vec2::new(800.0, 224.0), Affine::IDENTITY, true);
        table::sync_table(&mut doc, &mut tx.ed, table);
        let mut history = History::default();
        assert!(!tx.commit(&mut doc, &mut history).unwrap());
        assert_eq!(doc.to_json().unwrap(), before);
        assert!(!history.can_undo() && !history.can_redo());
    }

    #[test]
    fn managed_sync_never_speculatively_installs_or_increments_revisions() {
        let (mut doc, _, _, _) = frame_doc();
        manage(&mut doc);
        let before = doc.to_json().unwrap();
        let mut next = doc.clone();
        next.name = "installed projection".into();
        let command = components::with_sync(&mut doc, Command::CityosInstall { document: Box::new(next) });
        assert!(matches!(&command, Command::CityosInstall { .. }));
        assert_eq!(doc.to_json().unwrap(), before);
        let mut history = History::default();
        history.exec(&mut doc, command).unwrap();
        let revision = |doc: &Document| doc.cityos.as_ref().unwrap().exchange().unwrap()["draftRevision"].as_u64().unwrap();
        assert_eq!(revision(&doc), 1);
        assert_eq!(doc.name, "installed projection");
        assert!(history.undo(&mut doc).unwrap());
        assert_eq!(revision(&doc), 2);
        assert_eq!(doc.name, "native");
        assert!(history.redo(&mut doc).unwrap());
        assert_eq!(revision(&doc), 3);
        assert_eq!(doc.name, "installed projection");
    }

    #[test]
    fn ordinary_component_sync_still_updates_instances_in_one_undo_step() {
        let (mut doc, page, main, child) = frame_doc();
        let create = create_component(&mut doc, main);
        create.apply(&mut doc).unwrap();
        let instance = create_instance(&mut doc, main, page, Vec2::new(200.0, 0.0));
        instance.apply(&mut doc).unwrap();
        let instance = *doc.get(page).unwrap().children.last().unwrap();
        let copy = doc.get(instance).unwrap().children[0];
        let mut changed = doc.get(child).unwrap().clone();
        changed.opacity = 0.25;
        let command = components::with_sync(&mut doc, Command::Update { node: Box::new(changed) });
        assert_eq!(doc.get(child).unwrap().opacity, 1.0);
        assert_eq!(doc.get(copy).unwrap().opacity, 1.0);
        let mut history = History::default();
        history.exec(&mut doc, command).unwrap();
        assert_eq!(doc.get(child).unwrap().opacity, 0.25);
        assert_eq!(doc.get(copy).unwrap().opacity, 0.25);
        assert!(history.undo(&mut doc).unwrap());
        assert_eq!(doc.get(child).unwrap().opacity, 1.0);
        assert_eq!(doc.get(copy).unwrap().opacity, 1.0);
        assert!(!history.can_undo());
        assert!(history.redo(&mut doc).unwrap());
        assert_eq!(doc.get(copy).unwrap().opacity, 0.25);
    }

    #[test]
    fn ordinary_table_resize_scales_cells_and_undoes_together() {
        let (mut doc, table) = table_doc();
        let old_size = doc.get(table).unwrap().size;
        let mut tx = Tx::new();
        apply_resize(&mut doc, &mut tx.ed, table, Vec2::new(800.0, 224.0), Affine::IDENTITY, false);
        let last = doc.get(table).unwrap().children[3];
        assert_eq!(doc.get(last).unwrap().size, Vec2::new(400.0, 112.0));
        let mut history = History::default();
        assert!(tx.commit(&mut doc, &mut history).unwrap());
        assert!(history.undo(&mut doc).unwrap());
        assert_eq!(doc.get(table).unwrap().size, old_size);
        assert_eq!(doc.get(last).unwrap().size, Vec2::new(TABLE_COL_W, TABLE_ROW_H));
        assert!(!history.can_undo());
    }

    #[test]
    fn ordinary_section_resize_keeps_children_in_board_space() {
        let mut doc = Document::new("section");
        doc.kind = DocumentKind::Board;
        let page = doc.add_page("Board");
        let section = doc.alloc_id();
        doc.insert(page, Node::board_section(section, Rect::new(10.0, 20.0, 300.0, 200.0), "Ideas")).unwrap();
        let child = doc.alloc_id();
        doc.insert(section, Node::rectangle(child, Rect::new(5.0, 6.0, 10.0, 10.0))).unwrap();
        let world = doc.world_transform(child);
        let mut tx = Tx::new();
        apply_resize(&mut doc, &mut tx.ed, section, Vec2::new(400.0, 300.0), Affine::IDENTITY, false);
        assert_eq!(doc.world_transform(child), world);
        let mut history = History::default();
        assert!(tx.commit(&mut doc, &mut history).unwrap());
        assert!(history.undo(&mut doc).unwrap());
        assert_eq!(doc.world_transform(child), world);
        assert_eq!(doc.get(section).unwrap().transform, Affine::translate(10.0, 20.0));
        assert!(!history.can_undo());
    }
}
