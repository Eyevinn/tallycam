//! Provisioning v0 ("scan to connect"): the addresses a phone can reach the
//! hub on, the control and SRT URLs to hand out, and the deep link a QR code
//! carries. See docs/PROTOCOL.md, "Provisioning v0".
//!
//! LAN address discovery needs the `lan` feature and the QR code the `qr`
//! feature. Without `lan`, a hub on 0.0.0.0 advertises nothing and the
//! embedding server sets `public_url`.

use std::fmt;
#[cfg(feature = "lan")]
use std::net::Ipv6Addr;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

#[cfg(feature = "qr")]
use qrcode::render::svg;
#[cfg(feature = "qr")]
use qrcode::QrCode;
use serde::Serialize;

use crate::protocol::DEVICE_PATH;

/// URL scheme of the app's deep link. The app keeps the same value in one
/// constant.
pub const DEEP_LINK_SCHEME: &str = "tallycam";
/// Host part of the deep link: `tallycam://connect?control=...&srt=...`.
pub const DEEP_LINK_ACTION: &str = "connect";
/// Query parameter of the control URL that carries a one-time invite token
/// (`ws://host/path?token=<token>`). The app connects to the control URL as
/// it is, so the token reaches the server without any app support.
pub const INVITE_TOKEN_PARAM: &str = "token";
/// Placeholder in an SRT URL that the app replaces with its own stream id.
pub const DEVICE_PLACEHOLDER: &str = "{device}";
/// SRT port of the default SRT URL (a Strom SRT input listening there).
pub const DEFAULT_SRT_PORT: u16 = 9000;
/// SRT latency of the default SRT URL.
pub const DEFAULT_SRT_LATENCY_MS: u32 = 100;

/// Where phones send their media. The embedding server decides: the
/// standalone hub points at an SRT listener on the same host, Strom at the
/// SRT input of the flow the camera belongs to.
pub trait SrtTargets: Send + Sync {
    /// The SRT URL for a phone that reaches the hub at `host` (an IP, a
    /// bracketed IPv6 address or a DNS name). `{device}` in the URL is
    /// replaced by the app with its own stream id.
    fn srt_url(&self, host: &str) -> String;
}

/// `srt://<host>:<port>?streamid={device}&latency=<ms>`: an SRT listener on
/// the hub's own host.
#[derive(Debug, Clone, Copy)]
pub struct SameHostSrt {
    pub port: u16,
    pub latency_ms: u32,
}

impl Default for SameHostSrt {
    fn default() -> Self {
        Self {
            port: DEFAULT_SRT_PORT,
            latency_ms: DEFAULT_SRT_LATENCY_MS,
        }
    }
}

impl SrtTargets for SameHostSrt {
    fn srt_url(&self, host: &str) -> String {
        format!(
            "srt://{host}:{}?streamid={DEVICE_PLACEHOLDER}&latency={}",
            self.port, self.latency_ms
        )
    }
}

/// One fixed URL for every phone, whatever address it uses.
#[derive(Debug, Clone)]
pub struct FixedSrt(pub String);

impl SrtTargets for FixedSrt {
    fn srt_url(&self, _host: &str) -> String {
        self.0.clone()
    }
}

/// What the hub advertises. Built once at startup; addresses are enumerated
/// on every request (Wi-Fi and VPN addresses come and go).
#[derive(Clone)]
pub struct Provisioning {
    /// The address the hub listens on.
    pub listen: SocketAddr,
    /// Overrides the advertised control URL (a proxy, a DNS name).
    pub public_url: Option<String>,
    /// The SRT URL to hand out. Default: [`SameHostSrt`].
    pub srt: Arc<dyn SrtTargets>,
}

impl fmt::Debug for Provisioning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provisioning")
            .field("listen", &self.listen)
            .field("public_url", &self.public_url)
            .finish_non_exhaustive()
    }
}

/// `GET /api/info`.
#[derive(Debug, Clone, Serialize)]
pub struct Info {
    /// Port the hub listens on.
    pub port: u16,
    /// LAN addresses the phone may reach the hub on, best first.
    pub addresses: Vec<String>,
    /// One per address, or one for `--public-url`. The QR code at
    /// `/api/connect.svg?target=<index>` carries `targets[index].deep_link`.
    pub targets: Vec<Target>,
    /// Deep link scheme, for clients that build links themselves.
    pub scheme: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Target {
    /// Address or host this target is for (shown in a selector).
    pub label: String,
    pub control_url: String,
    pub srt_url: String,
    pub deep_link: String,
}

impl Provisioning {
    pub fn new(listen: SocketAddr) -> Self {
        Self {
            listen,
            public_url: None,
            srt: Arc::new(SameHostSrt::default()),
        }
    }

    /// The addresses to advertise. A hub bound to one address advertises
    /// only that one; a hub on 0.0.0.0 / :: advertises the LAN addresses.
    pub fn addresses(&self) -> Vec<IpAddr> {
        let ip = self.listen.ip();
        if !ip.is_unspecified() {
            return if ip.is_loopback() { vec![] } else { vec![ip] };
        }
        self.lan_addresses()
    }

    #[cfg(not(feature = "lan"))]
    fn lan_addresses(&self) -> Vec<IpAddr> {
        Vec::new()
    }

    #[cfg(feature = "lan")]
    fn lan_addresses(&self) -> Vec<IpAddr> {
        let mut found: Vec<(u8, IpAddr)> = if_addrs::get_if_addrs()
            .unwrap_or_default()
            .into_iter()
            .map(|i| (interface_rank(&i.name), i.ip()))
            .filter(|(_, ip)| is_advertisable(*ip))
            .filter(|(_, ip)| self.listen.is_ipv6() || ip.is_ipv4())
            .collect();
        // IPv4 first, then physical interfaces before VPN tunnels and VM
        // bridges, then the system's order (the sort is stable).
        found.sort_by_key(|(rank, ip)| (ip.is_ipv6(), *rank));
        let mut out: Vec<IpAddr> = Vec::new();
        for (_, ip) in found {
            if !out.contains(&ip) {
                out.push(ip);
            }
        }
        out
    }

    pub fn info(&self) -> Info {
        let addresses = self.addresses();
        let port = self.listen.port();
        let targets = match &self.public_url {
            Some(url) => {
                let label = host_of(url).unwrap_or_else(|| url.clone());
                let srt = self
                    .srt
                    .srt_url(&host_of(url).unwrap_or_else(|| "localhost".into()));
                vec![Target::new(label, url.clone(), srt)]
            }
            None => addresses
                .iter()
                .map(|ip| {
                    let host = url_host(*ip);
                    let control = format!("ws://{host}:{port}{DEVICE_PATH}");
                    let srt = self.srt.srt_url(&host);
                    Target::new(ip.to_string(), control, srt)
                })
                .collect(),
        };
        Info {
            port,
            addresses: addresses.iter().map(IpAddr::to_string).collect(),
            targets,
            scheme: DEEP_LINK_SCHEME,
        }
    }
}

impl Target {
    fn new(label: String, control_url: String, srt_url: String) -> Self {
        let deep_link = deep_link(&control_url, &srt_url);
        Self {
            label,
            control_url,
            srt_url,
            deep_link,
        }
    }
}

/// `tallycam://connect?control=<ws url>&srt=<srt url>`, both
/// percent-encoded.
pub fn deep_link(control_url: &str, srt_url: &str) -> String {
    format!(
        "{DEEP_LINK_SCHEME}://{DEEP_LINK_ACTION}?control={}&srt={}",
        percent_encode(control_url),
        percent_encode(srt_url)
    )
}

/// `tallycam://connect?control=<ws url>` with an optional SRT URL. A server
/// that pushes the stream URL later (Strom) leaves it out; the phone then
/// keeps its current one.
pub fn deep_link_control(control_url: &str, srt_url: Option<&str>) -> String {
    match srt_url {
        Some(srt) => deep_link(control_url, srt),
        None => format!(
            "{DEEP_LINK_SCHEME}://{DEEP_LINK_ACTION}?control={}",
            percent_encode(control_url)
        ),
    }
}

/// `control_url` with the invite token as its `token` query parameter.
pub fn with_invite_token(control_url: &str, token: &str) -> String {
    let sep = if control_url.contains('?') { '&' } else { '?' };
    format!(
        "{control_url}{sep}{INVITE_TOKEN_PARAM}={}",
        percent_encode(token)
    )
}

/// The QR code for `data` as SVG, one unit per module, with a quiet zone.
/// Scale it with CSS (`shape-rendering="crispEdges"` keeps it sharp).
#[cfg(feature = "qr")]
pub fn qr_svg(data: &str) -> Result<String, String> {
    let code = QrCode::new(data.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<svg::Color>()
        .module_dimensions(1, 1)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// Not loopback, not link-local, not unspecified; IPv6 only when global.
#[cfg(feature = "lan")]
fn is_advertisable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
        IpAddr::V6(v6) => is_global_v6(v6),
    }
}

/// Global unicast (2000::/3).
#[cfg(feature = "lan")]
fn is_global_v6(v6: Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xe000) == 0x2000
}

/// Lower is better: physical Ethernet/Wi-Fi, then anything else, then VPN
/// tunnels, VM bridges and container networks.
#[cfg(feature = "lan")]
fn interface_rank(name: &str) -> u8 {
    const PHYSICAL: [&str; 4] = ["en", "eth", "wl", "wlan"];
    const VIRTUAL: [&str; 9] = [
        "utun", "tun", "tap", "bridge", "docker", "br-", "veth", "vmnet", "vboxnet",
    ];
    if VIRTUAL.iter().any(|p| name.starts_with(p)) {
        2
    } else if PHYSICAL.iter().any(|p| name.starts_with(p)) {
        0
    } else {
        1
    }
}

fn url_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}

/// Host part of `scheme://host[:port]/...` (brackets kept for IPv6).
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// RFC 3986: everything but unreserved characters is encoded.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3 / 2);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_link_is_percent_encoded() {
        let l = deep_link(
            "ws://192.0.2.10:8090/lcp/v0/device",
            "srt://192.0.2.10:9000?streamid={device}&latency=100",
        );
        assert_eq!(
            l,
            "tallycam://connect?control=ws%3A%2F%2F192.0.2.10%3A8090%2Flcp%2Fv0%2Fdevice\
             &srt=srt%3A%2F%2F192.0.2.10%3A9000%3Fstreamid%3D%7Bdevice%7D%26latency%3D100"
        );
    }

    #[test]
    fn invite_token_rides_in_the_control_url() {
        let control = with_invite_token("ws://192.0.2.10:8080/api/cameras/device", "ab12");
        assert_eq!(
            control,
            "ws://192.0.2.10:8080/api/cameras/device?token=ab12"
        );
        assert_eq!(
            with_invite_token("wss://h.example.com/d?x=1", "t"),
            "wss://h.example.com/d?x=1&token=t"
        );
        // Without an SRT URL the link has only the control parameter; the
        // app keeps its stream URL.
        assert_eq!(
            deep_link_control(&control, None),
            "tallycam://connect?control=ws%3A%2F%2F192.0.2.10%3A8080%2Fapi%2Fcameras%2Fdevice%3Ftoken%3Dab12"
        );
        assert_eq!(
            deep_link_control("ws://h/d", Some("srt://h:1")),
            deep_link("ws://h/d", "srt://h:1")
        );
    }

    #[test]
    fn bound_address_is_advertised_alone() {
        let p = Provisioning::new("192.0.2.10:8090".parse().unwrap());
        let info = p.info();
        assert_eq!(info.addresses, ["192.0.2.10"]);
        assert_eq!(
            info.targets[0].control_url,
            "ws://192.0.2.10:8090/lcp/v0/device"
        );
        assert_eq!(
            info.targets[0].srt_url,
            "srt://192.0.2.10:9000?streamid={device}&latency=100"
        );
        let loopback = Provisioning::new("127.0.0.1:8090".parse().unwrap());
        assert!(
            loopback.info().targets.is_empty(),
            "never advertise localhost"
        );
    }

    #[test]
    fn public_url_overrides() {
        let mut p = Provisioning::new("0.0.0.0:8090".parse().unwrap());
        p.public_url = Some("wss://hub.example.com/lcp/v0/device".into());
        let info = p.info();
        assert_eq!(info.targets.len(), 1);
        assert_eq!(info.targets[0].label, "hub.example.com");
        assert_eq!(
            info.targets[0].control_url,
            "wss://hub.example.com/lcp/v0/device"
        );
        assert_eq!(
            info.targets[0].srt_url,
            "srt://hub.example.com:9000?streamid={device}&latency=100"
        );
    }

    #[test]
    fn fixed_srt_url_for_every_target() {
        let mut p = Provisioning::new("192.0.2.10:8090".parse().unwrap());
        p.srt = Arc::new(FixedSrt(
            "srt://media.example.com:9000?streamid={device}".into(),
        ));
        assert_eq!(
            p.info().targets[0].srt_url,
            "srt://media.example.com:9000?streamid={device}"
        );
    }

    #[test]
    fn host_of_url() {
        assert_eq!(
            host_of("ws://[2001:db8::1]:8090/x").as_deref(),
            Some("[2001:db8::1]")
        );
        assert_eq!(host_of("ws://u@h:1/x").as_deref(), Some("h"));
    }

    #[cfg(feature = "lan")]
    #[test]
    fn filters_and_ranks() {
        assert!(!is_advertisable("127.0.0.1".parse().unwrap()));
        assert!(!is_advertisable("169.254.1.2".parse().unwrap()));
        assert!(is_advertisable("192.168.1.2".parse().unwrap()));
        assert!(!is_advertisable("fe80::1".parse().unwrap()));
        assert!(!is_advertisable("fd00::1".parse().unwrap()));
        assert!(is_advertisable("2001:db8::1".parse().unwrap()));
        assert!(interface_rank("en0") < interface_rank("bridge100"));
        assert!(interface_rank("en0") < interface_rank("utun3"));
    }
}
