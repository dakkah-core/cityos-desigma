//! Host side: accepts guests, runs the handshake, fans messages in and out.
use crate::{
    auth::{ct_eq, Limiter},
    frame::{self, MAX_FRAME},
    proto::{clean_name, Hello, Reply, CLOSE_ENDED, CLOSE_KICKED, CLOSE_LEFT, CLOSE_LOST, CLOSE_REFUSED, MAX_HANDSHAKE, PROTOCOL_VERSION},
    rt::{self, EventQueue, Events},
    JoinError, NetError, Network, PeerId, Ticket,
};
use bytes::Bytes;
use iroh::{endpoint::Connection, Endpoint, EndpointId};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{sleep, timeout},
};

/// Per-peer outgoing queue length; a peer that falls this far behind is dropped.
const QUEUE: usize = 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_PASSWORD_FAILS: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);

/// A fresh random endpoint secret key, for [`HostOptions::secret_key`].
pub fn generate_secret_key() -> [u8; 32] {
    iroh::SecretKey::generate().to_bytes()
}

#[derive(Clone, Debug)]
pub struct HostOptions {
    /// Required from guests when set.
    pub password: Option<String>,
    /// Ask the app (via [`HostEvent::JoinRequest`]) before letting each guest in.
    pub require_approval: bool,
    pub network: Network,
    /// Guests allowed at once.
    pub max_peers: usize,
    /// Protocol version to announce; only tests change this.
    pub version: u32,
    /// Fixed endpoint identity (32 secret key bytes) so the ticket's endpoint id survives restarts;
    /// a fresh one is generated when `None`. For always-on hosts.
    pub secret_key: Option<[u8; 32]>,
    /// Fixed UDP port to listen on; a random free one when `None`.
    pub port: Option<u16>,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self { password: None, require_approval: false, network: Network::Internet, max_peers: 32, version: PROTOCOL_VERSION, secret_key: None, port: None }
    }
}

#[derive(Debug)]
pub enum HostEvent {
    /// Endpoint is bound and the ticket is final enough to share.
    Ready(Ticket),
    /// Binding failed; the host is dead.
    Failed(String),
    /// A guest waits for [`Host::respond`] with this request id.
    JoinRequest {
        request: u64,
        name: String,
        endpoint: EndpointId,
    },
    PeerJoined {
        peer: PeerId,
        name: String,
    },
    PeerLeft {
        peer: PeerId,
    },
    /// Reliable message from a guest.
    Message {
        peer: PeerId,
        bytes: Vec<u8>,
    },
    /// Unreliable presence datagram from a guest.
    Presence {
        peer: PeerId,
        bytes: Vec<u8>,
    },
    /// A guest was turned away (wrong password, version mismatch, declined, ...).
    JoinRefused {
        name: String,
        endpoint: EndpointId,
        error: JoinError,
    },
}

struct Slot {
    tx: mpsc::Sender<Arc<Vec<u8>>>,
    conn: Connection,
}

struct Inner {
    opts: HostOptions,
    events: Events<HostEvent>,
    peers: Mutex<HashMap<PeerId, Slot>>,
    next_peer: AtomicU32,
    next_request: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    /// Secrets of guests who may come back without being asked again.
    trusted: Mutex<HashSet<String>>,
    /// The `resume` secret each connected guest presented, until the app picks it up.
    presented: Mutex<HashMap<PeerId, String>>,
    limiter: Mutex<Limiter>,
    endpoint: OnceLock<Endpoint>,
    stop: watch::Sender<bool>,
}

/// A running session host. Dropping it ends the session.
pub struct Host {
    inner: Arc<Inner>,
    queue: EventQueue<HostEvent>,
    rt: Option<Runtime>,
    main: Option<JoinHandle<()>>,
}

impl Host {
    /// Start listening. Returns immediately; watch for [`HostEvent::Ready`] or call
    /// [`Host::wait_ticket`].
    pub fn start(opts: HostOptions) -> Result<Host, NetError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("desigma-net")
            .enable_all()
            .build()
            .map_err(|e| NetError::Bind(e.to_string()))?;
        let (events, queue) = rt::channel();
        let (stop, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            limiter: Mutex::new(Limiter::new(MAX_PASSWORD_FAILS, LOCKOUT)),
            opts,
            events,
            peers: Mutex::new(HashMap::new()),
            next_peer: AtomicU32::new(1),
            next_request: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            trusted: Mutex::new(HashSet::new()),
            presented: Mutex::new(HashMap::new()),
            endpoint: OnceLock::new(),
            stop,
        });
        let main = rt.spawn(run(inner.clone()));
        Ok(Host { inner, queue, rt: Some(rt), main: Some(main) })
    }

    /// Be nudged (e.g. `ctx.request_repaint()`) whenever an event is queued.
    pub fn set_wake(&self, f: Box<dyn Fn() + Send + Sync>) {
        self.queue.set_wake(f);
    }

    pub fn try_recv(&self) -> Option<HostEvent> {
        self.queue.try_recv()
    }

    pub fn drain(&self) -> Vec<HostEvent> {
        self.queue.drain()
    }

    /// Blocking receive for tests and headless tools; never call from the UI thread.
    pub fn recv_timeout(&self, d: Duration) -> Option<HostEvent> {
        self.queue.recv_timeout(d)
    }

    /// Current ticket once bound (addresses can change; ask again before re-sharing).
    pub fn ticket(&self) -> Option<Ticket> {
        self.inner.ticket()
    }

    /// Poll until the ticket exists. For tests and headless tools only.
    pub fn wait_ticket(&self, d: Duration) -> Option<Ticket> {
        let end = std::time::Instant::now() + d;
        loop {
            if let Some(t) = self.ticket() {
                return Some(t);
            }
            if std::time::Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Currently connected guests.
    pub fn peers(&self) -> Vec<PeerId> {
        let mut v: Vec<_> = self.inner.peers.lock().unwrap().keys().copied().collect();
        v.sort_unstable();
        v
    }

    /// Answer a [`HostEvent::JoinRequest`].
    pub fn respond(&self, request: u64, accept: bool) {
        if let Some(tx) = self.inner.pending.lock().unwrap().remove(&request) {
            let _ = tx.send(accept);
        }
    }

    /// Lets a guest who shows this `resume` secret in without asking again (approval and the
    /// password check still apply to everyone else). Call it for the secrets the sync layer
    /// hands out.
    pub fn allow_resume(&self, token: &str) {
        self.inner.trusted.lock().unwrap().insert(token.to_string());
    }

    pub fn revoke_resume(&self, token: &str) {
        self.inner.trusted.lock().unwrap().remove(token);
    }

    /// The `resume` secret `peer` presented when it joined, if any. Ask right after
    /// [`HostEvent::PeerJoined`]; it is handed out once.
    pub fn join_token(&self, peer: PeerId) -> Option<String> {
        self.inner.presented.lock().unwrap().remove(&peer)
    }

    /// Cuts a guest's connection the way a network failure would: the guest sees
    /// [`crate::DisconnectReason::Lost`], not a kick. For tests of reconnecting.
    #[doc(hidden)]
    pub fn sever(&self, peer: PeerId) {
        if let Some(slot) = self.inner.peers.lock().unwrap().get(&peer) {
            slot.conn.close(CLOSE_LOST.into(), b"connection lost");
        }
    }

    /// Reliable, ordered message to one guest.
    pub fn send(&self, peer: PeerId, bytes: Vec<u8>) -> Result<(), NetError> {
        self.inner.send_one(peer, Arc::new(bytes))
    }

    /// Reliable message to every guest except `except`.
    pub fn broadcast(&self, bytes: Vec<u8>, except: Option<PeerId>) {
        let bytes = Arc::new(bytes);
        let peers: Vec<PeerId> = self.inner.peers.lock().unwrap().keys().copied().collect();
        for p in peers.into_iter().filter(|p| Some(*p) != except) {
            let _ = self.inner.send_one(p, bytes.clone());
        }
    }

    /// Unreliable small message (cursors). Dropped if it does not fit one datagram.
    pub fn send_presence(&self, peer: PeerId, bytes: &[u8]) -> Result<(), NetError> {
        let peers = self.inner.peers.lock().unwrap();
        let slot = peers.get(&peer).ok_or(NetError::UnknownPeer)?;
        send_datagram(&slot.conn, bytes)
    }

    pub fn broadcast_presence(&self, bytes: &[u8], except: Option<PeerId>) {
        for (p, slot) in self.inner.peers.lock().unwrap().iter() {
            if Some(*p) != except {
                let _ = send_datagram(&slot.conn, bytes);
            }
        }
    }

    /// Disconnect a guest; it sees [`crate::DisconnectReason::Kicked`].
    pub fn kick(&self, peer: PeerId) {
        if let Some(slot) = self.inner.peers.lock().unwrap().remove(&peer) {
            slot.conn.close(CLOSE_KICKED.into(), b"kicked");
        }
    }

    /// End the session: guests see [`crate::DisconnectReason::HostEnded`]. Does not block.
    pub fn shutdown(&self) {
        self.inner.stop.send_replace(true);
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
        if let (Some(rt), Some(main)) = (self.rt.take(), self.main.take()) {
            // Give the close frames a moment to go out; bounded so the UI never hangs.
            rt.block_on(async {
                let _ = timeout(Duration::from_millis(1500), main).await;
            });
            rt.shutdown_background();
        }
    }
}

fn send_datagram(conn: &Connection, bytes: &[u8]) -> Result<(), NetError> {
    if !rt::presence_fits(conn, bytes.len()) {
        return Err(NetError::TooLarge(bytes.len()));
    }
    conn.send_datagram(Bytes::copy_from_slice(bytes)).map_err(|e| NetError::Io(e.to_string()))
}

impl Inner {
    fn ticket(&self) -> Option<Ticket> {
        let ep = self.endpoint.get()?;
        Some(Ticket { addr: ep.addr(), password_required: self.opts.password.is_some() })
    }

    fn send_one(&self, peer: PeerId, bytes: Arc<Vec<u8>>) -> Result<(), NetError> {
        let mut peers = self.peers.lock().unwrap();
        let slot = peers.get(&peer).ok_or(NetError::UnknownPeer)?;
        match slot.tx.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(NetError::Closed),
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Reliable lane overflow means the guest can never catch up: cut it loose
                // so it can rejoin from a fresh snapshot instead of silently missing edits.
                if let Some(slot) = peers.remove(&peer) {
                    slot.conn.close(CLOSE_KICKED.into(), b"too slow");
                }
                Err(NetError::Backlog)
            }
        }
    }
}

async fn run(inner: Arc<Inner>) {
    let ep = match rt::bind_with(inner.opts.network, true, inner.opts.secret_key, inner.opts.port).await {
        Ok(ep) => ep,
        Err(e) => {
            inner.events.emit(HostEvent::Failed(e.to_string()));
            return;
        }
    };
    if inner.opts.network == Network::Internet {
        // Wait briefly for the relay so the first ticket already carries it.
        let _ = timeout(Duration::from_secs(5), ep.online()).await;
    }
    let _ = inner.endpoint.set(ep.clone());
    if let Some(t) = inner.ticket() {
        inner.events.emit(HostEvent::Ready(t));
    }
    let mut stop = inner.stop.subscribe();
    loop {
        tokio::select! {
            inc = ep.accept() => match inc {
                Some(inc) => { tokio::spawn(handle_incoming(inner.clone(), inc)); }
                None => break,
            },
            _ = stop.wait_for(|s| *s) => break,
        }
    }
    let slots: Vec<Slot> = inner.peers.lock().unwrap().drain().map(|(_, s)| s).collect();
    for s in slots {
        s.conn.close(CLOSE_ENDED.into(), b"session ended");
    }
    ep.close().await;
}

async fn handle_incoming(inner: Arc<Inner>, incoming: iroh::endpoint::Incoming) {
    let Ok(accepting) = incoming.accept() else { return };
    let Ok(conn) = accepting.await else { return };
    let remote = conn.remote_id();
    let Ok(joined) = handshake(&inner, &conn, remote).await else {
        conn.close(CLOSE_REFUSED.into(), b"handshake failed");
        return;
    };
    let Joined { peer, name, mut rx, tx, resume } = joined;

    let (qtx, mut qrx) = mpsc::channel::<Arc<Vec<u8>>>(QUEUE);
    if let Some(t) = resume {
        inner.presented.lock().unwrap().insert(peer, t);
    }
    inner.peers.lock().unwrap().insert(peer, Slot { tx: qtx, conn: conn.clone() });
    inner.events.emit(HostEvent::PeerJoined { peer, name });

    let writer = {
        let conn = conn.clone();
        let mut tx = tx;
        tokio::spawn(async move {
            while let Some(msg) = qrx.recv().await {
                let sent = match frame::encode_async(msg).await {
                    Ok(wire) => frame::write_wire(&mut tx, &wire).await,
                    Err(e) => Err(e),
                };
                if sent.is_err() {
                    conn.close(CLOSE_LEFT.into(), b"write failed");
                    break;
                }
            }
        })
    };
    let datagrams = {
        let conn = conn.clone();
        let events = inner.events.clone();
        tokio::spawn(async move {
            while let Ok(d) = conn.read_datagram().await {
                events.emit(HostEvent::Presence { peer, bytes: d.to_vec() });
            }
        })
    };
    loop {
        tokio::select! {
            f = frame::read_frame(&mut rx, MAX_FRAME) => match f {
                Ok(Some(bytes)) => inner.events.emit(HostEvent::Message { peer, bytes }),
                _ => break,
            },
            _ = conn.closed() => break,
        }
    }
    writer.abort();
    datagrams.abort();
    inner.peers.lock().unwrap().remove(&peer);
    inner.presented.lock().unwrap().remove(&peer);
    conn.close(CLOSE_LEFT.into(), b"bye");
    inner.events.emit(HostEvent::PeerLeft { peer });
}

struct Joined {
    peer: PeerId,
    name: String,
    rx: iroh::endpoint::RecvStream,
    tx: iroh::endpoint::SendStream,
    resume: Option<String>,
}

/// Run the handshake. On `Err` the guest was refused (and told) or vanished.
async fn handshake(inner: &Arc<Inner>, conn: &Connection, remote: EndpointId) -> Result<Joined, ()> {
    let (mut tx, mut rx) = timeout(HANDSHAKE_TIMEOUT, conn.accept_bi()).await.map_err(|_| ())?.map_err(|_| ())?;
    let raw = timeout(HANDSHAKE_TIMEOUT, frame::read_frame(&mut rx, MAX_HANDSHAKE)).await.map_err(|_| ())?.map_err(|_| ())?.ok_or(())?;
    let hello: Hello = serde_json::from_slice(&raw).map_err(|_| ())?;
    let name = clean_name(&hello.name);

    if hello.version != inner.opts.version {
        let error = JoinError::VersionMismatch { host: inner.opts.version, guest: hello.version };
        return refuse(inner, conn, tx, name, remote, error).await;
    }
    let locked = inner.limiter.lock().unwrap().check(&remote);
    if let Err(left) = locked {
        return refuse(inner, conn, tx, name, remote, JoinError::Locked { retry_secs: left.as_secs() + 1 }).await;
    }
    if let Some(pw) = &inner.opts.password {
        if !ct_eq(pw, hello.password.as_deref().unwrap_or("")) {
            let delay = inner.limiter.lock().unwrap().fail(remote);
            sleep(delay).await;
            return refuse(inner, conn, tx, name, remote, JoinError::WrongPassword).await;
        }
        inner.limiter.lock().unwrap().success(&remote);
    }
    let trusted = hello.resume.as_ref().is_some_and(|t| inner.trusted.lock().unwrap().contains(t));
    if inner.peers.lock().unwrap().len() >= inner.opts.max_peers && !trusted {
        return refuse(inner, conn, tx, name, remote, JoinError::Full).await;
    }
    if inner.opts.require_approval && !trusted {
        let request = inner.next_request.fetch_add(1, Ordering::Relaxed);
        let (atx, arx) = oneshot::channel();
        inner.pending.lock().unwrap().insert(request, atx);
        inner.events.emit(HostEvent::JoinRequest { request, name: name.clone(), endpoint: remote });
        let mut stop = inner.stop.subscribe();
        let verdict = tokio::select! {
            v = timeout(APPROVAL_TIMEOUT, arx) => matches!(v, Ok(Ok(true))),
            _ = conn.closed() => { inner.pending.lock().unwrap().remove(&request); return Err(()); }
            _ = stop.wait_for(|s| *s) => false,
        };
        inner.pending.lock().unwrap().remove(&request);
        if !verdict {
            return refuse(inner, conn, tx, name, remote, JoinError::Denied).await;
        }
    }
    let peer = inner.next_peer.fetch_add(1, Ordering::Relaxed);
    let reply = serde_json::to_vec(&Reply::Welcome { peer }).map_err(|_| ())?;
    let wire = frame::encode(&reply).map_err(|_| ())?;
    frame::write_wire(&mut tx, &wire).await.map_err(|_| ())?;
    Ok(Joined { peer, name, rx, tx, resume: hello.resume })
}

/// Tell the guest why, record it, and wait for the guest to hang up.
async fn refuse(
    inner: &Arc<Inner>,
    conn: &Connection,
    mut tx: iroh::endpoint::SendStream,
    name: String,
    remote: EndpointId,
    error: JoinError,
) -> Result<Joined, ()> {
    let reply = serde_json::to_vec(&Reply::Refused(error.clone())).unwrap_or_default();
    if let Ok(wire) = frame::encode(&reply) {
        let _ = frame::write_wire(&mut tx, &wire).await;
        let _ = tx.finish();
    }
    inner.events.emit(HostEvent::JoinRefused { name, endpoint: remote, error });
    // Let the guest read the answer and hang up first; closing now could drop it.
    let _ = timeout(Duration::from_secs(5), conn.closed()).await;
    Err(())
}
