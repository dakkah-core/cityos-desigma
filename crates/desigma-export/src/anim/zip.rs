//! PNG frame sequences: a folder of `name_0001.png` files, or the same files in a `.zip`
//! (stored, not deflated: PNG data is already compressed).

use super::{report, AnimError, Animation, ProgressFn};
use std::io::Write;
use std::path::Path;

fn file_name(base: &str, index: usize, total: usize) -> String {
    let digits = total.to_string().len().max(4);
    format!("{}_{:0digits$}.png", crate::sanitize_file_name(base), index + 1)
}

struct Entry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Fixed DOS timestamp (1980-01-01 00:00): keeps archives reproducible.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021;

/// Writes `anim` at `fps` as a zip of PNG files. Returns the number of frames.
pub fn write_png_zip<W: Write>(anim: &mut Animation, fps: u32, base: &str, mut w: W, mut progress: Option<ProgressFn<'_>>) -> Result<u32, AnimError> {
    let times = anim.frame_times(fps);
    let total = times.len();
    let mut entries: Vec<Entry> = Vec::with_capacity(total);
    let mut pos: u64 = 0;
    for (i, &t) in times.iter().enumerate() {
        let png = anim.render(t).encode_png().map_err(|e| AnimError::Encode(e.to_string()))?;
        let name = file_name(base, i, total);
        let mut crc = flate2::Crc::new();
        crc.update(&png);
        if pos + png.len() as u64 + 1024 > u32::MAX as u64 {
            return Err(AnimError::TooLarge("the PNG sequence is larger than a zip file can hold (4 GB); lower the scale or fps".into()));
        }
        let mut h = Vec::with_capacity(30 + name.len());
        h.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        put16(&mut h, 20); // version needed
        put16(&mut h, 0x0800); // utf-8 names
        put16(&mut h, 0); // stored
        put16(&mut h, DOS_TIME);
        put16(&mut h, DOS_DATE);
        put32(&mut h, crc.sum());
        put32(&mut h, png.len() as u32);
        put32(&mut h, png.len() as u32);
        put16(&mut h, name.len() as u16);
        put16(&mut h, 0);
        h.extend_from_slice(name.as_bytes());
        w.write_all(&h)?;
        w.write_all(&png)?;
        entries.push(Entry { name, crc: crc.sum(), size: png.len() as u32, offset: pos as u32 });
        pos += h.len() as u64 + png.len() as u64;
        report(&mut progress, i as u32 + 1, total as u32)?;
    }
    let mut dir = Vec::new();
    for e in &entries {
        dir.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
        put16(&mut dir, 20); // made by
        put16(&mut dir, 20); // needed
        put16(&mut dir, 0x0800);
        put16(&mut dir, 0);
        put16(&mut dir, DOS_TIME);
        put16(&mut dir, DOS_DATE);
        put32(&mut dir, e.crc);
        put32(&mut dir, e.size);
        put32(&mut dir, e.size);
        put16(&mut dir, e.name.len() as u16);
        put16(&mut dir, 0); // extra
        put16(&mut dir, 0); // comment
        put16(&mut dir, 0); // disk
        put16(&mut dir, 0); // internal attrs
        put32(&mut dir, 0); // external attrs
        put32(&mut dir, e.offset);
        dir.extend_from_slice(e.name.as_bytes());
    }
    let mut end = Vec::new();
    end.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
    put16(&mut end, 0);
    put16(&mut end, 0);
    put16(&mut end, entries.len().min(u16::MAX as usize) as u16);
    put16(&mut end, entries.len().min(u16::MAX as usize) as u16);
    put32(&mut end, dir.len() as u32);
    put32(&mut end, pos as u32);
    put16(&mut end, 0);
    w.write_all(&dir)?;
    w.write_all(&end)?;
    Ok(total as u32)
}

/// Writes the frames as numbered PNG files into `dir` (created if missing). Returns their paths.
pub fn write_png_dir(
    anim: &mut Animation,
    fps: u32,
    base: &str,
    dir: &Path,
    mut progress: Option<ProgressFn<'_>>,
) -> Result<Vec<std::path::PathBuf>, AnimError> {
    std::fs::create_dir_all(dir)?;
    let times = anim.frame_times(fps);
    let total = times.len();
    let mut paths = Vec::with_capacity(total);
    for (i, &t) in times.iter().enumerate() {
        let png = anim.render(t).encode_png().map_err(|e| AnimError::Encode(e.to_string()))?;
        let path = dir.join(file_name(base, i, total));
        std::fs::write(&path, png)?;
        paths.push(path);
        report(&mut progress, i as u32 + 1, total as u32)?;
    }
    Ok(paths)
}
