//! UI tests of the board drawing tools: marker, highlighter, washi tape and eraser, straight
//! lines with Shift, the drawing tray, the contextual bar for strokes, and undo.

use super::*;
use crate::canvas::tools::Tool;
use desigma_doc::{Brush, NodeKind, WashiPattern, BRUSH_SIZES};

fn board_sim() -> Sim {
    Sim::with(Options { fixture: Some("board-empty".into()), ..Default::default() })
}

fn strokes(s: &Sim) -> Vec<NodeId> {
    let page = s.app().page;
    s.app().doc.get(page).unwrap().children.iter().copied().filter(|i| s.app().doc.get(*i).is_some_and(|n| n.brush.is_some())).collect()
}

fn shifted(s: &mut Sim, key: Key) {
    s.mods = Modifiers::SHIFT;
    s.key(key);
    s.mods = Modifiers::NONE;
}

#[test]
fn drawing_shortcuts_pick_the_tools() {
    let mut s = board_sim();
    s.key(Key::M);
    assert_eq!(s.app().tool, Tool::Marker);
    shifted(&mut s, Key::M);
    assert_eq!(s.app().tool, Tool::Highlighter);
    shifted(&mut s, Key::W);
    assert_eq!(s.app().tool, Tool::Washi);
    shifted(&mut s, Key::E);
    assert_eq!(s.app().tool, Tool::Eraser);
    s.key(Key::V);
    shifted(&mut s, Key::Backspace);
    assert_eq!(s.app().tool, Tool::Eraser, "Shift+Backspace picks the eraser instead of deleting");
}

#[test]
fn marker_strokes_are_selectable_vector_nodes() {
    let mut s = board_sim();
    s.key(Key::M);
    s.drag(s.screen(300.0, 300.0), s.screen(500.0, 380.0));
    let id = strokes(&s)[0];
    let n = s.app().doc.get(id).unwrap();
    assert!(matches!(n.kind, NodeKind::Vector { .. }));
    assert_eq!(n.brush, Some(Brush::Marker));
    // choosing the move tool and clicking the ink selects it
    s.key(Key::V);
    s.click(s.screen(400.0, 340.0));
    assert_eq!(s.app().selection, vec![id]);
    // the contextual bar offers thickness for it
    s.frame(vec![]);
    let _ = s.rect_of("board-thickness-0");
}

#[test]
fn highlighter_multiplies_over_what_is_below() {
    let mut s = board_sim();
    shifted(&mut s, Key::M);
    s.drag(s.screen(300.0, 300.0), s.screen(500.0, 300.0));
    let n = s.app().doc.get(strokes(&s)[0]).unwrap();
    assert_eq!(n.brush, Some(Brush::Highlighter));
    assert_eq!(n.blend, desigma_doc::BlendMode::Multiply);
    assert_eq!(n.stroke.weight, crate::theme::BOARD_HIGHLIGHTER_WIDTH);
}

#[test]
fn shift_draws_a_straight_line_snapped_to_45_degrees() {
    let mut s = board_sim();
    s.key(Key::M);
    s.mods = Modifiers::SHIFT;
    // a wobbly drag that is closest to horizontal
    s.drag(s.screen(300.0, 300.0), s.screen(500.0, 315.0));
    s.mods = Modifiers::NONE;
    let n = s.app().doc.get(strokes(&s)[0]).unwrap();
    assert!(n.size.y <= 1.01, "horizontal, got {:?}", n.size);
    assert!(n.size.x > 100.0);
    let lines = n.brush_polylines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].len(), 2, "two anchors, no curve");
}

#[test]
fn washi_tape_is_a_straight_patterned_strip() {
    let mut s = board_sim();
    shifted(&mut s, Key::W);
    s.drag(s.screen(300.0, 300.0), s.screen(500.0, 380.0));
    let id = strokes(&s)[0];
    let n = s.app().doc.get(id).unwrap();
    assert_eq!(n.brush, Some(Brush::Washi(WashiPattern::Stripes)));
    assert_eq!(n.stroke.weight, crate::theme::BOARD_WASHI_WIDTH);
    assert_eq!(n.brush_polylines()[0].len(), 2, "washi is always straight, whatever the hand did");
    assert_eq!(s.app().tool, Tool::Washi, "the tape stays selected for the next strip");
    s.cmd_key(Key::Z);
    assert!(strokes(&s).is_empty());
}

#[test]
fn the_tray_sets_colour_thickness_and_tape_pattern() {
    let mut s = board_sim();
    shifted(&mut s, Key::W);
    s.click(s.rect_of("board-chevron-Brush").center());
    s.click(s.rect_of("board-ink-2").center());
    s.click(s.rect_of("board-size-2").center());
    s.click(s.rect_of("board-pattern-Dots").center());
    assert_eq!(s.app().board.size, 2);
    assert_eq!(s.app().board.pattern, WashiPattern::Dots);
    s.click(s.screen(900.0, 150.0));
    s.drag(s.screen(300.0, 300.0), s.screen(500.0, 300.0));
    let n = s.app().doc.get(strokes(&s)[0]).unwrap();
    assert_eq!(n.brush, Some(Brush::Washi(WashiPattern::Dots)));
    assert_eq!(n.stroke.weight, crate::theme::BOARD_WASHI_WIDTH * BRUSH_SIZES[2]);
    assert_eq!(crate::board::bar::current_color(n), Some(crate::theme::BOARD_DRAW_INK[2]));
}

#[test]
fn the_bar_recolours_and_thickens_selected_strokes_in_one_undo_step() {
    let mut s = board_sim();
    s.key(Key::M);
    s.drag(s.screen(300.0, 300.0), s.screen(500.0, 380.0));
    let id = strokes(&s)[0];
    s.key(Key::V);
    s.click(s.screen(400.0, 340.0));
    s.frame(vec![]);
    s.click(s.rect_of("board-thickness-2").center());
    assert_eq!(s.app().doc.get(id).unwrap().stroke.weight, crate::theme::BOARD_MARKER_WIDTH * BRUSH_SIZES[2]);
    s.click(s.rect_of("board-swatch-5").center());
    assert_eq!(crate::board::bar::current_color(s.app().doc.get(id).unwrap()), Some(crate::theme::BOARD_DRAW_INK[5]));
    s.cmd_key(Key::Z);
    assert_eq!(crate::board::bar::current_color(s.app().doc.get(id).unwrap()), Some(crate::theme::BOARD_DRAW_INK[0]));
    s.cmd_key(Key::Z);
    assert_eq!(s.app().doc.get(id).unwrap().stroke.weight, crate::theme::BOARD_MARKER_WIDTH);
}

#[test]
fn the_eraser_deletes_only_the_strokes_it_touches_and_undoes_in_one_step() {
    let mut s = board_sim();
    s.key(Key::M);
    s.drag(s.screen(300.0, 300.0), s.screen(500.0, 300.0));
    s.drag(s.screen(300.0, 400.0), s.screen(500.0, 400.0));
    s.drag(s.screen(300.0, 500.0), s.screen(500.0, 500.0));
    assert_eq!(strokes(&s).len(), 3);
    let keep = strokes(&s)[1];
    shifted(&mut s, Key::E);
    // beside the ink erases nothing
    s.drag(s.screen(640.0, 280.0), s.screen(640.0, 320.0));
    assert_eq!(strokes(&s).len(), 3);
    s.drag(s.screen(400.0, 280.0), s.screen(400.0, 320.0));
    assert_eq!(strokes(&s).len(), 2);
    s.drag(s.screen(400.0, 480.0), s.screen(400.0, 520.0));
    assert_eq!(strokes(&s), vec![keep]);
    s.cmd_key(Key::Z);
    assert_eq!(strokes(&s).len(), 2);
    assert_eq!(s.app().tool, Tool::Eraser, "the eraser stays selected");
}

#[test]
fn one_eraser_swipe_removes_every_stroke_it_crosses() {
    let mut s = board_sim();
    s.key(Key::M);
    for y in [300.0, 340.0, 380.0] {
        s.drag(s.screen(300.0, y), s.screen(500.0, y));
    }
    shifted(&mut s, Key::E);
    s.drag(s.screen(400.0, 280.0), s.screen(400.0, 400.0));
    assert!(strokes(&s).is_empty());
    s.cmd_key(Key::Z);
    assert_eq!(strokes(&s).len(), 3, "all three come back with one undo");
}

#[test]
fn the_eraser_leaves_other_board_items_alone() {
    let mut s = board_sim();
    s.key(Key::S);
    s.click(s.screen(300.0, 300.0));
    s.key(Key::Escape);
    let n0 = s.count();
    shifted(&mut s, Key::E);
    s.drag(s.screen(250.0, 300.0), s.screen(450.0, 300.0));
    assert_eq!(s.count(), n0);
}
