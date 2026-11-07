use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use boringtun::x25519::{PublicKey, StaticSecret};
use tokio::net::UdpSocket;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::conf::parse_client_address;
use crate::stack::{self, Pipes};
use crate::stats::{ClientStats, Counters, ServerStats};
use crate::wg::{self, Peers};
use crate::{CoreError, keys};

/// The inner MTU: the same number as `MTU` in each client's config ([`crate::conf`]). 1420 leaves room for WireGuard
/// over IPv6.
pub const MTU: u16 = 1420;

/// Packets decrypted but not yet taken by the stack. Beyond this the WireGuard side waits.
const INBOUND_QUEUE: usize = 4096;

/// Socket buffers for the WireGuard socket; the kernel may grant less.
const SOCKET_BUFFER: usize = 4 << 20;

/// How long `stop` waits for the flows to wind down before taking the runtime away.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// How often the counters go to the log, which is where the device measurements read them.
const STATS_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// One client, as the app keeps it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ClientConfig {
    /// The client's public key, base64 as `wg pubkey` prints it.
    pub public_key: String,
    /// Its address inside the tunnel, a plain IPv4 address ("10.77.0.3").
    pub address: String,
    /// Where it was when this phone last ran a tunnel ("address:port"). If given, this side starts the handshake.
    pub resume_endpoint: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ServerConfig {
    /// This phone's private key, base64 as `wg genkey` prints it.
    pub private_key: String,
    /// The UDP port to listen on, on every interface, IPv4 and IPv6 alike.
    pub listen_port: u16,
    pub clients: Vec<ClientConfig>,
}

/// A client as [`Server::start_on`] and [`Server::set`] take it: [`ClientConfig`], parsed.
#[derive(Debug, Clone)]
pub struct Client {
    pub public_key: PublicKey,
    pub address: Ipv4Addr,
    pub resume_endpoint: Option<SocketAddr>,
}

/// A running tunnel endpoint. Created by [`Server::start`]; stopped by [`Server::stop`] or by being dropped.
#[derive(uniffi::Object)]
pub struct Server {
    running: Mutex<Option<Running>>,
    local_addr: SocketAddr,
    socket: Arc<UdpSocket>,
    counters: Arc<Counters>,
    peers: Arc<Peers>,
}

struct Running {
    runtime: Runtime,
    cancel: CancellationToken,
    tracker: TaskTracker,
}

#[uniffi::export]
impl Server {
    /// Starts listening on `[::]:listen_port` (dual-stack, so IPv4 too).
    #[uniffi::constructor]
    pub fn start(config: ServerConfig) -> Result<Arc<Self>, CoreError> {
        let private = keys::parse_private_key(&config.private_key)?;
        let clients = parse_clients(&config.clients)?;
        let bind = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), config.listen_port);
        Self::start_on(bind, private, clients).map(Arc::new)
    }

    /// Makes the server's clients exactly `clients`, while it runs. A client it already has with the same key and
    /// address keeps its session; one that is gone stops being answered at once; a new one can handshake straight away.
    pub fn set_clients(&self, clients: Vec<ClientConfig>) -> Result<(), CoreError> {
        self.set(parse_clients(&clients)?);
        Ok(())
    }

    pub fn stats(&self) -> ServerStats {
        self.counters.snapshot(self.local_addr.port(), client_stats(&self.peers))
    }

    /// Stops every task and closes every flow, then the socket. Safe to call more than once. When it returns the UDP
    /// port is free again, so a new server can start on it straight away.
    pub fn stop(&self) {
        let Some(running) = self.running.lock().unwrap().take() else {
            return;
        };
        running.cancel.cancel();
        running.tracker.close();
        // Let the flows end on the runtime's own workers before the runtime goes: each one sees the cancellation and
        // returns at once. (The wait was made for ipstack, the stack first used here, whose TCP stream blocked in place
        // when dropped; it costs nothing now.) Neither waiting nor `shutdown_timeout` is allowed inside another
        // runtime, so a caller in one (nothing in the app is) only gets the cancellation.
        if tokio::runtime::Handle::try_current().is_err() {
            let tracker = running.tracker.clone();
            let _ = running.runtime.block_on(async { tokio::time::timeout(STOP_GRACE, tracker.wait()).await });
            running.runtime.shutdown_timeout(Duration::from_secs(1));
        } else {
            running.runtime.shutdown_background();
        }
    }
}

impl Server {
    /// Binds `bind` rather than `[::]:port`. The app goes through [`Server::start`]; the tests use this to listen on
    /// loopback with an ephemeral port.
    pub fn start_on(bind: SocketAddr, private: StaticSecret, clients: Vec<Client>) -> Result<Self, CoreError> {
        check_unique(&clients)?;
        let start_err = |what: &str, e: &dyn std::fmt::Display| CoreError::Start { reason: format!("{what}: {e}") };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("wirebridge")
            .enable_all()
            .build()
            .map_err(|e| start_err("runtime", &e))?;
        let _entered = runtime.enter();

        let socket = bind_udp(bind).map_err(|e| start_err(&format!("bind {bind}"), &e))?;
        let local_addr = socket.local_addr().map_err(|e| start_err("local address", &e))?;
        let socket = Arc::new(socket);

        let peers = Arc::new(Peers::new(private));
        let counters = Arc::new(Counters::default());

        let (to_stack, from_wg) = mpsc::channel(INBOUND_QUEUE);
        let (to_wg, from_stack) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();

        let pipes = Pipes { inbound: from_wg, outbound: to_wg, counters: counters.clone(), cancel: cancel.clone() };
        stack::start(pipes, &tracker);

        tracker.spawn(until_cancelled(
            cancel.clone(),
            wg::network_rx(socket.clone(), peers.clone(), to_stack, counters.clone()),
        ));
        tracker.spawn(until_cancelled(
            cancel.clone(),
            wg::network_tx(socket.clone(), peers.clone(), from_stack, counters.clone()),
        ));
        tracker.spawn(until_cancelled(cancel.clone(), wg::timers(socket.clone(), peers.clone())));
        tracker.spawn(until_cancelled(
            cancel.clone(),
            log_stats(counters.clone(), peers.clone(), local_addr.port()),
        ));

        log::info!("listening on {local_addr}");
        drop(_entered);
        let server = Self {
            running: Mutex::new(Some(Running { runtime, cancel, tracker })),
            local_addr,
            socket,
            counters,
            peers,
        };
        server.set(clients);
        Ok(server)
    }

    /// [`Server::set_clients`] with the clients already parsed. Each new client whose last endpoint is known gets a
    /// handshake from this side.
    pub fn set(&self, clients: Vec<Client>) {
        if let Err(e) = check_unique(&clients) {
            log::warn!("clients not changed: {e}");
            return;
        }
        let wanted: Vec<wg::Client> = clients
            .iter()
            .map(|c| wg::Client { public_key: *c.public_key.as_bytes(), address: c.address })
            .collect();
        let added = self.peers.set(&wanted);
        log::info!("clients: {} ({} new)", wanted.len(), added.len());

        let running = self.running.lock().unwrap();
        let Some(running) = running.as_ref() else { return };
        for peer in added {
            let key = peer.client().public_key;
            let Some(endpoint) = clients.iter().find(|c| *c.public_key.as_bytes() == key).and_then(|c| c.resume_endpoint)
            else {
                continue;
            };
            log::info!("resuming {} towards {endpoint}", peer.client().address);
            let socket = self.socket.clone();
            let cancel = running.cancel.clone();
            running.tracker.spawn_on(
                until_cancelled(cancel, async move { wg::initiate(&socket, &peer, endpoint).await }),
                running.runtime.handle(),
            );
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The app's clients, parsed. A resume endpoint that does not parse is only a missed shortcut: that client will
/// handshake on its own.
fn parse_clients(clients: &[ClientConfig]) -> Result<Vec<Client>, CoreError> {
    let parsed = clients
        .iter()
        .map(|c| {
            Ok(Client {
                public_key: keys::parse_public_key(&c.public_key)?,
                address: parse_client_address(&c.address)?,
                resume_endpoint: c.resume_endpoint.as_deref().and_then(|a| a.parse().ok()),
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    check_unique(&parsed)?;
    Ok(parsed)
}

/// No two clients may share a key or an address: the key is how a client's handshakes are told apart, and the address
/// is how replies find it.
fn check_unique(clients: &[Client]) -> Result<(), CoreError> {
    let mut keys = HashSet::new();
    let mut addresses = HashSet::new();
    for client in clients {
        if !keys.insert(*client.public_key.as_bytes()) {
            return Err(CoreError::InvalidKey { reason: "two clients have the same public key".into() });
        }
        if !addresses.insert(client.address) {
            return Err(CoreError::InvalidAddress { reason: format!("two clients have the address {}", client.address) });
        }
    }
    Ok(())
}

fn client_stats(peers: &Peers) -> Vec<ClientStats> {
    peers
        .all()
        .iter()
        .map(|peer| ClientStats {
            public_key: keys::encode_key(&peer.client().public_key),
            address: peer.client().address.to_string(),
            endpoint: peer.endpoint().map(|a| SocketAddr::new(a.ip().to_canonical(), a.port()).to_string()),
            seconds_since_handshake: peer.time_since_last_handshake().map(|d| d.as_secs()),
            rx_bytes: peer.rx_bytes(),
            tx_bytes: peer.tx_bytes(),
        })
        .collect()
}

/// Runs `future` until it finishes or `cancel` fires, whichever comes first.
async fn until_cancelled(cancel: CancellationToken, future: impl Future<Output = ()>) {
    tokio::select! {
        () = cancel.cancelled() => {}
        () = future => {}
    }
}

/// The counters every few seconds, whenever they have moved: one line for every client together, then one for each
/// client.
async fn log_stats(counters: Arc<Counters>, peers: Arc<Peers>, port: u16) {
    let mut tick = tokio::time::interval(STATS_LOG_INTERVAL);
    let mut last = String::new();
    loop {
        tick.tick().await;
        let stats = counters.snapshot(port, client_stats(&peers));
        let lines: Vec<String> =
            std::iter::once(stats.summary()).chain(stats.clients.iter().map(ClientStats::summary)).collect();
        let text = lines.join("\n");
        if text == last {
            continue;
        }
        log::info!("{}", lines[0]);
        for (line, client) in lines[1..].iter().zip(&stats.clients) {
            let since = client.seconds_since_handshake.map_or("-".to_string(), |s| format!("{s}s ago"));
            log::info!("{line} (handshake {since})");
        }
        last = text;
    }
}

/// The WireGuard socket. An IPv6 wildcard is made dual-stack explicitly rather than trusting the platform default.
fn bind_udp(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    if addr.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    let _ = socket.set_recv_buffer_size(SOCKET_BUFFER);
    let _ = socket.set_send_buffer_size(SOCKET_BUFFER);
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    UdpSocket::from_std(socket.into())
}
