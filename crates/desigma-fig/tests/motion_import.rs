//! Figma Motion data in `.fig` files: tracks, keyframes, easing, animation presets and the page
//! timeline (synthetic fixture; the private real-file check is in `real_files.rs`).
use desigma_doc::motion::*;
use desigma_doc::*;
use desigma_fig::container::Compression;
use desigma_fig::testkit::motion_fig;
use desigma_fig::{export_rest_json, import_fig, import_rest_json};

fn doc() -> Document {
    import_fig(&motion_fig(Compression::Zstd)).unwrap()
}

fn boxed(doc: &Document) -> &Node {
    doc.nodes.values().find(|n| n.name == "Box").expect("Box")
}

fn vec2_keys(t: &Track) -> Vec<(u32, f32, f32)> {
    t.keys.iter().map(|k| (k.time_ms, k.value.vec2().unwrap().x, k.value.vec2().unwrap().y)).collect()
}

fn scalar_keys(t: &Track) -> Vec<(u32, f32)> {
    t.keys.iter().map(|k| (k.time_ms, k.value.scalar().unwrap())).collect()
}

#[test]
fn tracks_keyframes_and_easing_map() {
    let d = doc();
    let m = &boxed(&d).motion;
    // The unsupported shear track is skipped; the rest imports.
    let props: Vec<_> = m.tracks.iter().map(|t| t.property).collect();
    assert_eq!(props, vec![AnimProperty::Position, AnimProperty::Rotation, AnimProperty::Opacity]);

    // Position: offsets from the node's static position (30, 40).
    let pos = m.track(AnimProperty::Position).unwrap();
    assert_eq!(vec2_keys(pos), vec![(0, 30.0, 40.0), (800, 80.0, 50.0)]);
    // The spring is stored on the key that ends the segment; desigma keeps it on the key that starts it.
    match pos.keys[0].easing {
        Ease::CustomSpring { bounce } => assert!((bounce - 0.5).abs() < 1e-4, "damping ratio 0.5 -> bounce 0.5, got {bounce}"),
        e => panic!("expected a spring, got {e:?}"),
    }

    // Rotation: radians, clockwise positive in the file -> degrees counter-clockwise positive.
    let rot = scalar_keys(m.track(AnimProperty::Rotation).unwrap());
    assert_eq!(rot[0], (0, 0.0));
    assert_eq!(rot[1].0, 800);
    assert!((rot[1].1 - 60.0).abs() < 1e-3, "{rot:?}");
    assert_eq!(m.track(AnimProperty::Rotation).unwrap().keys[0].easing, Ease::bezier(0.25, 0.1, 0.25, 1.0));

    // Opacity: percent -> 0..1.
    assert_eq!(scalar_keys(m.track(AnimProperty::Opacity).unwrap()), vec![(0, 1.0), (800, 0.5)]);

    // Evaluated at the end: the keyframed position plus the finished slide-out clip (-40, 0).
    let ev = evaluate_node(boxed(&d), 800.0);
    assert_eq!(ev.position, Some(Vec2::new(40.0, 50.0)));
}

#[test]
fn animation_presets_become_clips() {
    let d = doc();
    let m = &boxed(&d).motion;
    assert_eq!(m.clips.len(), 2);

    let slide = &m.clips[0];
    assert_eq!((slide.kind, slide.mode, slide.direction), (ClipKind::Position, ClipMode::Out, ClipDirection::Left));
    assert_eq!((slide.amount, slide.delay_ms, slide.duration_ms), (40.0, 500, 250));
    assert_eq!(slide.easing, Ease::bezier(0.2, 0.0, 0.4, 1.0));
    assert_eq!(slide.endpoints(), (Vec2::ZERO, Vec2::new(-40.0, 0.0)));

    // No code component to name the parameters: inferred from the tracks (0 -> 1 is a fade in).
    let fade = &m.clips[1];
    assert_eq!((fade.kind, fade.mode, fade.amount), (ClipKind::Opacity, ClipMode::In, 0.0));
    assert_eq!((fade.delay_ms, fade.duration_ms), (600, 100));
    assert_eq!(fade.easing, Ease::Linear);
    assert!(fade.visible);
}

#[test]
fn page_timeline_is_imported() {
    let d = doc();
    let page = d.pages[0];
    assert_eq!(d.timeline(page), TimelineSettings { duration_ms: 3000, fps: 30, looping: true });
}

#[test]
fn motion_machinery_nodes_do_not_pollute_the_document() {
    let d = doc();
    assert!(!d.nodes.values().any(|n| matches!(n.kind, NodeKind::Unknown { .. })));
    // Only the page, the box (and no library page: the internal canvas held nothing else).
    assert_eq!(d.nodes.len(), 2, "{:?}", d.nodes.values().map(|n| &n.name).collect::<Vec<_>>());
    assert!(d.library_page.is_none());
}

#[test]
fn motion_round_trips_through_rest_json() {
    let d = doc();
    let json = export_rest_json(&d);
    let back = import_rest_json(&json.to_string()).unwrap();
    assert_eq!(boxed(&back).motion, boxed(&d).motion);
    assert_eq!(back.timeline(back.pages[0]), d.timeline(d.pages[0]));
    // And is stable.
    assert_eq!(export_rest_json(&back), json);
}

#[test]
fn plain_documents_export_no_motion() {
    let mut d = Document::new("plain");
    let page = d.add_page("P");
    let id = d.alloc_id();
    d.insert(page, Node::rectangle(id, Rect::new(0.0, 0.0, 5.0, 5.0))).unwrap();
    assert!(!export_rest_json(&d).to_string().contains("motion"));
}

#[test]
fn spring_and_easing_names_map() {
    use serde_json::json;
    // Through the public importer: one animated node per easing type.
    let ease = |ty: &str, v: serde_json::Value| -> Ease {
        let key = |id: u32, at: Option<i64>, val: f32, e: Option<serde_json::Value>| {
            let mut k = json!({
                "guid": {"sessionID": 1, "localID": id}, "type": "KEYFRAME", "name": "k",
                "parentIndex": {"guid": {"sessionID": 1, "localID": 3}, "position": "!"},
                "keyframeValue": {"value": {"floatValue": val}, "valueType": "FLOAT"},
            });
            if let Some(a) = at {
                k["timelinePosition"] = json!(a);
            }
            if let Some(e) = e {
                k["easingData"] = e;
            }
            k
        };
        let changes = json!([
            {"guid": {"sessionID": 0, "localID": 1}, "type": "CANVAS", "name": "P", "parentIndex": {"guid": {"sessionID": 0, "localID": 0}, "position": "a"}},
            {"guid": {"sessionID": 0, "localID": 2}, "type": "CANVAS", "name": "I", "internalOnly": true, "parentIndex": {"guid": {"sessionID": 0, "localID": 0}, "position": "b"}},
            {"guid": {"sessionID": 1, "localID": 2}, "type": "RECTANGLE", "name": "R", "size": {"x": 10.0, "y": 10.0},
             "parentIndex": {"guid": {"sessionID": 0, "localID": 1}, "position": "a"},
             "parameterConsumptionMap": {"entries": [{"variableField": "WIDTH", "variableData": {"dataType": "EXPRESSION", "value": {"expressionValue": {
                 "expressionFunction": "KEYFRAME", "expressionArguments": [
                     {"dataType": "FLOAT", "value": {"floatValue": 10.0}},
                     {"dataType": "KEYFRAME_TRACK_PARAMETER_DATA", "value": {"keyframeTrackParameterValue": {"parameters": [
                         {"type": "MANUAL", "value": {"manual": {"keyframeTrackId": {"guid": {"sessionID": 1, "localID": 3}}}}}]}}}]}}}}]}},
            {"guid": {"sessionID": 1, "localID": 3}, "type": "KEYFRAME_TRACK", "name": "t", "parentIndex": {"guid": {"sessionID": 0, "localID": 2}, "position": "a"}},
            key(4, None, 10.0, None),
            key(5, Some(1_000_000), 20.0, Some(json!({"easingType": ty, "easingValue": v}))),
        ]);
        let d = desigma_fig::import_fig_node_changes(&changes, "t").unwrap();
        let n = d.nodes.values().find(|n| n.name == "R").unwrap();
        n.motion.track(AnimProperty::Width).unwrap().keys[0].easing
    };
    assert_eq!(ease("LINEAR", json!({})), Ease::Linear);
    assert_eq!(ease("HOLD", json!({})), Ease::Hold);
    assert_eq!(ease("IN_CUBIC", json!({})), Ease::EaseIn);
    assert_eq!(ease("OUT_CUBIC", json!({})), Ease::EaseOut);
    assert_eq!(ease("INOUT_CUBIC", json!({})), Ease::EaseInOut);
    assert_eq!(ease("IN_BACK_CUBIC", json!({})), Ease::EaseInBack);
    assert_eq!(ease("OUT_BACK_CUBIC", json!({})), Ease::EaseOutBack);
    assert_eq!(ease("INOUT_BACK_CUBIC", json!({})), Ease::EaseInOutBack);
    assert_eq!(ease("CUSTOM_CUBIC", json!({"bezierEasing": {"p1x": 0.1, "p1y": 0.2, "p2x": 0.3, "p2y": 1.4}})), Ease::bezier(0.1, 0.2, 0.3, 1.4));
    // zeta = 15 / (2 sqrt(100)) = 0.75 -> bounce 0.25 (the default custom spring).
    match ease("CUSTOM_SPRING", json!({"springEasing": {"stiffness": 100.0, "damping": 15.0, "mass": 1.0}})) {
        Ease::CustomSpring { bounce } => assert!((bounce - 0.25).abs() < 1e-5),
        e => panic!("{e:?}"),
    }
    // Over-damped: zeta = 2 -> bounce -0.5.
    match ease("CUSTOM_SPRING", json!({"springEasing": {"stiffness": 100.0, "damping": 40.0, "mass": 1.0}})) {
        Ease::CustomSpring { bounce } => assert!((bounce + 0.5).abs() < 1e-5),
        e => panic!("{e:?}"),
    }
    assert_eq!(ease("GENTLE_SPRING", json!({})), Ease::SpringGentle);
    // Unknown easing types leave the segment linear instead of failing the import.
    assert_eq!(ease("SOMETHING_NEW", json!({})), Ease::Linear);
}
