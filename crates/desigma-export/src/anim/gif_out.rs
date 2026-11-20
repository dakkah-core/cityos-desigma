//! GIF encoder.
//!
//! One **global palette** is built for the whole animation (so colours do not shift between
//! frames): when a sample of frames holds at most 255 distinct colours the palette is exact,
//! otherwise it is trained with NeuQuant. Index 255 is the transparent index. Opaque animations
//! store each frame as the rectangle that changed since the previous one (unchanged pixels
//! transparent, frames that did not change at all merge into the previous delay), which makes
//! typical UI animations several times smaller. Optional Floyd-Steinberg dithering smooths
//! gradients in the NeuQuant case.
//!
//! GIF delays are whole centiseconds and browsers treat anything under 2 cs as 10 cs, so the
//! frame rate is capped at [`GIF_MAX_FPS`] (50) and the delays of non-divisor rates (30 fps is
//! 3.33 cs) alternate between neighbouring values so the total duration stays exact.

use super::{flatten_rgb, report, AnimError, Animation, ProgressFn};
use color_quant::NeuQuant;
use gif::{DisposalMethod, Encoder, Frame, Repeat};
use std::borrow::Cow;
use std::collections::HashSet;

/// Highest frame rate of a GIF (2 centiseconds per frame).
pub const GIF_MAX_FPS: u32 = 50;

const TRANSPARENT: u8 = 255;

#[derive(Clone, Debug, PartialEq)]
pub struct GifOptions {
    /// Frames per second, 1..=50 (higher values are capped).
    pub fps: u32,
    /// Colour the transparent parts are flattened onto. `None` keeps 1-bit transparency
    /// (pixels under 50% alpha become transparent) and stores full frames.
    pub background: Option<[u8; 3]>,
    /// Floyd-Steinberg dithering (only matters when the animation has more than 255 colours).
    pub dither: bool,
    /// NeuQuant sampling factor, 1 (best, slowest) ..= 30.
    pub sample_factor: i32,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self { fps: 30, background: Some([255, 255, 255]), dither: false, sample_factor: 4 }
    }
}

/// What an encode produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GifStats {
    /// Rendered frames.
    pub frames: u32,
    /// Frames stored in the file (identical neighbours are merged into one).
    pub frames_written: u32,
    pub palette_colors: u32,
    pub exact_palette: bool,
}

enum Palette {
    Exact(Vec<[u8; 3]>),
    Quant(Box<NeuQuant>),
}

struct Mapper {
    palette: Palette,
    /// Direct-mapped cache: (rgb key + 1, index).
    cache: Vec<(u32, u8)>,
    /// The 256-entry RGB table of the palette (entry 255 is the transparent index).
    table: Vec<u8>,
}

impl Mapper {
    fn new(palette: Palette) -> Self {
        let mut table: Vec<u8> = match &palette {
            Palette::Exact(cols) => cols.iter().flatten().copied().collect(),
            Palette::Quant(q) => q.color_map_rgb(),
        };
        table.resize(256 * 3, 0);
        Self { palette, cache: vec![(0, 0); 1 << 16], table }
    }

    fn lookup(&self, r: u8, g: u8, b: u8) -> u8 {
        match &self.palette {
            Palette::Exact(cols) => {
                let mut best = (u32::MAX, 0u8);
                for (i, c) in cols.iter().enumerate() {
                    let (dr, dg, db) = (c[0] as i32 - r as i32, c[1] as i32 - g as i32, c[2] as i32 - b as i32);
                    let d = (dr * dr * 3 + dg * dg * 4 + db * db * 2) as u32;
                    if d < best.0 {
                        best = (d, i as u8);
                        if d == 0 {
                            break;
                        }
                    }
                }
                best.1
            }
            Palette::Quant(q) => q.index_of(&[r, g, b, 255]) as u8,
        }
    }

    fn index(&mut self, r: u8, g: u8, b: u8) -> u8 {
        let key = ((r as u32) << 16 | (g as u32) << 8 | b as u32) + 1;
        let slot = (key.wrapping_mul(0x9E37_79B1) >> 16) as usize & 0xFFFF;
        let e = self.cache[slot];
        if e.0 == key {
            return e.1;
        }
        let i = self.lookup(r, g, b);
        self.cache[slot] = (key, i);
        i
    }

    fn color(&self, i: u8) -> [u8; 3] {
        let o = i as usize * 3;
        [self.table[o], self.table[o + 1], self.table[o + 2]]
    }
}

/// Un-premultiplied RGB of a pixel (None below half alpha when `cut`).
fn straight(px: &[u8]) -> Option<[u8; 3]> {
    let a = px[3] as u32;
    if a < 128 {
        return None;
    }
    let un = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
    Some([un(px[0]), un(px[1]), un(px[2])])
}

fn build_palette(anim: &mut Animation, times: &[f32], o: &GifOptions, progress: &mut Option<ProgressFn<'_>>) -> Result<Mapper, AnimError> {
    let want = times.len().min(12);
    let mut unique: HashSet<u32> = HashSet::new();
    let mut overflow = false;
    let mut rgba: Vec<u8> = Vec::new();
    let (w, h) = anim.size();
    let budget = 300_000usize / want.max(1);
    let stride = ((w as usize * h as usize / budget.max(1)).max(1)) | 1;
    for k in 0..want {
        let i = if want == 1 { 0 } else { k * (times.len() - 1) / (want - 1) };
        let pm = anim.render(times[i]);
        let flat = o.background.map(|bg| flatten_rgb(&pm, bg));
        for (p, px) in pm.data().chunks_exact(4).enumerate() {
            let c = match &flat {
                Some(f) => Some([f[p * 3], f[p * 3 + 1], f[p * 3 + 2]]),
                None => straight(px),
            };
            let Some(c) = c else { continue };
            if !overflow {
                unique.insert((c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32);
                overflow = unique.len() > 255;
            }
            if p % stride == 0 {
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        report(progress, 0, times.len() as u32)?;
    }
    if !overflow {
        let mut cols: Vec<u32> = unique.into_iter().collect();
        cols.sort_unstable();
        let mut cols: Vec<[u8; 3]> = cols.into_iter().map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).collect();
        if cols.is_empty() {
            cols.push([0, 0, 0]);
        }
        return Ok(Mapper::new(Palette::Exact(cols)));
    }
    let q = NeuQuant::new(o.sample_factor.clamp(1, 30), 255, &rgba);
    Ok(Mapper::new(Palette::Quant(Box::new(q))))
}

/// Floyd-Steinberg dithering of opaque `rgb` against the NeuQuant palette.
fn dither(rgb: &[u8], w: usize, h: usize, m: &mut Mapper) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    // error rows (current, next) with one pixel of padding on each side, 16.16-free: plain f32
    let mut cur = vec![[0f32; 3]; w + 2];
    let mut next = vec![[0f32; 3]; w + 2];
    for y in 0..h {
        for x in 0..w {
            let p = (y * w + x) * 3;
            let e = cur[x + 1];
            let adj = [0, 1, 2].map(|c| (rgb[p + c] as f32 + e[c]).clamp(0.0, 255.0));
            let idx = m.lookup(adj[0].round() as u8, adj[1].round() as u8, adj[2].round() as u8);
            out[y * w + x] = idx;
            let pc = m.color(idx);
            let err = [0, 1, 2].map(|c| adj[c] - pc[c] as f32);
            for c in 0..3 {
                cur[x + 2][c] += err[c] * 7.0 / 16.0;
                next[x][c] += err[c] * 3.0 / 16.0;
                next[x + 1][c] += err[c] * 5.0 / 16.0;
                next[x + 2][c] += err[c] / 16.0;
            }
        }
        std::mem::swap(&mut cur, &mut next);
        next.iter_mut().for_each(|e| *e = [0.0; 3]);
    }
    out
}

fn quantize(pm: &tiny_skia::Pixmap, m: &mut Mapper, o: &GifOptions) -> Vec<u8> {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    match o.background {
        Some(bg) => {
            let rgb = flatten_rgb(pm, bg);
            if o.dither && matches!(m.palette, Palette::Quant(_)) {
                dither(&rgb, w, h, m)
            } else {
                rgb.chunks_exact(3).map(|c| m.index(c[0], c[1], c[2])).collect()
            }
        }
        None => pm
            .data()
            .chunks_exact(4)
            .map(|px| match straight(px) {
                Some(c) => m.index(c[0], c[1], c[2]).min(TRANSPARENT - 1),
                None => TRANSPARENT,
            })
            .collect(),
    }
}

/// A frame waiting for the next one to decide whether it can absorb its delay.
struct Pending {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    data: Vec<u8>,
    delay_cs: u32,
    transparent: bool,
    dispose: DisposalMethod,
}

fn write_pending<W: std::io::Write>(enc: &mut Encoder<W>, p: Pending) -> Result<(), AnimError> {
    let frame = Frame {
        left: p.left,
        top: p.top,
        width: p.width,
        height: p.height,
        delay: p.delay_cs.min(u16::MAX as u32) as u16,
        dispose: p.dispose,
        transparent: p.transparent.then_some(TRANSPARENT),
        buffer: Cow::Owned(p.data),
        ..Frame::default()
    };
    enc.write_frame(&frame).map_err(|e| AnimError::Encode(e.to_string()))
}

/// Encodes the animation as a GIF. Returns the file bytes.
pub fn encode_gif(anim: &mut Animation, o: &GifOptions, mut progress: Option<ProgressFn<'_>>) -> Result<(Vec<u8>, GifStats), AnimError> {
    let fps = o.fps.clamp(1, GIF_MAX_FPS);
    let times = anim.frame_times(fps);
    let (w, h) = anim.size();
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return Err(AnimError::TooLarge("GIF frames are limited to 65535 px".into()));
    }
    let total = times.len() as u32;
    let mut mapper = build_palette(anim, &times, o, &mut progress)?;
    let exact = matches!(mapper.palette, Palette::Exact(_));
    let colors = match &mapper.palette {
        Palette::Exact(c) => c.len() as u32,
        Palette::Quant(_) => 255,
    };
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, w as u16, h as u16, &mapper.table).map_err(|e| AnimError::Encode(e.to_string()))?;
    if anim.timeline().looping {
        enc.set_repeat(Repeat::Infinite).map_err(|e| AnimError::Encode(e.to_string()))?;
    }
    let (wu, hu) = (w as usize, h as usize);
    let mut prev: Option<Vec<u8>> = None;
    let mut pending: Option<Pending> = None;
    let mut last_end_cs = 0u32;
    let mut written = 0u32;
    for (i, &t) in times.iter().enumerate() {
        let pm = anim.render(t);
        let idx = quantize(&pm, &mut mapper, o);
        let next_prev = o.background.is_some().then(|| idx.clone());
        let end_cs = ((i as u64 + 1) * 100 + fps as u64 / 2) as u32 / fps;
        let delay = end_cs.saturating_sub(last_end_cs).max(2);
        last_end_cs = end_cs.max(last_end_cs + 2);
        if o.background.is_none() {
            // full frames, each replacing the last
            if let Some(p) = pending.take() {
                write_pending(&mut enc, p)?;
                written += 1;
            }
            pending = Some(Pending {
                left: 0,
                top: 0,
                width: w as u16,
                height: h as u16,
                data: idx,
                delay_cs: delay,
                transparent: true,
                dispose: DisposalMethod::Background,
            });
        } else if let Some(pv) = &prev {
            let (mut x0, mut y0, mut x1, mut y1) = (wu, hu, 0usize, 0usize);
            for y in 0..hu {
                let (a, b) = (&idx[y * wu..(y + 1) * wu], &pv[y * wu..(y + 1) * wu]);
                if a != b {
                    let first = a.iter().zip(b).position(|(p, q)| p != q).unwrap_or(0);
                    let last = a.iter().zip(b).rposition(|(p, q)| p != q).unwrap_or(0);
                    x0 = x0.min(first);
                    x1 = x1.max(last + 1);
                    y0 = y0.min(y);
                    y1 = y1.max(y + 1);
                }
            }
            if x1 == 0 {
                // nothing changed: the previous frame stays up longer
                if let Some(p) = pending.as_mut() {
                    p.delay_cs += delay;
                }
            } else {
                if let Some(p) = pending.take() {
                    write_pending(&mut enc, p)?;
                    written += 1;
                }
                let (fw, fh) = (x1 - x0, y1 - y0);
                let mut data = Vec::with_capacity(fw * fh);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = y * wu + x;
                        data.push(if idx[i] == pv[i] { TRANSPARENT } else { idx[i] });
                    }
                }
                pending = Some(Pending {
                    left: x0 as u16,
                    top: y0 as u16,
                    width: fw as u16,
                    height: fh as u16,
                    data,
                    delay_cs: delay,
                    transparent: true,
                    dispose: DisposalMethod::Keep,
                });
            }
        } else {
            pending = Some(Pending {
                left: 0,
                top: 0,
                width: w as u16,
                height: h as u16,
                data: idx,
                delay_cs: delay,
                transparent: false,
                dispose: DisposalMethod::Keep,
            });
        }
        prev = next_prev;
        report(&mut progress, i as u32 + 1, total)?;
    }
    if let Some(p) = pending.take() {
        write_pending(&mut enc, p)?;
        written += 1;
    }
    drop(enc);
    Ok((out, GifStats { frames: total, frames_written: written, palette_colors: colors, exact_palette: exact }))
}
