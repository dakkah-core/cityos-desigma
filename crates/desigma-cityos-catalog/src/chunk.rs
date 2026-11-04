//! Bounded reassembly of `Materialize` responses using the reader's chunk protocol.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;
use sha2::{Digest, Sha256};

const CHUNK_BYTES: usize = 1024 * 1024;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = CHUNK_BYTES.div_ceil(3) * 4;
const MAX_DEPTH: usize = 64;

/// Reassemble a chunked `result`; `fetch` obtains the result for a byte offset.
/// The caller must handle direct, non-chunked materialization results separately
/// and maintain the same reader session, generation, and entry for each fetch.
pub fn reassemble<F>(first: Value, mut fetch: F) -> Result<Value, String>
where
    F: FnMut(usize) -> Result<Value, String>,
{
    let mut current = first;
    let mut bytes = Vec::new();
    let mut expected_total: Option<usize> = None;
    let mut expected_digest: Option<String> = None;

    loop {
        let obj = current.as_object().ok_or("Invalid catalog chunk")?;
        if obj.get("encoding").and_then(Value::as_str) != Some("base64-json") {
            return Err("Invalid catalog chunk encoding".into());
        }
        let number = |name: &str| -> Result<usize, String> {
            obj.get(name)
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| format!("Invalid catalog chunk {name}"))
        };
        let offset = number("offset")?;
        let total = number("totalBytes")?;
        let digest = obj.get("sha256").and_then(Value::as_str).ok_or("Invalid catalog chunk digest")?;
        if total == 0 || total > MAX_BYTES || offset != bytes.len() || offset % CHUNK_BYTES != 0 || offset >= total {
            return Err("Invalid catalog chunk bounds".into());
        }
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err("Invalid catalog chunk digest".into());
        }
        if expected_total.is_some_and(|value| value != total)
            || expected_digest.as_deref().is_some_and(|value| value != digest)
        {
            return Err("Catalog chunks changed during materialization".into());
        }
        if expected_total.is_none() {
            expected_total = Some(total);
            expected_digest = Some(digest.to_owned());
            bytes.reserve(total);
        }
        let encoded = obj.get("chunk").and_then(Value::as_str).ok_or("Invalid catalog chunk data")?;
        let required = (total - offset).min(CHUNK_BYTES);
        let padding = match required % 3 {
            1 => "==",
            2 => "=",
            _ => "",
        };
        if encoded.len() > MAX_ENCODED_BYTES
            || encoded.len() != required.div_ceil(3) * 4
            || !encoded.ends_with(padding)
        {
            return Err("Invalid catalog chunk base64 length".into());
        }
        // STANDARD requires canonical padding and rejects non-alphabet/trailing bits.
        let decoded = STANDARD.decode(encoded).map_err(|_| "Invalid catalog chunk base64")?;
        if decoded.len() != required {
            return Err("Invalid catalog chunk length".into());
        }
        let end = offset + decoded.len();
        let next = obj.get("nextOffset").ok_or("Missing catalog chunk nextOffset")?;
        if end < total {
            if next.as_u64().and_then(|n| usize::try_from(n).ok()) != Some(end) {
                return Err("Invalid catalog chunk nextOffset".into());
            }
        } else if !next.is_null() {
            return Err("Invalid catalog chunk nextOffset".into());
        }
        bytes.extend_from_slice(&decoded);
        if end == total {
            break;
        }
        current = fetch(end)?;
    }

    let actual = format!("{:x}", Sha256::digest(&bytes));
    if expected_digest.as_deref() != Some(actual.as_str()) {
        return Err("Catalog chunk digest mismatch".into());
    }
    check_depth(&bytes)?;
    serde_json::from_slice(&bytes).map_err(|error| format!("Invalid catalog chunk JSON: {error}"))
}

fn check_depth(bytes: &[u8]) -> Result<(), String> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        return Err("Catalog JSON depth exceeds 64".into());
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chunk(bytes: &[u8], offset: usize) -> Value {
        let end = (offset + CHUNK_BYTES).min(bytes.len());
        json!({"encoding":"base64-json", "chunk":STANDARD.encode(&bytes[offset..end]),
            "offset":offset, "totalBytes":bytes.len(), "sha256":format!("{:x}", Sha256::digest(bytes)),
            "nextOffset":if end < bytes.len() { Some(end) } else { None }})
    }

    fn one(bytes: &[u8]) -> Result<Value, String> {
        reassemble(chunk(bytes, 0), |_| Err("unexpected fetch".into()))
    }

    #[test]
    fn reassembles_reader_chunks_and_requests_exact_offset() {
        let original = json!({"text":"x".repeat(CHUNK_BYTES), "value":42});
        let bytes = serde_json::to_vec(&original).unwrap();
        let mut offsets = Vec::new();
        let value = reassemble(chunk(&bytes, 0), |offset| {
            offsets.push(offset);
            Ok(chunk(&bytes, offset))
        }).unwrap();
        assert_eq!(value, original);
        assert_eq!(offsets, vec![CHUNK_BYTES]);
    }

    #[test]
    fn rejects_malformed_bounds_encoding_and_next_offset() {
        let bytes = br#"{"ok":true}"#;
        for (field, value) in [
            ("encoding", json!("other")), ("offset", json!(1)), ("offset", json!(-1)),
            ("totalBytes", json!(0)), ("totalBytes", json!(MAX_BYTES + 1)),
            ("totalBytes", json!(bytes.len() + 1)), ("nextOffset", json!(1)),
            ("chunk", json!("e30")), ("chunk", json!("e31=")),
            ("chunk", json!("!")), ("sha256", json!("A".repeat(64))),
        ] {
            let mut response = chunk(bytes, 0);
            response[field] = value;
            assert!(reassemble(response, |_| panic!("unexpected fetch")).is_err(), "{field}");
        }
        let mut missing = chunk(bytes, 0);
        missing.as_object_mut().unwrap().remove("nextOffset");
        assert!(reassemble(missing, |_| panic!("unexpected fetch")).is_err());
        let mut oversized = chunk(bytes, 0);
        oversized["chunk"] = json!("A".repeat(MAX_ENCODED_BYTES + 4));
        assert!(reassemble(oversized, |_| panic!("unexpected fetch")).is_err());
    }

    #[test]
    fn rejects_changed_or_inconsistent_continuations_and_propagates_fetch_error() {
        let bytes = serde_json::to_vec(&json!("x".repeat(CHUNK_BYTES))).unwrap();
        let mut short_first = chunk(&bytes, 0);
        short_first["chunk"] = json!(STANDARD.encode(&bytes[..CHUNK_BYTES - 1]));
        assert!(reassemble(short_first, |_| panic!("unexpected fetch")).is_err());
        let mut wrong_next = chunk(&bytes, 0);
        wrong_next["nextOffset"] = json!(CHUNK_BYTES - 1);
        assert!(reassemble(wrong_next, |_| panic!("unexpected fetch")).is_err());
        for (field, value) in [
            ("offset", json!(0)), ("totalBytes", json!(bytes.len() + 1)),
            ("sha256", json!("0".repeat(64))), ("nextOffset", json!(CHUNK_BYTES)),
            ("chunk", json!("")),
        ] {
            assert!(reassemble(chunk(&bytes, 0), |offset| {
                let mut next = chunk(&bytes, offset);
                next[field] = value.clone();
                Ok(next)
            }).is_err(), "{field}");
        }
        assert_eq!(reassemble(chunk(&bytes, 0), |_| Err("reader stopped".into())).unwrap_err(), "reader stopped");
    }

    #[test]
    fn base64_requires_padding_and_zero_trailing_bits() {
        assert_eq!(one(b"{}").unwrap(), json!({}));
        for encoded in ["e30", "e31=", "e30!", "e30=\n"] {
            let mut response = chunk(b"{}", 0);
            response["chunk"] = json!(encoded);
            assert!(reassemble(response, |_| panic!("unexpected fetch")).is_err(), "{encoded:?}");
        }
    }

    #[test]
    fn verifies_digest_before_parsing_and_rejects_invalid_json() {
        let mut response = chunk(b"not json", 0);
        response["sha256"] = json!("0".repeat(64));
        assert_eq!(reassemble(response, |_| panic!("unexpected fetch")).unwrap_err(), "Catalog chunk digest mismatch");
        assert!(one(b"not json").unwrap_err().contains("Invalid catalog chunk JSON"));
        let mut changed = chunk(br#"{"a":1}"#, 0);
        changed["chunk"] = json!(STANDARD.encode(br#"{"a":2}"#));
        assert!(reassemble(changed, |_| panic!("unexpected fetch")).is_err());
    }

    #[test]
    fn enforces_depth_and_ignores_escaped_string_delimiters() {
        let valid = format!("{}0{}", "[".repeat(64), "]".repeat(64));
        assert!(one(valid.as_bytes()).is_ok());
        let deep = format!("{}0{}", "[".repeat(65), "]".repeat(65));
        assert_eq!(one(deep.as_bytes()).unwrap_err(), "Catalog JSON depth exceeds 64");
        let quoted = serde_json::to_vec(&json!({"text":format!("\"\\{}", "[".repeat(100))})).unwrap();
        assert!(one(&quoted).is_ok());
    }
}
