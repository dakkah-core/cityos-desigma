//! Serde helper (`#[serde(with = "desigma_collab::b64")]`): bytes travel as a base64 string, not
//! as a JSON array of numbers (about 3.5 bytes of JSON per byte against 1.33).

use data_encoding::BASE64;
use serde::{de::Error, Deserialize, Deserializer, Serializer};

#[allow(clippy::ptr_arg)]
pub fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&BASE64.encode(bytes))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let text = String::deserialize(d)?;
    BASE64.decode(text.as_bytes()).map_err(Error::custom)
}
