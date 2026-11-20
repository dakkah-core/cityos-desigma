//! Keeping local undo history from reverting other people's changes.

use crate::ops::{apply_mode, patch_node, Mode};
use crate::Op;
use desigma_doc::{Command, Document, History, NodeId};
use std::mem::swap;

/// Applies a remote change to what `history` stores, so undoing an earlier local edit does not
/// bring back values someone else has since changed.
///
/// * [`Op::Patch`]: the nodes stored in `Update` commands and in the subtrees of `Insert` /
///   `InsertPage` that undoing a delete restores, also inside `Batch`, get the patched fields.
/// * [`Op::DocEntry`]: the commands that snapshot a whole collection (`SetStyles`, `SetComments`,
///   `SetVariables`, `SetBoardSession`, `SetFlows`, and the per-page `SetGuides` / `SetTimeline`)
///   get that one entry. Entries nobody else touched stay as the command has them. A style (or
///   comment, vote...) someone else added is added to the snapshot, so undoing mine leaves it; one
///   they removed is removed from it, so undoing mine does not bring it back.
///
/// Other ops are ignored. Call it for each op in `Received::applied`.
pub fn rebase_history(history: &mut History, op: &Op) {
    match op {
        Op::Patch { id, fields } => history.for_each_command_mut(|c| match c {
            Command::Update { node } if node.id == *id => {
                let _ = patch_node(node, fields);
            }
            Command::Insert { nodes, .. } | Command::InsertPage { nodes, .. } => {
                for n in nodes.iter_mut().filter(|n| n.id == *id) {
                    let _ = patch_node(n, fields);
                }
            }
            _ => {}
        }),
        Op::DocEntry { field, key, .. } => history.for_each_command_mut(|c| rebase_entry(c, field, key, op)),
        _ => {}
    }
}

/// Runs `op` on a document holding only the collections the closure moves in.
fn on_scratch(op: &Op, load: impl FnOnce(&mut Document)) -> Document {
    let mut scratch = Document::default();
    load(&mut scratch);
    let _ = apply_mode(&mut scratch, op, Mode::Remote);
    scratch
}

fn rebase_entry(c: &mut Command, field: &str, key: &[String], op: &Op) {
    let page = |p: &NodeId| key == [p.0.to_string()];
    match c {
        Command::SetStyles { styles, style_meta } if field == "styles" => {
            let mut s = on_scratch(op, |d| {
                swap(&mut d.styles, styles);
                swap(&mut d.style_meta, style_meta);
            });
            swap(&mut s.styles, styles);
            swap(&mut s.style_meta, style_meta);
        }
        Command::SetComments { comments } if field == "comments" => {
            let mut s = on_scratch(op, |d| swap(&mut d.comments, comments));
            swap(&mut s.comments, comments);
        }
        Command::SetVariables { variables, collections } if field == "variables" || field == "variable_collections" => {
            let mut s = on_scratch(op, |d| {
                swap(&mut d.variables, variables);
                swap(&mut d.variable_collections, collections);
            });
            swap(&mut s.variables, variables);
            swap(&mut s.variable_collections, collections);
            // A collection's list, as the other person wrote it, names variables of mine this
            // snapshot does not have.
            for c in collections.values_mut() {
                c.variables.retain(|v| variables.contains_key(v));
            }
        }
        Command::SetBoardSession { session } if field == "board_session" => {
            let mut s = on_scratch(op, |d| swap(&mut d.board_session, session));
            swap(&mut s.board_session, session);
        }
        Command::SetFlows { flows } if field == "flows" => {
            let mut s = on_scratch(op, |d| swap(&mut d.flows, flows));
            swap(&mut s.flows, flows);
        }
        Command::SetGuides { page: p, guides } if field == "guides" && page(p) => {
            let mut s = on_scratch(op, |d| {
                d.guides.insert(*p, std::mem::take(guides));
            });
            *guides = s.guides.remove(p).unwrap_or_default();
        }
        Command::SetTimeline { page: p, settings } if field == "timelines" && page(p) => {
            let mut s = on_scratch(op, |d| {
                d.timelines.insert(*p, *settings);
            });
            *settings = s.timelines.remove(p).unwrap_or_default();
        }
        _ => {}
    }
}
