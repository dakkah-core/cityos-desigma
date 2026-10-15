// Texture (displacement) and Glass effects, ported from desigma-render's fx.rs.

struct P {
    origin: vec2<i32>,   // root pixel of texel (0,0) of the output region
    size: vec2<i32>,     // logical size of tex1
    mask_origin: vec2<i32>,
    mask_size: vec2<i32>,
    grid: vec2<i32>,
    seed: u32,
    has_mask: u32,
    amp: f32,
    cell: f32,
    ss: f32,
    depth: f32,
    light: vec2<f32>,
    intensity: f32,
    refraction: f32,
    dispersion: f32,
    // Size of the pass's viewport in pixels: fragments derive top-left pixel
    // coordinates from `uv`, because `@builtin(position)` runs bottom-up on GL.
    out_w: f32,
    out_h: f32,
    p1: f32,
    // tex2 origin/size for soft mask share mask_origin/size
};

@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var tex1: texture_2d<f32>;
@group(0) @binding(2) var tex2: texture_2d<f32>;
@group(0) @binding(3) var tex3: texture_2d<f32>;

struct FsIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FsIn {
    var o: FsIn;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

fn mix32(h0: u32) -> u32 {
    var h = h0;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    return h ^ (h >> 16u);
}

fn hash2(seed: u32, x: i32, y: i32) -> u32 {
    let a = bitcast<u32>(x) * 0x85ebca6bu;
    let b = bitcast<u32>(y) * 0xc2b2ae35u;
    let rb = (b << 13u) | (b >> 19u);
    return mix32((seed * 0x9e3779b1u) ^ mix32(a ^ rb));
}

fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

fn value_noise(seed: u32, u: f32, v: f32) -> f32 {
    let iu = floor(u);
    let iv = floor(v);
    let fu = u - iu;
    let fv = v - iv;
    let a = i32(iu);
    let b = i32(iv);
    let su = fu * fu * (3.0 - 2.0 * fu);
    let sv = fv * fv * (3.0 - 2.0 * fv);
    let c00 = unit(hash2(seed, a, b));
    let c10 = unit(hash2(seed, a + 1, b));
    let c01 = unit(hash2(seed, a, b + 1));
    let c11 = unit(hash2(seed, a + 1, b + 1));
    let top = c00 + (c10 - c00) * su;
    let bot = c01 + (c11 - c01) * su;
    return top + (bot - top) * sv;
}

fn texel(t: texture_2d<f32>, q: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    if (q.x < 0 || q.y < 0 || q.x >= size.x || q.y >= size.y) {
        return vec4<f32>(0.0);
    }
    return textureLoad(t, q, 0);
}

fn texel_clamped(t: texture_2d<f32>, q: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    return textureLoad(t, clamp(q, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

fn bilinear(t: texture_2d<f32>, pt: vec2<f32>, size: vec2<i32>, clamp_edges: bool) -> vec4<f32> {
    let f = pt - vec2<f32>(0.5);
    let f0 = floor(f);
    let t01 = f - f0;
    let q = vec2<i32>(f0);
    var a: vec4<f32>;
    var b: vec4<f32>;
    var c: vec4<f32>;
    var d: vec4<f32>;
    if (clamp_edges) {
        a = texel_clamped(t, q, size);
        b = texel_clamped(t, q + vec2<i32>(1, 0), size);
        c = texel_clamped(t, q + vec2<i32>(0, 1), size);
        d = texel_clamped(t, q + vec2<i32>(1, 1), size);
    } else {
        a = texel(t, q, size);
        b = texel(t, q + vec2<i32>(1, 0), size);
        c = texel(t, q + vec2<i32>(0, 1), size);
        d = texel(t, q + vec2<i32>(1, 1), size);
    }
    let top = a + (b - a) * t01.x;
    let bot = c + (d - c) * t01.x;
    return top + (bot - top) * t01.y;
}

// tex1: the layer; tex2: optional clip-to-shape mask (root-aligned by mask_origin)
@fragment
fn fs_displace(f: FsIn) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(f.uv * vec2<f32>(p.out_w, p.out_h)));
    let root = vec2<i32>(floor(vec2<f32>(px + p.origin) / p.ss)) + p.grid;
    let u = f32(root.x) / p.cell;
    let v = f32(root.y) / p.cell;
    let n0 = value_noise(p.seed, u + 17.3, v + 5.1);
    let n1 = value_noise(p.seed ^ 0x51edu, u * 2.3 + 17.3 * 2.3, v * 2.3 + 5.1 * 2.3);
    let m0 = value_noise(p.seed ^ 0x9bd1u, u + 17.3, v + 5.1);
    let m1 = value_noise(p.seed ^ 0x2f7cu, u * 2.3 + 17.3 * 2.3, v * 2.3 + 5.1 * 2.3);
    let dx = ((n0 - 0.5) + 0.5 * (n1 - 0.5)) * 1.33 * p.amp;
    let dy = ((m0 - 0.5) + 0.5 * (m1 - 0.5)) * 1.33 * p.amp;
    var c = bilinear(tex1, vec2<f32>(px) + vec2<f32>(0.5 + dx, 0.5 + dy), p.size, false);
    if (p.has_mask != 0u) {
        let m = texel(tex2, px + p.origin - p.mask_origin, p.mask_size).a;
        c = c * m;
    }
    return c;
}

// tex1: frosted backdrop, tex2: opaque silhouette, tex3: softened silhouette.
// All three cover the same `pad` rectangle (origin = mask_origin); the output
// covers the replaced region (origin = `origin`).
@fragment
fn fs_glass(f: FsIn) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(f.uv * vec2<f32>(p.out_w, p.out_h)));
    let m = px + p.origin - p.mask_origin;
    let cov = texel(tex2, m, p.mask_size).a;
    if (cov <= 0.0) {
        return vec4<f32>(0.0);
    }
    let gx = (texel_clamped(tex3, m + vec2<i32>(1, 0), p.mask_size).a - texel_clamped(tex3, m + vec2<i32>(-1, 0), p.mask_size).a) * 0.5;
    let gy = (texel_clamped(tex3, m + vec2<i32>(0, 1), p.mask_size).a - texel_clamped(tex3, m + vec2<i32>(0, -1), p.mask_size).a) * 0.5;
    let len = sqrt(gx * gx + gy * gy);
    var n = vec2<f32>(0.0);
    if (len > 1e-4) {
        n = vec2<f32>(gx, gy) / len;
    }
    let soft = texel_clamped(tex3, m, p.mask_size).a;
    let t = clamp((1.0 - soft) * 2.0, 0.0, 1.0);
    let fc = vec2<f32>(m) + vec2<f32>(0.5);
    let refr = clamp(p.refraction, 0.0, 1.0);
    let disp = clamp(p.dispersion, 0.0, 1.0);
    let inten = clamp(p.intensity, 0.0, 1.0);
    let bend = refr * p.depth * t;
    let split = disp * p.depth * 0.5 * t;
    let cr = bilinear(tex1, fc + n * (bend + split), p.mask_size, true);
    let cg = bilinear(tex1, fc + n * bend, p.mask_size, true);
    let cb = bilinear(tex1, fc + n * (bend - split), p.mask_size, true);
    var c = vec4<f32>(cr.r, cg.g, cb.b, cg.a);
    let facing = -(n.x * p.light.x + n.y * p.light.y);
    let hl = clamp(inten * t * pow(max(facing, 0.0), 1.5), 0.0, 1.0);
    let shade = clamp(inten * 0.3 * t * max(-facing, 0.0), 0.0, 1.0);
    let rgb = c.rgb * (1.0 - shade) * (1.0 - hl) + vec3<f32>(hl);
    let a = c.a * (1.0 - hl) + hl;
    return vec4<f32>(rgb, a) * cov;
}
