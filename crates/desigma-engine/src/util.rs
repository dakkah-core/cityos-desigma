use desigma_doc::{Rect, Vec2};

pub fn v2(x: f32, y: f32) -> Vec2 {
    Vec2::new(x, y)
}

pub fn aabb_of(points: &[Vec2]) -> Rect {
    if points.is_empty() {
        return Rect::default();
    }
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    for p in points {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

pub fn rect_corners(r: &Rect) -> [Vec2; 4] {
    [v2(r.x, r.y), v2(r.x + r.w, r.y), v2(r.x + r.w, r.y + r.h), v2(r.x, r.y + r.h)]
}
