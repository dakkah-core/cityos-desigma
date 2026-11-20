//! Animatable properties and their typed values.

use crate::{Color, Vec2};
use serde::{Deserialize, Serialize};

/// A node property that keyframes can animate.
///
/// Units are model units, not display units: [`AnimProperty::display_factor`] converts opacity
/// and scale to the percentages the inspector shows. Rotation is in degrees with Figma's
/// convention (positive = counter-clockwise), the same number the Rotation field shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AnimProperty {
    /// Layout position (x, y) of the node's unrotated, unscaled box in its parent, in px. This is
    /// what the X/Y fields show for an unrotated layer.
    Position,
    /// Scale factor (x, y) relative to the node's static transform, 1.0 = 100%. Applied about the
    /// node's centre; negative values mirror.
    Scale,
    /// Rotation in degrees, counter-clockwise positive, about the node's centre.
    Rotation,
    Width,
    Height,
    /// 0.0 ..= 1.0.
    Opacity,
    /// Uniform corner radius in px; sets all four corners.
    CornerRadius,
    /// Colour of the first solid fill (a node without one cannot animate this).
    FillColor,
    /// Colour of the first solid stroke paint.
    StrokeColor,
    StrokeWeight,
    /// Horizontal offset of the first drop shadow (a node without one cannot animate this).
    ShadowX,
    /// Vertical offset of the first drop shadow.
    ShadowY,
    /// Blur radius of the first drop shadow.
    ShadowBlur,
    /// Colour (with alpha) of the first drop shadow.
    ShadowColor,
    /// Radius of the first layer blur.
    BlurRadius,
}

/// The shape of an [`AnimValue`] a property takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Scalar,
    Vec2,
    Color,
}

impl AnimProperty {
    /// Every property in inspector order (Transform, Layout, Appearance, Fill, Stroke, Effects).
    pub const ALL: [AnimProperty; 15] = [
        AnimProperty::Position,
        AnimProperty::Scale,
        AnimProperty::Rotation,
        AnimProperty::Width,
        AnimProperty::Height,
        AnimProperty::Opacity,
        AnimProperty::CornerRadius,
        AnimProperty::FillColor,
        AnimProperty::StrokeColor,
        AnimProperty::StrokeWeight,
        AnimProperty::ShadowX,
        AnimProperty::ShadowY,
        AnimProperty::ShadowBlur,
        AnimProperty::ShadowColor,
        AnimProperty::BlurRadius,
    ];

    /// Label of the timeline's property row.
    pub fn name(&self) -> &'static str {
        match self {
            AnimProperty::Position => "Position",
            AnimProperty::Scale => "Scale",
            AnimProperty::Rotation => "Rotation",
            AnimProperty::Width => "Width",
            AnimProperty::Height => "Height",
            AnimProperty::Opacity => "Opacity",
            AnimProperty::CornerRadius => "Corner radius",
            AnimProperty::FillColor => "Fill",
            AnimProperty::StrokeColor => "Stroke",
            AnimProperty::StrokeWeight => "Stroke weight",
            AnimProperty::ShadowX => "Shadow X",
            AnimProperty::ShadowY => "Shadow Y",
            AnimProperty::ShadowBlur => "Shadow blur",
            AnimProperty::ShadowColor => "Shadow color",
            AnimProperty::BlurRadius => "Layer blur",
        }
    }

    pub fn kind(&self) -> ValueKind {
        match self {
            AnimProperty::Position | AnimProperty::Scale => ValueKind::Vec2,
            AnimProperty::FillColor | AnimProperty::StrokeColor | AnimProperty::ShadowColor => ValueKind::Color,
            _ => ValueKind::Scalar,
        }
    }

    /// Multiply the model value by this to get the number shown in the UI (100 for opacity and
    /// scale which display as percentages, else 1).
    pub fn display_factor(&self) -> f32 {
        match self {
            AnimProperty::Opacity | AnimProperty::Scale => 100.0,
            _ => 1.0,
        }
    }

    /// Unit suffix of the displayed value ("px", "%", "°"; empty for colours).
    pub fn unit(&self) -> &'static str {
        match self {
            AnimProperty::Opacity | AnimProperty::Scale => "%",
            AnimProperty::Rotation => "°",
            AnimProperty::FillColor | AnimProperty::StrokeColor | AnimProperty::ShadowColor => "",
            _ => "px",
        }
    }

    /// Whether `v` has the right shape for this property.
    pub fn accepts(&self, v: &AnimValue) -> bool {
        v.kind() == self.kind()
    }

    /// Neutral value used when a property has no base (e.g. before a first keyframe is read).
    pub fn neutral(&self) -> AnimValue {
        match self.kind() {
            ValueKind::Scalar => AnimValue::Scalar(if matches!(self, AnimProperty::Opacity) { 1.0 } else { 0.0 }),
            ValueKind::Vec2 => AnimValue::Vec2(if matches!(self, AnimProperty::Scale) { Vec2::new(1.0, 1.0) } else { Vec2::ZERO }),
            ValueKind::Color => AnimValue::Color(Color::BLACK),
        }
    }
}

/// A typed animated value.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AnimValue {
    Scalar(f32),
    Vec2(Vec2),
    Color(Color),
}

impl AnimValue {
    pub fn kind(&self) -> ValueKind {
        match self {
            AnimValue::Scalar(_) => ValueKind::Scalar,
            AnimValue::Vec2(_) => ValueKind::Vec2,
            AnimValue::Color(_) => ValueKind::Color,
        }
    }

    pub fn scalar(&self) -> Option<f32> {
        match self {
            AnimValue::Scalar(v) => Some(*v),
            _ => None,
        }
    }

    pub fn vec2(&self) -> Option<Vec2> {
        match self {
            AnimValue::Vec2(v) => Some(*v),
            _ => None,
        }
    }

    pub fn color(&self) -> Option<Color> {
        match self {
            AnimValue::Color(v) => Some(*v),
            _ => None,
        }
    }

    pub fn is_finite(&self) -> bool {
        match self {
            AnimValue::Scalar(v) => v.is_finite(),
            AnimValue::Vec2(v) => v.x.is_finite() && v.y.is_finite(),
            AnimValue::Color(c) => c.r.is_finite() && c.g.is_finite() && c.b.is_finite() && c.a.is_finite(),
        }
    }

    /// Interpolates `a` to `b` at progress `t`. `t` is not clamped, so eased overshoot works; colour
    /// channels are clamped to 0..=1 afterwards. Values of different kinds do not mix: `a` is
    /// returned.
    pub fn lerp(a: &AnimValue, b: &AnimValue, t: f32) -> AnimValue {
        match (a, b) {
            (AnimValue::Scalar(x), AnimValue::Scalar(y)) => AnimValue::Scalar(lerp_f32(*x, *y, t)),
            (AnimValue::Vec2(x), AnimValue::Vec2(y)) => AnimValue::Vec2(lerp_vec2(*x, *y, t)),
            (AnimValue::Color(x), AnimValue::Color(y)) => AnimValue::Color(lerp_color(*x, *y, t)),
            _ => *a,
        }
    }
}

pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn lerp_vec2(a: Vec2, b: Vec2, t: f32) -> Vec2 {
    Vec2::new(lerp_f32(a.x, b.x, t), lerp_f32(a.y, b.y, t))
}

/// Colour interpolation: **premultiplied alpha in sRGB-encoded space** (what CSS transitions
/// and Figma's own animations do).
///
/// Premultiplying keeps fades to/from transparent from dragging the colour towards the
/// (arbitrary) RGB of the transparent end, and doing it on the encoded values means the
/// midpoint of two colours is the average of the hex values designers see (black to white
/// passes through #808080). [`lerp_color_linear`] blends in linear light instead.
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    blend_premultiplied(a, b, t, |c| c, |c| c)
}

/// Like [`lerp_color`] but blends in linear light (physically correct light mixing; brighter
/// midpoints). Available for callers that want it, not used by default.
pub fn lerp_color_linear(a: Color, b: Color, t: f32) -> Color {
    blend_premultiplied(a, b, t, srgb_to_linear, linear_to_srgb)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn blend_premultiplied(a: Color, b: Color, t: f32, enc: impl Fn(f32) -> f32, dec: impl Fn(f32) -> f32) -> Color {
    let alpha = lerp_f32(a.a, b.a, t).clamp(0.0, 1.0);
    let ch = |x: f32, y: f32| {
        let (x, y) = (enc(x), enc(y));
        let p = lerp_f32(x * a.a, y * b.a, t);
        if alpha > 1e-6 {
            dec(p / alpha)
        } else {
            dec(lerp_f32(x, y, t))
        }
    };
    Color { r: ch(a.r, b.r).clamp(0.0, 1.0), g: ch(a.g, b.g).clamp(0.0, 1.0), b: ch(a.b, b.b).clamp(0.0, 1.0), a: alpha }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_and_vec_lerp() {
        let m = AnimValue::lerp(&AnimValue::Scalar(10.0), &AnimValue::Scalar(20.0), 0.25);
        assert_eq!(m, AnimValue::Scalar(12.5));
        let v = AnimValue::lerp(&AnimValue::Vec2(Vec2::new(0.0, 10.0)), &AnimValue::Vec2(Vec2::new(10.0, 0.0)), 0.5);
        assert_eq!(v, AnimValue::Vec2(Vec2::new(5.0, 5.0)));
        // Overshoot is allowed for scalars.
        assert_eq!(AnimValue::lerp(&AnimValue::Scalar(0.0), &AnimValue::Scalar(1.0), 1.2), AnimValue::Scalar(1.2));
        // Mismatched kinds keep the start value.
        assert_eq!(AnimValue::lerp(&AnimValue::Scalar(1.0), &AnimValue::Vec2(Vec2::ZERO), 0.5), AnimValue::Scalar(1.0));
    }

    #[test]
    fn rotation_is_numeric_not_shortest_path() {
        let v = AnimValue::lerp(&AnimValue::Scalar(0.0), &AnimValue::Scalar(720.0), 0.5);
        assert_eq!(v, AnimValue::Scalar(360.0));
    }

    #[test]
    fn color_midpoint_is_hex_average() {
        let m = lerp_color(Color::BLACK, Color::WHITE, 0.5);
        assert!((m.r - 0.5).abs() < 1e-5 && (m.a - 1.0).abs() < 1e-6);
        let lin = lerp_color_linear(Color::BLACK, Color::WHITE, 0.5);
        assert!(lin.r > 0.7, "linear midpoint is brighter: {}", lin.r);
        assert_eq!(lerp_color(Color::BLACK, Color::WHITE, 0.0), Color::BLACK);
        assert_eq!(lerp_color(Color::BLACK, Color::WHITE, 1.0), Color::WHITE);
    }

    #[test]
    fn color_fade_to_transparent_keeps_hue() {
        let red = Color::rgb(255, 0, 0);
        let m = lerp_color(red, Color::TRANSPARENT, 0.5);
        assert!((m.r - 1.0).abs() < 1e-5 && m.g.abs() < 1e-5, "{m:?}");
        assert!((m.a - 0.5).abs() < 1e-5);
        // Fully transparent result does not produce NaN.
        let z = lerp_color(Color::TRANSPARENT, Color::TRANSPARENT, 0.5);
        assert!(z.r.is_finite() && z.a == 0.0);
    }

    #[test]
    fn color_overshoot_is_clamped() {
        let c = lerp_color(Color::BLACK, Color::WHITE, 1.4);
        assert!(c.r <= 1.0 && c.g <= 1.0 && c.b <= 1.0);
        let c = lerp_color(Color::BLACK, Color::WHITE, -0.4);
        assert!(c.r >= 0.0 && c.a <= 1.0 && c.a >= 0.0);
    }

    #[test]
    fn property_metadata() {
        for p in AnimProperty::ALL {
            assert!(p.accepts(&p.neutral()), "{p:?}");
            assert!(!p.name().is_empty());
        }
        assert_eq!(AnimProperty::Opacity.display_factor(), 100.0);
        assert_eq!(AnimProperty::Rotation.unit(), "°");
        assert_eq!(AnimProperty::Scale.neutral(), AnimValue::Vec2(Vec2::new(1.0, 1.0)));
    }
}
