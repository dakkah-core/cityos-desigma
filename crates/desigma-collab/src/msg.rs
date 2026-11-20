//! Messages between host and guests. Plain serde types; a transport carries them as JSON.

use crate::{Entry, Op};
use desigma_doc::{Document, Node, NodeId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Peer index: host 0, guests 1+. Also the id range of the peer (`peer << 40`).
pub type PeerId = u32;

/// What a guest may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Editor,
    Viewer,
}

/// Guest to host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// A batch of local changes; `seq` counts the guest's batches from 1.
    Ops { seq: u64, ops: Vec<Op> },
    /// Ask for the current state of nodes (with their subtrees); answered with a
    /// [`ServerMsg::Fetched`]. A guest does this for nodes the host moved but that it deleted
    /// locally together with a subtree the node was moved out of. `deleting` are the roots of
    /// the guest's unacknowledged deletes; what lies under them is not sent.
    Fetch { ids: Vec<NodeId>, deleting: Vec<NodeId> },
    /// Ask for the bytes of an embedded image.
    ImageRequest { hash: String },
    /// Bytes of an image, sent on the host's request or in answer to one.
    ImageData {
        hash: String,
        #[serde(with = "crate::b64")]
        bytes: Vec<u8>,
    },
}

/// What the guest's `local_changes` produces.
pub type OutMsg = ClientMsg;

/// Host to guest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Join: the document without image bytes (image entries are empty), the sequence number
    /// it is current as of, and the id range index assigned to the guest.
    Snapshot {
        doc: Box<Document>,
        seq: u64,
        peer: PeerId,
    },
    /// Accepted ops of another peer (`from`), with their global sequence number.
    Ops {
        seq: u64,
        from: PeerId,
        ops: Vec<Op>,
    },
    /// The host handled the guest's batch `client_seq`. `rejected` lists indices of its ops the
    /// host refused; a [`ServerMsg::Correction`] follows. `orders` is the host's child order of
    /// every parent the batch touched, which settles sibling order the guest got wrong while
    /// its own edits were in flight. `doc` is the host's value of every whole-value document field
    /// the batch wrote, and `entries` its value of every keyed entry the batch wrote, for the
    /// same reason.
    Ack {
        client_seq: u64,
        seq: u64,
        rejected: Vec<usize>,
        orders: Vec<(NodeId, Vec<NodeId>)>,
        doc: BTreeMap<String, Value>,
        entries: Vec<Entry>,
    },
    /// The current state of what rejected ops touched.
    Correction {
        client_seq: u64,
        fixes: Vec<Fix>,
    },
    /// Answer to [`ClientMsg::Fetch`].
    Fetched {
        fixes: Vec<Fix>,
        orders: Vec<(NodeId, Vec<NodeId>)>,
    },
    ImageRequest {
        hash: String,
    },
    ImageData {
        hash: String,
        #[serde(with = "crate::b64")]
        bytes: Vec<u8>,
    },
    /// The secret that lets this guest rejoin as the same peer after a lost connection (see
    /// [`crate::HostSession::resume`]). Sent after the snapshot.
    Resume {
        token: String,
    },
}

/// Authoritative state of something a rejected op touched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Fix {
    /// The node as the host has it (no children), and the sibling before it.
    Node { node: Box<Node>, after: Option<NodeId> },
    /// The node does not exist (with its subtree).
    Gone(NodeId),
    /// Current values of whole-value document fields.
    Doc(BTreeMap<String, Value>),
    /// Current values of keyed document entries.
    Entries(Vec<Entry>),
}

/// Who receives a host message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    Peer(PeerId),
    /// Every guest except this one.
    AllBut(PeerId),
    /// Every guest.
    All,
}

/// A message the host wants delivered.
#[derive(Clone, Debug)]
pub struct Out {
    pub to: Dest,
    pub msg: ServerMsg,
}
