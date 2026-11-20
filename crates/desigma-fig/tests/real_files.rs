//! Import checks against the owner's private design files. They are ignored by
//! default and skip when a file is absent (the files are local and never
//! committed). Run with `cargo test -p desigma-fig --test real_files -- --ignored`;
//! set `DESIGMA_REFERENCE_DIR` to point at the folder holding them.

use desigma_doc::*;
use desigma_fig::import_fig;
use std::path::PathBuf;

fn reference(name: &str) -> Option<Vec<u8>> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("DESIGMA_REFERENCE_DIR").map(PathBuf::from).into_iter().collect();
    // `reference/` of this checkout or of any ancestor (worktrees live inside the main checkout).
    let mut cur = Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    while let Some(d) = cur {
        dirs.push(d.join("reference"));
        cur = d.parent().map(PathBuf::from);
    }
    dirs.into_iter().map(|d| d.join(name)).find_map(|p| std::fs::read(p).ok())
}

fn is_clone(n: &Node) -> bool {
    n.source_id.as_deref().is_some_and(|s| s.starts_with('I'))
}

fn subtree(doc: &Document, root: NodeId) -> Vec<&Node> {
    let mut out = Vec::new();
    doc.walk(root, |n, _| out.push(doc.get(n.id).unwrap()));
    out
}

#[test]
#[ignore = "needs the private reference/private-ui-kit.fig"]
fn private_ui_kit_imports_with_library_instances_resolved() {
    let Some(bytes) = reference("private-ui-kit.fig") else {
        eprintln!("private-ui-kit.fig not found: skipped");
        return;
    };
    let doc = import_fig(&bytes).expect("import");
    assert!(!doc.nodes.values().any(|n| matches!(n.kind, NodeKind::Unknown { .. })), "no Unknown node kinds");

    let kit = doc.pages.iter().copied().find(|p| doc.get(*p).unwrap().name.contains("UI Kit")).expect("UI Kit page");
    let own: Vec<&Node> = subtree(&doc, kit).into_iter().filter(|n| !is_clone(n)).collect();
    assert_eq!(own.len(), 224, "nodes authored on the UI Kit page");
    let instances: Vec<&&Node> = own.iter().filter(|n| matches!(n.kind, NodeKind::Instance { .. })).collect();
    assert_eq!(instances.len(), 66);
    for i in &instances {
        let comp = instance_component(i).unwrap_or_else(|| panic!("instance {:?} unresolved", i.name));
        assert!(is_library_node(&doc, comp), "points at a library component");
        assert!(!i.children.is_empty(), "instance {:?} has no children", i.name);
    }

    let lp = doc.library_page.expect("library page");
    assert!(!doc.pages.contains(&lp));
    assert!(doc.library.external.len() > 50, "remote components are external assets");
    assert!(doc.library.external.values().all(|e| e.library_id.starts_with(REMOTE_LIBRARY_PREFIX)));

    assert_eq!(doc.variables.len(), 112);
    assert_eq!(doc.variable_collections.len(), 8);
    assert!(
        doc.variables.values().any(|v| v.values.values().any(|x| matches!(x, VariableValue::Alias(t) if doc.variables.contains_key(t)))),
        "aliases resolve"
    );
    assert!(doc.nodes.values().any(|n| !n.variables.bindings.is_empty()), "nodes bind variables");
    assert_eq!(doc.images.len(), 34);
}

#[test]
#[ignore = "needs the private reference/shadcn-ui.fig"]
fn shadcn_ui_imports_with_local_components() {
    let Some(bytes) = reference("shadcn-ui.fig") else {
        eprintln!("shadcn-ui.fig not found: skipped");
        return;
    };
    let doc = import_fig(&bytes).expect("import");
    assert!(!doc.nodes.values().any(|n| matches!(n.kind, NodeKind::Unknown { .. })));
    assert_eq!(doc.pages.len(), 6);
    let (mut instances, mut resolved) = (0, 0);
    for n in doc.nodes.values().filter(|n| matches!(n.kind, NodeKind::Instance { .. })) {
        instances += 1;
        resolved += instance_component(n).is_some() as usize;
    }
    assert!(instances >= 400 && instances == resolved, "{resolved} of {instances} instances resolved");
    assert_eq!(doc.styles.len(), 35);
    assert!(doc.library.external.is_empty(), "everything is local");

    for (source, size, weight) in [("21:1325", 18.0, 600), ("13:2558", 16.0, 400)] {
        let n = doc.nodes.values().find(|n| n.source_id.as_deref() == Some(source)).expect("cover text");
        let NodeKind::Text(t) = &n.kind else { panic!("cover text node") };
        assert_eq!(t.style.font_size, size, "{} uses its linked style", n.name);
        assert_eq!(t.style.font_weight, weight);
    }
}

#[test]
#[ignore = "needs the private reference/motion-untitled.fig"]
fn motion_untitled_imports_figma_motion_data() {
    use desigma_doc::motion::*;
    let Some(bytes) = reference("motion-untitled.fig") else {
        eprintln!("motion-untitled.fig not found: skipped");
        return;
    };
    let doc = import_fig(&bytes).expect("import");
    // The tracks, keyframes and preset instances are folded into the node: no stray nodes.
    assert!(!doc.nodes.values().any(|n| matches!(n.kind, NodeKind::Unknown { .. })));
    assert!(doc.library_page.is_none());
    let rect = doc.nodes.values().find(|n| n.name == "Rectangle 1").expect("rectangle");
    let m = &rect.motion;
    let base = base_position(rect);

    // Position: x -7 -> 200 in the UI = an offset of 207 from the static position, spring segment.
    let pos = m.track(AnimProperty::Position).expect("position track");
    assert_eq!(pos.keys.iter().map(|k| k.time_ms).collect::<Vec<_>>(), vec![0, 997]);
    assert_eq!(pos.keys[0].value, AnimValue::Vec2(base));
    assert_eq!(pos.keys[1].value, AnimValue::Vec2(Vec2::new(base.x + 207.0, base.y)));
    assert_eq!(pos.keys[0].easing, Ease::spring(0.25), "damping 15, stiffness 100, mass 1");

    // Rotation 0 -> 90 degrees, opacity 100 -> 30 %.
    let rot = m.track(AnimProperty::Rotation).expect("rotation track");
    assert_eq!(rot.keys.iter().map(|k| k.time_ms).collect::<Vec<_>>(), vec![0, 997]);
    assert_eq!(rot.keys[0].value, AnimValue::Scalar(0.0));
    assert!((rot.keys[1].value.scalar().unwrap() - 90.0).abs() < 1e-4);
    assert_eq!(rot.keys[0].easing, Ease::bezier(0.5, 0.0, 0.5, 1.0));
    let op = m.track(AnimProperty::Opacity).expect("opacity track");
    assert!((op.keys[0].value.scalar().unwrap() - 1.0).abs() < 1e-6);
    assert!((op.keys[1].value.scalar().unwrap() - 0.3).abs() < 1e-5);
    assert_eq!(m.tracks.len(), 3);

    // The preset (Burst = four basic presets) applied at 997 ms.
    assert_eq!(m.clips.len(), 4);
    let by_kind = |k: ClipKind| m.clips.iter().find(|c| c.kind == k).unwrap_or_else(|| panic!("no {k:?} clip"));
    let burst = Ease::bezier(0.07, 0.97, 0.58, 1.0);
    let slide = by_kind(ClipKind::Position);
    assert_eq!((slide.mode, slide.direction, slide.amount), (ClipMode::In, ClipDirection::Bottom, 116.0));
    assert_eq!((slide.delay_ms, slide.duration_ms, slide.easing), (997, 300, burst));
    let scale = by_kind(ClipKind::Scale);
    assert_eq!((scale.mode, scale.amount, scale.delay_ms, scale.duration_ms, scale.easing), (ClipMode::In, 0.8, 997, 300, burst));
    let spin = by_kind(ClipKind::Rotation);
    assert_eq!((spin.mode, spin.direction, spin.amount, spin.delay_ms, spin.duration_ms), (ClipMode::In, ClipDirection::Clockwise, 90.0, 997, 300));
    let fade = by_kind(ClipKind::Opacity);
    assert_eq!((fade.mode, fade.amount, fade.delay_ms, fade.duration_ms), (ClipMode::In, 0.0, 997, 30));

    // Timeline: Figma stores nothing for the defaults (2000 ms, no loop).
    assert_eq!(doc.timeline(doc.pages[0]).duration_ms, 2000);
    assert!(!doc.timeline(doc.pages[0]).looping);
}
