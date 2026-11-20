//! Length-prefixed frames: `u32 BE len | u8 flags | body`, `len` counts flags + body.
//! Bodies of 4 KiB and up are deflated when that makes them smaller.
use crate::NetError;
use iroh::endpoint::{ReadExactError, RecvStream, SendStream};
use std::io::{Read, Write};

/// Largest frame body, before and after compression (snapshots can be big).
pub const MAX_FRAME: usize = 64 * 1024 * 1024;
/// Smallest body worth compressing.
const COMPRESS_MIN: usize = 4096;
/// Bodies above this are (de)compressed off the async workers.
const BLOCKING_MIN: usize = 256 * 1024;
const FLAG_DEFLATE: u8 = 1;
/// Deflate level. Not the fastest one: some zlib backends only use fixed Huffman codes at level 1,
/// which cannot shrink base64 text (images and fonts travel as base64 in JSON) at all.
const LEVEL: u32 = 4;

/// Encode one frame (header included).
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, NetError> {
    if payload.len() > MAX_FRAME {
        return Err(NetError::TooLarge(payload.len()));
    }
    let mut flags = 0u8;
    let mut packed = None;
    if payload.len() >= COMPRESS_MIN {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::with_capacity(payload.len() / 2), flate2::Compression::new(LEVEL));
        if enc.write_all(payload).is_ok() {
            if let Ok(out) = enc.finish() {
                if out.len() < payload.len() {
                    flags |= FLAG_DEFLATE;
                    packed = Some(out);
                }
            }
        }
    }
    let body = packed.as_deref().unwrap_or(payload);
    let mut out = Vec::with_capacity(5 + body.len());
    out.extend_from_slice(&(body.len() as u32 + 1).to_be_bytes());
    out.push(flags);
    out.extend_from_slice(body);
    Ok(out)
}

/// Decode a frame body given its flags.
fn decode(flags: u8, body: Vec<u8>) -> Result<Vec<u8>, NetError> {
    match flags {
        0 => Ok(body),
        FLAG_DEFLATE => {
            let mut out = Vec::new();
            let mut dec = flate2::read::DeflateDecoder::new(body.as_slice()).take(MAX_FRAME as u64 + 1);
            dec.read_to_end(&mut out).map_err(|e| NetError::Protocol(format!("bad compressed frame: {e}")))?;
            if out.len() > MAX_FRAME {
                return Err(NetError::TooLarge(out.len()));
            }
            Ok(out)
        }
        f => Err(NetError::Protocol(format!("unknown frame flags {f:#x}"))),
    }
}

/// Encode off the async workers when the payload is big.
pub async fn encode_async(payload: std::sync::Arc<Vec<u8>>) -> Result<Vec<u8>, NetError> {
    if payload.len() >= BLOCKING_MIN {
        tokio::task::spawn_blocking(move || encode(&payload)).await.map_err(|e| NetError::Protocol(e.to_string()))?
    } else {
        encode(&payload)
    }
}

/// Write an already encoded frame.
pub async fn write_wire(w: &mut SendStream, wire: &[u8]) -> Result<(), NetError> {
    w.write_all(wire).await.map_err(|e| NetError::Io(e.to_string()))
}

/// Read one frame; `Ok(None)` on a clean end of stream. `max` caps the body size.
pub async fn read_frame(r: &mut RecvStream, max: usize) -> Result<Option<Vec<u8>>, NetError> {
    let mut hdr = [0u8; 4];
    match r.read_exact(&mut hdr).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(NetError::Io(e.to_string())),
    }
    let len = u32::from_be_bytes(hdr) as usize;
    if len == 0 {
        return Err(NetError::Protocol("empty frame".into()));
    }
    // Compressed bodies are never larger than `max` either; decoding is capped separately.
    if len - 1 > max {
        return Err(NetError::TooLarge(len - 1));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.map_err(|e| NetError::Io(e.to_string()))?;
    let flags = buf[0];
    buf.remove(0);
    let out = if len > BLOCKING_MIN && flags != 0 {
        tokio::task::spawn_blocking(move || decode(flags, buf)).await.map_err(|e| NetError::Protocol(e.to_string()))??
    } else {
        decode(flags, buf)?
    };
    if out.len() > max {
        return Err(NetError::TooLarge(out.len()));
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_plain_and_deflated() {
        for payload in [vec![], b"hi".to_vec(), vec![7u8; 100_000]] {
            let wire = encode(&payload).unwrap();
            let len = u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize;
            assert_eq!(len, wire.len() - 4);
            let back = decode(wire[4], wire[5..].to_vec()).unwrap();
            assert_eq!(back, payload);
        }
        assert!(encode(&vec![7u8; 100_000]).unwrap().len() < 1000);
    }

    #[test]
    fn rejects_bad_flags_and_garbage() {
        assert!(decode(0x80, vec![1]).is_err());
        assert!(decode(FLAG_DEFLATE, vec![0xff, 0xff, 0xff]).is_err());
    }
}
