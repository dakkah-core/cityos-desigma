//! Animation export: renders a frame (or a whole page) of a Motion timeline frame by frame and
//! encodes it as
//!
//! * a **GIF** ([`encode_gif`]): global NeuQuant palette, optional Floyd-Steinberg dithering,
//!   changed-rectangle frames, loop flag from the timeline;
//! * a **WebM** ([`encode_webm`]): AV1 through `rav1e` (pure Rust, no assembly) in a Matroska
//!   container written by this crate;
//! * a **PNG sequence** ([`write_png_zip`] / [`write_png_dir`]).
//!
//! Everything goes through [`Animation`], which evaluates the timeline with a
//! [`MotionPlayer`](desigma_doc::motion::MotionPlayer) and draws with the same CPU renderer as the
//! static exports, so an exported frame is pixel-identical to a PNG export of that moment.
//! Encoders report [`Progress`] after each frame and stop when the callback returns `false`.

mod gif_out;
mod webm;
mod zip;

pub use gif_out::{encode_gif, GifOptions, GifStats, GIF_MAX_FPS};
pub use webm::{encode_webm, WebmOptions};
pub use zip::{write_png_dir, write_png_zip};

use crate::bounds::render_bounds;
use crate::raster::{render_export, render_size};
use desigma_doc::motion::{animated_nodes, MotionPlayer, TimelineSettings};
use desigma_doc::{Document, NodeId, NodeKind, Rect, Vec2};
use desigma_render::{render_page, Viewport};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

/// Largest side of an exported animation in pixels.
pub const MAX_SIDE: u32 = 8192;
/// Most frames one export may contain (10 minutes at 60 fps is 36000).
pub const MAX_FRAMES: u32 = 40_000;

#[derive(Debug)]
pub enum AnimError {
    /// The node does not exist or is not on a page.
    NoNode,
    /// The output would be larger than the encoders support.
    TooLarge(String),
    /// The progress callback asked to stop.
    Cancelled,
    Encode(String),
    Io(std::io::Error),
}

impl std::fmt::Display for AnimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoNode => write!(f, "nothing to export"),
            Self::TooLarge(s) => write!(f, "{s}"),
            Self::Cancelled => write!(f, "export cancelled"),
            Self::Encode(s) => write!(f, "{s}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AnimError {}

impl From<std::io::Error> for AnimError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Export progress: `done` of `total` frames are finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
}

/// Progress callback: return `false` to cancel the export.
pub type ProgressFn<'a> = &'a mut dyn FnMut(Progress) -> bool;

/// A node (frame, or a whole page) and its page's timeline, ready to be drawn at any time.
pub struct Animation {
    base: Document,
    player: MotionPlayer,
    node: NodeId,
    page: NodeId,
    timeline: TimelineSettings,
    scale: f32,
    size: (u32, u32),
    /// Top-left of the exported area for a page target.
    origin: Vec2,
    is_page: bool,
}

impl Animation {
    /// Prepares the export of `node` (a frame, any layer, or a page) at `scale` (1.0 = 100%).
    pub fn new(doc: &Document, node: NodeId, scale: f32) -> Result<Self, AnimError> {
        let n = doc.get(node).ok_or(AnimError::NoNode)?;
        let is_page = matches!(n.kind, NodeKind::Page { .. });
        let page = if is_page { node } else { doc.page_of(node).ok_or(AnimError::NoNode)? };
        let scale = if scale.is_finite() { scale.clamp(0.05, 16.0) } else { 1.0 };
        let timeline = doc.timeline(page);
        let mut player = MotionPlayer::new(doc, page);
        // measure on the evaluated document: sizes may be animated
        let (size, origin) = if is_page {
            let mut bounds: Option<Rect> = None;
            let samples = 24u32;
            for i in 0..=samples {
                let t = timeline.duration_ms as f32 * i as f32 / samples as f32;
                let r = render_bounds(player.seek(doc, t), node);
                bounds = Some(match bounds {
                    None => r,
                    Some(b) => union(b, r),
                });
            }
            let b = bounds.unwrap_or_default();
            let w = (b.w * scale).ceil().max(1.0);
            let h = (b.h * scale).ceil().max(1.0);
            ((w as u32, h as u32), Vec2::new(b.x, b.y))
        } else {
            let evaluated = player.seek(doc, 0.0);
            (render_size(evaluated, node, scale).ok_or(AnimError::NoNode)?, Vec2::ZERO)
        };
        if size.0 > MAX_SIDE || size.1 > MAX_SIDE {
            return Err(AnimError::TooLarge(format!("{}x{} px is too large to export as an animation (limit {MAX_SIDE})", size.0, size.1)));
        }
        Ok(Self { base: doc.clone(), player, node, page, timeline, scale, size, origin, is_page })
    }

    pub fn timeline(&self) -> TimelineSettings {
        self.timeline
    }

    pub fn page(&self) -> NodeId {
        self.page
    }

    /// Output size in pixels (constant over the animation).
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Whether anything on the page is animated.
    pub fn is_animated(&self) -> bool {
        !animated_nodes(&self.base, self.page).is_empty()
    }

    /// Timeline times of the frames of an export at `fps`. A looping timeline gets the frames
    /// `0 .. duration` (the last one leads back into the first); otherwise a final frame at the
    /// end is added so the animation ends on its last state.
    pub fn frame_times(&self, fps: u32) -> Vec<f32> {
        frame_times(&self.timeline, fps)
    }

    /// Draws the animation at `t_ms` on a transparent background (premultiplied RGBA).
    pub fn render(&mut self, t_ms: f32) -> Pixmap {
        let (w, h) = self.size;
        let doc = self.player.seek(&self.base, t_ms);
        if self.is_page {
            let mut pm = Pixmap::new(w, h).expect("size was validated");
            let vp = Viewport { offset: self.origin, zoom: self.scale, dpr: 1.0 };
            render_page(doc, self.node, &vp, &mut pm);
            return pm;
        }
        match render_export(doc, self.node, self.scale) {
            Some(pm) if pm.width() == w && pm.height() == h => pm,
            Some(pm) => {
                // the layer was resized by the animation: keep the output size constant
                let mut out = Pixmap::new(w, h).expect("size was validated");
                out.draw_pixmap(0, 0, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
                out
            }
            None => Pixmap::new(w, h).expect("size was validated"),
        }
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// See [`Animation::frame_times`].
pub fn frame_times(timeline: &TimelineSettings, fps: u32) -> Vec<f32> {
    let fps = fps.clamp(1, TimelineSettings::MAX_FPS);
    let d = timeline.duration_ms.max(1) as f64;
    let step = 1000.0 / fps as f64;
    let n = ((d / step).round() as u32).clamp(1, MAX_FRAMES);
    let mut out: Vec<f32> = (0..n).map(|i| ((i as f64 * step).min(d)) as f32).collect();
    if !timeline.looping && out.last().is_none_or(|l| (*l as f64) < d - 0.5) {
        out.push(d as f32);
    }
    out
}

/// Output formats of an animation export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnimFormat {
    Gif,
    Webm,
    /// A zip of numbered PNG files.
    PngZip,
}

impl AnimFormat {
    pub const ALL: [AnimFormat; 3] = [AnimFormat::Gif, AnimFormat::Webm, AnimFormat::PngZip];

    pub fn label(&self) -> &'static str {
        match self {
            Self::Gif => "GIF",
            Self::Webm => "WebM",
            Self::PngZip => "PNG frames",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::Webm => "webm",
            Self::PngZip => "zip",
        }
    }

    /// `gif`, `webm`, `png` / `zip` / `png-zip` (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gif" => Some(Self::Gif),
            "webm" => Some(Self::Webm),
            "png" | "zip" | "png-zip" | "png-frames" => Some(Self::PngZip),
            _ => None,
        }
    }

    /// Highest frame rate the format supports.
    pub fn max_fps(&self) -> u32 {
        match self {
            Self::Gif => GIF_MAX_FPS,
            Self::Webm | Self::PngZip => 120,
        }
    }
}

/// Everything an animation export needs besides the document.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimExport {
    pub format: AnimFormat,
    /// 0 uses the timeline's fps.
    pub fps: u32,
    /// Pixel scale (1.0 = 100%).
    pub scale: f32,
    /// Flatten onto this colour; `None` keeps transparency where the format can (GIF, PNG).
    pub background: Option<[u8; 3]>,
    pub gif_dither: bool,
    /// WebM quality 1..=100.
    pub webm_quality: u8,
    /// WebM encoder speed 0..=10.
    pub webm_speed: u8,
}

impl Default for AnimExport {
    fn default() -> Self {
        Self { format: AnimFormat::Gif, fps: 0, scale: 1.0, background: Some([255, 255, 255]), gif_dither: false, webm_quality: 85, webm_speed: 9 }
    }
}

impl AnimExport {
    /// The frame rate that will be used: the setting, else the page's timeline, within what the
    /// format supports.
    pub fn effective_fps(&self, timeline: &TimelineSettings) -> u32 {
        let f = if self.fps == 0 { timeline.fps } else { self.fps };
        f.clamp(1, self.format.max_fps())
    }
}

/// Whether the page of `node` (or `node` itself, if it is a page) has any animated layer.
pub fn has_motion(doc: &Document, node: NodeId) -> bool {
    let Some(n) = doc.get(node) else { return false };
    let page = if matches!(n.kind, NodeKind::Page { .. }) { Some(node) } else { doc.page_of(node) };
    page.is_some_and(|p| !animated_nodes(doc, p).is_empty())
}

/// Renders and encodes `node`'s animation. `name` names the files inside a PNG zip.
pub fn export_animation(doc: &Document, node: NodeId, o: &AnimExport, name: &str, progress: Option<ProgressFn<'_>>) -> Result<Vec<u8>, AnimError> {
    let mut anim = Animation::new(doc, node, o.scale)?;
    let fps = o.effective_fps(&anim.timeline());
    match o.format {
        AnimFormat::Gif => {
            let g = GifOptions { fps, background: o.background, dither: o.gif_dither, ..Default::default() };
            encode_gif(&mut anim, &g, progress).map(|(b, _)| b)
        }
        AnimFormat::Webm => {
            let w =
                WebmOptions { fps, quality: o.webm_quality, speed: o.webm_speed, background: o.background.unwrap_or([255, 255, 255]), ..Default::default() };
            encode_webm(&mut anim, &w, progress)
        }
        AnimFormat::PngZip => {
            let mut out = Vec::new();
            write_png_zip(&mut anim, fps, name, &mut out, progress)?;
            Ok(out)
        }
    }
}

/// Premultiplied RGBA over a solid background: opaque RGB bytes.
pub(crate) fn flatten_rgb(pm: &Pixmap, bg: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity((pm.width() * pm.height() * 3) as usize);
    for px in pm.data().chunks_exact(4) {
        let inv = 255 - px[3] as u32;
        for c in 0..3 {
            out.push((px[c] as u32 + (bg[c] as u32 * inv + 127) / 255).min(255) as u8);
        }
    }
    out
}

pub(crate) fn report(progress: &mut Option<ProgressFn<'_>>, done: u32, total: u32) -> Result<(), AnimError> {
    if let Some(p) = progress.as_mut() {
        if !p(Progress { done, total }) {
            return Err(AnimError::Cancelled);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
