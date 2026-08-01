//! Works out the address to hand out for the web UI.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use crate::config::Config;

/// The URL the bot replies with, and the one logged at startup.
///
/// `WEB_URL` wins when it is set. Otherwise the port comes from `BIND_ADDR`,
/// and the host from that same setting unless it is the wildcard address — in
/// which case the machine's own LAN address is looked up, since `0.0.0.0` is
/// not something a phone can open.
pub fn web_url(config: &Config) -> String {
    if let Some(url) = &config.web_url {
        return url.clone();
    }

    let host = match config.bind.ip() {
        ip if ip.is_unspecified() => lan_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        ip => ip,
    };

    match host {
        // IPv6 literals have to be bracketed in a URL.
        IpAddr::V6(ip) => format!("http://[{ip}]:{}", config.bind.port()),
        IpAddr::V4(ip) => format!("http://{ip}:{}", config.bind.port()),
    }
}

/// The address of the interface that carries the default route.
///
/// UDP is connectionless, so this sends nothing: `connect` only asks the
/// routing table which local address would be used to reach that destination.
/// The destination is a reserved documentation address for that reason — it
/// never has to exist. Returns `None` on a host with no route at all.
fn lan_ip() -> Option<IpAddr> {
    // TEST-NET-3, reserved by RFC 5737 for documentation.
    const PROBE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)), 80);

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(PROBE).ok()?;
    let address = socket.local_addr().ok()?.ip();

    if address.is_unspecified() || address.is_loopback() { None } else { Some(address) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn config(bind: &str, web_url: Option<&str>) -> Config {
        Config {
            save_dir: PathBuf::from("downloads"),
            bind: bind.parse().unwrap(),
            frontend_dir: PathBuf::from("dist/web"),
            max_upload_bytes: 1,
            bot_token: None,
            web_url: web_url.map(str::to_owned),
        }
    }

    #[test]
    fn explicit_url_wins() {
        assert_eq!(web_url(&config("0.0.0.0:8080", Some("https://media.example.com"))), "https://media.example.com");
    }

    #[test]
    fn keeps_an_explicit_bind_address() {
        assert_eq!(web_url(&config("192.168.1.50:9000", None)), "http://192.168.1.50:9000");
    }

    #[test]
    fn brackets_ipv6() {
        assert_eq!(web_url(&config("[fd00::1]:8080", None)), "http://[fd00::1]:8080");
    }

    #[test]
    fn wildcard_resolves_to_something_reachable() {
        let url = web_url(&config("0.0.0.0:8080", None));
        assert!(url.starts_with("http://"), "{url}");
        assert!(url.ends_with(":8080"), "{url}");
        assert!(!url.contains("0.0.0.0"), "{url}");
    }
}
