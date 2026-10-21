//! FigJam `SECTION`, `TABLE` and `TABLE_CELL` nodes: import into the board kinds and export back.

use desigma_doc::*;
use desigma_fig::{export_rest_json, import_rest_json};

const BOARD: &str = r##"{
  "name": "Planning",
  "editorType": "figjam",
  "document": {
    "id": "0:0", "name": "Document", "type": "DOCUMENT",
    "children": [{
      "id": "0:1", "name": "Board", "type": "CANVAS",
      "backgroundColor": { "r": 0.96, "g": 0.96, "b": 0.96, "a": 1 },
      "children": [
        {
          "id": "1:1", "name": "Backlog", "type": "SECTION",
          "sectionContentsHidden": true,
          "absoluteBoundingBox": { "x": 0, "y": 0, "width": 500, "height": 300 },
          "size": { "x": 500, "y": 300 },
          "fills": [{ "type": "SOLID", "color": { "r": 0.49, "g": 0.72, "b": 1, "a": 1 } }],
          "children": [{
            "id": "1:2", "name": "Note", "type": "STICKY", "characters": "Inside",
            "size": { "x": 100, "y": 100 }, "relativeTransform": [[1, 0, 20], [0, 1, 20]]
          }]
        },
        {
          "id": "2:1", "name": "Status", "type": "TABLE", "numRows": 2, "numColumns": 3,
          "absoluteBoundingBox": { "x": 600, "y": 0, "width": 480, "height": 120 },
          "size": { "x": 480, "y": 120 },
          "children": [
            { "id": "2:5", "name": "Cell", "type": "TABLE_CELL", "rowIndex": 1, "columnIndex": 0, "characters": "second row", "size": { "x": 100, "y": 70 } },
            { "id": "2:2", "name": "Cell", "type": "TABLE_CELL", "rowIndex": 0, "columnIndex": 0, "characters": "Task", "size": { "x": 100, "y": 50 } },
            { "id": "2:3", "name": "Cell", "type": "TABLE_CELL", "rowIndex": 0, "columnIndex": 1, "characters": "Owner", "size": { "x": 180, "y": 50 },
              "fills": [{ "type": "SOLID", "color": { "r": 1, "g": 0.898, "b": 0.561, "a": 1 } }] },
            { "id": "2:4", "name": "Cell", "type": "TABLE_CELL", "rowIndex": 0, "columnIndex": 2, "characters": "Done", "size": { "x": 200, "y": 50 } }
          ]
        }
      ]
    }]
  }
}"##;

fn find<'a>(doc: &'a Document, name: &str) -> &'a Node {
    doc.nodes.values().find(|n| n.name == name).unwrap_or_else(|| panic!("no node named {name}"))
}

fn cell_text(doc: &Document, t: NodeId, r: usize, c: usize) -> String {
    match &doc.get(table_cell_node(doc, t, r, c).unwrap()).unwrap().kind {
        NodeKind::TableCell(c) => c.text.content.clone(),
        k => panic!("{k:?}"),
    }
}

#[test]
fn figjam_sections_import_as_board_sections() {
    let doc = import_rest_json(BOARD).unwrap();
    let n = find(&doc, "Backlog");
    let NodeKind::BoardSection(s) = &n.kind else { panic!("{:?}", n.kind) };
    assert_eq!(s.color, SectionColor::Blue, "the nearest section colour to the fill");
    assert!(s.contents_hidden);
    assert!(n.fills.is_empty(), "the colour lives in the props");
    assert_eq!(n.children.len(), 1, "children come along");
    // the same node in a design file stays a plain design section
    let design = import_rest_json(&BOARD.replace("\"editorType\": \"figjam\",", "")).unwrap();
    assert!(matches!(find(&design, "Backlog").kind, NodeKind::Section));
}

#[test]
fn figjam_tables_import_cells_in_row_major_order_on_the_grid() {
    let doc = import_rest_json(BOARD).unwrap();
    let t = find(&doc, "Status");
    let NodeKind::Table(p) = &t.kind else { panic!("{:?}", t.kind) };
    assert_eq!((p.rows(), p.cols()), (2, 3));
    // widths come from the first row, heights from the first column
    assert_eq!(p.col_widths, vec![100.0, 180.0, 200.0]);
    assert_eq!(p.row_heights, vec![50.0, 70.0]);
    assert_eq!(t.children.len(), 6, "missing cells are padded");
    assert_eq!(cell_text(&doc, t.id, 0, 0), "Task");
    assert_eq!(cell_text(&doc, t.id, 0, 1), "Owner");
    assert_eq!(cell_text(&doc, t.id, 0, 2), "Done");
    assert_eq!(cell_text(&doc, t.id, 1, 0), "second row");
    assert_eq!(cell_text(&doc, t.id, 1, 1), "");
    let owner = doc.get(table_cell_node(&doc, t.id, 0, 1).unwrap()).unwrap();
    assert!(matches!(&owner.kind, NodeKind::TableCell(c) if c.fill.is_some_and(|f| f.to_hex() == "FFE58F")));
    assert_eq!((owner.transform.tx, owner.transform.ty, owner.size), (100.0, 0.0, Vec2::new(180.0, 50.0)));
    assert_eq!(t.size, Vec2::new(480.0, 120.0));
}

#[test]
fn sections_and_tables_roundtrip_through_rest_json() {
    let doc = import_rest_json(BOARD).unwrap();
    let json = export_rest_json(&doc);
    let text = serde_json::to_string(&json).unwrap();
    for ty in ["SECTION", "TABLE", "TABLE_CELL"] {
        assert!(text.contains(&format!("\"type\":\"{ty}\"")), "{ty} missing");
    }
    let back = import_rest_json(&text).unwrap();
    let (a, b) = (find(&doc, "Backlog"), find(&back, "Backlog"));
    assert_eq!(a.kind, b.kind);
    assert_eq!(a.size, b.size);
    let (ta, tb) = (find(&doc, "Status"), find(&back, "Status"));
    assert_eq!(ta.kind, tb.kind);
    for r in 0..2 {
        for c in 0..3 {
            assert_eq!(cell_text(&doc, ta.id, r, c), cell_text(&back, tb.id, r, c), "cell {r},{c}");
        }
    }
    let fill = |d: &Document, t: NodeId| match &d.get(table_cell_node(d, t, 0, 1).unwrap()).unwrap().kind {
        NodeKind::TableCell(c) => c.fill,
        _ => None,
    };
    assert_eq!(fill(&doc, ta.id), fill(&back, tb.id));
}

#[test]
fn native_tables_and_sections_survive_an_export_import_cycle() {
    let mut d = Document::new("native");
    d.kind = DocumentKind::Board;
    let page = d.add_page("p");
    let nodes = table_nodes(&mut d, Vec2::new(40.0, 40.0), 2, 3);
    let t = nodes[0].id;
    Command::Insert { parent: page, index: 0, nodes }.apply(&mut d).unwrap();
    let c = table_cell_node(&d, t, 2, 1).unwrap();
    if let NodeKind::TableCell(cell) = &mut d.get_mut(c).unwrap().kind {
        cell.text.content = "last".into();
    }
    let sid = d.alloc_id();
    let mut s = Node::board_section(sid, Rect::new(300.0, 0.0, 400.0, 300.0), "Roadmap");
    if let NodeKind::BoardSection(p) = &mut s.kind {
        p.color = SectionColor::LightViolet;
    }
    d.insert(page, s).unwrap();
    let back = import_rest_json(&serde_json::to_string(&export_rest_json(&d)).unwrap()).unwrap();
    assert_eq!(back.kind, DocumentKind::Board);
    let tb = find(&back, "Table");
    assert!(matches!(&tb.kind, NodeKind::Table(p) if p.rows() == 3 && p.cols() == 2));
    assert_eq!(cell_text(&back, tb.id, 2, 1), "last");
    assert_eq!(tb.size, Vec2::new(2.0 * TABLE_COL_W, 3.0 * TABLE_ROW_H));
    let sb = find(&back, "Roadmap");
    assert!(matches!(&sb.kind, NodeKind::BoardSection(p) if p.color == SectionColor::LightViolet && !p.contents_hidden));
}
