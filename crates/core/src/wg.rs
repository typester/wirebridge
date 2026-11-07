//! The WireGuard side: one UDP socket, any number of clients, and boringtun's `Tunn` doing each client's cryptography.
//!
//! Three loops share the clients: datagrams in (find the client, decrypt, hand the packet to the stack), packets out
//! (find the client by the packet's destination, encrypt, send), and the timers (rekeying, keepalives). No lock is held
//! across an await: each loop copies what boringtun wrote out of the scratch buffer, lets go, and only then sends.
//!
//! A datagram is matched to its client the way boringtun's own `device` does it. A handshake initiation carries the
//! client's static key, which only this side's private key can read (`parse_handshake_anon`); every other message
//! carries the index this side gave the client's session (`receiver_idx >> 8`). A client is also held to its address in
//! the tunnel: a packet it sends from any other address is dropped, so that one client cannot reach into another's
//! flows, and the stack's replies find their client by their destination.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use boringtun::noise::errors::WireGuardError;
use boringtun::noise::handshake::parse_handshake_anon;
use boringtun::noise::{Packet, Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::stats::{Counters, add, bump};

/// Big enough for any datagram and for a handshake message.
const SCRATCH: usize = 65_536;

/// How often boringtun's timers run. Its own device implementation ticks at a similar rate.
const TIMER_TICK: Duration = Duration::from_millis(100);

/// boringtun keeps the low 8 bits of a session's index for itself, so the index it is given has 24.
const INDEX_MASK: u32 = 0x00ff_ffff;

/// A client as the server is told about it: its public key and its address inside the tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Client {
    pub public_key: [u8; 32],
    pub address: Ipv4Addr,
}

/// One client while the server runs: its session state, where it is, and what it has sent and received.
pub struct Peer {
    tunn: Mutex<Tunn>,
    client: Client,
    index: u32,
    /// The address the client's last authenticated datagram came from. WireGuard roams: whatever address the client
    /// last spoke from is where replies go.
    endpoint: Mutex<Option<SocketAddr>>,
    rx_bytes: AtomicU64,
    tx_bytes: AtomicU64,
}

impl Peer {
    pub fn client(&self) -> Client {
        self.client
    }

    pub fn endpoint(&self) -> Option<SocketAddr> {
        *self.endpoint.lock().unwrap()
    }

    pub fn time_since_last_handshake(&self) -> Option<Duration> {
        self.tunn.lock().unwrap().time_since_last_handshake()
    }

    pub fn rx_bytes(&self) -> u64 {
        self.rx_bytes.load(Ordering::Relaxed)
    }

    pub fn tx_bytes(&self) -> u64 {
        self.tx_bytes.load(Ordering::Relaxed)
    }
}

/// Every client the server has, findable by key, by session index and by address.
pub struct Peers {
    private: StaticSecret,
    public: PublicKey,
    tables: RwLock<Tables>,
}

struct Tables {
    by_key: HashMap<[u8; 32], Arc<Peer>>,
    by_index: HashMap<u32, Arc<Peer>>,
    by_address: HashMap<Ipv4Addr, Arc<Peer>>,
    /// The last index given out. Indices go up from a random start and are not given out twice while the server runs,
    /// so a session left over from before a restart, or from a removed client, never lands on another client.
    last_index: u32,
}

impl Peers {
    pub fn new(private: StaticSecret) -> Self {
        let mut start = [0u8; 4];
        // A fixed start only costs the property above; the operating system's random source does not fail in practice.
        let _ = getrandom::fill(&mut start);
        let public = PublicKey::from(&private);
        Self {
            private,
            public,
            tables: RwLock::new(Tables {
                by_key: HashMap::new(),
                by_index: HashMap::new(),
                by_address: HashMap::new(),
                last_index: u32::from_le_bytes(start) & INDEX_MASK,
            }),
        }
    }

    /// Makes the clients exactly `clients`. A client already here with the same key and address keeps its session; the
    /// rest are dropped, and the new ones added. Returns the ones added. The caller checks that no key and no address
    /// comes twice.
    pub fn set(&self, clients: &[Client]) -> Vec<Arc<Peer>> {
        let mut tables = self.tables.write().unwrap();
        let wanted: HashMap<[u8; 32], Ipv4Addr> = clients.iter().map(|c| (c.public_key, c.address)).collect();
        let gone: Vec<[u8; 32]> = tables
            .by_key
            .iter()
            .filter(|(key, peer)| wanted.get(*key) != Some(&peer.client.address))
            .map(|(key, _)| *key)
            .collect();
        for key in gone {
            if let Some(peer) = tables.by_key.remove(&key) {
                tables.by_index.remove(&peer.index);
                tables.by_address.remove(&peer.client.address);
            }
        }
        let mut added = Vec::new();
        for client in clients {
            if tables.by_key.contains_key(&client.public_key) {
                continue;
            }
            let index = tables.next_index();
            let tunn = Tunn::new(self.private.clone(), PublicKey::from(client.public_key), None, None, index, None);
            let peer = Arc::new(Peer {
                tunn: Mutex::new(tunn),
                client: *client,
                index,
                endpoint: Mutex::new(None),
                rx_bytes: AtomicU64::new(0),
                tx_bytes: AtomicU64::new(0),
            });
            tables.by_key.insert(client.public_key, peer.clone());
            tables.by_index.insert(index, peer.clone());
            tables.by_address.insert(client.address, peer.clone());
            added.push(peer);
        }
        added
    }

    /// Every client, in the order of their addresses.
    pub fn all(&self) -> Vec<Arc<Peer>> {
        let mut peers: Vec<Arc<Peer>> = self.tables.read().unwrap().by_key.values().cloned().collect();
        peers.sort_by_key(|peer| peer.client.address);
        peers
    }

    fn by_address(&self, address: Ipv4Addr) -> Option<Arc<Peer>> {
        self.tables.read().unwrap().by_address.get(&address).cloned()
    }

    /// The client a datagram is from, or `None` when it is from no client this server has.
    fn for_datagram(&self, datagram: &[u8]) -> Result<Option<Arc<Peer>>, WireGuardError> {
        let index = match Tunn::parse_incoming_packet(datagram)? {
            Packet::HandshakeInit(init) => {
                // The key is read before the lock is taken: it costs a few Diffie-Hellman operations.
                let Ok(half) = parse_handshake_anon(&self.private, &self.public, &init) else {
                    return Ok(None);
                };
                return Ok(self.tables.read().unwrap().by_key.get(&half.peer_static_public).cloned());
            }
            Packet::HandshakeResponse(p) => p.receiver_idx,
            Packet::PacketCookieReply(p) => p.receiver_idx,
            Packet::PacketData(p) => p.receiver_idx,
        };
        Ok(self.tables.read().unwrap().by_index.get(&(index >> 8)).cloned())
    }
}

impl Tables {
    fn next_index(&mut self) -> u32 {
        loop {
            self.last_index = self.last_index.wrapping_add(1) & INDEX_MASK;
            if self.last_index != 0 && !self.by_index.contains_key(&self.last_index) {
                return self.last_index;
            }
        }
    }
}

/// The IPv4 destination of an IP packet, which names the client a reply is for.
fn destination(packet: &[u8]) -> Option<Ipv4Addr> {
    (packet.len() >= 20 && packet[0] >> 4 == 4).then(|| Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]))
}

/// Datagrams in. Decrypted packets go to the stack; handshake replies go straight back to the sender.
pub async fn network_rx(
    socket: Arc<UdpSocket>,
    peers: Arc<Peers>,
    to_stack: mpsc::Sender<Vec<u8>>,
    counters: Arc<Counters>,
) {
    let mut datagram = vec![0u8; SCRATCH];
    let mut scratch = vec![0u8; SCRATCH];
    loop {
        let (n, from) = match socket.recv_from(&mut datagram).await {
            Ok(received) => received,
            Err(e) => {
                // A connected-UDP error from an earlier send (ICMP unreachable) surfaces here on some systems. It
                // says nothing about the next datagram, so keep listening.
                log::debug!("recv_from failed: {e}");
                continue;
            }
        };

        let peer = match peers.for_datagram(&datagram[..n]) {
            Ok(Some(peer)) => peer,
            Ok(None) => {
                bump(&counters.unknown_peer);
                log::debug!("a datagram from {from} is from no client");
                continue;
            }
            Err(e) => {
                counters.note_wg_error(&e);
                log::debug!("rejected a datagram from {from}: {e:?}");
                continue;
            }
        };

        let mut replies: Vec<Vec<u8>> = Vec::new();
        let mut packet: Option<Vec<u8>> = None;
        let mut authenticated = false;
        {
            let mut tunn = peer.tunn.lock().unwrap();
            match tunn.decapsulate(Some(from.ip()), &datagram[..n], &mut scratch) {
                // A handshake response (or a cookie reply under load). boringtun asks to be called again with an
                // empty datagram until it is done, which flushes packets it queued while the handshake was pending.
                TunnResult::WriteToNetwork(reply) => {
                    replies.push(reply.to_vec());
                    while let TunnResult::WriteToNetwork(more) = tunn.decapsulate(None, &[], &mut scratch) {
                        replies.push(more.to_vec());
                    }
                }
                TunnResult::WriteToTunnelV4(inner, source) => {
                    authenticated = true;
                    if source == peer.client.address {
                        packet = Some(inner.to_vec());
                    } else {
                        bump(&counters.wrong_source);
                        log::debug!("client {} sent from {source}", peer.client.address);
                    }
                }
                // No client has an IPv6 address in the tunnel, so no IPv6 packet can be from its own address.
                TunnResult::WriteToTunnelV6(..) => {
                    authenticated = true;
                    bump(&counters.wrong_source);
                }
                // A keepalive: authenticated, carries nothing.
                TunnResult::Done => authenticated = true,
                TunnResult::Err(e) => {
                    counters.note_wg_error(&e);
                    log::debug!("rejected a datagram from {from}: {e:?}");
                }
            }
        }

        // The endpoint moves only on an authenticated data or keepalive packet. A handshake initiation is answered
        // where it came from, and the client's first packet after it (it always sends one) moves the endpoint.
        if authenticated {
            *peer.endpoint.lock().unwrap() = Some(from);
        }
        for reply in replies {
            if let Err(e) = socket.send_to(&reply, from).await {
                log::debug!("send_to {from} failed: {e}");
            }
        }
        if let Some(packet) = packet {
            bump(&counters.rx_packets);
            add(&counters.rx_bytes, packet.len());
            add(&peer.rx_bytes, packet.len());
            if to_stack.send(packet).await.is_err() {
                return;
            }
        }
    }
}

/// Packets out: whatever the stack writes goes to the client at its destination, encrypted, at the client's current
/// endpoint.
pub async fn network_tx(
    socket: Arc<UdpSocket>,
    peers: Arc<Peers>,
    mut from_stack: mpsc::UnboundedReceiver<Vec<u8>>,
    counters: Arc<Counters>,
) {
    let mut scratch = vec![0u8; SCRATCH];
    while let Some(packet) = from_stack.recv().await {
        // A reply for a client that has been removed since its flow began.
        let Some(peer) = destination(&packet).and_then(|address| peers.by_address(address)) else {
            bump(&counters.queue_dropped);
            continue;
        };
        let datagram = {
            let mut tunn = peer.tunn.lock().unwrap();
            match tunn.encapsulate(&packet, &mut scratch) {
                TunnResult::WriteToNetwork(datagram) => Some(datagram.to_vec()),
                // No session: boringtun queued the packet and will send it once a handshake completes.
                TunnResult::Done => None,
                TunnResult::Err(e) => {
                    counters.note_wg_error(&e);
                    log::debug!("encapsulate failed: {e:?}");
                    None
                }
                TunnResult::WriteToTunnelV4(..) | TunnResult::WriteToTunnelV6(..) => None,
            }
        };
        let Some(datagram) = datagram else { continue };
        let Some(endpoint) = peer.endpoint() else {
            bump(&counters.queue_dropped);
            continue;
        };
        match socket.send_to(&datagram, endpoint).await {
            Ok(_) => {
                bump(&counters.tx_packets);
                add(&counters.tx_bytes, packet.len());
                add(&peer.tx_bytes, packet.len());
            }
            Err(e) => log::debug!("send_to {endpoint} failed: {e}"),
        }
    }
}

/// Starts a handshake towards `endpoint`, which also becomes where everything for this client is sent until it roams.
/// The phone does this towards where each client was the last time it ran a tunnel.
///
/// A restarted server has no session, and on its own a client only notices after its keepalive and rekey timers run,
/// about fifteen seconds. WireGuard is symmetric, so this side can initiate instead; boringtun's timers retry if the
/// first attempt is lost, and a client that has moved simply never answers.
pub async fn initiate(socket: &UdpSocket, peer: &Peer, endpoint: SocketAddr) {
    *peer.endpoint.lock().unwrap() = Some(endpoint);
    let mut scratch = vec![0u8; SCRATCH];
    let initiation = match peer.tunn.lock().unwrap().format_handshake_initiation(&mut scratch, false) {
        TunnResult::WriteToNetwork(initiation) => Some(initiation.to_vec()),
        _ => None,
    };
    if let Some(initiation) = initiation
        && let Err(e) = socket.send_to(&initiation, endpoint).await
    {
        log::debug!("handshake initiation to {endpoint} failed: {e}");
    }
}

/// boringtun's timers, for every client: rekeying, keepalives, and the handshake retries if this side had to initiate.
pub async fn timers(socket: Arc<UdpSocket>, peers: Arc<Peers>) {
    let mut scratch = vec![0u8; SCRATCH];
    let mut tick = tokio::time::interval(TIMER_TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let mut outgoing = Vec::new();
        for peer in peers.all() {
            let datagram = {
                let mut tunn = peer.tunn.lock().unwrap();
                match tunn.update_timers(&mut scratch) {
                    TunnResult::WriteToNetwork(datagram) => Some(datagram.to_vec()),
                    // The session aged out because the client went quiet; it will handshake again when it next speaks.
                    TunnResult::Err(WireGuardError::ConnectionExpired) => None,
                    TunnResult::Err(e) => {
                        log::debug!("timers: {e:?}");
                        None
                    }
                    _ => None,
                }
            };
            if let (Some(datagram), Some(endpoint)) = (datagram, peer.endpoint()) {
                outgoing.push((datagram, endpoint));
            }
        }
        for (datagram, endpoint) in outgoing {
            if let Err(e) = socket.send_to(&datagram, endpoint).await {
                log::debug!("timer send_to {endpoint} failed: {e}");
            }
        }
    }
}
