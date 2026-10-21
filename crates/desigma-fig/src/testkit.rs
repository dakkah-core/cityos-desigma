//! Test support: a Figma-like kiwi schema and a synthetic message, so `.fig`
//! files can be synthesized without any real Figma file. Not part of the
//! stable API.

use crate::container::{self, Compression};
use crate::kiwi::{DefKind, FieldType, KiwiValue as V, Native, Schema, SchemaBuilder};
use std::collections::BTreeMap;

fn enum_def(b: &mut SchemaBuilder, name: &str, variants: &[&str]) {
    let d = b.define(name, DefKind::Enum);
    for (i, v) in variants.iter().enumerate() {
        b.variant(d, v, i as u32);
    }
}

/// A reduced version of Figma's schema (same names for the fields we read).
pub fn figma_like_schema() -> Schema {
    use Native::*;
    let mut b = SchemaBuilder::new();
    // Enum index 0 must exist: enum fields reference definition 0 in the binary form.
    enum_def(&mut b, "Phase", &["CREATED", "REMOVED"]);
    enum_def(
        &mut b,
        "NodeType",
        &[
            "NONE",
            "DOCUMENT",
            "CANVAS",
            "GROUP",
            "FRAME",
            "BOOLEAN_OPERATION",
            "VECTOR",
            "STAR",
            "LINE",
            "ELLIPSE",
            "RECTANGLE",
            "REGULAR_POLYGON",
            "ROUNDED_RECTANGLE",
            "TEXT",
            "SLICE",
            "SYMBOL",
            "INSTANCE",
            "SECTION",
            "STICKY",
        ],
    );
    enum_def(&mut b, "PaintType", &["SOLID", "GRADIENT_LINEAR", "GRADIENT_RADIAL", "GRADIENT_ANGULAR", "GRADIENT_DIAMOND", "IMAGE"]);
    enum_def(&mut b, "StrokeAlign", &["CENTER", "INSIDE", "OUTSIDE"]);
    enum_def(&mut b, "StrokeJoin", &["MITER", "BEVEL", "ROUND"]);
    enum_def(&mut b, "StrokeCap", &["NONE", "ROUND", "SQUARE", "ARROW_LINES", "ARROW_EQUILATERAL"]);
    enum_def(&mut b, "EffectType", &["INNER_SHADOW", "DROP_SHADOW", "FOREGROUND_BLUR", "BACKGROUND_BLUR"]);
    enum_def(&mut b, "StackMode", &["NONE", "HORIZONTAL", "VERTICAL"]);
    enum_def(&mut b, "StackAlign", &["MIN", "CENTER", "MAX", "SPACE_BETWEEN", "BASELINE"]);
    enum_def(&mut b, "StackSize", &["FIXED", "RESIZE_TO_FIT", "RESIZE_TO_FIT_WITH_IMPLICIT_SIZE"]);
    enum_def(&mut b, "StackCounterAlign", &["STRETCH", "AUTO"]);
    enum_def(&mut b, "StackPositioning", &["AUTO", "ABSOLUTE"]);
    enum_def(&mut b, "TextAlignHorizontal", &["LEFT", "CENTER", "RIGHT", "JUSTIFIED"]);
    enum_def(&mut b, "TextAlignVertical", &["TOP", "CENTER", "BOTTOM"]);
    enum_def(&mut b, "TextAutoResize", &["NONE", "WIDTH_AND_HEIGHT", "HEIGHT"]);
    enum_def(&mut b, "TextCase", &["ORIGINAL", "UPPER", "LOWER", "TITLE"]);
    enum_def(&mut b, "TextDecoration", &["NONE", "UNDERLINE", "STRIKETHROUGH"]);
    enum_def(&mut b, "NumberUnits", &["RAW", "PIXELS", "PERCENT"]);
    enum_def(
        &mut b,
        "BlendMode",
        &[
            "PASS_THROUGH",
            "NORMAL",
            "DARKEN",
            "MULTIPLY",
            "LINEAR_BURN",
            "COLOR_BURN",
            "LIGHTEN",
            "SCREEN",
            "LINEAR_DODGE",
            "COLOR_DODGE",
            "OVERLAY",
            "SOFT_LIGHT",
            "HARD_LIGHT",
            "DIFFERENCE",
            "EXCLUSION",
            "HUE",
            "SATURATION",
            "COLOR",
            "LUMINOSITY",
        ],
    );
    enum_def(&mut b, "TextTruncation", &["DISABLED", "ENDING"]);
    enum_def(&mut b, "StyleType", &["NONE", "FILL", "TEXT", "EFFECT", "EXPORT", "GRID"]);
    enum_def(&mut b, "GridPattern", &["GRID", "COLUMNS", "ROWS"]);
    enum_def(&mut b, "ImageScaleMode", &["STRETCH", "FIT", "FILL", "TILE"]);
    enum_def(&mut b, "WindingRule", &["NONZERO", "ODD"]);
    enum_def(&mut b, "ConstraintType", &["MIN", "CENTER", "MAX", "STRETCH", "SCALE"]);
    enum_def(&mut b, "BooleanOperation", &["UNION", "INTERSECT", "SUBTRACT", "XOR"]);

    let nat = FieldType::Native;
    let def = |b: &SchemaBuilder, n: &str| FieldType::Def(b.index(n));

    let g = b.define("GUID", DefKind::Struct);
    b.field(g, "sessionID", nat(Uint), false, 1);
    b.field(g, "localID", nat(Uint), false, 2);
    let vec = b.define("Vector", DefKind::Struct);
    b.field(vec, "x", nat(Float), false, 1);
    b.field(vec, "y", nat(Float), false, 2);
    let mx = b.define("Matrix", DefKind::Struct);
    for (i, n) in ["m00", "m01", "m02", "m10", "m11", "m12"].iter().enumerate() {
        b.field(mx, n, nat(Float), false, i as u32 + 1);
    }
    let c = b.define("Color", DefKind::Struct);
    for (i, n) in ["r", "g", "b", "a"].iter().enumerate() {
        b.field(c, n, nat(Float), false, i as u32 + 1);
    }
    let pi = b.define("ParentIndex", DefKind::Struct);
    let gt = def(&b, "GUID");
    b.field(pi, "guid", gt, false, 1);
    b.field(pi, "position", nat(String), false, 2);
    let cs = b.define("ColorStop", DefKind::Struct);
    let ct = def(&b, "Color");
    b.field(cs, "color", ct, false, 1);
    b.field(cs, "position", nat(Float), false, 2);
    let img = b.define("Image", DefKind::Message);
    b.field(img, "hash", nat(Byte), true, 1);
    b.field(img, "name", nat(String), false, 2);
    let num = b.define("Number", DefKind::Struct);
    b.field(num, "value", nat(Float), false, 1);
    let nu = def(&b, "NumberUnits");
    b.field(num, "units", nu, false, 2);
    let fnm = b.define("FontName", DefKind::Struct);
    b.field(fnm, "family", nat(String), false, 1);
    b.field(fnm, "style", nat(String), false, 2);
    b.field(fnm, "postscript", nat(String), false, 3);
    let arc = b.define("ArcData", DefKind::Struct);
    b.field(arc, "startingAngle", nat(Float), false, 1);
    b.field(arc, "endingAngle", nat(Float), false, 2);
    b.field(arc, "innerRadius", nat(Float), false, 3);
    let sid = b.define("StyleId", DefKind::Message);
    let gt = def(&b, "GUID");
    b.field(sid, "guid", gt, false, 1);

    let paint = b.define("Paint", DefKind::Message);
    let t = def(&b, "PaintType");
    b.field(paint, "type", t, false, 1);
    let ct = def(&b, "Color");
    b.field(paint, "color", ct, false, 2);
    b.field(paint, "opacity", nat(Float), false, 3);
    b.field(paint, "visible", nat(Bool), false, 4);
    let t = def(&b, "BlendMode");
    b.field(paint, "blendMode", t, false, 5);
    let cst = def(&b, "ColorStop");
    b.field(paint, "stops", cst, true, 6);
    let t = def(&b, "Matrix");
    b.field(paint, "transform", t, false, 7);
    let t = def(&b, "Image");
    b.field(paint, "image", t, false, 8);
    let t = def(&b, "ImageScaleMode");
    b.field(paint, "imageScaleMode", t, false, 9);

    let eff = b.define("Effect", DefKind::Message);
    let t = def(&b, "EffectType");
    b.field(eff, "type", t, false, 1);
    let ct = def(&b, "Color");
    b.field(eff, "color", ct, false, 2);
    let t = def(&b, "Vector");
    b.field(eff, "offset", t, false, 3);
    b.field(eff, "radius", nat(Float), false, 4);
    b.field(eff, "spread", nat(Float), false, 5);
    b.field(eff, "visible", nat(Bool), false, 6);
    b.field(eff, "showShadowBehindNode", nat(Bool), false, 7);

    let fg = b.define("FillGeometry", DefKind::Message);
    let t = def(&b, "WindingRule");
    b.field(fg, "windingRule", t, false, 1);
    b.field(fg, "commandsBlob", nat(Uint), false, 2);

    let lg = b.define("LayoutGrid", DefKind::Message);
    let t = def(&b, "GridPattern");
    b.field(lg, "pattern", t, false, 1);
    b.field(lg, "sectionSize", nat(Float), false, 2);
    b.field(lg, "gutterSize", nat(Float), false, 3);
    b.field(lg, "offset", nat(Float), false, 4);
    b.field(lg, "count", nat(Uint), false, 5);
    let ct = def(&b, "Color");
    b.field(lg, "color", ct, false, 6);
    b.field(lg, "visible", nat(Bool), false, 7);
    let gp = b.define("GuidPath", DefKind::Message);
    let gt = def(&b, "GUID");
    b.field(gp, "guids", gt, true, 1);

    let so = b.define("StyleOverride", DefKind::Message);
    b.field(so, "styleID", nat(Uint), false, 1);
    b.field(so, "fontSize", nat(Float), false, 2);
    let t = def(&b, "FontName");
    b.field(so, "fontName", t, false, 3);
    let t = def(&b, "Paint");
    b.field(so, "fillPaints", t, true, 4);
    let t = def(&b, "TextDecoration");
    b.field(so, "textDecoration", t, false, 5);

    enum_def(&mut b, "LineType", &["PLAIN", "ORDERED_LIST", "UNORDERED_LIST"]);
    enum_def(&mut b, "Directionality", &["AUTO", "LTR", "RTL"]);
    enum_def(&mut b, "LeadingTrim", &["NONE", "CAP_HEIGHT"]);
    enum_def(&mut b, "OpenTypeFeature", &["LIGA", "SS01", "TNUM"]);
    let tl = b.define("TextLine", DefKind::Message);
    let t = def(&b, "LineType");
    b.field(tl, "lineType", t, false, 1);
    b.field(tl, "indentationLevel", nat(Uint), false, 2);
    let t = def(&b, "Directionality");
    b.field(tl, "sourceDirectionality", t, false, 3);
    let td = b.define("TextData", DefKind::Message);
    b.field(td, "characters", nat(String), false, 1);
    b.field(td, "characterStyleIDs", nat(Uint), true, 2);
    let t = def(&b, "StyleOverride");
    b.field(td, "styleOverrideTable", t, true, 3);
    let t = def(&b, "TextLine");
    b.field(td, "lines", t, true, 4);

    let nc = b.define("NodeChange", DefKind::Message);
    let mut id = 0;
    let mut f = |b: &mut SchemaBuilder, name: &str, ty: FieldType, arr: bool| {
        id += 1;
        b.field(nc, name, ty, arr, id);
    };
    let (gt, pit, nt, ph) = (def(&b, "GUID"), def(&b, "ParentIndex"), def(&b, "NodeType"), def(&b, "Phase"));
    f(&mut b, "guid", gt, false);
    f(&mut b, "phase", ph, false);
    f(&mut b, "parentIndex", pit, false);
    f(&mut b, "type", nt, false);
    f(&mut b, "name", nat(String), false);
    f(&mut b, "visible", nat(Bool), false);
    f(&mut b, "locked", nat(Bool), false);
    f(&mut b, "opacity", nat(Float), false);
    let (v, m, bm) = (def(&b, "Vector"), def(&b, "Matrix"), def(&b, "BlendMode"));
    f(&mut b, "size", v, false);
    f(&mut b, "transform", m, false);
    f(&mut b, "blendMode", bm, false);
    let (p, e) = (def(&b, "Paint"), def(&b, "Effect"));
    f(&mut b, "fillPaints", p, true);
    f(&mut b, "strokePaints", p, true);
    f(&mut b, "effects", e, true);
    f(&mut b, "strokeWeight", nat(Float), false);
    let (sa, sj, sc) = (def(&b, "StrokeAlign"), def(&b, "StrokeJoin"), def(&b, "StrokeCap"));
    f(&mut b, "strokeAlign", sa, false);
    f(&mut b, "strokeJoin", sj, false);
    f(&mut b, "strokeCap", sc, false);
    f(&mut b, "dashPattern", nat(Float), true);
    f(&mut b, "cornerRadius", nat(Float), false);
    f(&mut b, "cornerSmoothing", nat(Float), false);
    f(&mut b, "rectangleCornerRadiiIndependent", nat(Bool), false);
    for n in ["rectangleTopLeftCornerRadius", "rectangleTopRightCornerRadius", "rectangleBottomLeftCornerRadius", "rectangleBottomRightCornerRadius"] {
        f(&mut b, n, nat(Float), false);
    }
    f(&mut b, "mask", nat(Bool), false);
    f(&mut b, "frameMaskDisabled", nat(Bool), false);
    f(&mut b, "internalOnly", nat(Bool), false);
    f(&mut b, "isStateGroup", nat(Bool), false);
    let c = def(&b, "Color");
    f(&mut b, "backgroundColor", c, false);
    let (sm, al, sz) = (def(&b, "StackMode"), def(&b, "StackAlign"), def(&b, "StackSize"));
    f(&mut b, "stackMode", sm, false);
    f(&mut b, "stackSpacing", nat(Float), false);
    f(&mut b, "stackCounterSpacing", nat(Float), false);
    f(&mut b, "stackHorizontalPadding", nat(Float), false);
    f(&mut b, "stackVerticalPadding", nat(Float), false);
    f(&mut b, "stackPaddingRight", nat(Float), false);
    f(&mut b, "stackPaddingBottom", nat(Float), false);
    f(&mut b, "stackPrimaryAlignItems", al, false);
    f(&mut b, "stackCounterAlignItems", al, false);
    f(&mut b, "stackPrimarySizing", sz, false);
    f(&mut b, "stackCounterSizing", sz, false);
    let (ca, sp) = (def(&b, "StackCounterAlign"), def(&b, "StackPositioning"));
    f(&mut b, "stackChildAlignSelf", ca, false);
    f(&mut b, "stackChildPrimaryGrow", nat(Float), false);
    f(&mut b, "stackPositioning", sp, false);
    let cn = def(&b, "ConstraintType");
    f(&mut b, "horizontalConstraint", cn, false);
    f(&mut b, "verticalConstraint", cn, false);
    let fgt = def(&b, "FillGeometry");
    f(&mut b, "fillGeometry", fgt, true);
    let ad = def(&b, "ArcData");
    f(&mut b, "arcData", ad, false);
    f(&mut b, "count", nat(Uint), false);
    f(&mut b, "starInnerScale", nat(Float), false);
    let bo = def(&b, "BooleanOperation");
    f(&mut b, "booleanOperation", bo, false);
    let sd = b.define("SymbolData", DefKind::Message);
    let gt = def(&b, "GUID");
    b.field(sd, "symbolID", gt, false, 1);
    let sdt = def(&b, "SymbolData");
    f(&mut b, "symbolData", sdt, false);
    let tdt = def(&b, "TextData");
    f(&mut b, "textData", tdt, false);
    let (fnt, nm) = (def(&b, "FontName"), def(&b, "Number"));
    f(&mut b, "fontName", fnt, false);
    f(&mut b, "fontSize", nat(Float), false);
    f(&mut b, "letterSpacing", nm, false);
    f(&mut b, "lineHeight", nm, false);
    let (ah, av, ar, tc, tdc) =
        (def(&b, "TextAlignHorizontal"), def(&b, "TextAlignVertical"), def(&b, "TextAutoResize"), def(&b, "TextCase"), def(&b, "TextDecoration"));
    f(&mut b, "textAlignHorizontal", ah, false);
    f(&mut b, "textAlignVertical", av, false);
    f(&mut b, "textAutoResize", ar, false);
    f(&mut b, "textCase", tc, false);
    f(&mut b, "textDecoration", tdc, false);
    let (lt, otf) = (def(&b, "LeadingTrim"), def(&b, "OpenTypeFeature"));
    f(&mut b, "leadingTrim", lt, false);
    f(&mut b, "paragraphIndent", nat(Float), false);
    f(&mut b, "toggledOnOTFeatures", otf, true);
    f(&mut b, "toggledOffOTFeatures", otf, true);
    let sidt = def(&b, "StyleId");
    f(&mut b, "styleIdForFill", sidt, false);
    for n in ["styleIdForStrokeFill", "styleIdForText", "styleIdForEffect", "styleIdForGrid"] {
        f(&mut b, n, sidt, false);
    }
    f(&mut b, "strokeGeometry", fgt, true);
    let lgt = def(&b, "LayoutGrid");
    f(&mut b, "layoutGrids", lgt, true);
    f(&mut b, "maxLines", nat(Uint), false);
    let (ttr, sty, gpt) = (def(&b, "TextTruncation"), def(&b, "StyleType"), def(&b, "GuidPath"));
    f(&mut b, "textTruncation", ttr, false);
    f(&mut b, "styleType", sty, false);
    f(&mut b, "styleDescription", nat(String), false);
    f(&mut b, "key", nat(String), false);
    f(&mut b, "guidPath", gpt, false);
    f(&mut b, "overriddenSymbolID", gt, false);
    // Overrides are NodeChange-shaped messages.
    b.field(sd, "symbolOverrides", FieldType::Def(nc), true, 2);
    f(&mut b, "derivedSymbolData", FieldType::Def(nc), true);

    let blob = b.define("Blob", DefKind::Message);
    b.field(blob, "bytes", nat(Byte), true, 1);
    let msg = b.define("Message", DefKind::Message);
    let (nct, bt) = (def(&b, "NodeChange"), def(&b, "Blob"));
    b.field(msg, "nodeChanges", nct, true, 1);
    b.field(msg, "blobs", bt, true, 2);
    b.build()
}

// ---- value builders

pub fn msg(fields: Vec<(&str, V)>) -> V {
    V::Message(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub fn st(fields: Vec<(&str, V)>) -> V {
    V::Struct(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub fn en(n: &str) -> V {
    V::Enum(n.into())
}
pub fn fl(x: f32) -> V {
    V::Float(x)
}
pub fn guid(s: u32, l: u32) -> V {
    st(vec![("sessionID", V::Uint(s)), ("localID", V::Uint(l))])
}
pub fn vec2(x: f32, y: f32) -> V {
    st(vec![("x", fl(x)), ("y", fl(y))])
}
pub fn matrix(tx: f32, ty: f32) -> V {
    st(vec![("m00", fl(1.0)), ("m01", fl(0.0)), ("m02", fl(tx)), ("m10", fl(0.0)), ("m11", fl(1.0)), ("m12", fl(ty))])
}
pub fn color(r: f32, g: f32, b: f32, a: f32) -> V {
    st(vec![("r", fl(r)), ("g", fl(g)), ("b", fl(b)), ("a", fl(a))])
}
pub fn solid(r: f32, g: f32, b: f32) -> V {
    msg(vec![("type", en("SOLID")), ("color", color(r, g, b, 1.0)), ("opacity", fl(1.0)), ("visible", V::Bool(true))])
}
pub fn node(id: (u32, u32), parent: (u32, u32), pos: &str, ty: &str, name: &str, mut extra: Vec<(&str, V)>) -> V {
    let mut f = vec![
        ("guid", guid(id.0, id.1)),
        ("parentIndex", st(vec![("guid", guid(parent.0, parent.1)), ("position", V::String(pos.into()))])),
        ("type", en(ty)),
        ("name", V::String(name.into())),
    ];
    f.append(&mut extra);
    msg(f)
}

/// Commands blob for a closed triangle: M(0,0) L(10,0) L(5,8) Z.
pub fn triangle_blob() -> Vec<u8> {
    let mut b = Vec::new();
    for (op, pts) in [(1u8, &[0.0f32, 0.0][..]), (2, &[10.0, 0.0]), (2, &[5.0, 8.0])] {
        b.push(op);
        for p in pts {
            b.extend(p.to_le_bytes());
        }
    }
    b.push(0);
    b
}

pub const IMAGE_HASH: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];

/// A small but representative document. Siblings are deliberately listed out
/// of z-order to exercise fractional-index sorting.
pub fn sample_message() -> V {
    let doc = node((0, 0), (0, 0), "!", "DOCUMENT", "Document", vec![]);
    let page = node((0, 1), (0, 0), "a", "CANVAS", "Page 1", vec![("backgroundColor", color(0.9, 0.9, 0.9, 1.0))]);
    let internal = node((0, 2), (0, 0), "b", "CANVAS", "Internal Only Canvas", vec![("internalOnly", V::Bool(true))]);
    let frame = node(
        (1, 1),
        (0, 1),
        "a",
        "FRAME",
        "Card",
        vec![
            ("size", vec2(200.0, 300.0)),
            ("transform", matrix(40.0, 50.0)),
            ("fillPaints", V::Array(vec![solid(1.0, 1.0, 1.0)])),
            ("cornerRadius", fl(12.0)),
            (
                "effects",
                V::Array(vec![msg(vec![
                    ("type", en("DROP_SHADOW")),
                    ("color", color(0.0, 0.0, 0.0, 0.25)),
                    ("offset", vec2(0.0, 4.0)),
                    ("radius", fl(8.0)),
                    ("spread", fl(0.0)),
                    ("visible", V::Bool(true)),
                ])]),
            ),
            ("stackMode", en("VERTICAL")),
            ("stackSpacing", fl(8.0)),
            ("stackHorizontalPadding", fl(16.0)),
            ("stackVerticalPadding", fl(12.0)),
            ("stackPrimarySizing", en("RESIZE_TO_FIT")),
            ("stackCounterAlignItems", en("CENTER")),
        ],
    );
    let rect = node(
        (1, 2),
        (1, 1),
        "b", // listed first, but sorts after the text
        "ROUNDED_RECTANGLE",
        "Image rect",
        vec![
            ("size", vec2(100.0, 40.0)),
            ("transform", matrix(16.0, 12.0)),
            (
                "fillPaints",
                V::Array(vec![msg(vec![("type", en("IMAGE")), ("image", msg(vec![("hash", V::Bytes(IMAGE_HASH.to_vec()))])), ("imageScaleMode", en("FILL"))])]),
            ),
            ("strokePaints", V::Array(vec![solid(1.0, 0.0, 0.0)])),
            ("strokeWeight", fl(2.0)),
            ("strokeAlign", en("OUTSIDE")),
            ("stackChildAlignSelf", en("STRETCH")),
            ("blendMode", en("MULTIPLY")),
        ],
    );
    let text = node(
        (1, 3),
        (1, 1),
        "a",
        "TEXT",
        "Title",
        vec![
            ("size", vec2(120.0, 20.0)),
            ("transform", matrix(16.0, 60.0)),
            ("fillPaints", V::Array(vec![solid(0.1, 0.2, 0.3)])),
            ("fontSize", fl(16.0)),
            (
                "fontName",
                st(vec![("family", V::String("Inter".into())), ("style", V::String("Medium".into())), ("postscript", V::String("Inter-Medium".into()))]),
            ),
            ("letterSpacing", st(vec![("value", fl(10.0)), ("units", en("PERCENT"))])),
            ("lineHeight", st(vec![("value", fl(24.0)), ("units", en("PIXELS"))])),
            ("textAlignHorizontal", en("CENTER")),
            ("textAutoResize", en("WIDTH_AND_HEIGHT")),
            (
                "textData",
                msg(vec![
                    ("characters", V::String("Hi \u{1F600}!".into())),
                    // 'H','i',' ', surrogate pair (2 units), '!' -> UTF-16 length 6
                    ("characterStyleIDs", V::Array([0u32, 0, 0, 1, 1, 0].iter().map(|x| V::Uint(*x)).collect())),
                    (
                        "styleOverrideTable",
                        V::Array(vec![msg(vec![
                            ("styleID", V::Uint(1)),
                            (
                                "fontName",
                                st(vec![
                                    ("family", V::String("Inter".into())),
                                    ("style", V::String("Bold Italic".into())),
                                    ("postscript", V::String("".into())),
                                ]),
                            ),
                            ("fillPaints", V::Array(vec![solid(1.0, 0.0, 0.0)])),
                            ("textDecoration", en("UNDERLINE")),
                        ])]),
                    ),
                ]),
            ),
        ],
    );
    let vector = node(
        (1, 4),
        (1, 1),
        "c",
        "VECTOR",
        "Tri",
        vec![
            ("size", vec2(10.0, 8.0)),
            ("transform", matrix(0.0, 100.0)),
            ("fillPaints", V::Array(vec![solid(0.0, 0.0, 0.0)])),
            ("fillGeometry", V::Array(vec![msg(vec![("windingRule", en("NONZERO")), ("commandsBlob", V::Uint(0))])])),
        ],
    );
    let comp = node(
        (1, 5),
        (1, 1),
        "d",
        "SYMBOL",
        "Button",
        vec![("size", vec2(80.0, 30.0)), ("transform", matrix(0.0, 120.0)), ("fillPaints", V::Array(vec![solid(0.0, 0.5, 1.0)]))],
    );
    let comp_child = node((1, 6), (1, 5), "a", "RECTANGLE", "Bg", vec![("size", vec2(80.0, 30.0)), ("fillPaints", V::Array(vec![solid(0.2, 0.2, 0.2)]))]);
    let inst = node(
        (1, 7),
        (1, 1),
        "e",
        "INSTANCE",
        "Button copy",
        vec![("size", vec2(80.0, 30.0)), ("transform", matrix(100.0, 120.0)), ("symbolData", msg(vec![("symbolID", guid(1, 5))]))],
    );
    let ell = node(
        (1, 8),
        (0, 1),
        "b",
        "ELLIPSE",
        "Pie",
        vec![
            ("size", vec2(50.0, 50.0)),
            ("transform", matrix(300.0, 50.0)),
            ("arcData", st(vec![("startingAngle", fl(0.0)), ("endingAngle", fl(3.0)), ("innerRadius", fl(0.5))])),
        ],
    );
    let removed = V::Message(match node((1, 9), (1, 1), "z", "RECTANGLE", "Removed", vec![("phase", en("REMOVED"))]) {
        V::Message(f) => f,
        _ => unreachable!(),
    });
    let star = node((1, 10), (0, 1), "c", "STAR", "Star", vec![("size", vec2(40.0, 40.0)), ("count", V::Uint(6)), ("starInnerScale", fl(0.5))]);
    let sticky = node((1, 11), (0, 1), "d", "STICKY", "Note", vec![("size", vec2(40.0, 40.0))]);

    msg(vec![
        // Child before parent and shuffled positions on purpose.
        ("nodeChanges", V::Array(vec![inst, rect, text, vector, comp_child, comp, frame, ell, removed, star, sticky, internal, page, doc])),
        ("blobs", V::Array(vec![msg(vec![("bytes", V::Bytes(triangle_blob()))])])),
    ])
}

/// Builds a complete `.fig` byte stream from the sample message.
pub fn sample_fig(how: Compression, zipped: bool) -> Vec<u8> {
    let schema = figma_like_schema();
    let root = schema.find("Message").unwrap();
    let body = schema.encode_message(root, &sample_message()).unwrap();
    let canvas = container::write_container(&schema.encode(), &body, how, 101);
    if !zipped {
        return canvas;
    }
    let mut images = BTreeMap::new();
    images.insert(crate::kiwi::hex(&IMAGE_HASH), vec![0x89, b'P', b'N', b'G']);
    container::write_zip(&canvas, &images, Some(&serde_json::json!({"file_name": "Sample file"})))
}

// ---- upgrade fixture: geometry, styles, grids, text clamping, instance overrides

/// Commands blob for a closed square: M(0,0) L(10,0) L(10,10) L(0,10) Z.
pub fn square_blob() -> Vec<u8> {
    let mut b = Vec::new();
    for (op, pts) in [(1u8, [0.0f32, 0.0]), (2, [10.0, 0.0]), (2, [10.0, 10.0]), (2, [0.0, 10.0])] {
        b.push(op);
        for p in pts {
            b.extend(p.to_le_bytes());
        }
    }
    b.push(0);
    b
}

fn path_of(guids: &[(u32, u32)]) -> V {
    msg(vec![("guids", V::Array(guids.iter().map(|g| guid(g.0, g.1)).collect()))])
}

fn ov(path: &[(u32, u32)], mut fields: Vec<(&str, V)>) -> V {
    let mut f = vec![("guidPath", path_of(path))];
    f.append(&mut fields);
    msg(f)
}

fn num(value: f32, units: &str) -> V {
    st(vec![("value", fl(value)), ("units", en(units))])
}

fn text_data(s: &str) -> V {
    msg(vec![("characters", V::String(s.into()))])
}

/// Message exercising the `.fig` upgrade features (see `tests/fig_upgrade.rs`).
/// Blob 0 is the triangle, blob 1 the square.
pub fn upgrade_message() -> V {
    let geo = |rule: &str, blob: u32| V::Array(vec![msg(vec![("windingRule", en(rule)), ("commandsBlob", V::Uint(blob))])]);
    let size = |w: f32, h: f32| ("size", vec2(w, h));
    let doc = node((0, 0), (0, 0), "!", "DOCUMENT", "Document", vec![]);
    let page = node((0, 1), (0, 0), "a", "CANVAS", "Up", vec![("backgroundColor", color(1.0, 1.0, 1.0, 1.0))]);
    let internal = node((0, 2), (0, 0), "b", "CANVAS", "Internal Only Canvas", vec![("internalOnly", V::Bool(true))]);
    let style_node = |l: u32, name: &str, ty: &str, desc: &str, key: Option<&str>| {
        let mut f = vec![("styleType", en(ty)), ("styleDescription", V::String(desc.into())), size(1.0, 1.0)];
        if let Some(k) = key {
            f.push(("key", V::String(k.into())));
        }
        node((2, l), (0, 2), &format!("{l}"), "RECTANGLE", name, f)
    };
    let styles = [
        style_node(100, "Brand/Primary", "FILL", "Main brand fill", Some("key-primary")),
        style_node(101, "Border/Strong", "FILL", "", None),
        style_node(102, "Body", "TEXT", "Body copy", Some("key-body")),
    ];
    let sref = |l: u32| msg(vec![("guid", guid(2, l))]);

    let grids = node(
        (2, 1),
        (0, 1),
        "a",
        "FRAME",
        "Grids",
        vec![
            size(300.0, 200.0),
            (
                "layoutGrids",
                V::Array(vec![
                    msg(vec![
                        ("pattern", en("COLUMNS")),
                        ("count", V::Uint(4)),
                        ("gutterSize", fl(8.0)),
                        ("offset", fl(2.0)),
                        ("color", color(1.0, 0.0, 0.0, 0.1)),
                        ("visible", V::Bool(true)),
                    ]),
                    msg(vec![("pattern", en("GRID")), ("sectionSize", fl(10.0)), ("color", color(0.0, 0.0, 1.0, 0.2))]),
                    msg(vec![("pattern", en("ROWS")), ("count", V::Uint(3)), ("gutterSize", fl(4.0)), ("visible", V::Bool(false))]),
                ]),
            ),
        ],
    );
    let boolean = node(
        (2, 2),
        (0, 1),
        "b",
        "BOOLEAN_OPERATION",
        "Bool",
        vec![
            size(10.0, 10.0),
            ("fillGeometry", geo("NONZERO", 0)),
            ("strokeGeometry", geo("ODD", 1)),
            ("blendMode", en("LINEAR_BURN")),
            ("styleIdForFill", sref(100)),
            ("styleIdForStrokeFill", sref(101)),
            ("fillPaints", V::Array(vec![solid(1.0, 0.0, 0.0)])),
            ("strokePaints", V::Array(vec![solid(0.0, 0.0, 0.0)])),
        ],
    );
    let star = node(
        (2, 3),
        (0, 1),
        "c",
        "STAR",
        "Star",
        vec![size(10.0, 10.0), ("count", V::Uint(5)), ("fillGeometry", geo("NONZERO", 0)), ("blendMode", en("LINEAR_DODGE"))],
    );
    let text = |l: u32, pos: &str, name: &str, mut extra: Vec<(&str, V)>| {
        let mut f = vec![size(80.0, 40.0), ("fontSize", fl(10.0)), ("textData", text_data(name))];
        f.append(&mut extra);
        node((2, l), (0, 1), pos, "TEXT", name, f)
    };
    let clamp = text(
        4,
        "d",
        "Clamp",
        vec![("textTruncation", en("ENDING")), ("maxLines", V::Uint(2)), ("lineHeight", num(100.0, "PERCENT")), ("styleIdForText", sref(102))],
    );
    let lh_auto = text(5, "e", "LhAuto", vec![("lineHeight", num(1.0, "RAW"))]);
    let lh_pct = text(6, "f", "LhPct", vec![("lineHeight", num(150.0, "PERCENT"))]);
    let lh_none = text(7, "g", "LhNone", vec![]);
    let lh_px = text(8, "h", "LhPx", vec![("lineHeight", num(18.0, "PIXELS"))]);
    let plain = text(9, "i", "Plain", vec![]);
    // Lists: bullet, nested number, plain line, in a right-to-left, cap-trimmed layer.
    let line = |ty: &str, level: u32| msg(vec![("lineType", en(ty)), ("indentationLevel", V::Uint(level)), ("sourceDirectionality", en("RTL"))]);
    let listy = node(
        (2, 50),
        (0, 1),
        "o",
        "TEXT",
        "Listy",
        vec![
            size(80.0, 40.0),
            ("fontSize", fl(10.0)),
            (
                "textData",
                msg(vec![
                    (
                        "characters",
                        V::String(
                            "one
two
three"
                                .into(),
                        ),
                    ),
                    ("lines", V::Array(vec![line("UNORDERED_LIST", 1), line("ORDERED_LIST", 2), line("PLAIN", 0)])),
                ]),
            ),
            ("leadingTrim", en("CAP_HEIGHT")),
            ("paragraphIndent", fl(6.0)),
            ("toggledOnOTFeatures", V::Array(vec![en("TNUM"), en("SS01")])),
            ("toggledOffOTFeatures", V::Array(vec![en("LIGA")])),
        ],
    );

    // Component with a label, background and a nested frame.
    let card = node((2, 10), (0, 1), "j", "SYMBOL", "Card", vec![size(100.0, 50.0), ("transform", matrix(0.0, 300.0))]);
    let label = node(
        (2, 11),
        (2, 10),
        "a",
        "TEXT",
        "Label",
        vec![size(60.0, 12.0), ("fontSize", fl(10.0)), ("textData", text_data("Old")), ("fillPaints", V::Array(vec![solid(0.0, 0.0, 0.0)]))],
    );
    let bg = node((2, 12), (2, 10), "b", "RECTANGLE", "Bg", vec![size(100.0, 50.0), ("fillPaints", V::Array(vec![solid(0.5, 0.5, 0.5)]))]);
    let inner = node((2, 13), (2, 10), "c", "FRAME", "Inner", vec![size(20.0, 20.0)]);
    let dot = node((2, 14), (2, 13), "a", "RECTANGLE", "Dot", vec![size(5.0, 5.0), ("fillPaints", V::Array(vec![solid(0.0, 1.0, 0.0)]))]);

    // Wrapper component containing a nested instance of Card with its own override.
    let wrapper = node((2, 20), (0, 1), "k", "SYMBOL", "Wrapper", vec![size(100.0, 50.0), ("transform", matrix(200.0, 300.0))]);
    let card_in = node(
        (2, 21),
        (2, 20),
        "a",
        "INSTANCE",
        "CardIn",
        vec![
            size(100.0, 50.0),
            (
                "symbolData",
                msg(vec![("symbolID", guid(2, 10)), ("symbolOverrides", V::Array(vec![ov(&[(2, 11)], vec![("textData", text_data("Inner text"))])]))]),
            ),
        ],
    );
    let wrap_inst = node(
        (2, 30),
        (0, 1),
        "l",
        "INSTANCE",
        "WrapInst",
        vec![
            size(100.0, 50.0),
            ("transform", matrix(400.0, 300.0)),
            (
                "symbolData",
                msg(vec![("symbolID", guid(2, 20)), ("symbolOverrides", V::Array(vec![ov(&[(2, 21), (2, 11)], vec![("textData", text_data("Deep"))])]))]),
            ),
        ],
    );

    let card_a = node(
        (2, 40),
        (0, 1),
        "m",
        "INSTANCE",
        "Card A",
        vec![
            size(200.0, 80.0),
            ("transform", matrix(0.0, 400.0)),
            (
                "symbolData",
                msg(vec![
                    ("symbolID", guid(2, 10)),
                    (
                        "symbolOverrides",
                        V::Array(vec![
                            ov(
                                &[(2, 11)],
                                vec![
                                    ("textData", text_data("New \u{1F600}")),
                                    ("fillPaints", V::Array(vec![solid(1.0, 0.0, 0.0)])),
                                    ("letterSpacing", num(50.0, "PERCENT")),
                                    ("opacity", fl(0.75)),
                                ],
                            ),
                            ov(&[(2, 12)], vec![("visible", V::Bool(false)), ("blendMode", en("LINEAR_DODGE")), ("cornerRadius", fl(4.0))]),
                            ov(&[(2, 13), (2, 14)], vec![("fillPaints", V::Array(vec![solid(0.0, 0.0, 1.0)])), size(7.0, 7.0)]),
                            ov(&[(9, 99)], vec![("visible", V::Bool(false))]), // dangling path: ignored
                        ]),
                    ),
                ]),
            ),
            (
                "derivedSymbolData",
                V::Array(vec![
                    ov(
                        &[(2, 12)],
                        vec![
                            size(200.0, 80.0),
                            ("transform", matrix(5.0, 6.0)),
                            ("fillGeometry", geo("NONZERO", 1)),
                            ("name", V::String("ignored by derived".into())),
                        ],
                    ),
                    ov(&[], vec![size(200.0, 80.0)]),
                ]),
            ),
        ],
    );
    let card_b = node(
        (2, 41),
        (0, 1),
        "n",
        "INSTANCE",
        "Card B",
        vec![size(100.0, 50.0), ("transform", matrix(300.0, 400.0)), ("symbolData", msg(vec![("symbolID", guid(2, 10))]))],
    );

    let mut nodes = vec![doc, page, internal];
    nodes.extend(styles);
    nodes.extend([grids, boolean, star, clamp, lh_auto, lh_pct, lh_none, lh_px, plain, listy]);
    nodes.extend([card, label, bg, inner, dot, wrapper, card_in, wrap_inst, card_a, card_b]);
    msg(vec![
        ("nodeChanges", V::Array(nodes)),
        ("blobs", V::Array(vec![msg(vec![("bytes", V::Bytes(triangle_blob()))]), msg(vec![("bytes", V::Bytes(square_blob()))])])),
    ])
}

/// `.fig` bytes for [`upgrade_message`].
pub fn upgrade_fig(how: Compression) -> Vec<u8> {
    let schema = figma_like_schema();
    let root = schema.find("Message").unwrap();
    let body = schema.encode_message(root, &upgrade_message()).unwrap();
    container::write_container(&schema.encode(), &body, how, 101)
}

// ---- nested instance swap fixture (see `tests/nested_swap.rs`)

/// A `ListRow` component whose nested icon instance is swapped on one of its
/// instances through a `symbolOverrides` entry carrying `overriddenSymbolID`,
/// followed by a deeper override addressing a child of the *new* icon.
pub fn nested_swap_message() -> V {
    let doc = node((0, 0), (0, 0), "!", "DOCUMENT", "Document", vec![]);
    let page = node((0, 1), (0, 0), "a", "CANVAS", "Page 1", vec![]);
    let size = |w: f32, h: f32| ("size", vec2(w, h));

    let gear_icon = node((2, 10), (0, 1), "a", "SYMBOL", "Icon / settings", vec![size(16.0, 16.0)]);
    let gear = node((2, 11), (2, 10), "a", "RECTANGLE", "Gear", vec![size(16.0, 16.0), ("fillPaints", V::Array(vec![solid(0.0, 0.0, 0.0)]))]);
    let users_icon = node((2, 20), (0, 1), "b", "SYMBOL", "Icon / users", vec![size(16.0, 16.0)]);
    let people = node((2, 21), (2, 20), "a", "RECTANGLE", "People", vec![size(8.0, 16.0), ("fillPaints", V::Array(vec![solid(0.0, 0.0, 0.0)]))]);
    let people2 = node((2, 22), (2, 20), "b", "RECTANGLE", "People2", vec![size(8.0, 16.0), ("transform", matrix(8.0, 0.0))]);

    let row = node((2, 30), (0, 1), "c", "SYMBOL", "ListRow", vec![size(100.0, 24.0)]);
    let slot = node((2, 31), (2, 30), "a", "INSTANCE", "Icon", vec![size(16.0, 16.0), ("symbolData", msg(vec![("symbolID", guid(2, 10))]))]);

    let row_a = node(
        (2, 40),
        (0, 1),
        "d",
        "INSTANCE",
        "Row A",
        vec![
            size(100.0, 24.0),
            ("transform", matrix(0.0, 100.0)),
            (
                "symbolData",
                msg(vec![
                    ("symbolID", guid(2, 30)),
                    (
                        "symbolOverrides",
                        V::Array(vec![
                            ov(&[(2, 31)], vec![("overriddenSymbolID", guid(2, 20))]),
                            ov(&[(2, 31), (2, 21)], vec![("fillPaints", V::Array(vec![solid(1.0, 0.0, 0.0)]))]),
                        ]),
                    ),
                ]),
            ),
        ],
    );
    let row_b = node(
        (2, 41),
        (0, 1),
        "e",
        "INSTANCE",
        "Row B",
        vec![size(100.0, 24.0), ("transform", matrix(0.0, 200.0)), ("symbolData", msg(vec![("symbolID", guid(2, 30))]))],
    );

    msg(vec![("nodeChanges", V::Array(vec![row_a, row_b, slot, row, people2, people, users_icon, gear, gear_icon, page, doc])), ("blobs", V::Array(vec![]))])
}

/// `.fig` bytes for [`nested_swap_message`].
pub fn nested_swap_fig(how: Compression) -> Vec<u8> {
    let schema = figma_like_schema();
    let root = schema.find("Message").unwrap();
    let body = schema.encode_message(root, &nested_swap_message()).unwrap();
    container::write_container(&schema.encode(), &body, how, 101)
}
