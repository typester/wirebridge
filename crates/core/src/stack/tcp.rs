//! One TCP connection, terminated by a smoltcp stack of its own.
//!
//! The shape follows wgslirpy's `serve_tcp.rs` (MIT/Apache-2.0, https://github.com/vi/wgslirpy): connect to the real
//! destination first, and only then let a one-socket smoltcp interface, whose only address is that destination, answer
//! the Mac's SYN. A destination that cannot be reached therefore gets the Mac a reset, as a closed port would, rather
//! than a connection that opens and leads nowhere (what ipstack, the stack first used here, did, and what made
//! unreachable IPv6 hang).

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant as StdInstant};

use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp::{self, CongestionControl, State};
use smoltcp::time::{Duration as SmolDuration, Instant};
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::packet::{self, FlowKey};
use crate::server::MTU;
use crate::stack::CONNECT_TIMEOUT;
use crate::stats::{Active, Counters, bump};

/// What the socket buffers from the Mac (uploads), which is also its advertised window.
const RX_BUFFER: usize = 256 << 10;

/// What it may hold towards the Mac (downloads): data in flight plus what waits behind it. The round trip is a few
/// milliseconds of Wi-Fi, so this covers far more than a cellular link delivers.
const TX_BUFFER: usize = 512 << 10;

/// The largest single read from the real connection.
const READ_CHUNK: usize = 64 << 10;

/// Keep-alive probes towards the Mac on an idle connection, and how long it may go unanswered. A Mac that went away
/// (its tunnel switched off) would otherwise leave this connection, and the real one behind it, open for good.
const KEEP_ALIVE: SmolDuration = SmolDuration::from_secs(60);
const PEER_TIMEOUT: SmolDuration = SmolDuration::from_secs(120);

/// When smoltcp has no timer running, wake up this often anyway.
const IDLE_WAKE: Duration = Duration::from_secs(60);

pub(super) async fn serve(
    key: FlowKey,
    mut from_router: mpsc::Receiver<Vec<u8>>,
    to_wg: mpsc::UnboundedSender<Vec<u8>>,
    counters: Arc<Counters>,
    cancel: CancellationToken,
) {
    bump(&counters.tcp_total);
    let _active = Active::new(&counters.tcp_active);

    // The SYN, and any retransmission of it, waits in the channel meanwhile.
    let connected = tokio::select! {
        () = cancel.cancelled() => return,
        connected = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(key.server)) => connected,
    };
    let upstream = match connected {
        Ok(Ok(upstream)) => upstream,
        Ok(Err(e)) => return refuse(&key, &mut from_router, &to_wg, &counters, &e.to_string()),
        Err(_) => return refuse(&key, &mut from_router, &to_wg, &counters, "connect timed out"),
    };
    let _ = upstream.set_nodelay(true);
    relay(key, from_router, to_wg, upstream, cancel).await;
}

/// The real destination did not accept: answer what the Mac has sent so far with resets, as a closed port would.
fn refuse(
    key: &FlowKey,
    from_router: &mut mpsc::Receiver<Vec<u8>>,
    to_wg: &mpsc::UnboundedSender<Vec<u8>>,
    counters: &Counters,
    reason: &str,
) {
    bump(&counters.tcp_failed);
    log::debug!("tcp {}: {reason}", key.server);
    while let Ok(packet) = from_router.try_recv() {
        if let Some(reset) = packet::rst_reply(&packet) {
            let _ = to_wg.send(reset);
        }
    }
}

/// What to do with the real connection's write side this time round.
enum Upstream {
    /// Write what the Mac sent that has not been written yet.
    Write,
    /// The Mac sent its FIN and everything before it is written: pass the FIN on.
    Shutdown,
    Idle,
}

async fn upstream_io(writer: &mut OwnedWriteHalf, action: Upstream, data: &[u8]) -> io::Result<Option<usize>> {
    match action {
        Upstream::Write => writer.write(data).await.map(Some),
        Upstream::Shutdown => writer.shutdown().await.map(|()| None),
        Upstream::Idle => std::future::pending().await,
    }
}

async fn relay(
    key: FlowKey,
    mut from_router: mpsc::Receiver<Vec<u8>>,
    to_wg: mpsc::UnboundedSender<Vec<u8>>,
    upstream: TcpStream,
    cancel: CancellationToken,
) {
    let mut device = FlowDevice { rx: VecDeque::new(), to_wg };
    let mut config = Config::new(HardwareAddress::Ip);
    config.random_seed = random_seed();
    let mut iface = Interface::new(config, &mut device, Instant::now());
    iface.update_ip_addrs(|addrs| {
        // A /0, so that every source counts as on-link and no route is needed back to the Mac.
        let _ = addrs.push(IpCidr::new(IpAddress::from(key.server.ip()), 0));
    });

    let mut socket = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; RX_BUFFER]),
        tcp::SocketBuffer::new(vec![0; TX_BUFFER]),
    );
    socket.set_nagle_enabled(false);
    socket.set_congestion_control(CongestionControl::Cubic);
    socket.set_keep_alive(Some(KEEP_ALIVE));
    socket.set_timeout(Some(PEER_TIMEOUT));
    let listen = IpListenEndpoint { addr: Some(IpAddress::from(key.server.ip())), port: key.server.port() };
    if let Err(e) = socket.listen(listen) {
        log::warn!("tcp {}: listen: {e}", key.server);
        return;
    }
    let mut sockets = SocketSet::new(Vec::with_capacity(1));
    let handle = sockets.add(socket);

    // What queued up while connecting: the SYN, perhaps retransmitted.
    while let Ok(packet) = from_router.try_recv() {
        device.rx.push_back(packet);
    }

    let (mut up_read, mut up_write) = upstream.into_split();
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut to_upstream: Vec<u8> = Vec::new();
    let mut written = 0usize;
    let mut upstream_eof = false;
    let mut upstream_shut = false;
    let mut established = false;
    let handshake_deadline = StdInstant::now() + CONNECT_TIMEOUT;

    loop {
        let now = Instant::now();
        iface.poll(now, &mut device, &mut sockets);

        let socket = sockets.get_mut::<tcp::Socket>(handle);
        let state = socket.state();
        match state {
            State::Closed | State::TimeWait => break,
            // The Mac never completed the handshake (it gave up, or its ACK was lost for good).
            State::Listen | State::SynReceived if !established => {
                if StdInstant::now() >= handshake_deadline {
                    break;
                }
            }
            _ => established = true,
        }

        // Mac to upstream: take more from the socket once the last batch is written out.
        if written == to_upstream.len() {
            to_upstream.clear();
            written = 0;
            if socket.can_recv() {
                let _ = socket.recv(|data| {
                    to_upstream.extend_from_slice(data);
                    (data.len(), ())
                });
            }
        }
        let mac_finished = established && !socket.may_recv() && !socket.can_recv();
        let action = if written < to_upstream.len() {
            Upstream::Write
        } else if mac_finished && !upstream_shut {
            Upstream::Shutdown
        } else {
            Upstream::Idle
        };

        // Upstream to Mac: read no more than the socket can take, so a slow Mac slows the read down.
        let room = if !upstream_eof && socket.may_send() {
            (socket.send_capacity() - socket.send_queue()).min(READ_CHUNK)
        } else {
            0
        };

        let delay = iface
            .poll_delay(now, &sockets)
            .map(|d| Duration::from_micros(d.total_micros()))
            .unwrap_or(IDLE_WAKE);

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                sockets.get_mut::<tcp::Socket>(handle).abort();
                iface.poll(Instant::now(), &mut device, &mut sockets);
                return;
            }
            packet = from_router.recv() => match packet {
                Some(packet) => {
                    device.rx.push_back(packet);
                    while let Ok(packet) = from_router.try_recv() {
                        device.rx.push_back(packet);
                    }
                }
                None => return,
            },
            done = upstream_io(&mut up_write, action, &to_upstream[written..]) => match done {
                Ok(Some(n)) => written += n,
                Ok(None) => upstream_shut = true,
                Err(e) => {
                    log::debug!("tcp {}: write: {e}", key.server);
                    sockets.get_mut::<tcp::Socket>(handle).abort();
                }
            },
            read = up_read.read(&mut chunk[..room]), if room > 0 => {
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                match read {
                    Ok(0) => {
                        upstream_eof = true;
                        socket.close();
                    }
                    Ok(n) => {
                        let _ = socket.send_slice(&chunk[..n]);
                    }
                    Err(e) => {
                        log::debug!("tcp {}: read: {e}", key.server);
                        socket.abort();
                    }
                }
            },
            () = tokio::time::sleep(delay) => {}
        }
    }
}

fn random_seed() -> u64 {
    let mut seed = [0u8; 8];
    let _ = getrandom::fill(&mut seed);
    u64::from_le_bytes(seed)
}

/// smoltcp's view of the link: packets from the router in a queue, packets out straight to the WireGuard side.
struct FlowDevice {
    rx: VecDeque<Vec<u8>>,
    to_wg: mpsc::UnboundedSender<Vec<u8>>,
}

struct RxPacket(Vec<u8>);

struct TxPacket<'a>(&'a mpsc::UnboundedSender<Vec<u8>>);

impl RxToken for RxPacket {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

impl TxToken for TxPacket<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0; len];
        let result = f(&mut packet);
        let _ = self.0.send(packet);
        result
    }
}

impl Device for FlowDevice {
    type RxToken<'a>
        = RxPacket
    where
        Self: 'a;
    type TxToken<'a>
        = TxPacket<'a>
    where
        Self: 'a;

    fn receive(&mut self, _: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.rx.pop_front()?;
        Some((RxPacket(packet), TxPacket(&self.to_wg)))
    }

    fn transmit(&mut self, _: Instant) -> Option<Self::TxToken<'_>> {
        Some(TxPacket(&self.to_wg))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = usize::from(MTU);
        caps
    }
}
