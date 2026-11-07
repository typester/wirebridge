//! The phone half of wirebridge.
//!
//! WireGuard from any number of clients (the Mac, a tablet, ...) is terminated here, in user space, and every TCP
//! connection and UDP flow inside the tunnel is re-originated from this process's own sockets. Nothing is forwarded at
//! the IP layer, which an app without root cannot do: the phone's network stack only ever sees an app opening ordinary
//! connections. See ../../README.md.
//!
//! The pieces:
//! - [`wg`] runs boringtun's `Tunn`, one per client, over one UDP socket: tells the clients' datagrams apart, decrypts
//!   what they send, encrypts what goes back.
//! - [`stack`] terminates the clients' TCP and UDP and dials each flow's real destination: smoltcp, one stack per TCP
//!   connection. smoltcp, because on a lossy link it recovers without wasting much, where ipstack retransmitted
//!   without restraint.
//! - [`server`] owns the runtime and is the object the Android app drives through UniFFI.
//! - [`conf`] writes a client's config for the official WireGuard apps, which the app serves, shows and shares.

uniffi::setup_scaffolding!();

pub mod conf;
pub mod defaults;
mod error;
mod keys;
mod logging;
mod server;
mod stack;
mod stats;
pub mod wg;

pub use error::CoreError;
pub use keys::{encode_key, generate_private_key, parse_private_key, parse_public_key, public_key_from_private};
pub use logging::init_logging;
pub use server::{Client, ClientConfig, MTU, Server, ServerConfig};
pub use stats::{ClientStats, Counters, ServerStats};
