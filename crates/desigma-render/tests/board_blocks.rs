//! Pixel tests of board sections and tables on the CPU renderer.

mod common;

use common::*;
use desigma_doc::*;
use desigma_render::board;

fn dark(p: [u8; 4]) -> bool {
    (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3 < 100
}

fn section(t: &mut T, r: Rect, title: &str, color: SectionColor) -> NodeId {
    let id = t.add(None, |id| Node::board_section(id, r, title));
    if let NodeKind::BoardSection(s) = &mut t.node(id).kind {
        s.color = color;
    }
    id
}

fn table(t: &mut T, at: Vec2, cols: usize, rows: usize) -> NodeId {
    let nodes = table_nodes(&mut t.doc, at, cols, rows);
    let id = nodes[0].id;
    Command::Insert { parent: t.page, index: usize::MAX, nodes }.apply(&mut t.doc).unwrap();
    id
}

fn set_cell(t: &mut T, table: NodeId, r: usize, c: usize, f: impl FnOnce(&mut TableCellProps)) {
    let id = table_cell_node(&t.doc, table, r, c).unwrap();
    if let NodeKind::TableCell(cell) = &mut t.node(id).kind {
        f(cell);
    }
}

#[test]
fn section_is_a_translucent_rounded_region_with_a_title_pill() {
    let mut t = T::new();
    section(&mut t, Rect::new(40.0, 80.0, 300.0, 200.0), "Ideas", SectionColor::Blue);
    let pm = t.render(400, 320);
    // semi-transparent: the blue shows through as a tint of the white page, not the pure colour
    let inside = px(&pm, 200, 180);
    let pure = SectionColor::Blue.color();
    let want = [(pure.r * 255.0 * 0.5 + 127.5) as u8, (pure.g * 255.0 * 0.5 + 127.5) as u8, (pure.b * 255.0 * 0.5 + 127.5) as u8, 255];
    assert!(near(inside, want, 4), "{inside:?} vs {want:?}");
    assert!(inside != [0x7D, 0xB8, 0xFF, 255]);
    // rounded corners: the very corner is the page
    assert_eq!(px(&pm, 41, 81), WHITE);
    // the pill sits above the top left corner in the solid colour, with the title in it
    let pill = board::section_title_rect(&{
        let mut n = Node::board_section(NodeId(1), Rect::new(40.0, 80.0, 300.0, 200.0), "Ideas");
        n.size = Vec2::new(300.0, 200.0);
        n
    });
    assert!(pill.y < 0.0 && pill.w > 48.0 && pill.w < 300.0, "{pill:?}");
    let (px0, py0) = (40.0 + pill.x, 80.0 + pill.y);
    let edge = px(&pm, (px0 + 4.0 + pill.w / 2.0) as u32, (py0 + 3.0) as u32);
    assert!(near(edge, [0x7D, 0xB8, 0xFF, 255], 6), "pill colour {edge:?}");
    let mut ink = 0;
    for y in py0 as u32..(py0 + pill.h) as u32 {
        for x in px0 as u32..(px0 + pill.w) as u32 {
            if dark(px(&pm, x, y)) {
                ink += 1;
            }
        }
    }
    assert!(ink > 30, "the title is drawn in the pill: {ink}");
    // nothing above the pill
    assert_eq!(px(&pm, 60, (py0 - 4.0) as u32), WHITE);
}

#[test]
fn the_pill_grows_with_its_title_but_stays_within_the_section() {
    let mk = |title: &str, w: f32| {
        let mut n = Node::board_section(NodeId(1), Rect::new(0.0, 0.0, w, 100.0), title);
        n.size = Vec2::new(w, 100.0);
        board::section_title_rect(&n).w
    };
    assert!(mk("A much longer title", 600.0) > mk("Hi", 600.0) + 40.0);
    assert!(mk(&"very long title ".repeat(20), 200.0) <= 200.0 + 0.01);
    assert!(mk("", 200.0) >= 48.0);
}

#[test]
fn section_draws_its_children_and_hides_them_on_request() {
    let mut t = T::new();
    let s = section(&mut t, Rect::new(20.0, 60.0, 300.0, 240.0), "Group", SectionColor::LightYellow);
    let note = t.add(Some(s), |id| Node::sticky(id, Rect::new(30.0, 30.0, 120.0, 120.0), ""));
    if let NodeKind::Sticky(p) = &mut t.node(note).kind {
        p.color = Color::rgb(0xFF, 0x00, 0x00);
    }
    let pm = t.render(360, 340);
    assert!(near(px(&pm, 20 + 30 + 60, 60 + 30 + 60), [255, 0, 0, 255], 3), "the note inside the section is drawn");
    if let NodeKind::BoardSection(p) = &mut t.node(s).kind {
        p.contents_hidden = true;
    }
    let pm = t.render(360, 340);
    let p = px(&pm, 20 + 30 + 60, 60 + 30 + 60);
    assert!(p[1] > 150, "hidden contents are not drawn: {p:?}");
    // the section itself is still there
    assert!(px(&pm, 300, 280) != WHITE);
}

#[test]
fn moving_a_section_moves_what_is_inside_it() {
    let mut t = T::new();
    let s = section(&mut t, Rect::new(20.0, 60.0, 300.0, 240.0), "Group", SectionColor::Gray);
    let note = t.add(Some(s), |id| Node::sticky(id, Rect::new(30.0, 30.0, 120.0, 120.0), ""));
    let before = t.doc.world_transform(note).apply(Vec2::ZERO);
    t.node(s).transform = Affine::translate(120.0, 160.0);
    let after = t.doc.world_transform(note).apply(Vec2::ZERO);
    assert_eq!((after.x - before.x, after.y - before.y), (100.0, 100.0));
}

#[test]
fn table_draws_white_cells_grid_lines_fills_and_text() {
    let mut t = T::new();
    let tb = table(&mut t, Vec2::new(20.0, 20.0), 2, 3);
    set_cell(&mut t, tb, 1, 1, |c| c.fill = Some(Color::rgb(0xFF, 0xE5, 0x8F)));
    set_cell(&mut t, tb, 0, 0, |c| c.text.content = "Name".into());
    let pm = t.render(460, 220);
    // cells are white, the filled one is yellow
    assert_eq!(px(&pm, 20 + 100, 20 + 28), WHITE);
    let yellow = px(&pm, 20 + 300, 20 + 56 + 28);
    assert!(near(yellow, [0xFF, 0xE5, 0x8F, 255], 3), "{yellow:?}");
    // grid lines: the vertical divider and a horizontal one are dark-ish grey, the outline too
    let vline = px(&pm, 20 + 200, 20 + 28);
    assert!(vline[0] < 235 && vline[0] > 150, "vertical divider {vline:?}");
    let hline = px(&pm, 20 + 100, 20 + 56);
    assert!(hline[0] < 235, "horizontal divider {hline:?}");
    assert!(px(&pm, 20, 20 + 28)[0] < 235, "outline");
    // text in the first cell
    let mut ink = 0;
    for y in 20 + 4..20 + 52 {
        for x in 20 + 4..20 + 196 {
            if dark(px(&pm, x, y)) {
                ink += 1;
            }
        }
    }
    assert!(ink > 40, "cell text is drawn: {ink}");
    // the empty cell next to it has none
    let mut other = 0;
    for y in 20 + 4..20 + 52 {
        for x in 20 + 204..20 + 396 {
            if dark(px(&pm, x, y)) {
                other += 1;
            }
        }
    }
    assert_eq!(other, 0);
}

#[test]
fn table_follows_column_and_row_edits() {
    let mut t = T::new();
    let tb = table(&mut t, Vec2::new(10.0, 10.0), 2, 2);
    set_cell(&mut t, tb, 0, 1, |c| c.fill = Some(Color::rgb(0xFF, 0x00, 0x00)));
    if let NodeKind::Table(p) = &mut t.node(tb).kind {
        p.col_widths[0] = 80.0;
    }
    sync_table(&mut t.doc, tb);
    let pm = t.render(320, 140);
    // the red cell now starts at x = 10 + 80
    assert!(near(px(&pm, 10 + 85, 10 + 20), [255, 0, 0, 255], 3));
    assert_eq!(px(&pm, 10 + 70, 10 + 20), WHITE);
}

#[test]
fn lowered_document_keeps_a_sections_lowered_children() {
    let mut t = T::new();
    let s = section(&mut t, Rect::new(0.0, 0.0, 300.0, 200.0), "S", SectionColor::Pink);
    let note = t.add(Some(s), |id| Node::sticky(id, Rect::new(10.0, 10.0, 100.0, 100.0), "hi"));
    let tb = table(&mut t, Vec2::new(400.0, 0.0), 2, 2);
    let lowered = board::lowered_document(&t.doc).unwrap();
    // the note is its lowered rectangle, whatever order the items were lowered in
    assert!(matches!(lowered.get(note).unwrap().kind, NodeKind::Rectangle));
    assert!(lowered.get(s).unwrap().children.contains(&note));
    // the table is a rectangle whose children are the synthetic grid
    let tn = lowered.get(tb).unwrap();
    assert!(matches!(tn.kind, NodeKind::Rectangle));
    assert!(tn.children.iter().all(|c| lowered.get(*c).is_some()));
}

#[test]
fn board_gallery_of_sections_and_tables() {
    let Ok(dir) = std::env::var("DESIGMA_BOARD_SHOTS") else { return };
    let mut t = T::new();
    t.doc.get_mut(t.page).unwrap().kind = NodeKind::Page { background: Color::rgb(0xF4, 0xF4, 0xF4) };
    for (i, c) in SectionColor::ALL.iter().enumerate() {
        section(&mut t, Rect::new(40.0 + (i % 4) as f32 * 260.0, 60.0 + (i / 4) as f32 * 190.0, 240.0, 150.0), c.label(), *c);
    }
    let tb = table(&mut t, Vec2::new(40.0, 860.0), 3, 3);
    set_cell(&mut t, tb, 0, 0, |c| c.text.content = "Task".into());
    set_cell(&mut t, tb, 0, 1, |c| c.text.content = "Owner".into());
    set_cell(&mut t, tb, 1, 1, |c| c.fill = Some(Color::rgb(0xB6, 0xD4, 0xFF)));
    let pm = t.render(1100, 1100);
    pm.save_png(std::path::Path::new(&dir).join("board_blocks.png")).unwrap();
}
