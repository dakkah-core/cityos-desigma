//! File thumbnails: rendering the first frame to a PNG, and the texture cache the home screen draws from.

use desigma_doc::{Document, DocumentKind, NodeId};
use desigma_library::Library;
use std::collections::HashMap;
use std::time::SystemTime;

/// Longest edge of a stored thumbnail in pixels.
pub const THUMB_PX: f32 = 480.0;

/// The node a thumbnail shows: the first top-level frame of the first page, else its first layer.
pub fn thumbnail_node(doc: &Document) -> Option<NodeId> {
    let page = doc.get(*doc.pages.first()?)?;
    let kids: Vec<_> = page.children.iter().filter_map(|c| doc.get(*c)).filter(|n| n.visible).collect();
    kids.iter().find(|n| n.kind.frame_props().is_some() && !matches!(n.kind, desigma_doc::NodeKind::Instance { .. })).or(kids.first()).map(|n| n.id)
}

/// Size of a board thumbnail: the whole content on the dotted canvas, 8:5 like the cards.
const BOARD_THUMB: (u32, u32) = (THUMB_PX as u32, (THUMB_PX * 0.625) as u32);

/// The whole first page of a board on its dotted canvas, fitted to [`BOARD_THUMB`] with some air
/// around the content; `None` for an empty board.
fn board_thumbnail_png(doc: &Document) -> Option<Vec<u8>> {
    use desigma_render::tiny_skia as sk;
    let page = *doc.pages.first()?;
    let kids = &doc.get(page)?.children;
    let mut bounds: Option<desigma_doc::Rect> = None;
    for k in kids {
        let n = doc.get(*k).filter(|n| n.visible)?;
        let w = doc.world_transform(*k);
        let corners =
            [w.apply(desigma_doc::Vec2::ZERO), w.apply(desigma_doc::Vec2::new(n.size.x, 0.0)), w.apply(n.size), w.apply(desigma_doc::Vec2::new(0.0, n.size.y))];
        let (x0, x1) = corners.iter().fold((f32::MAX, f32::MIN), |a, c| (a.0.min(c.x), a.1.max(c.x)));
        let (y0, y1) = corners.iter().fold((f32::MAX, f32::MIN), |a, c| (a.0.min(c.y), a.1.max(c.y)));
        let r = desigma_doc::Rect::new(x0, y0, x1 - x0, y1 - y0);
        bounds = Some(bounds.map_or(r, |b| b.union(&r)));
    }
    let b = bounds?;
    let (tw, th) = (BOARD_THUMB.0 as f32, BOARD_THUMB.1 as f32);
    let pad = 24.0;
    let zoom = ((tw - 2.0 * pad) / b.w.max(1.0)).min((th - 2.0 * pad) / b.h.max(1.0)).clamp(0.001, 1.0);
    let offset = desigma_doc::Vec2::new(b.x + b.w / 2.0 - tw / 2.0 / zoom, b.y + b.h / 2.0 - th / 2.0 / zoom);
    let mut content = sk::Pixmap::new(BOARD_THUMB.0, BOARD_THUMB.1)?;
    desigma_render::render_page(doc, page, &desigma_render::Viewport { offset, zoom, dpr: 1.0 }, &mut content);
    // the canvas colour and its dots go underneath (the page itself is transparent)
    let mut out = sk::Pixmap::new(BOARD_THUMB.0, BOARD_THUMB.1)?;
    let bg = crate::theme::BOARD_BG;
    out.fill(sk::Color::from_rgba8((bg >> 16) as u8, (bg >> 8) as u8, bg as u8, 255));
    let mut dot = sk::Paint::default();
    dot.set_color(sk::Color::from_rgba8(0, 0, 0, 46));
    let step = 16.0;
    let mut y = step / 2.0;
    while y < th {
        let mut x = step / 2.0;
        while x < tw {
            if let Some(r) = sk::Rect::from_xywh(x - 0.75, y - 0.75, 1.5, 1.5) {
                out.fill_rect(r, &dot, sk::Transform::identity(), None);
            }
            x += step;
        }
        y += step;
    }
    out.draw_pixmap(0, 0, content.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), None);
    out.encode_png().ok()
}

/// PNG bytes of the document's thumbnail, scaled to fit [`THUMB_PX`]; `None` for an empty document.
/// A board shows its whole first page on the dotted canvas instead of one frame.
pub fn thumbnail_png(doc: &Document) -> Option<Vec<u8>> {
    if doc.kind == DocumentKind::Board {
        return board_thumbnail_png(doc);
    }
    let id = thumbnail_node(doc)?;
    let size = doc.get(id)?.size;
    let longest = size.x.max(size.y);
    if !longest.is_finite() || longest < 1.0 {
        return None;
    }
    let scale = (THUMB_PX / longest).clamp(0.01, 2.0);
    let pm = desigma_render::render_node(doc, id, scale)?;
    pm.encode_png().ok()
}

/// Renders and stores the thumbnail now.
pub fn refresh(lib: &Library, file: &str, doc: &Document) -> bool {
    match thumbnail_png(doc) {
        Some(png) => lib.set_thumbnail(file, &png).is_ok(),
        None => false,
    }
}

struct ThumbnailJob { lib: Library, file: String, doc: Document, bytes: usize, cancel: std::sync::Arc<std::sync::atomic::AtomicBool> }
#[derive(Default)]
struct ThumbnailQueue {
    jobs: std::collections::VecDeque<ThumbnailJob>,
    bytes: usize,
    active: Option<(std::path::PathBuf, String, std::sync::Arc<std::sync::atomic::AtomicBool>)>,
}
type SharedQueue = std::sync::Arc<(std::sync::Mutex<ThumbnailQueue>, std::sync::Condvar)>;
fn queue() -> &'static SharedQueue {
    static QUEUE: std::sync::OnceLock<SharedQueue> = std::sync::OnceLock::new();
    QUEUE.get_or_init(|| {
        let shared = std::sync::Arc::new((std::sync::Mutex::new(ThumbnailQueue::default()),std::sync::Condvar::new()));
        let worker = shared.clone();
        std::thread::spawn(move || loop {
            let (lock, wake) = &*worker;
            let mut state = lock.lock().unwrap_or_else(|e|e.into_inner());
            while state.jobs.is_empty() { state=wake.wait(state).unwrap_or_else(|e|e.into_inner()); }
            let job = state.jobs.pop_front().unwrap(); state.bytes-=job.bytes;
            state.active=Some((job.lib.root().to_path_buf(),job.file.clone(),job.cancel.clone()));
            drop(state);
            let _=crate::cityos_jobs::run(false,Some(&job.cancel),|| {
                if let Some(png)=thumbnail_png(&job.doc) {
                    if !job.cancel.load(std::sync::atomic::Ordering::Acquire) { let _=job.lib.set_thumbnail(&job.file,&png); }
                }
                Ok(())
            });
            let mut state=lock.lock().unwrap_or_else(|e|e.into_inner());state.active=None;wake.notify_all();
        });
        shared
    })
}
/// One bounded, coalescing thumbnail worker shares the application's heavy-job budget.
/// Oversized optional thumbnails are omitted; the saved document remains intact.
pub fn refresh_in_background(lib: Library, file: String, doc: Document) {
    const BUDGET:usize=64*1024*1024;
    let bytes=doc.to_json().map(|s|s.len()).unwrap_or(BUDGET+1);
    if bytes>BUDGET{return;}
    let(lock,wake)=&**queue();let mut state=lock.lock().unwrap_or_else(|e|e.into_inner());
    if let Some((root,id,cancel))=&state.active {if root==lib.root()&&id==&file {cancel.store(true,std::sync::atomic::Ordering::Release);}}
    let mut index=0;
    while index<state.jobs.len() {if state.jobs[index].lib.root()==lib.root()&&state.jobs[index].file==file {
        let old=state.jobs.remove(index).unwrap();state.bytes-=old.bytes;
    }else{index+=1;}}
    while state.jobs.len()>=8 || state.bytes+bytes>BUDGET {let Some(old)=state.jobs.pop_front() else{break;};state.bytes-=old.bytes;}
    state.bytes+=bytes;state.jobs.push_back(ThumbnailJob{lib,file,doc,bytes,cancel:Default::default()});wake.notify_one();
}

#[cfg(test)]
pub(crate) fn finish_library_jobs(root:&std::path::Path) {
    let(lock,wake)=&**queue();let mut state=lock.lock().unwrap_or_else(|e|e.into_inner());
    state.jobs.retain(|job|job.lib.root()!=root);state.bytes=state.jobs.iter().map(|job|job.bytes).sum();
    if let Some((active,_,cancel))=&state.active {if active==root {cancel.store(true,std::sync::atomic::Ordering::Release);}}
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
    while state.active.as_ref().is_some_and(|(active,_,_)|active==root) {
        let remaining=deadline.saturating_duration_since(std::time::Instant::now());assert!(!remaining.is_zero(),"Thumbnail worker did not finish");
        state=wake.wait_timeout(state,remaining).unwrap_or_else(|e|e.into_inner()).0;
    }
}

/// Which files are boards, read from the documents (each file is probed once, and again after it
/// was modified). A file not probed yet counts as a design file for the frame.
#[derive(Default)]
pub struct Kinds {
    map: HashMap<String, (u64, DocumentKind)>,
}

impl Kinds {
    /// The kind of `entry`; at most `budget` files are read per call.
    pub fn get(&mut self, lib: &Library, entry: &desigma_library::FileEntry, budget: &mut u32) -> DocumentKind {
        if let Some((modified, kind)) = self.map.get(&entry.id) {
            if *modified == entry.modified {
                return *kind;
            }
        }
        if *budget == 0 {
            return self.map.get(&entry.id).map(|k| k.1).unwrap_or_default();
        }
        *budget -= 1;
        let kind = lib.file_kind(&entry.id).unwrap_or_default();
        self.map.insert(entry.id.clone(), (entry.modified, kind));
        kind
    }
}

/// Decoded thumbnails as GPU textures, reloaded when the PNG changes on disk.
#[derive(Default)]
pub struct Textures {
    map: HashMap<String, (Option<SystemTime>, Option<egui::TextureHandle>)>,
    touched: HashMap<String, u64>,
    serial: u64,
    bytes: usize,
}

impl Textures {
    fn prune(&mut self, limit: usize, entries: usize) {
        while self.bytes > limit || self.map.len() > entries {
            let Some(id)=self.touched.iter().min_by_key(|(_,time)|**time).map(|(id,_)|id.clone()) else {break;};
            self.touched.remove(&id);
            if let Some((_,Some(texture)))=self.map.remove(&id) {self.bytes=self.bytes.saturating_sub(texture.size()[0]*texture.size()[1]*4);}
        }
    }
    /// The texture for `file`'s thumbnail, loading it if new or changed. At most
    /// `budget` decodes happen per call so a big grid does not stall a frame.
    pub fn get(&mut self, ctx: &egui::Context, lib: &Library, entry: &desigma_library::FileEntry, budget: &mut u32) -> Option<(egui::TextureId, egui::Vec2)> {
        let path = lib.thumbnail_path(entry)?;
        self.serial=self.serial.wrapping_add(1);self.touched.insert(entry.id.clone(),self.serial);
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Some((seen, tex)) = self.map.get(&entry.id) {
            if *seen == mtime {
                return tex.as_ref().map(|t| (t.id(), t.size_vec2()));
            }
        }
        if *budget == 0 {
            ctx.request_repaint();
            return self.map.get(&entry.id).and_then(|(_, t)| t.as_ref().map(|t| (t.id(), t.size_vec2())));
        }
        *budget -= 1;
        let tex: Option<egui::TextureHandle> = (|| -> Option<image::DynamicImage> {
            use std::io::Read;
            let mut bytes=Vec::new();std::fs::File::open(&path).ok()?.take(4*1024*1024+1).read_to_end(&mut bytes).ok()?;
            if bytes.len()>4*1024*1024{return None;}
            let dimensions=image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
            if dimensions.0>1024||dimensions.1>1024{return None;}
            image::load_from_memory(&bytes).ok()
        })().map(|img| {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            ctx.load_texture(format!("thumb-{}", entry.id), egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), egui::TextureOptions::LINEAR)
        });
        let out = tex.as_ref().map(|t| (t.id(), t.size_vec2()));
        if let Some((_,Some(previous)))=self.map.remove(&entry.id){self.bytes=self.bytes.saturating_sub(previous.size()[0]*previous.size()[1]*4);}
        if let Some(texture)=&tex{self.bytes+=texture.size()[0]*texture.size()[1]*4;}
        self.map.insert(entry.id.clone(), (mtime, tex));
        self.prune(64*1024*1024,256);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{Node, NodeKind, Rect, Vec2};
    #[test]
    fn texture_budget_evicts_the_least_recently_used_handle() {
        let ctx=egui::Context::default();let mut textures=Textures::default();
        for(index,id)in ["old","recent","new"].into_iter().enumerate(){
            let texture=ctx.load_texture(id,egui::ColorImage::filled([64,64],egui::Color32::WHITE),Default::default());
            textures.bytes+=64*64*4;textures.map.insert(id.into(),(None,Some(texture)));textures.touched.insert(id.into(),index as u64);
        }
        textures.prune(2*64*64*4,256);
        assert!(!textures.map.contains_key("old"));assert!(textures.map.contains_key("recent"));
        assert_eq!(textures.bytes,2*64*64*4);
    }

    fn doc_with_frame() -> Document {
        let mut d = Document::new("t");
        let page = d.add_page("P");
        let rect = d.alloc_id();
        d.insert(page, Node::rectangle(rect, Rect::new(0.0, 0.0, 5.0, 5.0))).unwrap();
        let fid = d.alloc_id();
        let mut f = Node::new(fid, NodeKind::Frame(Default::default()));
        f.size = Vec2::new(960.0, 540.0);
        d.insert(page, f).unwrap();
        d
    }

    #[test]
    fn picks_the_first_frame_over_loose_layers() {
        let d = doc_with_frame();
        let id = thumbnail_node(&d).unwrap();
        assert!(matches!(d.get(id).unwrap().kind, NodeKind::Frame(_)));
    }

    #[test]
    fn png_fits_the_thumbnail_size() {
        let d = doc_with_frame();
        let png = thumbnail_png(&d).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.width(), THUMB_PX as u32);
        assert!((img.height() as f32 - 270.0).abs() <= 1.0);
    }

    #[test]
    fn empty_documents_have_no_thumbnail() {
        let mut d = Document::new("e");
        d.add_page("P");
        assert!(thumbnail_node(&d).is_none());
        assert!(thumbnail_png(&d).is_none());
    }
}
