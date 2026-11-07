/// What the Android app can be told went wrong.
///
/// The fields are called `reason`, not `message`: UniFFI turns each variant into a Kotlin exception, and a property
/// called `message` would hide `Throwable.message` so the generated file does not compile (beyondmap's docs/android.md
/// records the same trap).
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("invalid key: {reason}")]
    InvalidKey { reason: String },
    #[error("could not start: {reason}")]
    Start { reason: String },
    /// A client's config was asked for an endpoint that is not an IPv4 `address:port`.
    #[error("invalid endpoint: {reason}")]
    InvalidEndpoint { reason: String },
    /// A client's address inside the tunnel is not a plain IPv4 address, or two clients were given the same one.
    #[error("invalid address: {reason}")]
    InvalidAddress { reason: String },
}
