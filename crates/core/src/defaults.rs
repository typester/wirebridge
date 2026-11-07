//! The tunnel's fixed values: what the phone's server listens on, and what [`crate::conf`] writes for every client.
//!
//! Each client's address inside the tunnel is the app's to give out (10.77.0.2 to 10.77.0.254). IPv4 only: given an
//! IPv6 address too, a client's apps try IPv6, and where the phone has no IPv6 to the internet (home Wi-Fi had only ULA)
//! every one of those attempts fails. The phone needs no address of its own in the tunnel: it terminates every flow,
//! whatever destination the flow names.

/// The UDP port the phone's WireGuard listens on.
pub const PORT: u16 = 51820;

/// The resolver, reached through the tunnel as an ordinary UDP flow. IPv4 for the same reason.
pub const DNS: &str = "1.1.1.1";

/// Everything goes into the tunnel, IPv6 included even though a client gets no IPv6 address in it: routed into a tunnel
/// with no IPv6 source, IPv6 goes nowhere instead of leaking out of the link beside it.
pub const ALL_TRAFFIC: [&str; 2] = ["0.0.0.0/0", "::/0"];

/// Keeps the phone's view of each client's address fresh across NAT and roaming.
pub const PERSISTENT_KEEPALIVE: u16 = 25;
