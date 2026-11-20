#![cfg(feature = "server")]
//! Motion tools of the file session: keyframes, presets, timeline and animation export.
use desigma_mcp::server::Session;
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("desigma-mcp-motion-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A session with a 120x80 frame holding a 20x20 square; returns (session, frame id, square id).
fn animated_session(dir: &Workspace) -> (Session, Value, Value) {
    let mut s = Session::with_discovery(&dir.0, None).unwrap();
    let page = s.call("desigma_get_document", json!({})).unwrap()["pages"][0]["node_id"].clone();
    let frame = s.call("desigma_create_node", json!({"parent_id":page,"kind":"frame","width":120,"height":80})).unwrap()["node"]["id"].clone();
    let sq = s.call("desigma_create_node", json!({"parent_id":frame,"kind":"rectangle","x":0,"y":30,"width":20,"height":20})).unwrap()["node"]["id"].clone();
    (s, frame, sq)
}

#[test]
fn keyframes_presets_and_the_timeline_are_edited_with_undo() {
    let dir = Workspace::new();
    let (mut s, _frame, sq) = animated_session(&dir);
    let r = s.call("desigma_add_keyframe", json!({"node_id":sq,"property":"position","time_ms":0,"value":[0,30],"easing":"ease in and out"})).unwrap();
    assert_eq!(r["keyframes_in_track"], 1);
    let r = s.call("desigma_add_keyframe", json!({"node_id":sq,"property":"position","time_ms":1000,"value":[100,30]})).unwrap();
    assert_eq!(r["keyframes_in_track"], 2);
    s.call("desigma_add_keyframe", json!({"node_id":sq,"property":"fill","time_ms":500,"value":"#FF0000","bezier":[0.4,0,0.2,1]})).unwrap();
    s.call("desigma_add_keyframe", json!({"node_id":sq,"property":"rotation","time_ms":0,"value":0,"spring_bounce":0.4})).unwrap();
    let n = s.call("desigma_get_node", json!({"node_id":sq})).unwrap();
    assert_eq!(n["node"]["motion"]["tracks"].as_array().unwrap().len(), 3, "{n}");
    s.call("desigma_apply_animation_preset", json!({"node_ids":[sq],"preset":"Pulse","start_ms":200})).unwrap();
    let n = s.call("desigma_get_node", json!({"node_id":sq})).unwrap();
    assert!(!n["node"]["motion"]["clips"].as_array().unwrap().is_empty());
    let t = s.call("desigma_set_timeline", json!({"duration_ms":1500,"fps":30,"loop":true})).unwrap();
    assert_eq!(t["timeline"], json!({"duration_ms":1500,"fps":30,"loop":true}));
    assert_eq!(s.call("desigma_set_timeline", json!({"fps":30})).unwrap()["changed"], false);
    // undo walks back through the timeline, the preset and the last keyframe
    for _ in 0..3 {
        assert_eq!(s.call("desigma_undo", json!({})).unwrap()["changed"], true);
    }
    let n = s.call("desigma_get_node", json!({"node_id":sq})).unwrap();
    assert_eq!(n["node"]["motion"]["tracks"].as_array().map(|a| a.len()), Some(2), "the rotation keyframe is undone: {n}");
}

#[test]
fn bad_input_is_rejected_without_changing_anything() {
    let dir = Workspace::new();
    let (mut s, frame, sq) = animated_session(&dir);
    let before = s.engine.doc.to_json().unwrap();
    let page = s.call("desigma_get_document", json!({})).unwrap()["pages"][0]["node_id"].clone();
    for (tool, args) in [
        ("desigma_add_keyframe", json!({"node_id":sq,"property":"nope","time_ms":0})),
        ("desigma_add_keyframe", json!({"node_id":sq,"property":"position","time_ms":0,"value":5})),
        ("desigma_add_keyframe", json!({"node_id":sq,"property":"fill","time_ms":0,"value":"red"})),
        ("desigma_add_keyframe", json!({"node_id":sq,"property":"opacity","time_ms":0,"easing":"wobbly"})),
        ("desigma_add_keyframe", json!({"node_id":sq,"property":"opacity","time_ms":99999999})),
        ("desigma_add_keyframe", json!({"node_id":page,"property":"opacity","time_ms":0})),
        ("desigma_add_keyframe", json!({"node_id":9999,"property":"opacity","time_ms":0})),
        ("desigma_add_keyframe", json!({"node_id":sq,"property":"opacity","time_ms":0,"extra":1})),
        ("desigma_apply_animation_preset", json!({"node_ids":[sq],"preset":"No such preset"})),
        ("desigma_apply_animation_preset", json!({"node_ids":[],"preset":"Pulse"})),
        ("desigma_set_timeline", json!({})),
        ("desigma_set_timeline", json!({"fps":0})),
        ("desigma_set_timeline", json!({"duration_ms":0})),
        ("desigma_set_timeline", json!({"page_id":frame,"fps":30})),
    ] {
        assert!(s.call(tool, args.clone()).is_err(), "{tool} {args}");
    }
    assert_eq!(s.engine.doc.to_json().unwrap(), before);
}

#[test]
fn export_animation_writes_gif_webm_and_zip_inside_the_workspace_only() {
    let dir = Workspace::new();
    let (mut s, frame, sq) = animated_session(&dir);
    assert!(s.call("desigma_export_animation", json!({"node_id":frame,"format":"gif","path":"a.gif"})).unwrap_err().contains("nothing is animated"));
    s.call("desigma_add_keyframe", json!({"node_id":sq,"property":"position","time_ms":0,"value":[0,30],"easing":"linear"})).unwrap();
    s.call("desigma_add_keyframe", json!({"node_id":sq,"property":"position","time_ms":500,"value":[100,30]})).unwrap();
    s.call("desigma_set_timeline", json!({"duration_ms":500,"fps":10,"loop":true})).unwrap();
    let out = s.call("desigma_export_animation", json!({"node_id":frame,"format":"gif","fps":10,"path":"a.gif"})).unwrap();
    assert_eq!((out["width"].as_u64(), out["height"].as_u64(), out["frames"].as_u64()), (Some(120), Some(80), Some(5)));
    assert_eq!(&fs::read(dir.0.join("a.gif")).unwrap()[..6], b"GIF89a");
    let out = s.call("desigma_export_animation", json!({"node_id":frame,"format":"webm","fps":10,"scale":0.5,"path":"a.webm"})).unwrap();
    assert_eq!(out["width"], 60);
    assert_eq!(&fs::read(dir.0.join("a.webm")).unwrap()[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
    s.call("desigma_export_animation", json!({"node_id":frame,"format":"png","fps":10,"path":"frames.zip"})).unwrap();
    assert_eq!(&fs::read(dir.0.join("frames.zip")).unwrap()[..2], b"PK");
    // overwrite of the same kind is fine; wrong extension, escapes and missing dirs are not
    s.call("desigma_export_animation", json!({"node_id":frame,"format":"gif","path":"a.gif"})).unwrap();
    for (format, path) in [("gif", "a.png"), ("webm", "a.gif"), ("png", "a.png"), ("gif", "../a.gif"), ("gif", "missing/a.gif"), ("mov", "a.mov")] {
        assert!(s.call("desigma_export_animation", json!({"node_id":frame,"format":format,"path":path})).is_err(), "{format} {path}");
    }
    assert!(s.call("desigma_export_animation", json!({"node_id":frame,"format":"gif","fps":60,"path":"b.gif"})).is_err(), "gif is limited to 50 fps");
    assert!(s.call("desigma_export_animation", json!({"node_id":frame,"format":"gif","scale":9,"path":"b.gif"})).is_err());
    assert!(!dir.0.join("b.gif").exists());
}

#[cfg(unix)]
#[test]
fn export_animation_refuses_to_write_through_a_symlink() {
    let dir = Workspace::new();
    let outside = Workspace::new();
    let target = outside.0.join("outside.gif");
    fs::write(&target, b"untouched").unwrap();
    std::os::unix::fs::symlink(&target, dir.0.join("link.gif")).unwrap();
    let (mut s, frame, sq) = animated_session(&dir);
    s.call("desigma_add_keyframe", json!({"node_id":sq,"property":"opacity","time_ms":0,"value":1})).unwrap();
    s.call("desigma_add_keyframe", json!({"node_id":sq,"property":"opacity","time_ms":500,"value":0})).unwrap();
    assert!(s.call("desigma_export_animation", json!({"node_id":frame,"format":"gif","path":"link.gif"})).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
}
