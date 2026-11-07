//! Reading the Mac's packets, and building the few that are written by hand: UDP replies and TCP resets.

use std::net::{IpAddr, SocketAddr};

use etherparse::{IpHeaders, NetSlice, PacketBuilder, PacketBuilderStep, SlicedPacket, TransportSlice};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Proto {
    Tcp,
    Udp,
}

/// One flow, from the Mac's side: the Mac's address and port, and the destination it asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FlowKey {
    pub proto: Proto,
    pub client: SocketAddr,
    pub server: SocketAddr,
}

/// What the router needs to know about a packet.
pub(super) enum Parsed {
    Tcp { key: FlowKey, syn: bool, ack: bool, rst: bool },
    Udp { key: FlowKey },
    /// IP, but neither TCP nor UDP (ICMP above all).
    Other,
    /// Not a packet that could be read.
    Invalid,
}

fn addresses(sliced: &SlicedPacket<'_>) -> Option<(IpAddr, IpAddr)> {
    match &sliced.net {
        Some(NetSlice::Ipv4(ip)) => {
            let header = ip.header();
            Some((header.source_addr().into(), header.destination_addr().into()))
        }
        Some(NetSlice::Ipv6(ip)) => {
            let header = ip.header();
            Some((header.source_addr().into(), header.destination_addr().into()))
        }
        _ => None,
    }
}

pub(super) fn parse(packet: &[u8]) -> Parsed {
    let Ok(sliced) = SlicedPacket::from_ip(packet) else {
        return Parsed::Invalid;
    };
    let Some((source, destination)) = addresses(&sliced) else {
        return Parsed::Invalid;
    };
    match &sliced.transport {
        Some(TransportSlice::Tcp(tcp)) => Parsed::Tcp {
            key: FlowKey {
                proto: Proto::Tcp,
                client: SocketAddr::new(source, tcp.source_port()),
                server: SocketAddr::new(destination, tcp.destination_port()),
            },
            syn: tcp.syn(),
            ack: tcp.ack(),
            rst: tcp.rst(),
        },
        Some(TransportSlice::Udp(udp)) => Parsed::Udp {
            key: FlowKey {
                proto: Proto::Udp,
                client: SocketAddr::new(source, udp.source_port()),
                server: SocketAddr::new(destination, udp.destination_port()),
            },
        },
        _ => Parsed::Other,
    }
}

/// The payload of a UDP packet.
pub(super) fn udp_payload(packet: &[u8]) -> Option<&[u8]> {
    let sliced = SlicedPacket::from_ip(packet).ok()?;
    match sliced.transport {
        Some(TransportSlice::Udp(udp)) => Some(udp.payload()),
        _ => None,
    }
}

fn ip(from: IpAddr, to: IpAddr) -> Option<PacketBuilderStep<IpHeaders>> {
    match (from, to) {
        (IpAddr::V4(from), IpAddr::V4(to)) => Some(PacketBuilder::ipv4(from.octets(), to.octets(), 64)),
        (IpAddr::V6(from), IpAddr::V6(to)) => Some(PacketBuilder::ipv6(from.octets(), to.octets(), 64)),
        _ => None,
    }
}

/// A UDP datagram from `from` to `to`.
pub(super) fn udp_packet(from: SocketAddr, to: SocketAddr, payload: &[u8]) -> Option<Vec<u8>> {
    let builder = ip(from.ip(), to.ip())?.udp(from.port(), to.port());
    let mut packet = Vec::with_capacity(builder.size(payload.len()));
    builder.write(&mut packet, payload).ok()?;
    Some(packet)
}

/// The reset a closed port answers `packet` with (RFC 9293 §3.10.7.1): nothing for a reset; `SEQ=SEG.ACK` with RST for
/// a segment carrying an ACK; otherwise `SEQ=0`, `ACK=SEG.SEQ+SEG.LEN` with RST and ACK, where a SYN and a FIN each
/// count one towards the length.
pub(super) fn rst_reply(packet: &[u8]) -> Option<Vec<u8>> {
    let sliced = SlicedPacket::from_ip(packet).ok()?;
    let (source, destination) = addresses(&sliced)?;
    let Some(TransportSlice::Tcp(tcp)) = &sliced.transport else {
        return None;
    };
    if tcp.rst() {
        return None;
    }
    let step = ip(destination, source)?;
    let builder = if tcp.ack() {
        step.tcp(tcp.destination_port(), tcp.source_port(), tcp.acknowledgment_number(), 0)
            .rst()
    } else {
        let length = tcp.payload().len() as u32 + u32::from(tcp.syn()) + u32::from(tcp.fin());
        step.tcp(tcp.destination_port(), tcp.source_port(), 0, 0)
            .rst()
            .ack(tcp.sequence_number().wrapping_add(length))
    };
    let mut reply = Vec::with_capacity(builder.size(0));
    builder.write(&mut reply, &[]).ok()?;
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syn(from: SocketAddr, to: SocketAddr, seq: u32) -> Vec<u8> {
        let builder = ip(from.ip(), to.ip()).unwrap().tcp(from.port(), to.port(), seq, 65_535).syn();
        let mut packet = Vec::new();
        builder.write(&mut packet, &[]).unwrap();
        packet
    }

    #[test]
    fn a_syn_is_refused_with_rst_ack_one_past_its_sequence() {
        let mac: SocketAddr = "10.77.0.2:50000".parse().unwrap();
        let server: SocketAddr = "192.0.2.1:443".parse().unwrap();
        let reply = rst_reply(&syn(mac, server, 1000)).unwrap();

        let sliced = SlicedPacket::from_ip(&reply).unwrap();
        assert_eq!(addresses(&sliced), Some((server.ip(), mac.ip())));
        let Some(TransportSlice::Tcp(tcp)) = sliced.transport else { panic!("not TCP") };
        assert!(tcp.rst() && tcp.ack());
        assert_eq!((tcp.source_port(), tcp.destination_port()), (443, 50000));
        assert_eq!(tcp.acknowledgment_number(), 1001);
    }

    #[test]
    fn a_reset_is_never_answered() {
        let mac: SocketAddr = "10.77.0.2:50000".parse().unwrap();
        let server: SocketAddr = "192.0.2.1:443".parse().unwrap();
        let builder = ip(mac.ip(), server.ip()).unwrap().tcp(50000, 443, 5, 0).rst();
        let mut packet = Vec::new();
        builder.write(&mut packet, &[]).unwrap();
        assert!(rst_reply(&packet).is_none());
    }

    #[test]
    fn a_udp_reply_round_trips() {
        let server: SocketAddr = "[2001:db8::1]:53".parse().unwrap();
        let mac: SocketAddr = "[fd77::2]:40000".parse().unwrap();
        let packet = udp_packet(server, mac, b"answer").unwrap();
        assert_eq!(udp_payload(&packet), Some(&b"answer"[..]));
        let Parsed::Udp { key } = parse(&packet) else { panic!("not UDP") };
        assert_eq!((key.client, key.server), (server, mac));
    }
}
