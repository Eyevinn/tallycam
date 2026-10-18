//! Standalone TallyCam hub: the `tallycam-hub` library served on its own.
//!
//! Usage: control-hub [--listen ADDR] [--public-url URL] [--srt-url URL] [--state-file PATH]
//! [--open] [--operator-token TOKEN] [--preview-fps N] [--mains-hz HZ] [--shutter mains|free|N]
//! (env LCP_HUB_LISTEN, LCP_HUB_PUBLIC_URL, LCP_HUB_SRT_URL, LCP_HUB_STATE_FILE, LCP_HUB_OPEN=1,
//! LCP_HUB_OPERATOR_TOKEN, LCP_HUB_PREVIEW_FPS, LCP_HUB_MAINS_HZ, LCP_HUB_SHUTTER)

use std::net::SocketAddr;
use std::sync::Arc;

use tallycam_hub::hub::DEFAULT_PREVIEW_FPS;
use tallycam_hub::protocol::{ShutterPolicy, CONTROL_PATH, DEVICE_PATH};
use tallycam_hub::provision::{FixedSrt, Provisioning};
use tallycam_hub::{Hub, JsonFileStore};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_LISTEN: &str = "0.0.0.0:8090";

struct Args {
    listen: SocketAddr,
    public_url: Option<String>,
    srt_url: Option<String>,
    state_file: Option<String>,
    open: bool,
    operator_token: Option<String>,
    preview_fps: Option<f64>,
    /// Given explicitly: replace the stored production's values.
    mains_hz: Option<u32>,
    shutter: Option<ShutterPolicy>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut listen = env("LCP_HUB_LISTEN");
    let mut public_url = env("LCP_HUB_PUBLIC_URL");
    let mut srt_url = env("LCP_HUB_SRT_URL");
    let mut state_file = env("LCP_HUB_STATE_FILE");
    // Invite-only unless asked to be open (a closed test network).
    let mut open = env("LCP_HUB_OPEN").is_some_and(|v| v != "0" && v != "false");
    let mut operator_token = env("LCP_HUB_OPERATOR_TOKEN");
    let mut preview_fps = env("LCP_HUB_PREVIEW_FPS");
    let mut mains_hz = env("LCP_HUB_MAINS_HZ");
    let mut shutter = env("LCP_HUB_SHUTTER");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" | "-l" => listen = Some(args.next().ok_or("--listen needs an address")?),
            "--public-url" => public_url = Some(args.next().ok_or("--public-url needs a URL")?),
            "--srt-url" => srt_url = Some(args.next().ok_or("--srt-url needs a URL")?),
            "--state-file" => state_file = Some(args.next().ok_or("--state-file needs a path")?),
            "--open" => open = true,
            // The default since invites became it; kept for old scripts.
            "--invites" => open = false,
            "--operator-token" => {
                operator_token = Some(args.next().ok_or("--operator-token needs a value")?)
            }
            "--preview-fps" => {
                preview_fps = Some(args.next().ok_or("--preview-fps needs a number")?)
            }
            "--mains-hz" => mains_hz = Some(args.next().ok_or("--mains-hz needs 50, 60 or 0")?),
            "--shutter" => {
                shutter = Some(
                    args.next()
                        .ok_or("--shutter needs mains, free or a number")?,
                )
            }
            "--help" | "-h" => {
                println!(
                    "control-hub [--listen ADDR] [--public-url URL] [--srt-url URL] [--state-file PATH]\n\
                     \x20           [--open] [--operator-token TOKEN] [--preview-fps N]\n\
                     \x20           [--mains-hz HZ] [--shutter mains|free|N]\n\n\
                     --listen      ADDR to listen on ($LCP_HUB_LISTEN, default {DEFAULT_LISTEN}).\n\
                     --public-url  control URL handed to phones instead of ws://<lan ip>:<port>{DEVICE_PATH}\n\
                     \x20             (behind a proxy; $LCP_HUB_PUBLIC_URL).\n\
                     --srt-url     SRT URL handed to phones ($LCP_HUB_SRT_URL), default\n\
                     \x20             srt://<lan ip>:9000?streamid={{device}}&latency=100. The app\n\
                     \x20             replaces {{device}} with its own stream id.\n\
                     --state-file  JSON file that keeps the known devices and the production\n\
                     \x20             settings across restarts ($LCP_HUB_STATE_FILE). Default:\n\
                     \x20             none, the list starts empty.\n\
                     --open        any phone on the network joins, and the page needs no token\n\
                     \x20             unless --operator-token is given ($LCP_HUB_OPEN=1). Only for a\n\
                     \x20             closed test network. Default: a new phone needs a one-time\n\
                     \x20             invite (POST /api/invite, or \"Add a camera\" on the page) and\n\
                     \x20             a device key; known phones prove their key.\n\
                     --operator-token  token the operator page, invites, forgetting devices and\n\
                     \x20             camera control need ($LCP_HUB_OPERATOR_TOKEN). Without\n\
                     \x20             --open and none given, one is made and the page link printed.\n\
                     --preview-fps preview images per second from each phone while the page is\n\
                     \x20             open, 0.1 to 10 ($LCP_HUB_PREVIEW_FPS, default {DEFAULT_PREVIEW_FPS}).\n\
                     --mains-hz    mains frequency of the production's lights: 50, 60, or 0 for\n\
                     \x20             none ($LCP_HUB_MAINS_HZ, default 50). Sent to phones with\n\
                     \x20             protocol 2; the operator page can change it (live cameras apply\n\
                     \x20             it when their stream stops).\n\
                     --shutter     production shutter: mains (one mains period, default), free\n\
                     \x20             (no mains-lit light: the shutter is set in manual exposure),\n\
                     \x20             or N for 1/N s ($LCP_HUB_SHUTTER). Given, these two replace\n\
                     \x20             the production kept in --state-file.\n\
                     Log level: RUST_LOG (default info)."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    for (flag, url, schemes) in [
        ("--public-url", &public_url, &["ws://", "wss://"][..]),
        ("--srt-url", &srt_url, &["srt://"][..]),
    ] {
        if let Some(url) = url {
            if !schemes.iter().any(|s| url.starts_with(s)) {
                return Err(format!("{flag} must start with {}", schemes.join(" or ")));
            }
        }
    }
    let preview_fps = match preview_fps {
        Some(text) => match text.parse::<f64>() {
            Ok(fps) if (0.1..=10.0).contains(&fps) => Some(fps),
            _ => {
                return Err(format!(
                    "--preview-fps must be a number from 0.1 to 10, not '{text}'"
                ))
            }
        },
        None => None,
    };
    let mains_hz = match mains_hz {
        Some(text) => match text.parse() {
            Ok(hz @ (0 | 50 | 60)) => Some(hz),
            _ => return Err(format!("--mains-hz must be 50, 60 or 0, not '{text}'")),
        },
        None => None,
    };
    let shutter = shutter.as_deref().map(parse_shutter).transpose()?;
    // An empty token would let an empty header or `?token=` through.
    if operator_token
        .as_deref()
        .is_some_and(|t| t.trim().is_empty())
    {
        return Err("--operator-token must not be empty".into());
    }
    let listen = listen.unwrap_or_else(|| DEFAULT_LISTEN.to_string());
    let listen = listen
        .parse()
        .map_err(|e| format!("bad listen address '{listen}': {e}"))?;
    Ok(Args {
        listen,
        public_url,
        srt_url,
        state_file,
        open,
        operator_token,
        preview_fps,
        mains_hz,
        shutter,
    })
}

/// `mains`, `free` or the denominator of a fixed shutter (`100` for 1/100 s).
fn parse_shutter(text: &str) -> Result<ShutterPolicy, String> {
    match text {
        "mains" => Ok(ShutterPolicy::Mains),
        "free" => Ok(ShutterPolicy::Free),
        n => match n.trim_start_matches("1/").parse::<u32>() {
            Ok(x) if (1..=100_000).contains(&x) => Ok(ShutterPolicy::Fixed(x)),
            _ => Err(format!(
                "--shutter must be mains, free or a number like 100, not '{text}'"
            )),
        },
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("control-hub: {e}");
            std::process::exit(2);
        }
    };

    let addr = args.listen;
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("control-hub: cannot listen on {addr}: {e}");
            std::process::exit(1);
        }
    };
    let mut provisioning = Provisioning::new(listener.local_addr().unwrap_or(addr));
    provisioning.public_url = args.public_url;
    if let Some(url) = args.srt_url {
        provisioning.srt = Arc::new(FixedSrt(url));
    }
    let hub = match &args.state_file {
        Some(path) => {
            info!("Keeping known devices in {path}");
            Hub::with_store(Arc::new(JsonFileStore::open(path)))
        }
        None => Hub::new(),
    };
    if let Some(fps) = args.preview_fps {
        hub.set_preview_fps(fps);
    }
    info!(
        "Previews at {} per second while a controller is connected",
        args.preview_fps.unwrap_or(DEFAULT_PREVIEW_FPS)
    );
    // The stored production (or the default), with what the flags say.
    let mut production = hub.production();
    if let Some(hz) = args.mains_hz {
        production.mains_hz = hz;
    }
    if let Some(shutter) = args.shutter {
        production.shutter = shutter;
    }
    info!(
        "Starting with production: mains {} Hz, shutter {:?}",
        production.mains_hz, production.shutter
    );
    if let Err(e) = hub.set_production(production) {
        eprintln!("control-hub: {e}");
        std::process::exit(2);
    }
    if !args.open {
        hub.set_invite_required(true);
        info!("New phones need an invite and a device key");
    }
    info!("LCP control hub listening on http://{addr}");
    info!("Devices connect to ws://<host>{DEVICE_PATH}, operator UIs to ws://<host>{CONTROL_PATH}");
    for target in provisioning.info().targets {
        info!(
            "Advertising {} and {}",
            target.control_url,
            tallycam_hub::hub::redact_srt_url(&target.srt_url)
        );
    }
    // Not open, the hub is closed: the operator page, invites,
    // forgetting devices and camera control then need a token.
    let operator_token = args
        .operator_token
        .or_else(|| (!args.open).then(tallycam_hub::server::generate_operator_token));
    if let Some(token) = &operator_token {
        // The page lives where the phones' control URL points (the public URL
        // behind a proxy), with http for ws and https for wss.
        let mut origins: Vec<String> = provisioning
            .info()
            .targets
            .iter()
            .filter_map(|t| page_origin(&t.control_url))
            .collect();
        if origins.is_empty() {
            origins.push(format!("http://{}", provisioning.listen));
        }
        for origin in origins {
            info!("Operator page: {origin}/#token={token}");
        }
    }
    if operator_token.is_none() {
        warn!(
            "Open hub (--open): anyone who can reach {addr} can join a camera, control every \
             camera and send its picture anywhere. Only for a closed test network; otherwise \
             leave --open out (invites, an operator token and device keys) and run behind TLS \
             (docs/SECURITY.md)."
        );
    }
    let options = tallycam_hub::server::ServerOptions { operator_token };
    let app = tallycam_hub::server::router_with(Arc::new(hub), provisioning, options);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server error");
}

/// `ws://host:port/path` -> `http://host:port` (`wss` -> `https`).
fn page_origin(control_url: &str) -> Option<String> {
    let (scheme, rest) = control_url.split_once("://")?;
    let scheme = match scheme {
        "ws" => "http",
        "wss" => "https",
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| format!("{scheme}://{authority}"))
}

#[cfg(test)]
mod tests {
    use super::{page_origin, parse_shutter};
    use tallycam_hub::protocol::ShutterPolicy;

    #[test]
    fn shutter_flag() {
        assert_eq!(parse_shutter("mains"), Ok(ShutterPolicy::Mains));
        assert_eq!(parse_shutter("free"), Ok(ShutterPolicy::Free));
        assert_eq!(parse_shutter("100"), Ok(ShutterPolicy::Fixed(100)));
        assert_eq!(parse_shutter("1/120"), Ok(ShutterPolicy::Fixed(120)));
        assert!(parse_shutter("0").is_err());
        assert!(parse_shutter("fast").is_err());
    }

    #[test]
    fn page_origin_follows_the_control_url() {
        assert_eq!(
            page_origin("ws://192.0.2.10:8090/lcp/v0/device").as_deref(),
            Some("http://192.0.2.10:8090")
        );
        assert_eq!(
            page_origin("wss://hub.example.com/lcp/v0/device?token=x").as_deref(),
            Some("https://hub.example.com")
        );
        assert_eq!(
            page_origin("ws://[2001:db8::1]:8090/lcp/v0/device").as_deref(),
            Some("http://[2001:db8::1]:8090")
        );
        assert_eq!(page_origin("srt://x"), None);
    }
}
