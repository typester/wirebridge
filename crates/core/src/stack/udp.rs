//! UDP needs no stack: each flow is a connected UDP socket on this side, and replies are built by hand.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::packet::{self, FlowKey};
use crate::stack::{UDP_BUFFER, UDP_IDLE};
use crate::stats::{Active, Counters, bump};

pub(super) async fn serve(
    key: FlowKey,
    mut from_router: mpsc::Receiver<Vec<u8>>,
    to_wg: mpsc::UnboundedSender<Vec<u8>>,
    counters: Arc<Counters>,
    cancel: CancellationToken,
) {
    bump(&counters.udp_total);
    let _active = Active::new(&counters.udp_active);

    let local: SocketAddr = match key.server {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let upstream = match UdpSocket::bind(local).await {
        Ok(socket) => socket,
        Err(e) => {
            bump(&counters.udp_failed);
            log::debug!("udp {}: bind: {e}", key.server);
            return;
        }
    };
    if let Err(e) = upstream.connect(key.server).await {
        bump(&counters.udp_failed);
        log::debug!("udp {}: {e}", key.server);
        return;
    }

    let mut from_upstream = vec![0u8; UDP_BUFFER];
    let idle = tokio::time::sleep(UDP_IDLE);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            () = &mut idle => return,
            packet = from_router.recv() => {
                let Some(packet) = packet else { return };
                if let Some(payload) = packet::udp_payload(&packet)
                    && let Err(e) = upstream.send(payload).await
                {
                    log::debug!("udp {}: send: {e}", key.server);
                    return;
                }
                idle.as_mut().reset(tokio::time::Instant::now() + UDP_IDLE);
            }
            received = upstream.recv(&mut from_upstream) => match received {
                Ok(n) => {
                    if let Some(reply) = packet::udp_packet(key.server, key.client, &from_upstream[..n]) {
                        let _ = to_wg.send(reply);
                    }
                    idle.as_mut().reset(tokio::time::Instant::now() + UDP_IDLE);
                }
                // On a connected socket an ICMP unreachable comes back as an error; the flow is over.
                Err(e) => {
                    log::debug!("udp {}: recv: {e}", key.server);
                    return;
                }
            },
        }
    }
}
