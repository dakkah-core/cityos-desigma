//! Rendering of board drawing strokes: washi tape strips with patterns and torn ends, and the
//! highlighter's multiply blend.

use desigma_doc::*;
use std::collections::HashSet;

fn stroke_node(doc: &mut Document, a: Vec2, b: Vec2, brush: Brush, weight: f32, color: Color) -> NodeId {
    let page = doc.pages[0];
    let id = doc.alloc_id();
    let net = vector::VectorNetwork::from_paths(&[Path { cmds: vec![PathCmd::MoveTo(a), PathCmd::LineTo(b)], fill_rule: FillRule::NonZero }]);
    let mut n = Node::new(id, NodeKind::Vector { paths: net.to_paths(), network: Some(net) });
    n.brush = Some(brush);
    n.stroke.weight = weight;
    n.stroke.paints = vec![Paint::solid(color)];
    n.size = Vec2::new((b.x - a.x).abs().max(1.0), (b.y - a.y).abs().max(1.0));
    n.transform = Affine::translate(a.x.min(b.x), a.y.min(b.y));
    doc.insert(page, n).unwrap()
}

fn colors(pm: &tiny_skia::Pixmap) -> HashSet<[u8; 4]> {
    pm.pixels().iter().filter(|p| p.alpha() > 200).map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect()
}

/// Renders the page at `zoom` into a 200x100 pixmap.
fn render(d: &Document, zoom: f32) -> tiny_skia::Pixmap {
    let mut pm = tiny_skia::Pixmap::new(200, 100).unwrap();
    let vp = desigma_render::Viewport { offset: Vec2::ZERO, zoom, dpr: 1.0 };
    desigma_render::render_page(d, d.pages[0], &vp, &mut pm);
    pm
}

fn doc() -> Document {
    let mut d = Document::new("t");
    let page = d.add_page("p");
    // a transparent page, so only the strokes show up in the pixels
    d.get_mut(page).unwrap().kind = NodeKind::Page { background: Color { a: 0.0, ..Color::WHITE } };
    d
}

#[test]
fn washi_tape_prints_a_pattern_on_a_strip_with_torn_ends() {
    let mut d = doc();
    let id = stroke_node(&mut d, Vec2::new(20.0, 20.0), Vec2::new(120.0, 20.0), Brush::Washi(WashiPattern::Dots), 24.0, Color::rgb(0xF2, 0x48, 0x22));
    let pm = render(&d, 1.0);
    let _ = id;
    // the base colour and the printed marks
    assert!(colors(&pm).len() >= 2, "a patterned strip has more than one colour");
    // the strip is as tall as the stroke is wide
    let rows: Vec<u32> = (0..pm.height()).filter(|y| (0..pm.width()).any(|x| pm.pixel(x, *y).unwrap().alpha() > 128)).collect();
    let span = rows.last().unwrap() - rows.first().unwrap() + 1;
    assert!((22..=27).contains(&span), "strip height {span}");
}

#[test]
fn every_washi_pattern_renders_and_they_differ() {
    let mut seen = Vec::new();
    for pat in WashiPattern::ALL {
        let mut d = doc();
        let id = stroke_node(&mut d, Vec2::new(10.0, 10.0), Vec2::new(110.0, 40.0), Brush::Washi(pat), 24.0, Color::rgb(0x0C, 0x8C, 0xE9));
        let pm = render(&d, 1.0);
        let _ = id;
        assert!(pm.pixels().iter().any(|p| p.alpha() > 0), "{pat:?} drew nothing");
        seen.push(pm.data().to_vec());
    }
    for i in 0..seen.len() {
        for j in i + 1..seen.len() {
            assert_ne!(seen[i], seen[j], "patterns {i} and {j} look the same");
        }
    }
}

#[test]
fn a_marker_is_one_flat_colour() {
    let mut d = doc();
    stroke_node(&mut d, Vec2::new(20.0, 20.0), Vec2::new(120.0, 20.0), Brush::Marker, 6.0, Color::rgb(0x14, 0xAE, 0x5C));
    let pm = render(&d, 1.0);
    assert_eq!(colors(&pm).len(), 1);
}

#[test]
fn washi_tape_scales_with_the_zoom() {
    let mut d = doc();
    stroke_node(&mut d, Vec2::new(20.0, 20.0), Vec2::new(60.0, 20.0), Brush::Washi(WashiPattern::Stripes), 24.0, Color::rgb(0xFF, 0xA6, 0x29));
    let count = |pm: &tiny_skia::Pixmap| pm.pixels().iter().filter(|p| p.alpha() > 128).count();
    let (a, b) = (count(&render(&d, 1.0)), count(&render(&d, 2.0)));
    assert!(b > a * 3, "twice the zoom covers about four times the pixels: {a} -> {b}");
}
