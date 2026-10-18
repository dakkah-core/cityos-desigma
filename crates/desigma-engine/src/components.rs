use desigma_doc::*;
use std::collections::BTreeMap;
fn is_noop(c: &Command) -> bool {
    matches!(c, Command::Batch(v) if v.is_empty())
}

fn ids_of(doc: &Document, cmd: &Command, out: &mut Vec<NodeId>) {
    match cmd {
        Command::CityosInstall { .. } | Command::RestoreDocument { .. } => {},
        Command::Insert { parent, nodes, .. } => {
            out.push(*parent);
            out.extend(nodes.iter().map(|n| n.id));
        }
        Command::Remove { id } => {
            out.push(*id);
            out.extend(doc.get(*id).and_then(|n| n.parent));
        }
        Command::Reparent { id, parent, .. } => {
            out.push(*id);
            out.push(*parent);
            out.extend(doc.get(*id).and_then(|n| n.parent));
        }
        Command::Update { node } => {
            out.push(node.id);
            out.extend(doc.get(node.id).and_then(|n| n.parent));
        }
        Command::Batch(v) => v.iter().for_each(|c| ids_of(doc, c, out)),
        Command::InsertPage { .. }
        | Command::RemovePage { .. }
        | Command::SetFlows { .. }
        | Command::SetVariables { .. }
        | Command::SetGuides { .. }
        | Command::SetPrototypeDevice { .. }
        | Command::SetBoardSession { .. }
        | Command::SetComments { .. }
        | Command::SetLibraryState { .. }
        | Command::SetStyles { .. } => {}
    }
}

/// The component (or set) whose instances must follow an edit to `id`.
fn owner_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    match doc.get(id)?.kind {
        NodeKind::ComponentSet(_) => Some(id),
        _ => enclosing_component(doc, id),
    }
}

/// Flags the fields a user edit changed on nodes that mirror a main component,
/// so later syncs keep them. `before` holds the pre-edit state of touched nodes.
pub fn record_overrides(doc: &Document, before: &BTreeMap<NodeId, Node>, cmds: &mut [Command]) {
    for c in cmds.iter_mut() {
        let Command::Update { node } = c else { continue };
        let Some(old) = before.get(&node.id) else { continue };
        if !is_linked(node) || old.component.source != node.component.source {
            continue;
        }
        let mut ov = record_override(old, node);
        if is_top_instance(node) {
            // Moving, hiding or locking an instance is not an override of its main.
            for f in [OverrideField::Transform, OverrideField::Visible, OverrideField::Locked] {
                ov.remove(&f);
            }
        } else {
            // Constraint relayout after resizing the instance is not a user override.
            if let Some(root) = instance_root_of(doc, node.id) {
                if before.get(&root).zip(doc.get(root)).is_some_and(|(a, b)| a.size != b.size) {
                    ov.remove(&OverrideField::Size);
                    ov.remove(&OverrideField::Transform);
                }
            }
            // Positions and fitted sizes inside auto layout come from the layout pass.
            let parent_al = node.parent.and_then(|p| doc.get(p)).and_then(|p| p.kind.frame_props()).is_some_and(|f| f.auto_layout.is_some());
            if parent_al {
                ov.remove(&OverrideField::Transform);
                let derived = node.layout_child.horizontal != Sizing::Fixed
                    || node.layout_child.vertical != Sizing::Fixed
                    || matches!(&node.kind, NodeKind::Text(t) if t.auto_resize != TextAutoResize::None);
                if derived {
                    ov.remove(&OverrideField::Size);
                }
            } else if matches!(&node.kind, NodeKind::Text(t) if t.auto_resize == TextAutoResize::WidthAndHeight) {
                ov.remove(&OverrideField::Size);
            }
        }
        node.component.overrides.extend(ov);
    }
}

/// Wraps `cmd` so that editing a main component also re-syncs its instances in
/// the same undo step. Cascades through components nested in other components.
pub fn with_sync(doc: &mut Document, cmd: Command) -> Command {
    // Speculative apply/reversal is unsafe for managed commands with monotonic revisions.
    // A saved snapshot already owns the complete component/instance state.
    // Deriving extra sync commands would change the document being restored.
    fn restores_document(cmd: &Command) -> bool {
        match cmd {
            Command::RestoreDocument { .. } => true,
            Command::Batch(commands) => commands.iter().any(restores_document),
            _ => false,
        }
    }
    if doc.is_managed() || restores_document(&cmd) {
        return cmd;
    }
    let mut ids = Vec::new();
    ids_of(doc, &cmd, &mut ids);
    let mut queue: Vec<NodeId> = ids.iter().filter_map(|i| owner_of(doc, *i)).collect();
    let Ok(inv) = cmd.clone().apply(doc) else { return cmd };
    queue.extend(ids.iter().filter_map(|i| owner_of(doc, *i)));
    queue.sort();
    queue.dedup();
    let mut syncs = Vec::new();
    let mut invs = Vec::new();
    let mut rounds = 0;
    while let Some(o) = queue.pop() {
        rounds += 1;
        if rounds > 16 {
            break;
        }
        let s = sync_instances(doc, o);
        if is_noop(&s) {
            continue;
        }
        let mut touched = Vec::new();
        ids_of(doc, &s, &mut touched);
        let Ok(i) = s.clone().apply(doc) else { continue };
        invs.push(i);
        syncs.push(s);
        for t in touched {
            if let Some(o2) = owner_of(doc, t) {
                if o2 != o && !queue.contains(&o2) {
                    queue.push(o2);
                }
            }
        }
    }
    for i in invs.into_iter().rev() {
        let _ = i.apply(doc);
    }
    let _ = inv.apply(doc);
    if syncs.is_empty() {
        cmd
    } else {
        Command::Batch(std::iter::once(cmd).chain(syncs).collect())
    }
}

#[cfg(test)]
mod restore_regressions {
    use super::*;

    #[test]
    fn whole_document_restore_preserves_saved_component_state_and_is_undoable() {
        let mut document = Document::new("current");
        let page = document.add_page("Page");
        let main = document.alloc_id();
        document.insert(page, Node::frame(main, Rect::new(0.0, 0.0, 100.0, 100.0))).unwrap();
        create_component(&mut document, main).apply(&mut document).unwrap();
        let instance = create_instance(&mut document, main, page, Vec2::new(200.0, 0.0));
        instance.apply(&mut document).unwrap();
        let current = document.to_json().unwrap();
        let mut restored = document.clone();
        restored.name = "saved snapshot".into();
        restored.get_mut(main).unwrap().name = "saved main".into();
        // An archived snapshot is restored exactly, even if its linked instance
        // deliberately retains different state from the main component.
        let saved = restored.to_json().unwrap();
        let command = with_sync(&mut document, Command::RestoreDocument { document: Box::new(restored) });
        assert!(matches!(command, Command::RestoreDocument { .. }));
        assert_eq!(document.to_json().unwrap(), current);
        let inverse = command.apply(&mut document).unwrap();
        assert_eq!(document.to_json().unwrap(), saved);
        inverse.apply(&mut document).unwrap();
        assert_eq!(document.to_json().unwrap(), current);
    }

    #[test]
    fn nested_restore_batch_does_not_speculatively_apply_or_add_sync_commands() {
        let mut document = Document::new("current");
        let current = document.to_json().unwrap();
        let restored = Document::new("saved");
        let command = Command::Batch(vec![Command::Batch(vec![Command::RestoreDocument {
            document: Box::new(restored),
        }])]);
        let expected = format!("{command:?}");
        let result = with_sync(&mut document, command);
        assert_eq!(format!("{result:?}"), expected);
        assert_eq!(document.to_json().unwrap(), current);
    }
}
