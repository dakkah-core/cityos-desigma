//! Live collaboration core: pure, synchronous, no network. See `docs/collaboration.md`.
//!
//! Edits are found by diffing the document against a shadow copy of the last synced state
//! ([`diff`]) and expressed as [`Op`]s. The host ([`HostSession`]) validates, orders and
//! broadcasts them; guests ([`GuestSession`]) apply their own edits at once, hold back remote
//! writes to fields (and keyed entries) with unacknowledged local changes, and settle on the host's order. A
//! transport only has to carry [`ClientMsg`] / [`ServerMsg`] as JSON.

pub mod b64;
mod diff;
mod entries;
mod guest;
mod host;
mod msg;
mod ops;
mod rebase;
#[cfg(test)]
mod sim;
pub mod wire;

pub use diff::diff;
pub use entries::{entries, Entry, ENTRY_FIELDS};
pub use guest::{GuestSession, Received};
pub use host::HostSession;
pub use msg::*;
pub use ops::{apply, doc_fields, ApplyError, Op, DOC_FIELDS, LWW_FIELDS};
pub use rebase::rebase_history;
