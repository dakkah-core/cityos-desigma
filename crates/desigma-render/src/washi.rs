//! Washi tape: a straight strip of the stroke's colour with torn ends and a pattern printed on it.
//! The strip runs from the first to the last point of the stroke path and is as wide as the
//! stroke weight.

use crate::{mask_mul, shape_mask};
use desigma_doc as d;
use tiny_skia as sk;

/// Frame of a strip: both ends and the unit vectors along and across it.
#[derive(Clone, Copy, Debug)]
pub struct Strip {
    pub a: (f32, f32),
    pub dir: (f32, f32),
    pub len: f32,
    pub half: f32,
}

impl Strip {
    /// A strip from `a` to `b` that is `width` wide. Degenerate strips point right.
    pub fn new(a: (f32, f32), b: (f32, f32), width: f32) -> Strip {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = (dx * dx + dy * dy).sqrt();
        let (dir, len) = if l < 1e-3 { ((1.0, 0.0), 1.0) } else { ((dx / l, dy / l), l) };
        Strip { a, dir, len, half: (width / 2.0).max(0.5) }
    }

    /// Point at `u` along the strip and `v` across it (0 is the centre line).
    pub fn at(&self, u: f32, v: f32) -> sk::Point {
        sk::Point::from_xy(self.a.0 + self.dir.0 * u - self.dir.1 * v, self.a.1 + self.dir.1 * u + self.dir.0 * v)
    }

    /// Depth of the torn teeth at both ends.
    fn tooth(&self) -> f32 {
        (self.half * 0.28).min(self.len / 4.0)
    }

    /// Number of teeth across one end.
    fn teeth(&self) -> usize {
        ((self.half * 2.0 / 3.5).round() as usize).clamp(3, 14)
    }

    /// The strip's silhouette: straight sides, zigzag torn ends.
    pub fn outline(&self) -> Option<sk::Path> {
        let k = self.teeth();
        let depth = self.tooth();
        let mut pb = sk::PathBuilder::new();
        for i in 0..=k {
            let v = -self.half + 2.0 * self.half * i as f32 / k as f32;
            let u = if i % 2 == 1 { depth * (0.55 + 0.15 * ((i * 5) % 4) as f32) } else { 0.0 };
            let p = self.at(u, v);
            if i == 0 {
                pb.move_to(p.x, p.y);
            } else {
                pb.line_to(p.x, p.y);
            }
        }
        for i in (0..=k).rev() {
            let v = -self.half + 2.0 * self.half * i as f32 / k as f32;
            let u = self.len - if i % 2 == 0 { depth * (0.55 + 0.15 * ((i * 3) % 4) as f32) } else { 0.0 };
            let p = self.at(u, v);
            pb.line_to(p.x, p.y);
        }
        pb.close();
        pb.finish()
    }

    /// The printed marks of `pattern` as one fillable path (clip it to the outline).
    pub fn pattern(&self, pattern: d::WashiPattern) -> Option<sk::Path> {
        let mut pb = sk::PathBuilder::new();
        let h = self.half;
        let quad = |pb: &mut sk::PathBuilder, pts: [(f32, f32); 4]| {
            for (i, (u, v)) in pts.iter().enumerate() {
                let p = self.at(*u, *v);
                if i == 0 {
                    pb.move_to(p.x, p.y);
                } else {
                    pb.line_to(p.x, p.y);
                }
            }
            pb.close();
        };
        match pattern {
            d::WashiPattern::Stripes => {
                let (period, w, slant) = (h * 0.9, h * 0.38, h * 1.4);
                let mut u = -slant;
                while u < self.len + slant {
                    quad(&mut pb, [(u, -h), (u + w, -h), (u + w + slant, h), (u + slant, h)]);
                    u += period;
                }
            }
            d::WashiPattern::Dots => {
                let step = h * 0.8;
                let r = h * 0.17;
                let mut row = 0;
                let mut v = -h + step / 2.0;
                while v < h {
                    let mut u = if row % 2 == 0 { step / 2.0 } else { step };
                    while u < self.len {
                        let c = self.at(u, v);
                        pb.push_circle(c.x, c.y, r);
                        u += step;
                    }
                    v += step * 0.8;
                    row += 1;
                }
            }
            d::WashiPattern::Grid => {
                let (step, t) = (h * 0.7, (h * 0.06).max(0.35));
                let mut u = step / 2.0;
                while u < self.len {
                    quad(&mut pb, [(u - t, -h), (u + t, -h), (u + t, h), (u - t, h)]);
                    u += step;
                }
                let mut v = -h + step / 2.0;
                while v < h {
                    quad(&mut pb, [(0.0, v - t), (self.len, v - t), (self.len, v + t), (0.0, v + t)]);
                    v += step;
                }
            }
            d::WashiPattern::Zigzag => {
                let (step, t, amp) = (h * 0.7, (h * 0.09).max(0.4), h * 0.45);
                for row in [-0.5f32, 0.5] {
                    let cy = row * h;
                    let mut u = 0.0;
                    let mut up = false;
                    while u < self.len + step {
                        let (y0, y1) = (cy + if up { amp } else { -amp } * 0.5, cy + if up { -amp } else { amp } * 0.5);
                        quad(&mut pb, [(u, y0 - t), (u + step, y1 - t), (u + step, y1 + t), (u, y0 + t)]);
                        u += step;
                        up = !up;
                    }
                }
            }
            d::WashiPattern::Checks => {
                let s = h * 0.5;
                let mut i = 0;
                let mut u = 0.0;
                while u < self.len {
                    let mut j = 0;
                    let mut v = -h;
                    while v < h {
                        if (i + j) % 2 == 0 {
                            quad(&mut pb, [(u, v), (u + s, v), (u + s, v + s), (u, v + s)]);
                        }
                        v += s;
                        j += 1;
                    }
                    u += s;
                    i += 1;
                }
            }
        }
        pb.finish()
    }
}

/// First and last point of the stroke's path.
fn endpoints(paths: &[(sk::Path, sk::FillRule)]) -> Option<((f32, f32), (f32, f32))> {
    let first = paths.first()?.0.points().first()?;
    let last = paths.last()?.0.points().last()?;
    Some(((first.x, first.y), (last.x, last.y)))
}

/// Colour of the tape: the stroke's first visible solid paint.
fn tape_color(n: &d::Node) -> (f32, f32, f32, f32) {
    for p in n.stroke.paints.iter().filter(|p| p.visible) {
        if let d::PaintKind::Solid(c) = &p.kind {
            return (c.r, c.g, c.b, c.a * p.opacity);
        }
    }
    (0.6, 0.6, 0.6, 1.0)
}

fn paint_of(c: (f32, f32, f32), a: f32) -> sk::Paint<'static> {
    let mut p = sk::Paint::default();
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    p.set_color_rgba8(b(c.0), b(c.1), b(c.2), b(a));
    p.anti_alias = true;
    p
}

/// Draws the washi tape stroke of `n` along its path `paths`.
pub fn draw(
    n: &d::Node,
    pattern: d::WashiPattern,
    paths: &[(sk::Path, sk::FillRule)],
    ts: sk::Transform,
    target: &mut sk::Pixmap,
    clip: Option<&sk::Mask>,
    alpha: f32,
) {
    let Some((a, b)) = endpoints(paths) else { return };
    let strip = Strip::new(a, b, n.stroke.weight);
    let Some(outline) = strip.outline() else { return };
    let (r, g, bl, ca) = tape_color(n);
    let base_alpha = (ca * alpha).clamp(0.0, 1.0);
    target.fill_path(&outline, &paint_of((r, g, bl), base_alpha), sk::FillRule::Winding, ts, clip);
    let Some(marks) = strip.pattern(pattern) else { return };
    // light tape gets dark marks, everything else gets white ones
    let luma = 0.299 * r + 0.587 * g + 0.114 * bl;
    let ink = if luma > 0.72 { ((0.0, 0.0, 0.0), 0.16) } else { ((1.0, 1.0, 1.0), 0.55) };
    let mut mask = shape_mask(target.width(), target.height(), &[(outline, sk::FillRule::Winding)], ts);
    if let (Some(m), Some(c)) = (mask.as_mut(), clip) {
        mask_mul(m, c);
    }
    target.fill_path(&marks, &paint_of(ink.0, ink.1 * base_alpha), sk::FillRule::Winding, ts, mask.as_ref().or(clip));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_is_as_wide_as_the_stroke_and_has_torn_ends() {
        let s = Strip::new((0.0, 0.0), (100.0, 0.0), 20.0);
        let o = s.outline().unwrap();
        let b = o.bounds();
        assert!((b.height() - 20.0).abs() < 0.01);
        assert!(b.width() <= 100.0 && b.width() > 95.0);
        // a torn end is not a straight edge: the left side reaches different depths
        let xs: Vec<f32> = o.points().iter().filter(|p| p.x < 10.0).map(|p| p.x).collect();
        assert!(xs.contains(&0.0) && xs.iter().any(|x| *x > 1.0));
    }

    #[test]
    fn every_pattern_has_marks_inside_the_strip() {
        let s = Strip::new((10.0, 10.0), (90.0, 50.0), 24.0);
        for p in d::WashiPattern::ALL {
            let m = s.pattern(p).unwrap_or_else(|| panic!("{p:?} has no marks"));
            assert!(m.bounds().width() > 10.0);
        }
    }

    #[test]
    fn degenerate_strip_points_right() {
        let s = Strip::new((5.0, 5.0), (5.0, 5.0), 10.0);
        assert_eq!(s.dir, (1.0, 0.0));
        assert!(s.outline().is_some());
    }
}
