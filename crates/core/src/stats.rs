use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use boringtun::noise::errors::WireGuardError;

/// Live counters, written by the tasks and read by [`crate::Server::stats`] and the periodic log line. They count every
/// client together; [`ClientStats`] has each client's own bytes.
#[derive(Default)]
pub struct Counters {
    /// Inner IP packets the clients sent, after decryption.
    pub rx_packets: AtomicU64,
    pub rx_bytes: AtomicU64,
    /// Inner IP packets sent back to the clients, before encryption. Retransmissions count every time, which is what
    /// makes `tx_bytes` against what a client actually received a measure of waste.
    pub tx_packets: AtomicU64,
    pub tx_bytes: AtomicU64,
    pub tcp_active: AtomicU64,
    pub tcp_total: AtomicU64,
    /// TCP connections whose real destination could not be reached.
    pub tcp_failed: AtomicU64,
    pub udp_active: AtomicU64,
    pub udp_total: AtomicU64,
    pub udp_failed: AtomicU64,
    /// Packets that are neither TCP nor UDP (ICMP above all). Not forwarded.
    pub other_dropped: AtomicU64,
    /// Packets lost inside wirebridge: no endpoint known for the client yet, no client at the packet's destination (it
    /// was removed), a full flow queue, or a packet too large.
    pub queue_dropped: AtomicU64,
    /// Datagrams boringtun rejected, and why.
    pub wg_errors: AtomicU64,
    wg_error_kinds: Mutex<BTreeMap<String, u64>>,
    /// Datagrams from no client this server has: a handshake from a key it was not given, or a session index it never
    /// gave out (a client that was removed, or a session from before a restart).
    pub unknown_peer: AtomicU64,
    /// Packets a client sent from an address other than its own in the tunnel, dropped so that one client cannot reach
    /// into another's flows.
    pub wrong_source: AtomicU64,
}

pub(crate) fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn add(counter: &AtomicU64, n: usize) {
    counter.fetch_add(n as u64, Ordering::Relaxed);
}

/// Counts one live flow for as long as it is held.
pub(crate) struct Active<'a>(&'a AtomicU64);

impl<'a> Active<'a> {
    pub fn new(counter: &'a AtomicU64) -> Self {
        bump(counter);
        Self(counter)
    }
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One client, for the app's screen.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ClientStats {
    /// The client's public key, base64.
    pub public_key: String,
    /// Its address inside the tunnel.
    pub address: String,
    /// Where its datagrams last came from, once one has been authenticated.
    pub endpoint: Option<String>,
    pub seconds_since_handshake: Option<u64>,
    /// Inner bytes from the client, after decryption.
    pub rx_bytes: u64,
    /// Inner bytes to the client, before encryption.
    pub tx_bytes: u64,
}

/// A snapshot for the app's screen.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ServerStats {
    pub listen_port: u16,
    /// Every client the server has, in the order of their addresses.
    pub clients: Vec<ClientStats>,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub tcp_active: u64,
    pub tcp_total: u64,
    pub tcp_failed: u64,
    pub udp_active: u64,
    pub udp_total: u64,
    pub udp_failed: u64,
    pub other_dropped: u64,
    pub queue_dropped: u64,
    pub wg_errors: u64,
    /// The rejections by boringtun's reason, as "Reason=count, ...".
    pub wg_error_kinds: String,
    pub unknown_peer: u64,
    pub wrong_source: u64,
}

impl Counters {
    /// Counts a datagram boringtun rejected, under the error's own name.
    pub fn note_wg_error(&self, error: &WireGuardError) {
        bump(&self.wg_errors);
        let kind = format!("{error:?}");
        *self.wg_error_kinds.lock().unwrap().entry(kind).or_default() += 1;
    }

    fn wg_error_kinds(&self) -> String {
        let kinds = self.wg_error_kinds.lock().unwrap();
        kinds.iter().map(|(kind, n)| format!("{kind}={n}")).collect::<Vec<_>>().join(", ")
    }

    pub fn snapshot(&self, listen_port: u16, clients: Vec<ClientStats>) -> ServerStats {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        ServerStats {
            listen_port,
            clients,
            rx_packets: get(&self.rx_packets),
            rx_bytes: get(&self.rx_bytes),
            tx_packets: get(&self.tx_packets),
            tx_bytes: get(&self.tx_bytes),
            tcp_active: get(&self.tcp_active),
            tcp_total: get(&self.tcp_total),
            tcp_failed: get(&self.tcp_failed),
            udp_active: get(&self.udp_active),
            udp_total: get(&self.udp_total),
            udp_failed: get(&self.udp_failed),
            other_dropped: get(&self.other_dropped),
            queue_dropped: get(&self.queue_dropped),
            wg_errors: get(&self.wg_errors),
            wg_error_kinds: self.wg_error_kinds(),
            unknown_peer: get(&self.unknown_peer),
            wrong_source: get(&self.wrong_source),
        }
    }
}

impl ServerStats {
    /// One line for the log, with the totals the device measurements read: bytes and packets each way, flows, and
    /// every kind of loss. `tools/tunnel-check.sh` reads its `rx=`, `tx=` and `tcp=` fields, so they keep their form.
    pub fn summary(&self) -> String {
        format!(
            "stats rx={}B/{}p tx={}B/{}p tcp={}/{}/{}f udp={}/{}/{}f other={} lost={} wg_errors={}{} unknown={} \
             wrong_source={} clients={}",
            self.rx_bytes,
            self.rx_packets,
            self.tx_bytes,
            self.tx_packets,
            self.tcp_active,
            self.tcp_total,
            self.tcp_failed,
            self.udp_active,
            self.udp_total,
            self.udp_failed,
            self.other_dropped,
            self.queue_dropped,
            self.wg_errors,
            if self.wg_error_kinds.is_empty() { String::new() } else { format!(" ({})", self.wg_error_kinds) },
            self.unknown_peer,
            self.wrong_source,
            self.clients.len(),
        )
    }
}

impl ClientStats {
    /// One line for the log. The handshake's age is left out, so that the line only changes when traffic does.
    pub fn summary(&self) -> String {
        format!(
            "client {} {} rx={}B tx={}B from {}",
            self.address,
            self.public_key.get(..8).unwrap_or(&self.public_key),
            self.rx_bytes,
            self.tx_bytes,
            self.endpoint.as_deref().unwrap_or("-"),
        )
    }
}
