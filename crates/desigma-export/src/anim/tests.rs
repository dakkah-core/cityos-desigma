use super::*;
use desigma_doc::motion::{AnimProperty, AnimValue, Ease};
use desigma_doc::{Color, Document, Node, NodeId, Paint, Rect, Vec2};

/// A 100x60 white frame with a 20x20 square that slides from x = 0 to x = 80 in 1 s (linear) and
/// turns from red to blue. Returns (doc, page, frame, square).
fn slide_doc(duration_ms: u32, fps: u32, looping: bool) -> (Document, NodeId, NodeId, NodeId) {
    let mut d = Document::new("anim");
    let page = d.add_page("P");
    let frame = d.alloc_id();
    let mut f = Node::frame(frame, Rect::new(0.0, 0.0, 100.0, 60.0));
    f.fills = vec![Paint::solid(Color::WHITE)];
    d.insert(page, f).unwrap();
    let sq = d.alloc_id();
    let mut n = Node::rectangle(sq, Rect::new(0.0, 20.0, 20.0, 20.0));
    n.fills = vec![Paint::solid(Color::rgb(255, 0, 0))];
    let pos = n.motion.ensure_track(AnimProperty::Position);
    pos.set_key(0, AnimValue::Vec2(Vec2::new(0.0, 20.0)), Some(Ease::Linear));
    pos.set_key(1000, AnimValue::Vec2(Vec2::new(80.0, 20.0)), None);
    let fill = n.motion.ensure_track(AnimProperty::FillColor);
    fill.set_key(0, AnimValue::Color(Color::rgb(255, 0, 0)), Some(Ease::Linear));
    fill.set_key(1000, AnimValue::Color(Color::rgb(0, 0, 255)), None);
    d.insert(frame, n).unwrap();
    d.timelines.insert(page, desigma_doc::motion::TimelineSettings { duration_ms, fps, looping });
    (d, page, frame, sq)
}

fn px(pm: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
    let c = pm.pixel(x, y).unwrap();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

#[test]
fn frame_times_loop_and_stop() {
    let t = desigma_doc::motion::TimelineSettings { duration_ms: 1000, fps: 60, looping: true };
    let v = frame_times(&t, 10);
    assert_eq!(v.len(), 10);
    assert_eq!(v[0], 0.0);
    assert!((v[9] - 900.0).abs() < 1e-3, "a looping export stops one frame before the end");
    let t = desigma_doc::motion::TimelineSettings { looping: false, ..t };
    let v = frame_times(&t, 10);
    assert_eq!(v.len(), 11);
    assert_eq!(*v.last().unwrap(), 1000.0, "a one-shot export ends on the last state");
    // 24 fps does not divide 1000 ms: the end is still exact
    let v = frame_times(&t, 24);
    assert_eq!(*v.last().unwrap(), 1000.0);
}

#[test]
fn rendered_frames_follow_the_timeline() {
    let (d, _page, frame, _) = slide_doc(1000, 30, true);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    assert_eq!(a.size(), (100, 60));
    assert!(a.is_animated());
    let p0 = a.render(0.0);
    assert_eq!(px(&p0, 10, 30), [255, 0, 0, 255]);
    assert_eq!(px(&p0, 90, 30), [255, 255, 255, 255]);
    let p5 = a.render(500.0);
    // halfway: square at x = 40..60, colour midway
    assert_eq!(px(&p5, 10, 30), [255, 255, 255, 255]);
    let c = px(&p5, 50, 30);
    assert!((c[0] as i32 - 128).abs() <= 2 && (c[2] as i32 - 128).abs() <= 2, "{c:?}");
    let p10 = a.render(1000.0);
    assert_eq!(px(&p10, 90, 30), [0, 0, 255, 255]);
    // and it equals the static export of the evaluated document
    let evaluated = desigma_doc::motion::evaluate(&d, d.page_of(frame).unwrap(), 500.0);
    let still = crate::render_export(&evaluated, frame, 1.0).unwrap();
    assert_eq!(still.data(), p5.data());
}

#[test]
fn scale_changes_the_output_size() {
    let (d, _p, frame, _) = slide_doc(1000, 30, true);
    let mut a = Animation::new(&d, frame, 2.0).unwrap();
    assert_eq!(a.size(), (200, 120));
    let p = a.render(0.0);
    assert_eq!((p.width(), p.height()), (200, 120));
}

#[test]
fn page_export_covers_all_frames_of_the_motion() {
    let (d, page, _frame, _) = slide_doc(1000, 30, true);
    let a = Animation::new(&d, page, 1.0).unwrap();
    assert_eq!(a.size(), (100, 60));
}

#[test]
fn too_large_is_refused() {
    let (mut d, _p, frame, _) = slide_doc(1000, 30, true);
    d.get_mut(frame).unwrap().size = Vec2::new(1000.0, 60.0);
    assert!(matches!(Animation::new(&d, frame, 16.0), Err(AnimError::TooLarge(_))));
    assert!(matches!(Animation::new(&d, NodeId(9999), 1.0), Err(AnimError::NoNode)));
}

// ------------------------------------------------------------------------------------ GIF

struct GifInfo {
    width: u16,
    height: u16,
    delays: Vec<u16>,
    repeat: gif::Repeat,
    /// The canvas after each frame (RGBA), composited like a viewer would.
    canvases: Vec<Vec<u8>>,
}

fn decode_gif(bytes: &[u8]) -> GifInfo {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts.read_info(std::io::Cursor::new(bytes)).unwrap();
    let (w, h) = (dec.width() as usize, dec.height() as usize);
    let repeat = dec.repeat();
    let mut canvas = vec![0u8; w * h * 4];
    let mut delays = Vec::new();
    let mut canvases = Vec::new();
    while let Some(f) = dec.read_next_frame().unwrap() {
        for y in 0..f.height as usize {
            for x in 0..f.width as usize {
                let s = (y * f.width as usize + x) * 4;
                if f.buffer[s + 3] != 0 {
                    let o = ((f.top as usize + y) * w + f.left as usize + x) * 4;
                    canvas[o..o + 4].copy_from_slice(&f.buffer[s..s + 4]);
                }
            }
        }
        delays.push(f.delay);
        canvases.push(canvas.clone());
        if f.dispose == gif::DisposalMethod::Background {
            canvas = vec![0u8; w * h * 4];
        }
    }
    GifInfo { width: w as u16, height: h as u16, delays, repeat, canvases }
}

#[test]
fn gif_has_the_right_frames_delays_and_loop() {
    let (d, _p, frame, _) = slide_doc(1000, 25, true);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let mut seen = Vec::new();
    let mut cb = |p: Progress| {
        seen.push(p);
        true
    };
    let (bytes, stats) = encode_gif(&mut a, &GifOptions { fps: 25, ..Default::default() }, Some(&mut cb)).unwrap();
    assert_eq!(&bytes[..6], b"GIF89a");
    let g = decode_gif(&bytes);
    assert_eq!((g.width, g.height), (100, 60));
    assert_eq!(stats.frames, 25);
    assert_eq!(g.delays.len() as u32, stats.frames_written);
    assert_eq!(g.delays.len(), 25, "every frame differs, so none are merged");
    assert!(g.delays.iter().all(|d| *d == 4), "25 fps is 4 cs per frame: {:?}", g.delays);
    assert_eq!(g.delays.iter().map(|d| *d as u32).sum::<u32>(), 100, "1 s in total");
    assert!(matches!(g.repeat, gif::Repeat::Infinite));
    assert_eq!(seen.last().copied(), Some(Progress { done: 25, total: 25 }));
    // frame 0: red square at the left; the composited last frame has the square near the right, bluish
    let at = |c: &Vec<u8>, x: usize, y: usize| {
        let o = (y * 100 + x) * 4;
        [c[o], c[o + 1], c[o + 2]]
    };
    assert_eq!(at(&g.canvases[0], 10, 30), [255, 0, 0]);
    assert_eq!(at(&g.canvases[0], 90, 30), [255, 255, 255]);
    let last = &g.canvases[24];
    let c = at(last, 87, 30);
    assert!(c[2] > 200 && c[0] < 60, "{c:?}");
    assert_eq!(at(last, 5, 30), [255, 255, 255], "the square left the start position");
}

#[test]
fn gif_without_loop_plays_once_and_ends_on_the_last_state() {
    let (d, _p, frame, _) = slide_doc(500, 20, false);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let (bytes, stats) = encode_gif(&mut a, &GifOptions { fps: 20, ..Default::default() }, None).unwrap();
    let g = decode_gif(&bytes);
    assert!(matches!(g.repeat, gif::Repeat::Finite(0)), "{:?}", g.repeat);
    assert_eq!(stats.frames, 11, "10 frames + the final state");
    assert_eq!(g.delays.len(), 11);
}

#[test]
fn gif_delays_average_out_for_fps_that_do_not_divide_100() {
    let (d, _p, frame, _) = slide_doc(1000, 30, true);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let (bytes, _) = encode_gif(&mut a, &GifOptions { fps: 30, ..Default::default() }, None).unwrap();
    let g = decode_gif(&bytes);
    assert_eq!(g.delays.len(), 30);
    assert_eq!(g.delays.iter().map(|d| *d as u32).sum::<u32>(), 100);
    assert!(g.delays.iter().all(|d| *d == 3 || *d == 4));
}

#[test]
fn gif_fps_is_capped_at_50() {
    let (d, _p, frame, _) = slide_doc(1000, 60, true);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let (bytes, stats) = encode_gif(&mut a, &GifOptions { fps: 60, ..Default::default() }, None).unwrap();
    assert_eq!(stats.frames, 50);
    assert!(decode_gif(&bytes).delays.iter().all(|d| *d == 2));
}

#[test]
fn gif_merges_identical_frames_and_keeps_the_duration() {
    // nothing moves after 200 ms
    let (mut d, page, _frame, sq) = slide_doc(1000, 20, true);
    let n = d.get_mut(sq).unwrap();
    n.motion.ensure_track(AnimProperty::Position).set_key(200, AnimValue::Vec2(Vec2::new(16.0, 20.0)), Some(Ease::Linear));
    n.motion.ensure_track(AnimProperty::Position).remove_key(1000);
    n.motion.ensure_track(AnimProperty::FillColor).remove_key(1000);
    let frame = d.get(sq).unwrap().parent.unwrap();
    let _ = page;
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let (bytes, stats) = encode_gif(&mut a, &GifOptions { fps: 20, ..Default::default() }, None).unwrap();
    let g = decode_gif(&bytes);
    assert_eq!(stats.frames, 20);
    assert!(g.delays.len() < 20, "static tail merged: {}", g.delays.len());
    assert_eq!(g.delays.iter().map(|d| *d as u32).sum::<u32>(), 100);
}

#[test]
fn gif_transparent_background_and_gradient_palette() {
    let (mut d, _p, frame, _) = slide_doc(400, 10, true);
    d.get_mut(frame).unwrap().fills.clear();
    // a gradient fill forces the NeuQuant path
    let sq = d.get(frame).unwrap().children[0];
    d.get_mut(sq).unwrap().fills = vec![Paint {
        kind: desigma_doc::PaintKind::LinearGradient {
            stops: vec![
                desigma_doc::GradientStop { pos: 0.0, color: Color::rgb(255, 0, 0) },
                desigma_doc::GradientStop { pos: 1.0, color: Color::rgb(0, 0, 255) },
            ],
            handles: [Vec2::new(0.0, 0.5), Vec2::new(1.0, 0.5), Vec2::new(0.0, 1.0)],
        },
        opacity: 1.0,
        visible: true,
        blend: desigma_doc::BlendMode::Normal,
    }];
    d.get_mut(sq).unwrap().motion.tracks.retain(|t| t.property == AnimProperty::Position);
    // two full-width gradient bars add enough distinct colours to need a trained palette
    for (i, (a, b)) in [
        (Color::rgb(0, 255, 0), Color::rgb(255, 255, 0)),
        (Color::rgb(255, 0, 255), Color::rgb(0, 255, 255)),
        (Color::rgb(0, 0, 0), Color::rgb(200, 100, 50)),
        (Color::rgb(90, 10, 200), Color::rgb(10, 200, 90)),
    ]
    .into_iter()
    .enumerate()
    {
        let id = d.alloc_id();
        let mut bar = Node::rectangle(id, Rect::new(0.0, i as f32 * 15.0, 100.0, 10.0));
        bar.fills = vec![Paint {
            kind: desigma_doc::PaintKind::LinearGradient {
                stops: vec![desigma_doc::GradientStop { pos: 0.0, color: a }, desigma_doc::GradientStop { pos: 1.0, color: b }],
                handles: [Vec2::new(0.0, 0.5), Vec2::new(1.0, 0.5), Vec2::new(0.0, 1.0)],
            },
            opacity: 1.0,
            visible: true,
            blend: desigma_doc::BlendMode::Normal,
        }];
        d.insert(frame, bar).unwrap();
    }
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let o = GifOptions { fps: 10, background: None, dither: true, ..Default::default() };
    let (bytes, stats) = encode_gif(&mut a, &o, None).unwrap();
    assert!(!stats.exact_palette);
    let g = decode_gif(&bytes);
    assert_eq!(g.delays.len(), 4);
    let o0 = (27 * 100 + 90) * 4;
    assert_eq!(g.canvases[0][o0 + 3], 0, "outside the square the frame is transparent");
}

#[test]
fn gif_cancel_stops_the_encode() {
    let (d, _p, frame, _) = slide_doc(1000, 30, true);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let mut n = 0;
    let mut cb = |_: Progress| {
        n += 1;
        n < 5
    };
    let r = encode_gif(&mut a, &GifOptions::default(), Some(&mut cb));
    assert!(matches!(r, Err(AnimError::Cancelled)));
}

// ------------------------------------------------------------------------------- PNG zip

fn le16(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}

fn le32(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

#[test]
fn png_zip_lists_every_frame() {
    let (d, _p, frame, _) = slide_doc(500, 10, false);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let mut out = Vec::new();
    let n = write_png_zip(&mut a, 10, "My anim", &mut out, None).unwrap();
    assert_eq!(n, 6);
    // end of central directory
    let eocd = out.len() - 22;
    assert_eq!(&out[eocd..eocd + 4], &[0x50, 0x4B, 5, 6]);
    assert_eq!(le16(&out, eocd + 10), 6);
    let (dir_size, dir_off) = (le32(&out, eocd + 12), le32(&out, eocd + 16));
    assert_eq!(dir_off + dir_size, eocd);
    // walk the directory
    let mut o = dir_off;
    let mut names = Vec::new();
    for _ in 0..6 {
        assert_eq!(&out[o..o + 4], &[0x50, 0x4B, 1, 2]);
        let (crc, size, nl) = (le32(&out, o + 16), le32(&out, o + 24), le16(&out, o + 28));
        let off = le32(&out, o + 42);
        names.push(String::from_utf8(out[o + 46..o + 46 + nl].to_vec()).unwrap());
        // local header, then the PNG bytes
        let lnl = le16(&out, off + 26);
        let data = &out[off + 30 + lnl..off + 30 + lnl + size];
        assert_eq!(&data[1..4], b"PNG");
        let mut c = flate2::Crc::new();
        c.update(data);
        assert_eq!(c.sum() as usize, crc);
        o += 46 + nl;
    }
    assert_eq!(names[0], "My anim_0001.png");
    assert_eq!(names[5], "My anim_0006.png");
}

#[test]
fn png_dir_writes_numbered_files() {
    let (d, _p, frame, _) = slide_doc(300, 10, true);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let dir = std::env::temp_dir().join(format!("desigma-anim-{}", std::process::id()));
    let paths = write_png_dir(&mut a, 10, "f", &dir, None).unwrap();
    assert_eq!(paths.len(), 3);
    let pm = tiny_skia::Pixmap::load_png(&paths[2]).unwrap();
    assert_eq!((pm.width(), pm.height()), (100, 60));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ------------------------------------------------------------------------------- WebM

/// A minimal EBML reader for the tests: (id, payload start, payload end, element start).
fn elements(b: &[u8], mut o: usize, end: usize) -> Vec<(u32, usize, usize, usize)> {
    let mut v = Vec::new();
    while o < end {
        let start = o;
        let mut id = 0u32;
        let first = b[o];
        let idlen = first.leading_zeros() as usize + 1;
        for k in 0..idlen {
            id = (id << 8) | b[o + k] as u32;
        }
        o += idlen;
        let first = b[o];
        let slen = first.leading_zeros() as usize + 1;
        let mut size = (first as u64) & ((1 << (8 - slen)) - 1);
        for k in 1..slen {
            size = (size << 8) | b[o + k] as u64;
        }
        o += slen;
        let e = o + size as usize;
        assert!(e <= end, "element {id:x} overruns its parent");
        v.push((id, o, e, start));
        o = e;
    }
    v
}

fn find(v: &[(u32, usize, usize, usize)], id: u32) -> Vec<(u32, usize, usize, usize)> {
    v.iter().copied().filter(|e| e.0 == id).collect()
}

#[test]
fn webm_is_a_valid_matroska_with_the_right_tracks_clusters_and_duration() {
    let (d, _p, frame, _) = slide_doc(1000, 12, true);
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let o = WebmOptions { fps: 12, speed: 10, quality: 60, ..Default::default() };
    let mut last = Progress { done: 0, total: 0 };
    let mut cb = |p: Progress| {
        last = p;
        true
    };
    let bytes = encode_webm(&mut a, &o, Some(&mut cb)).unwrap();
    assert_eq!(last.done, last.total);
    let top = elements(&bytes, 0, bytes.len());
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].0, 0x1A45DFA3);
    let hdr = elements(&bytes, top[0].1, top[0].2);
    let doctype = find(&hdr, 0x4282)[0];
    assert_eq!(&bytes[doctype.1..doctype.2], b"webm");
    assert_eq!(top[1].0, 0x18538067, "Segment");
    let seg = elements(&bytes, top[1].1, top[1].2);
    let ids: Vec<u32> = seg.iter().map(|e| e.0).collect();
    assert_eq!(ids[..3], [0x114D9B74, 0x1549A966, 0x1654AE6B], "SeekHead, Info, Tracks");
    assert_eq!(*ids.last().unwrap(), 0x1C53BB6B, "Cues last");
    // seek positions point at the elements
    let seg_start = top[1].1;
    let sh = elements(&bytes, seg[0].1, seg[0].2);
    for s in sh {
        let f = elements(&bytes, s.1, s.2);
        let pos_e = find(&f, 0x53AC)[0];
        let pos = bytes[pos_e.1..pos_e.2].iter().fold(0usize, |a, b| (a << 8) | *b as usize);
        let id_e = find(&f, 0x53AB)[0];
        let id = bytes[id_e.1..id_e.2].iter().fold(0u32, |a, b| (a << 8) | *b as u32);
        assert_eq!(seg.iter().find(|e| e.3 == seg_start + pos).map(|e| e.0), Some(id));
    }
    // info: 12 frames at 12 fps = 1000 ms
    let info = elements(&bytes, seg[1].1, seg[1].2);
    let dur = find(&info, 0x4489)[0];
    let dur = f64::from_be_bytes(bytes[dur.1..dur.2].try_into().unwrap());
    assert!((dur - 1000.0).abs() < 1e-6, "{dur}");
    // tracks
    let tracks = elements(&bytes, seg[2].1, seg[2].2);
    let entry = elements(&bytes, tracks[0].1, tracks[0].2);
    let codec = find(&entry, 0x86)[0];
    assert_eq!(&bytes[codec.1..codec.2], b"V_AV1");
    let private = find(&entry, 0x63A2)[0];
    assert_eq!(bytes[private.1] & 0x80, 0x80, "av1C marker bit");
    let video = elements(&bytes, find(&entry, 0xE0)[0].1, find(&entry, 0xE0)[0].2);
    let pw = find(&video, 0xB0)[0];
    let ph = find(&video, 0xBA)[0];
    assert_eq!((bytes[pw.1..pw.2].iter().fold(0u32, |a, b| (a << 8) | *b as u32), bytes[ph.1..ph.2].iter().fold(0u32, |a, b| (a << 8) | *b as u32)), (100, 60));
    // clusters and blocks
    let clusters = find(&seg, 0x1F43B675);
    assert!(!clusters.is_empty());
    let mut blocks = 0;
    let mut keys = 0;
    let mut stamps = Vec::new();
    for c in &clusters {
        let ch = elements(&bytes, c.1, c.2);
        let ts_e = find(&ch, 0xE7)[0];
        let ts = bytes[ts_e.1..ts_e.2].iter().fold(0u64, |a, b| (a << 8) | *b as u64);
        for b in find(&ch, 0xA3) {
            assert_eq!(bytes[b.1], 0x81);
            let rel = i16::from_be_bytes([bytes[b.1 + 1], bytes[b.1 + 2]]);
            stamps.push(ts + rel as u64);
            if bytes[b.1 + 3] & 0x80 != 0 {
                keys += 1;
            }
            assert!(b.2 - b.1 > 4, "block has a payload");
            blocks += 1;
        }
    }
    assert_eq!(blocks, 12, "one block per frame");
    assert!(keys >= 1 && keys == clusters.len(), "a cluster per key frame");
    assert_eq!(stamps, (0..12).map(|i| (i as f64 * 1000.0 / 12.0).round() as u64).collect::<Vec<_>>());
    let cues = elements(&bytes, seg.last().unwrap().1, seg.last().unwrap().2);
    assert_eq!(cues.len(), clusters.len());
}

#[test]
fn webm_quality_maps_to_the_quantizer() {
    let o = WebmOptions { quality: 75, ..Default::default() };
    assert_eq!(o.quantizer(), 105);
    assert_eq!(WebmOptions::default().quantizer(), 85);
    assert!(WebmOptions { quality: 100, ..Default::default() }.quantizer() < o.quantizer());
    assert!(WebmOptions { quality: 0, ..Default::default() }.quantizer() <= 253);
}

/// Manual check: `DESIGMA_ANIM_OUT=dir cargo test -p desigma-export write_samples -- --ignored
/// --nocapture` writes a GIF, WebMs and a PNG zip of a 640x360 two second scene (with timings) so
/// they can be opened in a browser or inspected with ffprobe.
#[test]
#[ignore]
fn write_samples() {
    let Ok(dir) = std::env::var("DESIGMA_ANIM_OUT") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut d, _page, frame, sq) = slide_doc(2000, 60, true);
    d.get_mut(frame).unwrap().size = Vec2::new(640.0, 360.0);
    d.get_mut(sq).unwrap().size = Vec2::new(120.0, 120.0);
    let track = d.get_mut(sq).unwrap().motion.ensure_track(AnimProperty::Position);
    track.set_key(0, AnimValue::Vec2(Vec2::new(20.0, 120.0)), Some(Ease::bezier(0.4, 0.0, 0.2, 1.0)));
    track.set_key(1000, AnimValue::Vec2(Vec2::new(500.0, 120.0)), Some(Ease::spring(0.4)));
    track.set_key(2000, AnimValue::Vec2(Vec2::new(20.0, 120.0)), None);
    let rot = d.get_mut(sq).unwrap().motion.ensure_track(AnimProperty::Rotation);
    rot.set_key(0, AnimValue::Scalar(0.0), Some(Ease::Linear));
    rot.set_key(2000, AnimValue::Scalar(360.0), None);
    d.get_mut(sq).unwrap().fills = vec![Paint::solid(Color::rgb(255, 80, 40))];
    d.get_mut(sq).unwrap().motion.tracks.retain(|t| t.property != AnimProperty::FillColor);
    {
        let mut a = Animation::new(&d, frame, 1.0).unwrap();
        let t = std::time::Instant::now();
        let (bytes, st) = encode_gif(&mut a, &GifOptions { fps: 30, ..Default::default() }, None).unwrap();
        println!("sample.gif: {} bytes, {:?} ({} of {} frames stored, palette {})", bytes.len(), t.elapsed(), st.frames_written, st.frames, st.palette_colors);
        std::fs::write(dir.join("sample.gif"), bytes).unwrap();
    }
    for fps in [30, 60] {
        let mut a = Animation::new(&d, frame, 1.0).unwrap();
        let t = std::time::Instant::now();
        let bytes = encode_webm(&mut a, &WebmOptions { fps, ..Default::default() }, None).unwrap();
        println!("sample{fps}.webm: {} bytes, {:?}", bytes.len(), t.elapsed());
        std::fs::write(dir.join(format!("sample{fps}.webm")), bytes).unwrap();
    }
    let mut a = Animation::new(&d, frame, 1.0).unwrap();
    let mut f = std::fs::File::create(dir.join("sample.zip")).unwrap();
    write_png_zip(&mut a, 15, "sample", &mut f, None).unwrap();
}

/// Encode-speed experiments: `cargo test --release -p desigma-export bench_webm -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_webm() {
    let (mut d, _page, frame, sq) = slide_doc(1000, 30, true);
    d.get_mut(sq).unwrap().size = Vec2::new(120.0, 120.0);
    for (w, h) in [(640.0, 360.0), (1280.0, 720.0), (1920.0, 1080.0)] {
        d.get_mut(frame).unwrap().size = Vec2::new(w, h);
        for speed in [10, 9, 8] {
            let mut a = Animation::new(&d, frame, 1.0).unwrap();
            let t = std::time::Instant::now();
            let bytes = encode_webm(&mut a, &WebmOptions { fps: 30, speed, ..Default::default() }, None).unwrap();
            println!("{w}x{h} speed {speed}: {} bytes, {:?} for 30 frames", bytes.len(), t.elapsed());
        }
    }
}
