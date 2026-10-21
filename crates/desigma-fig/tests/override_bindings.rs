//! An instance override whose paint is bound to a variable (`colorVar`) takes
//! the variable's value and keeps the binding; the literal colour stored next
//! to the alias in `.fig` overrides can be stale.

use desigma_doc::*;
use desigma_fig::import_fig_node_changes;
use serde_json::{json, Value};

fn guid(s: u32, l: u32) -> Value {
    json!({"sessionID": s, "localID": l})
}

fn node(id: (u32, u32), parent: Option<(u32, u32)>, pos: &str, ty: &str, name: &str, extra: Value) -> Value {
    let mut v = json!({"guid": guid(id.0, id.1), "type": ty, "name": name, "phase": "CREATED"});
    if let Some(p) = parent {
        v["parentIndex"] = json!({"guid": guid(p.0, p.1), "position": pos});
    }
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

fn solid(r: f32, g: f32, b: f32, var: Option<(u32, u32)>) -> Value {
    let mut p = json!({"type": "SOLID", "color": {"r": r, "g": g, "b": b, "a": 1.0}, "opacity": 1.0, "visible": true});
    if let Some((s, l)) = var {
        p["colorVar"] = json!({"dataType": "ALIAS", "resolvedDataType": "COLOR", "value": {"alias": {"guid": guid(s, l)}}});
    }
    p
}

fn find<'a>(doc: &'a Document, name: &str) -> &'a Node {
    doc.nodes.values().find(|n| n.name == name).unwrap_or_else(|| panic!("no node named {name}"))
}

fn child<'a>(doc: &'a Document, parent: &Node, name: &str) -> &'a Node {
    parent.children.iter().map(|c| doc.get(*c).unwrap()).find(|n| n.name == name).unwrap_or_else(|| panic!("{} has no child {name}", parent.name))
}

fn text_fill(n: &Node) -> Color {
    match &n.kind {
        NodeKind::Text(t) => match t.style.fills.first().map(|p| &p.kind) {
            Some(PaintKind::Solid(c)) => *c,
            other => panic!("{}: unexpected fill {other:?}", n.name),
        },
        other => panic!("{} is not text: {other:?}", n.name),
    }
}

#[test]
fn override_paint_bound_to_a_variable_takes_the_variable_value() {
    let light = (0.07, 0.15, 0.2);
    let changes = json!([
        node((0, 0), None, "", "DOCUMENT", "Document", json!({})),
        node((0, 1), Some((0, 0)), "a", "CANVAS", "Page", json!({})),
        node((0, 2), Some((0, 0)), "b", "CANVAS", "Internal", json!({"internalOnly": true})),
        node(
            (1, 25),
            Some((0, 2)),
            "a",
            "VARIABLE_SET",
            "Colors Dark",
            json!({"variableSetModes": [{"id": guid(1, 2), "name": "Mode 1", "sortPosition": "!"}]})
        ),
        node(
            (1, 33),
            Some((0, 2)),
            "b",
            "VARIABLE",
            "text/primary",
            json!({
                "variableResolvedType": "COLOR",
                "variableSetID": {"guid": guid(1, 25)},
                "variableDataValues": {"entries": [{"modeID": guid(1, 2), "variableData": {
                    "dataType": "COLOR", "resolvedDataType": "COLOR",
                    "value": {"colorValue": {"r": 0.9, "g": 0.95, "b": 1.0, "a": 1.0}}}}]}
            })
        ),
        node((2, 1), Some((0, 1)), "a", "SYMBOL", "Row", json!({"size": {"x": 100.0, "y": 20.0}})),
        node(
            (2, 2),
            Some((2, 1)),
            "a",
            "TEXT",
            "Label",
            json!({
                "size": {"x": 60.0, "y": 12.0}, "fontSize": 10.0,
                "textData": {"characters": "Title"},
                "fillPaints": [solid(light.0, light.1, light.2, None)]
            })
        ),
        node(
            (3, 1),
            Some((0, 1)),
            "b",
            "INSTANCE",
            "Row dark",
            json!({
                "size": {"x": 100.0, "y": 20.0},
                "symbolData": {"symbolID": guid(2, 1), "symbolOverrides": [
                    {"guidPath": {"guids": [guid(2, 2)]}, "fillPaints": [solid(0.04, 0.04, 0.04, Some((1, 33)))]}
                ]}
            })
        ),
        node(
            (3, 2),
            Some((0, 1)),
            "c",
            "INSTANCE",
            "Row plain",
            json!({
                "size": {"x": 100.0, "y": 20.0}, "symbolData": {"symbolID": guid(2, 1)}
            })
        ),
    ]);
    let doc = import_fig_node_changes(&changes, "t").unwrap();
    assert!(doc.variables.contains_key("VariableID:1:33"), "variable imported");

    let label = child(&doc, find(&doc, "Row dark"), "Label");
    assert_eq!(label.variables.bindings.get("fills/0").map(String::as_str), Some("VariableID:1:33"), "clone is bound to the overriding variable");
    let c = text_fill(label);
    assert!((c.r - 0.9).abs() < 1e-5 && (c.g - 0.95).abs() < 1e-5 && (c.b - 1.0).abs() < 1e-5, "colour is the variable's value, not the stale literal: {c:?}");

    let plain = child(&doc, find(&doc, "Row plain"), "Label");
    let c = text_fill(plain);
    assert!((c.r - light.0).abs() < 1e-5, "untouched instance keeps the component colour: {c:?}");
}
