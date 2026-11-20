//! Guest side: dials the host from a ticket, runs the handshake, exchanges messages.
use crate::{
    frame::{self, MAX_FRAME},
    proto::{Hello, Reply, CLOSE_ENDED, CLOSE_KICKED, CLOSE_LEFT, MAX_HANDSHAKE, PROTOCOL_VERSION},
    rt::{self, EventQueue, Events},
    JoinError, NetError, Network, PeerId, Ticket, ALPN,
};
use bytes::Bytes;
use iroh::endpoint::{Connection, ConnectionError};
use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, watch},
    task::JoinHandle,
    time::timeout,
};

const QUEUE: usize = 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The host may sit on an approval prompt for a while.
const REPLY_TIMEOUT: Duration = Duration::from_secs(150);

#[derive(Clone, Debug)]
pub struct GuestOptions {
    pub network: Network,
    /// Protocol version to announce; only tests change this.
    pub version: u32,
}

impl Default for GuestOptions {
    fn default() -> Self {
        Self { network: Network::Internet, version: PROTOCOL_VERSION }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    /// The host removed you.
    Kicked,
    /// The host closed the session.
    HostEnded,
    /// You called [`Guest::leave`].
    Left,
    /// Network failure or the host vanished.
    Lost(String),
}

#[derive(Debug)]
pub enum GuestEvent {
    /// Accepted; this is your peer index.
    Joined {
        peer: PeerId,
    },
    /// Refused or unreachable; the guest is dead.
    JoinFailed(JoinError),
    /// Reliable message from the host.
    Message(Vec<u8>),
    /// Unreliable presence datagram from the host.
    Presence(Vec<u8>),
    Disconnected(DisconnectReason),
}

type JoinState = Arc<(Mutex<Option<Result<PeerId, JoinError>>>, Condvar)>;

struct Shared {
    events: Events<GuestEvent>,
    state: JoinState,
    conn: Mutex<Option<Connection>>,
    stop: watch::Sender<bool>,
}

/// A connection to a session host. Dropping it leaves the session.
pub struct Guest {
    shared: Arc<Shared>,
    queue: EventQueue<GuestEvent>,
    tx: mpsc::Sender<Arc<Vec<u8>>>,
    rt: Option<Runtime>,
    main: Option<JoinHandle<()>>,
}

impl Guest {
    /// Start joining. Returns immediately; watch for [`GuestEvent::Joined`] /
    /// [`GuestEvent::JoinFailed`], or call [`Guest::wait_joined`].
    pub fn join(ticket: &Ticket, name: &str, password: Option<&str>, opts: GuestOptions) -> Result<Guest, NetError> {
        Guest::join_resume(ticket, name, password, opts, None)
    }

    /// [`Guest::join`] that shows the host a `resume` secret from an earlier visit (after a lost
    /// connection). See [`Host::join_token`](crate::Host::join_token).
    pub fn join_resume(ticket: &Ticket, name: &str, password: Option<&str>, opts: GuestOptions, resume: Option<String>) -> Result<Guest, NetError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("desigma-net")
            .enable_all()
            .build()
            .map_err(|e| NetError::Bind(e.to_string()))?;
        let (events, queue) = rt::channel();
        let (stop, _) = watch::channel(false);
        let (tx, rx) = mpsc::channel(QUEUE);
        let shared = Arc::new(Shared { events, state: Arc::new((Mutex::new(None), Condvar::new())), conn: Mutex::new(None), stop });
        let hello = Hello { version: opts.version, name: name.to_string(), password: password.map(str::to_string), resume };
        let main = rt.spawn(run(shared.clone(), ticket.clone(), hello, opts.network, rx));
        Ok(Guest { shared, queue, tx, rt: Some(rt), main: Some(main) })
    }

    /// Join and wait for the verdict. For tests and headless tools; never on the UI thread.
    pub fn join_blocking(ticket: &Ticket, name: &str, password: Option<&str>, opts: GuestOptions, wait: Duration) -> Result<(Guest, PeerId), JoinError> {
        let g = Guest::join(ticket, name, password, opts).map_err(|e| JoinError::Connect(e.to_string()))?;
        let peer = g.wait_joined(wait)?;
        Ok((g, peer))
    }

    /// Block until accepted or refused. For tests and headless tools only.
    pub fn wait_joined(&self, wait: Duration) -> Result<PeerId, JoinError> {
        let (lock, cv) = &*self.shared.state;
        let g = lock.lock().unwrap();
        let (g, _) = cv.wait_timeout_while(g, wait, |s| s.is_none()).unwrap();
        g.clone().unwrap_or(Err(JoinError::Timeout))
    }

    /// Our peer index once accepted.
    pub fn peer(&self) -> Option<PeerId> {
        self.shared.state.0.lock().unwrap().clone().and_then(Result::ok)
    }

    pub fn set_wake(&self, f: Box<dyn Fn() + Send + Sync>) {
        self.queue.set_wake(f);
    }

    pub fn try_recv(&self) -> Option<GuestEvent> {
        self.queue.try_recv()
    }

    pub fn drain(&self) -> Vec<GuestEvent> {
        self.queue.drain()
    }

    /// Blocking receive for tests and headless tools; never call from the UI thread.
    pub fn recv_timeout(&self, d: Duration) -> Option<GuestEvent> {
        self.queue.recv_timeout(d)
    }

    /// Reliable, ordered message to the host.
    pub fn send(&self, bytes: Vec<u8>) -> Result<(), NetError> {
        self.tx.try_send(Arc::new(bytes)).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => NetError::Backlog,
            mpsc::error::TrySendError::Closed(_) => NetError::Closed,
        })
    }

    /// Unreliable small message (cursors). Dropped if it does not fit one datagram.
    pub fn send_presence(&self, bytes: &[u8]) -> Result<(), NetError> {
        let conn = self.shared.conn.lock().unwrap();
        let conn = conn.as_ref().ok_or(NetError::Closed)?;
        if !rt::presence_fits(conn, bytes.len()) {
            return Err(NetError::TooLarge(bytes.len()));
        }
        conn.send_datagram(Bytes::copy_from_slice(bytes)).map_err(|e| NetError::Io(e.to_string()))
    }

    /// Disconnect; a [`GuestEvent::Disconnected`] with [`DisconnectReason::Left`] follows.
    pub fn leave(&self) {
        self.shared.stop.send_replace(true);
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        self.leave();
        if let (Some(rt), Some(main)) = (self.rt.take(), self.main.take()) {
            rt.block_on(async {
                let _ = timeout(Duration::from_millis(1500), main).await;
            });
            rt.shutdown_background();
        }
    }
}

fn settle(shared: &Shared, result: Result<PeerId, JoinError>) {
    let (lock, cv) = &*shared.state;
    *lock.lock().unwrap() = Some(result);
    cv.notify_all();
}

async fn run(shared: Arc<Shared>, ticket: Ticket, hello: Hello, network: Network, mut outbox: mpsc::Receiver<Arc<Vec<u8>>>) {
    let mut stop = shared.stop.subscribe();
    let ep = match rt::bind(network, false).await {
        Ok(ep) => ep,
        Err(e) => {
            fail(&shared, JoinError::Connect(e.to_string()));
            return;
        }
    };
    let joined = tokio::select! {
        r = dial(&ep, &ticket, &hello) => r,
        _ = stop.wait_for(|s| *s) => Err(JoinError::Connect("cancelled".into())),
    };
    let (conn, mut tx, mut rx, peer) = match joined {
        Ok(j) => j,
        Err(e) => {
            fail(&shared, e);
            ep.close().await;
            return;
        }
    };
    *shared.conn.lock().unwrap() = Some(conn.clone());
    settle(&shared, Ok(peer));
    shared.events.emit(GuestEvent::Joined { peer });

    let writer = {
        let conn = conn.clone();
        tokio::spawn(async move {
            while let Some(msg) = outbox.recv().await {
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
        let events = shared.events.clone();
        tokio::spawn(async move {
            while let Ok(d) = conn.read_datagram().await {
                events.emit(GuestEvent::Presence(d.to_vec()));
            }
        })
    };
    let mut left = false;
    loop {
        tokio::select! {
            f = frame::read_frame(&mut rx, MAX_FRAME) => match f {
                Ok(Some(bytes)) => shared.events.emit(GuestEvent::Message(bytes)),
                _ => break,
            },
            _ = conn.closed() => break,
            _ = stop.wait_for(|s| *s) => { left = true; break; }
        }
    }
    writer.abort();
    datagrams.abort();
    // Read the close reason before our own close overwrites it.
    let reason = if left {
        DisconnectReason::Left
    } else {
        match conn.close_reason() {
            Some(ConnectionError::ApplicationClosed(ac)) => match ac.error_code.into_inner() {
                c if c == u64::from(CLOSE_KICKED) => DisconnectReason::Kicked,
                c if c == u64::from(CLOSE_ENDED) => DisconnectReason::HostEnded,
                _ => DisconnectReason::Lost(String::from_utf8_lossy(&ac.reason).into_owned()),
            },
            Some(e) => DisconnectReason::Lost(e.to_string()),
            None => DisconnectReason::Lost("connection broke".into()),
        }
    };
    conn.close(CLOSE_LEFT.into(), b"bye");
    *shared.conn.lock().unwrap() = None;
    shared.events.emit(GuestEvent::Disconnected(reason));
    ep.close().await;
}

fn fail(shared: &Shared, e: JoinError) {
    settle(shared, Err(e.clone()));
    shared.events.emit(GuestEvent::JoinFailed(e));
}

type Dialed = (Connection, iroh::endpoint::SendStream, iroh::endpoint::RecvStream, PeerId);

async fn dial(ep: &iroh::Endpoint, ticket: &Ticket, hello: &Hello) -> Result<Dialed, JoinError> {
    let proto = |e: NetError| JoinError::Protocol(e.to_string());
    let conn = timeout(CONNECT_TIMEOUT, ep.connect(ticket.addr.clone(), ALPN))
        .await
        .map_err(|_| JoinError::Timeout)?
        .map_err(|e| JoinError::Connect(e.to_string()))?;
    let (mut tx, mut rx) = conn.open_bi().await.map_err(|e| JoinError::Connect(e.to_string()))?;
    let hello = serde_json::to_vec(hello).map_err(|e| JoinError::Protocol(e.to_string()))?;
    let wire = frame::encode(&hello).map_err(proto)?;
    frame::write_wire(&mut tx, &wire).await.map_err(proto)?;
    let reply = timeout(REPLY_TIMEOUT, frame::read_frame(&mut rx, MAX_HANDSHAKE))
        .await
        .map_err(|_| JoinError::Timeout)?
        .map_err(proto)?
        .ok_or_else(|| JoinError::Protocol("host closed during handshake".into()))?;
    match serde_json::from_slice::<Reply>(&reply) {
        Ok(Reply::Welcome { peer }) => Ok((conn, tx, rx, peer)),
        Ok(Reply::Refused(e)) => {
            // Hang up so the host (waiting on us) can finish promptly.
            conn.close(CLOSE_LEFT.into(), b"refused");
            Err(e)
        }
        Err(e) => Err(JoinError::Protocol(e.to_string())),
    }
}
