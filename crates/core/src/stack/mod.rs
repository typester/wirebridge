//! The user-space TCP/IP stack that terminates the Mac's flows. A router keeps a table of flows and hands each of the
//! Mac's packets to its flow's task. TCP flows each run a smoltcp stack of their own (tcp.rs); UDP flows need none
//! (udp.rs). smoltcp rather than ipstack: measured side by side on a lossy link, ipstack retransmitted without
//! restraint, and smoltcp wasted almost nothing.

mod packet;
mod tcp;
mod udp;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::stats::{Counters, bump};
use packet::{FlowKey, Parsed, Proto};

/// How long a flow waits for its real destination to accept.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a UDP flow may sit idle before it is closed. QUIC keeps its flows busier than this.
pub(crate) const UDP_IDLE: Duration = Duration::from_secs(60);

/// The largest UDP payload, so no datagram is ever cut.
pub(crate) const UDP_BUFFER: usize = 65_535;

/// Packets a flow may have waiting before further ones are dropped.
const FLOW_QUEUE: usize = 1024;

/// What connects the stack to the WireGuard side.
pub(crate) struct Pipes {
    /// Decrypted IP packets from the Mac.
    pub inbound: mpsc::Receiver<Vec<u8>>,
    /// IP packets for the Mac, still to be encrypted.
    pub outbound: mpsc::UnboundedSender<Vec<u8>>,
    pub counters: Arc<Counters>,
    pub cancel: CancellationToken,
}

/// Starts the stack on `pipes`. Must run inside the runtime; every task goes on `tracker`.
pub(crate) fn start(pipes: Pipes, tracker: &TaskTracker) {
    tracker.spawn(route(pipes, tracker.clone()));
}

struct Flow {
    tx: mpsc::Sender<Vec<u8>>,
    /// Tells a flow from a later one with the same addresses, so that the first one's end does not remove the second.
    id: u64,
}

struct Router {
    flows: HashMap<FlowKey, Flow>,
    next_id: u64,
    outbound: mpsc::UnboundedSender<Vec<u8>>,
    counters: Arc<Counters>,
    cancel: CancellationToken,
    tracker: TaskTracker,
    ended: mpsc::UnboundedSender<(FlowKey, u64)>,
}

async fn route(pipes: Pipes, tracker: TaskTracker) {
    let Pipes { mut inbound, outbound, counters, cancel } = pipes;
    let (ended, mut ended_rx) = mpsc::unbounded_channel();
    let mut router = Router {
        flows: HashMap::new(),
        next_id: 0,
        outbound,
        counters,
        cancel: cancel.clone(),
        tracker,
        ended,
    };
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            Some((key, id)) = ended_rx.recv() => {
                if router.flows.get(&key).is_some_and(|flow| flow.id == id) {
                    router.flows.remove(&key);
                }
            }
            packet = inbound.recv() => match packet {
                Some(packet) => router.dispatch(packet),
                None => return,
            },
        }
    }
}

impl Router {
    fn dispatch(&mut self, packet: Vec<u8>) {
        match packet::parse(&packet) {
            Parsed::Tcp { key, syn, ack, rst } => {
                let Some(packet) = self.deliver(&key, packet) else { return };
                if syn && !ack {
                    self.open(key, packet);
                } else if !rst && let Some(reset) = packet::rst_reply(&packet) {
                    // A segment for a connection this side does not have (it ended, or the stack was restarted):
                    // refuse it, so the Mac gives up at once rather than retransmitting into silence.
                    let _ = self.outbound.send(reset);
                }
            }
            Parsed::Udp { key } => {
                if let Some(packet) = self.deliver(&key, packet) {
                    self.open(key, packet);
                }
            }
            Parsed::Other | Parsed::Invalid => bump(&self.counters.other_dropped),
        }
    }

    /// Hands `packet` to the live flow for `key`, and gives it back when there is none.
    fn deliver(&mut self, key: &FlowKey, packet: Vec<u8>) -> Option<Vec<u8>> {
        let Some(flow) = self.flows.get(key) else {
            return Some(packet);
        };
        match flow.tx.try_send(packet) {
            Ok(()) => None,
            Err(TrySendError::Full(_)) => {
                bump(&self.counters.queue_dropped);
                None
            }
            Err(TrySendError::Closed(packet)) => {
                self.flows.remove(key);
                Some(packet)
            }
        }
    }

    fn open(&mut self, key: FlowKey, first: Vec<u8>) {
        let (tx, rx) = mpsc::channel(FLOW_QUEUE);
        let _ = tx.try_send(first);
        let id = self.next_id;
        self.next_id += 1;
        self.flows.insert(key, Flow { tx, id });

        let outbound = self.outbound.clone();
        let counters = self.counters.clone();
        let cancel = self.cancel.clone();
        let ended = self.ended.clone();
        self.tracker.spawn(async move {
            match key.proto {
                Proto::Tcp => tcp::serve(key, rx, outbound, counters, cancel).await,
                Proto::Udp => udp::serve(key, rx, outbound, counters, cancel).await,
            }
            let _ = ended.send((key, id));
        });
    }
}
