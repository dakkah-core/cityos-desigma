#![allow(dead_code)]

pub mod demo;

use desigma_diff::{diff_image, DiffOptions, DiffReport};
use desigma_doc::*;
use desigma_gpu::{GpuError, HeadlessGpu};
use desigma_render::tiny_skia::Pixmap;
use desigma_render::{render_page, Viewport};
use image::RgbaImage;

/// A headless GPU, or `None` (with a printed note) when the machine has no adapter.
pub fn gpu() -> Option<HeadlessGpu> {
    match HeadlessGpu::new() {
        Ok(g) => {
            eprintln!("GPU: {} ({:?})", g.ctx.adapter_info.name, g.ctx.adapter_info.backend);
            Some(g)
        }
        Err(GpuError::NoAdapter(e)) => {
            eprintln!("SKIP: no GPU adapter ({e}); GPU fidelity checks not run");
            None
        }
        Err(e) => panic!("GPU renderer initialization failed: {e}"),
    }
}

pub fn to_img(pm: &Pixmap) -> RgbaImage {
    RgbaImage::from_raw(pm.width(), pm.height(), pm.data().to_vec()).unwrap()
}

pub fn out_dir() -> std::path::PathBuf {
    let p = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("gpu-fidelity");
    std::fs::create_dir_all(&p).unwrap();
    p
}

pub struct Cmp {
    /// Strict: every pixel counts.
    pub report: DiffReport,
    /// With edge pixels that look anti-aliased in either image forgiven.
    pub report_aa: DiffReport,
    pub cpu: Pixmap,
    pub gpu: Pixmap,
}

/// Renders `page` with both renderers and compares them.
pub fn compare(g: &mut HeadlessGpu, name: &str, doc: &Document, page: NodeId, vp: Viewport, size: (u32, u32), tolerance: u8) -> Cmp {
    let mut cpu = Pixmap::new(size.0, size.1).unwrap();
    render_page(doc, page, &vp, &mut cpu);
    let gpu = g.render_page(doc, page, &vp, size);
    let (a, b) = (to_img(&cpu), to_img(&gpu));
    let opts = DiffOptions { tolerance, detect_aa: false, ignore: vec![] };
    let (report, diff) = diff_image(&a, &b, &opts);
    let dir = out_dir();
    cpu.save_png(dir.join(format!("{name}-cpu.png"))).unwrap();
    gpu.save_png(dir.join(format!("{name}-gpu.png"))).unwrap();
    diff.save(dir.join(format!("{name}-diff.png"))).unwrap();
    eprintln!(
        "{name}: mismatch {:.4}% ({} px of {}) at tolerance {tolerance}, ssim {:.4}",
        report.ratio * 100.0,
        report.mismatched,
        size.0 * size.1,
        report.ssim
    );
    // the same comparison with desigma-diff's default anti-aliasing forgiveness
    let aa = desigma_diff::compare(&a, &b, &DiffOptions { tolerance, detect_aa: true, ignore: vec![] });
    eprintln!("{name}: {:.4}% with AA pixels forgiven ({} px)", aa.ratio * 100.0, aa.mismatched);
    Cmp { report, report_aa: aa, cpu, gpu }
}

pub struct T {
    pub doc: Document,
    pub page: NodeId,
}

impl T {
    pub fn new() -> Self {
        let mut doc = Document::new("t");
        let page = doc.add_page("p");
        doc.get_mut(page).unwrap().kind = NodeKind::Page { background: Color::WHITE };
        Self { doc, page }
    }
    pub fn add(&mut self, parent: Option<NodeId>, f: impl FnOnce(NodeId) -> Node) -> NodeId {
        let id = self.doc.alloc_id();
        self.doc.insert(parent.unwrap_or(self.page), f(id)).unwrap()
    }
    pub fn rect(&mut self, parent: Option<NodeId>, r: Rect, c: Color) -> NodeId {
        self.add(parent, |id| {
            let mut n = Node::rectangle(id, r);
            n.fills = vec![Paint::solid(c)];
            n
        })
    }
    pub fn node(&mut self, id: NodeId) -> &mut Node {
        self.doc.get_mut(id).unwrap()
    }
}
