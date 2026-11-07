//! A client's WireGuard config. The app serves it over the phone's Wi-Fi Direct group, shows it as a QR code and shares
//! it; the official WireGuard apps import it.

use std::net::{Ipv4Addr, SocketAddrV4};

use crate::keys::{encode_key, parse_private_key, parse_public_key};
use crate::{CoreError, MTU, defaults};

/// A client's config, as a WireGuard `.conf`: the client's own private key and its address inside the tunnel, the
/// phone's public key, the phone at `endpoint` (an IPv4 `address:port`), and the fixed values in [`defaults`].
/// Everything goes through the tunnel, DNS included.
#[uniffi::export]
pub fn client_config(
    client_private_key: String,
    client_address: String,
    phone_public_key: String,
    endpoint: String,
) -> Result<String, CoreError> {
    let private_key = parse_private_key(&client_private_key)?;
    let address = parse_client_address(&client_address)?;
    let phone_key = parse_public_key(&phone_public_key)?;
    let endpoint: SocketAddrV4 = endpoint
        .trim()
        .parse()
        .map_err(|e| CoreError::InvalidEndpoint { reason: format!("{endpoint:?}: {e}") })?;
    let lines = [
        "# A Wirebridge client, made on the phone.".to_string(),
        "[Interface]".to_string(),
        format!("PrivateKey = {}", encode_key(&private_key.to_bytes())),
        format!("Address = {address}/32"),
        format!("DNS = {}", defaults::DNS),
        format!("MTU = {MTU}"),
        String::new(),
        "[Peer]".to_string(),
        format!("PublicKey = {}", encode_key(phone_key.as_bytes())),
        format!("Endpoint = {endpoint}"),
        format!("AllowedIPs = {}", defaults::ALL_TRAFFIC.join(", ")),
        format!("PersistentKeepalive = {}", defaults::PERSISTENT_KEEPALIVE),
    ];
    Ok(lines.join("\n") + "\n")
}

/// A client's address inside the tunnel: a plain IPv4 address, without a prefix length.
pub(crate) fn parse_client_address(text: &str) -> Result<Ipv4Addr, CoreError> {
    text.trim().parse().map_err(|e| CoreError::InvalidAddress { reason: format!("{text:?}: {e}") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{generate_private_key, public_key_from_private};

    fn keys() -> (String, String) {
        (generate_private_key(), public_key_from_private(generate_private_key()).unwrap())
    }

    #[test]
    fn writes_the_full_tunnel() {
        let (client, phone) = keys();
        let conf = client_config(client.clone(), "10.77.0.3".into(), phone.clone(), "192.168.49.1:51820".into()).unwrap();
        let lines: Vec<&str> = conf.lines().collect();
        let expected = [
            "[Interface]".to_string(),
            format!("PrivateKey = {client}"),
            "Address = 10.77.0.3/32".to_string(),
            "DNS = 1.1.1.1".to_string(),
            "MTU = 1420".to_string(),
            "[Peer]".to_string(),
            format!("PublicKey = {phone}"),
            "Endpoint = 192.168.49.1:51820".to_string(),
            "AllowedIPs = 0.0.0.0/0, ::/0".to_string(),
            "PersistentKeepalive = 25".to_string(),
        ];
        for line in &expected {
            assert!(lines.contains(&line.as_str()), "missing {line:?} in\n{conf}");
        }
        assert!(conf.ends_with('\n'));
    }

    #[test]
    fn refuses_what_it_cannot_write() {
        let (client, phone) = keys();
        let address = "10.77.0.2".to_string();
        let endpoint = "192.168.49.1:51820".to_string();
        let bad_client = client_config("nope".into(), address.clone(), phone.clone(), endpoint.clone());
        assert!(matches!(bad_client, Err(CoreError::InvalidKey { .. })));
        let bad_phone = client_config(client.clone(), address.clone(), "nope".into(), endpoint.clone());
        assert!(matches!(bad_phone, Err(CoreError::InvalidKey { .. })));
        for bad in ["10.77.0.300", "10.77.0.2/32", "fd00::2", ""] {
            let result = client_config(client.clone(), bad.into(), phone.clone(), endpoint.clone());
            assert!(matches!(result, Err(CoreError::InvalidAddress { .. })), "{bad}: {result:?}");
        }
        for bad in ["192.168.49.1", "[::1]:51820", "phone:51820", ""] {
            let result = client_config(client.clone(), address.clone(), phone.clone(), bad.into());
            assert!(matches!(result, Err(CoreError::InvalidEndpoint { .. })), "{bad}: {result:?}");
        }
    }
}
