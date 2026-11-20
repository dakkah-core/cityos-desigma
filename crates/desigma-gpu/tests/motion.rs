//! Motion: the GPU draws a page evaluated at a time exactly like the CPU renderer does, and the
//! incremental compile used while playing (new revision every frame, geometry cache kept) gives
//! the same pixels as compiling the evaluated page from scratch. Tests print a note and return
//! when the machine has no GPU adapter.

mod common;

use common::*;
use desigma_doc::motion::{find_preset, AnimProperty, AnimValue, Ease, MotionPlayer};
use desigma_doc::*;
use desigma_gpu::SceneCompiler;
use desigma_render::Viewport;

const SIZE: (u32, u32) = (320, 200);

/// A static card with an animated square (position, rotation, opacity, fill), an animated
/// ellipse (size, corner-less) and a layer driven by the Pulse preset.
fn scene() -> (Document, NodeId) {
    let mut t = T::new();
    t.rect(None, Rect::new(10.0, 10.0, 300.0, 180.0), Color::rgb(0xEE, 0xEE, 0xF4));
    let sq = t.rect(None, Rect::new(20.0, 30.0, 50.0, 50.0), Color::rgb(255, 0, 0));
    {
        let n = t.node(sq).motion.ensure_track(AnimProperty::Position);
        n.set_key(0, AnimValue::Vec2(Vec2::new(20.0, 30.0)), Some(Ease::bezier(0.4, 0.0, 0.2, 1.0)));
        n.set_key(1000, AnimValue::Vec2(Vec2::new(220.0, 30.0)), None);
        let r = t.node(sq).motion.ensure_track(AnimProperty::Rotation);
        r.set_key(0, AnimValue::Scalar(0.0), Some(Ease::Linear));
        r.set_key(1000, AnimValue::Scalar(90.0), None);
        let o = t.node(sq).motion.ensure_track(AnimProperty::Opacity);
        o.set_key(0, AnimValue::Scalar(1.0), Some(Ease::Linear));
        o.set_key(1000, AnimValue::Scalar(0.4), None);
        let f = t.node(sq).motion.ensure_track(AnimProperty::FillColor);
        f.set_key(0, AnimValue::Color(Color::rgb(255, 0, 0)), Some(Ease::Linear));
        f.set_key(1000, AnimValue::Color(Color::rgb(0, 0, 255)), None);
    }
    let el = t.add(None, |id| Node::ellipse(id, Rect::new(30.0, 110.0, 60.0, 40.0)));
    t.node(el).fills = vec![Paint::solid(Color::rgb(0x18, 0xA0, 0xFB))];
    {
        let w = t.node(el).motion.ensure_track(AnimProperty::Width);
        w.set_key(0, AnimValue::Scalar(60.0), Some(Ease::spring(0.4)));
        w.set_key(1000, AnimValue::Scalar(180.0), None);
    }
    let pulse = t.rect(None, Rect::new(240.0, 120.0, 40.0, 40.0), Color::rgb(0x1B, 0xC4, 0x7D));
    t.node(pulse).motion.clips = find_preset("Pulse").unwrap().instantiate(0, 0);
    (t.doc, t.page)
}

#[test]
fn evaluated_pages_match_the_cpu_renderer() {
    let Some(mut g) = gpu() else { return };
    let (doc, page) = scene();
    let mut player = MotionPlayer::new(&doc, page);
    for t in [0.0, 250.0, 600.0, 1000.0] {
        let shown = player.seek(&doc, t).clone();
        // `compare` compiles every page as revision 0: drop the geometry cached for the last time
        g.compiler.geom.clear();
        let c = compare(&mut g, &format!("motion_{t}"), &shown, page, Viewport::default(), SIZE, 8);
        assert!(c.report.ratio < 0.03, "t = {t} ms: mismatch {:.3}%", c.report.ratio * 100.0);
    }
    // the animation really moved: frame 0 and frame 1000 differ
    let a = player.seek(&doc, 0.0).clone();
    g.compiler.geom.clear();
    let first = g.render_page(&a, page, &Viewport::default(), SIZE);
    let b = player.seek(&doc, 1000.0).clone();
    g.compiler.geom.clear();
    let last = g.render_page(&b, page, &Viewport::default(), SIZE);
    assert_ne!(first.data(), last.data());
}

#[test]
fn incremental_compile_equals_a_fresh_compile() {
    let Some(mut g) = gpu() else { return };
    let (doc, page) = scene();
    let mut player = MotionPlayer::new(&doc, page);
    let vp = Viewport::default();
    let mut compiles = Vec::new();
    for (i, t) in [0.0, 100.0, 220.0, 450.0, 451.0, 800.0, 1000.0, 300.0].into_iter().enumerate() {
        let shown = player.seek(&doc, t);
        // the compiler g keeps its caches across revisions, like the canvas does while playing
        let frame = g.compile(shown, page, i as u64 + 1, &vp, SIZE);
        let incremental = g.render_frame(&frame, SIZE);
        let mut fresh = SceneCompiler::new();
        fresh.opts.max_texture = g.renderer.max_texture_dimension();
        let f2 = fresh.compile(shown, page, 1, &vp, SIZE);
        let from_scratch = g.render_frame(&f2, SIZE);
        assert_eq!(incremental.data(), from_scratch.data(), "t = {t} ms differs between incremental and fresh compile");
        compiles.push(g.compiler.geom.builds);
    }
    // position, rotation, opacity and colour changes do not rebuild geometry: only the animated
    // widths of the ellipse and the pulse's scale do, and the counter stays far below frames x nodes
    let total = compiles.last().unwrap() - compiles.first().unwrap();
    assert!(total < 40, "geometry rebuilt {total} times over 7 frames");
}

/// Compile speed while playing: `cargo test --release -p desigma-gpu --test motion -- --ignored --nocapture`.
#[test]
#[ignore]
fn compile_speed_while_playing() {
    let Some(mut g) = gpu() else { return };
    let (mut doc, page) = demo::demo_scene();
    // animate 40 of the demo's layers
    let ids: Vec<NodeId> = doc.nodes.values().filter(|n| matches!(n.kind, NodeKind::Rectangle | NodeKind::Ellipse(_))).map(|n| n.id).take(40).collect();
    for id in &ids {
        let n = doc.get_mut(*id).unwrap();
        let base = Vec2::new(n.transform.tx, n.transform.ty);
        let p = n.motion.ensure_track(AnimProperty::Position);
        p.set_key(0, AnimValue::Vec2(base), Some(Ease::Linear));
        p.set_key(2000, AnimValue::Vec2(Vec2::new(base.x + 40.0, base.y)), None);
    }
    let vp = Viewport { offset: Vec2::ZERO, zoom: 1.0, dpr: 1.0 };
    let size = (1280, 800);
    let mut player = MotionPlayer::new(&doc, page);
    let (mut seek, mut compile) = (0.0f64, 0.0f64);
    let n = 240;
    for i in 0..n {
        let t0 = std::time::Instant::now();
        let shown = player.seek(&doc, (i * 16 % 2000) as f32);
        let t1 = std::time::Instant::now();
        let _frame = g.compile(shown, page, i as u64 + 1, &vp, size);
        seek += (t1 - t0).as_secs_f64();
        compile += t1.elapsed().as_secs_f64();
    }
    eprintln!(
        "{} animated layers of {} nodes: seek {:.3} ms/frame, compile {:.3} ms/frame (CPU side, {n} frames)",
        ids.len(),
        doc.nodes.len(),
        seek / n as f64 * 1000.0,
        compile / n as f64 * 1000.0
    );
}
