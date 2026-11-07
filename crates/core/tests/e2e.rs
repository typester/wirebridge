//! End to end on loopback. WireGuard clients written with boringtun stand in for the Mac and a tablet; inside the tunnel
//! UDP datagrams and TCP connections (driven by smoltcp, a whole TCP stack) reach echo servers on this machine through
//! the server's own sockets, and come back. Several clients at once, clients added and removed while the server runs,
//! a key the server was never given, and a client that sends from another client's address are checked too.
//!
//! The destinations are 127.0.0.1 because the server dials whatever address the packet names, exactly as it does on
//! the phone; only the address differs.
//!
//! `bakeoff` is a measurement rather than a check. It puts a link that loses and delays datagrams between the client and
//! the server and downloads through it:
//!     cargo test --release -p wirebridge-core --test e2e -- --ignored --nocapture

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
use wirebridge_core::{
    Client, ClientConfig, CoreError, MTU, Server, encode_key, generate_private_key, parse_private_key,
};

/// The first client's address inside the tunnel, the Mac's.
const MAC_IP: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 2);

/// A second client's.
const TABLET_IP: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 3);

struct Keys {
    server: StaticSecret,
    client: StaticSecret,
}

fn new_key() -> StaticSecret {
    parse_private_key(&generate_private_key()).unwrap()
}

fn keys() -> Keys {
    Keys { server: new_key(), client: new_key() }
}

fn client(key: &StaticSecret, address: Ipv4Addr) -> Client {
    Client { public_key: PublicKey::from(key), address, resume_endpoint: None }
}

/// The server with `keys.client` as its one client, at the Mac's address.
fn start_server(keys: &Keys) -> Server {
    start_with(&keys.server, vec![client(&keys.client, MAC_IP)])
}

fn start_with(server: &StaticSecret, clients: Vec<Client>) -> Server {
    let _ = env_logger::builder().is_test(true).try_init();
    Server::start_on("127.0.0.1:0".parse().unwrap(), server.clone(), clients).unwrap()
}

/// Stops the server and checks that stopping does not hang, which is what a flow left open would cause.
fn stop_within(server: Server, limit: Duration) {
    let started = Instant::now();
    server.stop();
    assert!(started.elapsed() < limit, "stop took {:?}", started.elapsed());
}

/// A client's half of WireGuard, over a plain UDP socket.
struct WgClient {
    socket: UdpSocket,
    tunn: Tunn,
    scratch: Vec<u8>,
    datagram: Vec<u8>,
}

impl WgClient {
    /// `keys.client` towards `keys.server`.
    fn new(server: SocketAddr, keys: &Keys) -> Self {
        Self::with(server, &keys.client, &keys.server)
    }

    fn with(server: SocketAddr, own: &StaticSecret, servers: &StaticSecret) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(server).unwrap();
        // Short, so that a polling loop with nothing to read moves on quickly.
        socket.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
        let tunn = Tunn::new(own.clone(), PublicKey::from(servers), None, None, 1, None);
        Self { socket, tunn, scratch: vec![0; 65_536], datagram: vec![0; 65_536] }
    }

    fn handshake(&mut self) {
        assert!(self.try_handshake(Duration::from_secs(5)), "the handshake did not complete");
    }

    /// Sends one handshake initiation and waits up to `limit` for the answer.
    fn try_handshake(&mut self, limit: Duration) -> bool {
        match self.tunn.format_handshake_initiation(&mut self.scratch, false) {
            TunnResult::WriteToNetwork(initiation) => {
                self.socket.send(initiation).unwrap();
            }
            other => panic!("no handshake initiation: {other:?}"),
        }
        let deadline = Instant::now() + limit;
        while self.tunn.time_since_last_handshake().is_none() {
            if Instant::now() >= deadline {
                return false;
            }
            assert!(self.receive().is_empty());
        }
        true
    }

    fn send_ip(&mut self, packet: &[u8]) {
        match self.tunn.encapsulate(packet, &mut self.scratch) {
            TunnResult::WriteToNetwork(datagram) => {
                self.socket.send(datagram).unwrap();
            }
            TunnResult::Done => {}
            other => panic!("encapsulate: {other:?}"),
        }
    }

    /// Everything that has arrived, decrypted. Waits up to the read timeout for the first datagram.
    fn receive(&mut self) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        loop {
            let n = match self.socket.recv(&mut self.datagram) {
                Ok(n) => n,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return packets,
                Err(e) => panic!("recv: {e}"),
            };
            match self.tunn.decapsulate(None, &self.datagram[..n], &mut self.scratch) {
                TunnResult::WriteToNetwork(reply) => {
                    self.socket.send(reply).unwrap();
                    while let TunnResult::WriteToNetwork(more) = self.tunn.decapsulate(None, &[], &mut self.scratch) {
                        self.socket.send(more).unwrap();
                    }
                }
                TunnResult::WriteToTunnelV4(packet, _) | TunnResult::WriteToTunnelV6(packet, _) => {
                    packets.push(packet.to_vec());
                }
                TunnResult::Done => {}
                // A lossy link can drop a datagram that a later one depended on; WireGuard shrugs those off.
                TunnResult::Err(_) => {}
            }
        }
    }

    fn tick(&mut self) {
        if let TunnResult::WriteToNetwork(datagram) = self.tunn.update_timers(&mut self.scratch) {
            self.socket.send(datagram).unwrap();
        }
    }
}

/// smoltcp's view of a network card: packets in a queue each way.
#[derive(Default)]
struct Queues {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}

struct Rx(Vec<u8>);

struct Tx<'a>(&'a mut VecDeque<Vec<u8>>);

impl RxToken for Rx {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

impl TxToken for Tx<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0; len];
        let result = f(&mut packet);
        self.0.push_back(packet);
        result
    }
}

impl Device for Queues {
    type RxToken<'a>
        = Rx
    where
        Self: 'a;
    type TxToken<'a>
        = Tx<'a>
    where
        Self: 'a;

    fn receive(&mut self, _: smoltcp::time::Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.rx.pop_front()?;
        Some((Rx(packet), Tx(&mut self.tx)))
    }

    fn transmit(&mut self, _: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(Tx(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = MTU as usize;
        caps
    }
}

/// A TCP connection made by smoltcp, carried by [`WgClient`]. Reno, not smoltcp's Cubic: two smoltcp peers that both
/// run Cubic can stall each other after losing their ACKs together (smoltcp #1221), and the Mac's own TCP is neither.
struct TcpClient {
    iface: Interface,
    sockets: SocketSet<'static>,
    handle: SocketHandle,
    queues: Queues,
    wg: WgClient,
    started: Instant,
    last_tick: Instant,
}

impl TcpClient {
    /// A connection from `address`, the client's own in the tunnel, to `target`.
    fn connect(wg: WgClient, address: Ipv4Addr, target: SocketAddr, buffer: usize) -> Self {
        let started = Instant::now();
        let mut queues = Queues::default();
        let mut iface = Interface::new(Config::new(HardwareAddress::Ip), &mut queues, smoltcp::time::Instant::ZERO);
        iface.update_ip_addrs(|addrs| {
            addrs.push(IpCidr::new(IpAddress::Ipv4(address), 24)).unwrap();
        });
        iface.routes_mut().add_default_ipv4_route(Ipv4Addr::new(10, 77, 0, 1)).unwrap();

        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; buffer]),
            tcp::SocketBuffer::new(vec![0; buffer]),
        );
        socket.set_congestion_control(tcp::CongestionControl::Reno);
        let mut sockets = SocketSet::new(vec![]);
        let handle = sockets.add(socket);
        let IpAddr::V4(target_ip) = target.ip() else { panic!("IPv4 only") };
        sockets
            .get_mut::<tcp::Socket>(handle)
            .connect(iface.context(), (IpAddress::Ipv4(target_ip), target.port()), 49_152)
            .unwrap();
        Self { iface, sockets, handle, queues, wg, started, last_tick: started }
    }

    fn now(&self) -> smoltcp::time::Instant {
        smoltcp::time::Instant::from_micros(self.started.elapsed().as_micros() as i64)
    }

    /// One round: smoltcp runs, its packets go into the tunnel, the tunnel's packets go into smoltcp.
    fn pump(&mut self) {
        let now = self.now();
        self.iface.poll(now, &mut self.queues, &mut self.sockets);
        while let Some(packet) = self.queues.tx.pop_front() {
            self.wg.send_ip(&packet);
        }
        let arrived = self.wg.receive();
        self.queues.rx.extend(arrived);
        let now = self.now();
        self.iface.poll(now, &mut self.queues, &mut self.sockets);
        while let Some(packet) = self.queues.tx.pop_front() {
            self.wg.send_ip(&packet);
        }
        if self.last_tick.elapsed() >= Duration::from_millis(100) {
            self.wg.tick();
            self.last_tick = Instant::now();
        }
    }

    fn socket(&mut self) -> &mut tcp::Socket<'static> {
        self.sockets.get_mut::<tcp::Socket>(self.handle)
    }
}

/// What the link between client and server does to each datagram, each way.
#[derive(Clone, Copy, Debug)]
struct Link {
    loss: f64,
    delay: Duration,
    jitter: Duration,
}

impl Link {
    const CLEAN: Link = Link { loss: 0.0, delay: Duration::ZERO, jitter: Duration::ZERO };
}

/// A UDP relay between the WireGuard client and the server that loses and delays datagrams. Delays never reorder, as
/// on Wi-Fi: a datagram leaves no earlier than the one before it. The random numbers come from a fixed seed, so a run
/// can be repeated.
struct Relay {
    addr: SocketAddr,
    link: Arc<Mutex<Link>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Relay {
    fn start(server: SocketAddr, seed: u64) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_micros(500))).unwrap();
        let addr = socket.local_addr().unwrap();
        let link = Arc::new(Mutex::new(Link::CLEAN));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (link, stop) = (link.clone(), stop.clone());
            thread::spawn(move || relay_loop(socket, server, link, stop, seed))
        };
        Self { addr, link, stop, thread: Some(thread) }
    }

    fn set(&self, link: Link) {
        *self.link.lock().unwrap() = link;
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn relay_loop(socket: UdpSocket, server: SocketAddr, link: Arc<Mutex<Link>>, stop: Arc<AtomicBool>, seed: u64) {
    let mut rng = seed | 1;
    let mut random = move || {
        // xorshift64*
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        (rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut client: Option<SocketAddr> = None;
    // (release time, datagram), one FIFO per direction.
    let mut to_server: VecDeque<(Instant, Vec<u8>)> = VecDeque::new();
    let mut to_client: VecDeque<(Instant, Vec<u8>)> = VecDeque::new();
    let mut buf = vec![0u8; 65_536];
    while !stop.load(Ordering::Relaxed) {
        if let Ok((n, from)) = socket.recv_from(&mut buf) {
            let link = *link.lock().unwrap();
            let queue = if from == server {
                &mut to_client
            } else {
                client = Some(from);
                &mut to_server
            };
            if random() >= link.loss {
                let delay = link.delay + link.jitter.mul_f64(random());
                let release = (Instant::now() + delay).max(queue.back().map_or(Instant::now(), |(t, _)| *t));
                queue.push_back((release, buf[..n].to_vec()));
            }
        }
        let now = Instant::now();
        while to_server.front().is_some_and(|(t, _)| *t <= now) {
            let (_, datagram) = to_server.pop_front().unwrap();
            let _ = socket.send_to(&datagram, server);
        }
        while let Some(client) = client
            && to_client.front().is_some_and(|(t, _)| *t <= now)
        {
            let (_, datagram) = to_client.pop_front().unwrap();
            let _ = socket.send_to(&datagram, client);
        }
    }
}


/// A UDP echo server on loopback, answering every datagram where it came from.
fn udp_echo_server() -> SocketAddr {
    let echo = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = echo.local_addr().unwrap();
    thread::spawn(move || {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = echo.recv_from(&mut buf) {
            let _ = echo.send_to(&buf[..n], from);
        }
    });
    addr
}

/// A TCP echo server on loopback, one thread per connection.
fn tcp_echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { return };
            thread::spawn(move || {
                let mut buf = [0u8; 16_384];
                loop {
                    match conn.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            if conn.write_all(&buf[..n]).is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    addr
}

/// Sends `payload` to the UDP echo server at `echo` from `from`:40000 inside the tunnel, and waits up to `limit` for it
/// to come back.
fn udp_echo(wg: &mut WgClient, from: Ipv4Addr, echo: SocketAddr, payload: &[u8], limit: Duration) -> bool {
    let mut packet = Vec::new();
    PacketBuilder::ipv4(from.octets(), [127, 0, 0, 1], 64).udp(40_000, echo.port()).write(&mut packet, payload).unwrap();
    wg.send_ip(&packet);
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        for packet in wg.receive() {
            let Ok(sliced) = SlicedPacket::from_ip(&packet) else { continue };
            if let Some(TransportSlice::Udp(udp)) = sliced.transport {
                assert_eq!(udp.source_port(), echo.port());
                assert_eq!(udp.destination_port(), 40_000);
                if udp.payload() == payload {
                    return true;
                }
            }
        }
        wg.tick();
    }
    false
}

// --------------------------------------------------------------- checks

#[test]
fn udp_round_trip() {
    let keys = keys();
    let server = start_server(&keys);

    let echo = udp_echo_server();
    let mut wg = WgClient::new(server.local_addr(), &keys);
    wg.handshake();
    assert!(udp_echo(&mut wg, MAC_IP, echo, b"hello through the tunnel", Duration::from_secs(5)), "no echo came back");

    let stats = server.stats();
    assert_eq!(stats.udp_total, 1);
    assert_eq!(stats.udp_failed, 0);
    assert!(stats.rx_packets >= 1 && stats.tx_packets >= 1, "{stats:?}");
    let [mac] = stats.clients.as_slice() else { panic!("{stats:?}") };
    assert_eq!(mac.address, MAC_IP.to_string());
    assert!(mac.endpoint.is_some() && mac.seconds_since_handshake.is_some(), "{mac:?}");
    assert!(mac.rx_bytes > 0 && mac.tx_bytes > 0, "{mac:?}");
    stop_within(server, Duration::from_secs(5));
}

#[test]
fn tcp_echo() {
    let keys = keys();
    let server = start_server(&keys);

    let echo_addr = tcp_echo_server();

    let mut wg = WgClient::new(server.local_addr(), &keys);
    wg.handshake();
    let mut client = TcpClient::connect(wg, MAC_IP, echo_addr, 256 * 1024);

    let message: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut sent = 0;
    let mut echoed = Vec::with_capacity(message.len());
    let mut chunk = vec![0u8; 65_536];
    let deadline = Instant::now() + Duration::from_secs(20);
    while echoed.len() < message.len() {
        let state = client.socket().state();
        assert!(Instant::now() < deadline, "echo stalled at {} of {} bytes, socket {state}", echoed.len(), message.len());
        client.pump();
        let socket = client.socket();
        if socket.can_send() && sent < message.len() {
            sent += socket.send_slice(&message[sent..]).unwrap();
        }
        if socket.can_recv() {
            let n = socket.recv_slice(&mut chunk).unwrap();
            echoed.extend_from_slice(&chunk[..n]);
        }
    }
    assert!(echoed == message, "the echo differs from what was sent");

    let stats = server.stats();
    assert_eq!(stats.tcp_total, 1);
    assert_eq!(stats.tcp_active, 1, "the connection is still open");
    assert_eq!(stats.tcp_failed, 0);
    // Stopped with the connection still open: the flow has to be cancelled and dropped on the runtime.
    stop_within(server, Duration::from_secs(5));
}

/// The server answers the SYN only once the real destination accepted, so a closed port is refused and the client
/// never sees the connection established.
#[test]
fn tcp_to_a_closed_port() {
    let keys = keys();
    let server = start_server(&keys);

    // Bind and drop, so that nothing listens on the port.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();

    let mut wg = WgClient::new(server.local_addr(), &keys);
    wg.handshake();
    let mut client = TcpClient::connect(wg, MAC_IP, closed, 16 * 1024);

    let mut was_established = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no outcome: {:?}", server.stats());
        client.pump();
        let state = client.socket().state();
        was_established |= state == tcp::State::Established;
        if server.stats().tcp_failed == 1 && state == tcp::State::Closed {
            break;
        }
    }
    assert!(!was_established, "the client established a connection to a closed port");
    assert_eq!(server.stats().tcp_total, 1);
    stop_within(server, Duration::from_secs(5));
}

// --------------------------------------------------------------- several clients

/// The Mac and a tablet at once, each with a TCP connection to the same echo server from the same port: only the
/// clients' addresses tell the two flows apart, and each reply has to find its own client.
#[test]
fn two_clients_at_once() {
    let keys = keys();
    let tablet = new_key();
    let server = start_with(&keys.server, vec![client(&keys.client, MAC_IP), client(&tablet, TABLET_IP)]);
    let echo = tcp_echo_server();

    let mut mac = WgClient::new(server.local_addr(), &keys);
    mac.handshake();
    let mut pad = WgClient::with(server.local_addr(), &tablet, &keys.server);
    pad.handshake();
    let mut runs = [
        (TcpClient::connect(mac, MAC_IP, echo, 64 * 1024), pattern(150_000, 7), 0, Vec::new()),
        (TcpClient::connect(pad, TABLET_IP, echo, 64 * 1024), pattern(150_000, 13), 0, Vec::new()),
    ];

    let mut chunk = vec![0u8; 65_536];
    let deadline = Instant::now() + Duration::from_secs(20);
    while runs.iter().any(|(_, message, _, echoed)| echoed.len() < message.len()) {
        assert!(Instant::now() < deadline, "the echoes stalled: {:?}", server.stats());
        for (client, message, sent, echoed) in runs.iter_mut() {
            client.pump();
            let socket = client.socket();
            if socket.can_send() && *sent < message.len() {
                *sent += socket.send_slice(&message[*sent..]).unwrap();
            }
            if socket.can_recv() {
                let n = socket.recv_slice(&mut chunk).unwrap();
                echoed.extend_from_slice(&chunk[..n]);
            }
        }
    }
    for (_, message, _, echoed) in &runs {
        assert!(echoed == message, "an echo differs from what was sent");
    }

    let stats = server.stats();
    assert_eq!(stats.tcp_total, 2);
    assert_eq!(stats.wrong_source, 0);
    assert_eq!(stats.clients.len(), 2, "{stats:?}");
    for client in &stats.clients {
        assert!(client.rx_bytes > 150_000 && client.tx_bytes > 150_000, "{client:?}");
    }
    stop_within(server, Duration::from_secs(5));
}

/// A key the server was never given gets no answer at all.
#[test]
fn a_stranger_gets_no_answer() {
    let keys = keys();
    let server = start_server(&keys);
    let mut stranger = WgClient::with(server.local_addr(), &new_key(), &keys.server);
    assert!(!stranger.try_handshake(Duration::from_secs(1)), "a stranger completed a handshake");
    assert!(server.stats().unknown_peer >= 1, "{:?}", server.stats());
    stop_within(server, Duration::from_secs(5));
}

/// Once `stop` returns, the port is free while the server object still lives. The app needs exactly that: it keeps a
/// stopped server around until the garbage collector frees it, and Start right after Stop binds the same port.
#[test]
fn stopping_frees_the_port() {
    let keys = keys();
    let server = start_server(&keys);
    let addr = server.local_addr();
    let mut mac = WgClient::new(addr, &keys);
    mac.handshake();
    server.stop();
    let rebound = std::net::UdpSocket::bind(addr);
    assert!(rebound.is_ok(), "{addr} still taken after stop: {:?}", rebound.err());
    drop(server);
}

/// A client removed while the server runs is cut off at once; the client left keeps its session.
#[test]
fn a_removed_client_is_cut_off() {
    let keys = keys();
    let tablet = new_key();
    let server = start_with(&keys.server, vec![client(&keys.client, MAC_IP), client(&tablet, TABLET_IP)]);
    let echo = udp_echo_server();
    let mut mac = WgClient::new(server.local_addr(), &keys);
    mac.handshake();
    let mut pad = WgClient::with(server.local_addr(), &tablet, &keys.server);
    pad.handshake();
    assert!(udp_echo(&mut pad, TABLET_IP, echo, b"before", Duration::from_secs(5)));

    server.set(vec![client(&keys.client, MAC_IP)]);
    assert!(!udp_echo(&mut pad, TABLET_IP, echo, b"after", Duration::from_secs(1)), "a removed client got through");
    assert!(server.stats().unknown_peer >= 1);
    // No handshake in between: the Mac's session survived the change.
    assert!(udp_echo(&mut mac, MAC_IP, echo, b"still here", Duration::from_secs(5)));
    assert_eq!(server.stats().clients.len(), 1);
    stop_within(server, Duration::from_secs(5));
}

/// A client added while the server runs, through the app's own call, can handshake straight away.
#[test]
fn a_client_added_while_running() {
    let keys = keys();
    let tablet = new_key();
    let server = start_server(&keys);
    let echo = udp_echo_server();

    let config = |key: &StaticSecret, address: Ipv4Addr| ClientConfig {
        public_key: encode_key(PublicKey::from(key).as_bytes()),
        address: address.to_string(),
        resume_endpoint: None,
    };
    server.set_clients(vec![config(&keys.client, MAC_IP), config(&tablet, TABLET_IP)]).unwrap();

    let mut pad = WgClient::with(server.local_addr(), &tablet, &keys.server);
    pad.handshake();
    assert!(udp_echo(&mut pad, TABLET_IP, echo, b"new here", Duration::from_secs(5)));

    // Two clients on one address, or one key twice, are refused, and the clients stay as they were.
    let same_address = server.set_clients(vec![config(&keys.client, MAC_IP), config(&tablet, MAC_IP)]);
    assert!(matches!(same_address, Err(CoreError::InvalidAddress { .. })), "{same_address:?}");
    let same_key = server.set_clients(vec![config(&tablet, MAC_IP), config(&tablet, TABLET_IP)]);
    assert!(matches!(same_key, Err(CoreError::InvalidKey { .. })), "{same_key:?}");
    assert_eq!(server.stats().clients.len(), 2);
    stop_within(server, Duration::from_secs(5));
}

/// A client sending from another client's address is dropped before the stack: it cannot open flows in that client's
/// name, nor reach into its flows.
#[test]
fn a_client_cannot_use_another_address() {
    let keys = keys();
    let server = start_server(&keys);
    let echo = udp_echo_server();
    let mut mac = WgClient::new(server.local_addr(), &keys);
    mac.handshake();

    assert!(!udp_echo(&mut mac, TABLET_IP, echo, b"not mine", Duration::from_secs(1)), "a spoofed source got through");
    let stats = server.stats();
    assert_eq!(stats.wrong_source, 1, "{stats:?}");
    assert_eq!(stats.udp_total, 0, "{stats:?}");
    stop_within(server, Duration::from_secs(5));
}

/// `len` bytes that differ by `seed`, so that two clients' echoes cannot be mistaken for each other.
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    (0..len as u32).map(|i| ((i * seed) % 251) as u8).collect()
}

// --------------------------------------------------------------- the measurement

/// Downloads `total` bytes from a local server through the tunnel and a relay set to `link`. Returns the bytes that
/// arrived, the seconds it took, and what the server sent towards the client (retransmissions included).
fn download(keys: &Keys, link: Link, total: usize, limit: Duration) -> (usize, f64, u64) {
    let server = start_server(keys);
    let relay = Relay::start(server.local_addr(), 0x5eed);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let source_addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let Ok((mut conn, _)) = listener.accept() else { return };
        let block = vec![0x5au8; 64 * 1024];
        let mut left = total;
        while left > 0 {
            let n = left.min(block.len());
            if conn.write_all(&block[..n]).is_err() {
                return;
            }
            left -= n;
        }
    });

    // The handshake over a clean link, so that a lost handshake does not cost a five-second retry; then the damage.
    let mut wg = WgClient::new(relay.addr, keys);
    wg.handshake();
    relay.set(link);
    let mut client = TcpClient::connect(wg, MAC_IP, source_addr, 4 * 1024 * 1024);

    let started = Instant::now();
    let mut received = 0usize;
    let mut chunk = vec![0u8; 1 << 20];
    while received < total && started.elapsed() < limit {
        client.pump();
        let socket = client.socket();
        while socket.can_recv() {
            received += socket.recv_slice(&mut chunk).unwrap();
        }
    }
    let secs = started.elapsed().as_secs_f64();
    let sent = server.stats().tx_bytes;
    stop_within(server, Duration::from_secs(5));
    (received, secs, sent)
}

#[test]
#[ignore = "a measurement: cargo test --release -p wirebridge-core --test e2e -- --ignored --nocapture"]
fn bakeoff() {
    const TOTAL: usize = 20 * 1024 * 1024;
    const LIMIT: Duration = Duration::from_secs(60);
    let ms = Duration::from_millis;
    let links = [
        ("clean", Link::CLEAN),
        ("0.5% loss, 10+0..20 ms", Link { loss: 0.005, delay: ms(10), jitter: ms(20) }),
        ("2% loss, 10+0..50 ms", Link { loss: 0.02, delay: ms(10), jitter: ms(50) }),
    ];
    let keys = keys();
    println!("\n{} MiB down through the relay, at most {} s each", TOTAL >> 20, LIMIT.as_secs());
    println!("{:<24} {:>10} {:>9} {:>10}", "link", "Mbit/s", "sent/got", "result");
    for (link_name, link) in links {
        let (got, secs, sent) = download(&keys, link, TOTAL, LIMIT);
        let mbps = got as f64 * 8.0 / secs / 1e6;
        let waste = if got > 0 { sent as f64 / got as f64 } else { f64::INFINITY };
        let result = if got >= TOTAL { "done".to_string() } else { format!("{:.1} MiB", got as f64 / 1_048_576.0) };
        println!("{link_name:<24} {mbps:>10.1} {waste:>9.2} {result:>10}");
    }
}
