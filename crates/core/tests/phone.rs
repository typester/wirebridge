//! A check of a running phone from a computer on the same network: a WireGuard handshake with it, then one question
//! through the tunnel whose answer names the address it left for the internet from. Cloudflare's resolver answers
//! `whoami.cloudflare` (class CH, type TXT) with the address it was asked from, so the answer tells a phone's own
//! connection from a VPN on it.
//!
//! It takes a client's config as the phone hands it out, with `Endpoint` changed to the phone's address on the network
//! this computer shares with it (its home Wi-Fi address, say). The path must be absolute, since the test runs in
//! crates/core:
//!     WIREBRIDGE_CONF=/path/to/client.conf cargo test -p wirebridge-core --test phone -- --ignored --nocapture
//!
//! Only this test's own UDP socket goes through the tunnel; the computer's routes are left alone.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use boringtun::noise::{Tunn, TunnResult};
use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
use wirebridge_core::{parse_private_key, parse_public_key};

const RESOLVER: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);

/// The question's ID, which its answer repeats.
const QUESTION_ID: u16 = 0x7762;

/// How long the phone has to answer the handshake, and the resolver the question.
const LIMIT: Duration = Duration::from_secs(5);

#[test]
#[ignore = "needs a running phone: WIREBRIDGE_CONF=<client config> cargo test -p wirebridge-core --test phone -- --ignored --nocapture"]
fn through_the_phone() {
    let path = std::env::var("WIREBRIDGE_CONF").expect("WIREBRIDGE_CONF names a client's config");
    let conf = Conf::read(&path);
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect(conf.endpoint).unwrap();
    socket.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let mut wg = Wg { socket, tunn: Tunn::new(conf.private_key, conf.public_key, None, None, 1, None) };

    let started = Instant::now();
    wg.initiate();
    while wg.tunn.time_since_last_handshake().is_none() {
        assert!(started.elapsed() < LIMIT, "no handshake answer from {} in {LIMIT:?}", conf.endpoint);
        wg.receive();
    }
    println!("handshake with {} in {} ms", conf.endpoint, started.elapsed().as_millis());

    let mut question = Vec::new();
    PacketBuilder::ipv4(conf.address.octets(), RESOLVER.octets(), 64)
        .udp(53_000, 53)
        .write(&mut question, &whoami_question())
        .unwrap();
    let asked = Instant::now();
    let mut last_sent = None::<Instant>;
    loop {
        assert!(asked.elapsed() < LIMIT, "no answer from {RESOLVER} through the tunnel in {LIMIT:?}");
        // A datagram lost on Wi-Fi is asked again.
        if last_sent.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            wg.send_ip(&question);
            last_sent = Some(Instant::now());
        }
        for packet in wg.receive() {
            let Ok(sliced) = SlicedPacket::from_ip(&packet) else { continue };
            let Some(TransportSlice::Udp(udp)) = sliced.transport else { continue };
            if let Some(address) = whoami_answer(udp.payload()) {
                println!("{RESOLVER} answered in {} ms: asked from {address}", asked.elapsed().as_millis());
                return;
            }
        }
        wg.tick();
    }
}

/// The parts of a client's config this check uses.
struct Conf {
    private_key: boringtun::x25519::StaticSecret,
    address: Ipv4Addr,
    public_key: boringtun::x25519::PublicKey,
    endpoint: SocketAddr,
}

impl Conf {
    fn read(path: &str) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        // `Key = value`; a key's base64 ends in `=`, so only the first one splits.
        let value = |key: &str| {
            text.lines()
                .filter_map(|line| line.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().to_string())
                .unwrap_or_else(|| panic!("{path} has no {key}"))
        };
        let address = value("Address");
        Self {
            private_key: parse_private_key(&value("PrivateKey")).unwrap(),
            address: address.split('/').next().unwrap().parse().unwrap(),
            public_key: parse_public_key(&value("PublicKey")).unwrap(),
            endpoint: value("Endpoint").parse().unwrap(),
        }
    }
}

/// A client's half of WireGuard, over a plain UDP socket.
struct Wg {
    socket: UdpSocket,
    tunn: Tunn,
}

impl Wg {
    fn initiate(&mut self) {
        let mut scratch = vec![0u8; 2048];
        match self.tunn.format_handshake_initiation(&mut scratch, false) {
            TunnResult::WriteToNetwork(initiation) => {
                self.socket.send(initiation).unwrap();
            }
            other => panic!("no handshake initiation: {other:?}"),
        }
    }

    fn send_ip(&mut self, packet: &[u8]) {
        let mut scratch = vec![0u8; 65_536];
        if let TunnResult::WriteToNetwork(datagram) = self.tunn.encapsulate(packet, &mut scratch) {
            self.socket.send(datagram).unwrap();
        }
    }

    /// Everything that has arrived, decrypted. Waits up to the read timeout for the first datagram.
    fn receive(&mut self) -> Vec<Vec<u8>> {
        let mut datagram = vec![0u8; 65_536];
        let mut scratch = vec![0u8; 65_536];
        let mut packets = Vec::new();
        while let Ok(n) = self.socket.recv(&mut datagram) {
            match self.tunn.decapsulate(None, &datagram[..n], &mut scratch) {
                TunnResult::WriteToNetwork(reply) => {
                    self.socket.send(reply).unwrap();
                    while let TunnResult::WriteToNetwork(more) = self.tunn.decapsulate(None, &[], &mut scratch) {
                        self.socket.send(more).unwrap();
                    }
                }
                TunnResult::WriteToTunnelV4(packet, _) => packets.push(packet.to_vec()),
                _ => {}
            }
        }
        packets
    }

    fn tick(&mut self) {
        let mut scratch = vec![0u8; 2048];
        if let TunnResult::WriteToNetwork(datagram) = self.tunn.update_timers(&mut scratch) {
            self.socket.send(datagram).unwrap();
        }
    }
}

/// `whoami.cloudflare`, class CH, type TXT, recursion desired.
fn whoami_question() -> Vec<u8> {
    let mut question = QUESTION_ID.to_be_bytes().to_vec();
    question.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in ["whoami", "cloudflare"] {
        question.push(label.len() as u8);
        question.extend_from_slice(label.as_bytes());
    }
    question.extend_from_slice(&[0, 0, 16, 0, 3]);
    question
}

/// The first string of the first answer, if `reply` answers [`whoami_question`].
fn whoami_answer(reply: &[u8]) -> Option<String> {
    if reply.get(..2)? != QUESTION_ID.to_be_bytes() || u16::from_be_bytes([*reply.get(6)?, *reply.get(7)?]) == 0 {
        return None;
    }
    // The question (its name, type and class), then the answer's name, type, class and time to live.
    let at = skip_name(reply, skip_name(reply, 12)? + 4)? + 8;
    let length = usize::from(u16::from_be_bytes([*reply.get(at)?, *reply.get(at + 1)?]));
    let data = reply.get(at + 2..at + 2 + length)?;
    let text = data.get(1..1 + usize::from(*data.first()?))?;
    String::from_utf8(text.to_vec()).ok()
}

/// Where the name at `at` ends: after its last label, or after a pointer to an earlier name.
fn skip_name(message: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let length = *message.get(at)?;
        if length == 0 {
            return Some(at + 1);
        }
        if length & 0xc0 == 0xc0 {
            return Some(at + 2);
        }
        at += 1 + usize::from(length);
    }
}

#[test]
fn reads_a_whoami_answer() {
    // As 1.1.1.1 answers: the question, then one TXT record whose name points back at the question's.
    let mut reply = whoami_question();
    reply[2] = 0x81;
    reply[3] = 0x80;
    reply[7] = 1;
    reply.extend_from_slice(&[0xc0, 12, 0, 16, 0, 3, 0, 0, 0, 0, 0, 14, 13]);
    reply.extend_from_slice(b"203.0.113.200");
    assert_eq!(whoami_answer(&reply).as_deref(), Some("203.0.113.200"));
    assert_eq!(whoami_answer(&whoami_question()), None, "a question with no answer");
}
