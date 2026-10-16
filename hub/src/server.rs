//! Axum routes and WebSocket glue between sockets and the [`Hub`].
//!
//! [`router`] is the whole standalone hub: web UI, provisioning API and both
//! WebSocket endpoints. A server with its own routes (Strom, with its OpenAPI
//! annotations) upgrades the socket itself and hands it to
//! [`device_session`] or [`controller_session`].

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRef, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, timeout, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::hub::{check_hello_key, AdmitError, Challenge, Hub, Proof, DEFAULT_INVITE_TTL};
use crate::protocol::{ControllerEvent, ControllerInput, DeviceMessage, CONTROL_PATH, DEVICE_PATH};
use crate::protocol::{Hello, ServerToDevice};
use crate::provision::{deep_link, qr_svg, with_invite_token, Provisioning};

/// WebSocket ping interval towards phones and controllers.
pub const PING_INTERVAL: Duration = Duration::from_secs(5);
/// A phone that sends nothing (not even a pong) for this long is offline.
pub const DEVICE_DEAD_AFTER: Duration = Duration::from_secs(10);
/// Same for operator UIs, a bit more lenient (background browser tabs).
pub const CONTROLLER_DEAD_AFTER: Duration = Duration::from_secs(30);
/// A phone that was sent a `challenge` must answer with `auth` within this.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

const INDEX_HTML: &str = include_str!("../web/index.html");
/// The digital clapper: flash and beep at whole seconds, to film for A/V sync.
pub const CLAPPER_HTML: &str = include_str!("../web/clapper.html");
/// A clickable join link for e-mail or messages: opens the deep link in its
/// URL fragment (`/join#tallycam://connect?...`) in the app.
pub const JOIN_HTML: &str = include_str!("../web/join.html");
/// Where [`operator_page`] puts its configuration script.
const CONFIG_MARKER: &str = "<!--TALLYCAM_CONFIG-->";

/// The operator web UI for an embedding server, with `config` as
/// `window.TALLYCAM_CONFIG` (keys: `controlPath`, `infoUrl`, `qrUrl`,
/// `clapperUrl`, `joinPath`, `inviteUrl`, `devicesUrl`, `title`; see the top of the
/// page's script).
pub fn operator_page(config: &serde_json::Value) -> String {
    // "</" inside a script would end it early.
    let json = config.to_string().replace("</", "<\\/");
    INDEX_HTML.replace(
        CONFIG_MARKER,
        &format!("<script>window.TALLYCAM_CONFIG = {json};</script>"),
    )
}

/// Options of the standalone server ([`router_with`]).
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    /// When set, creating invites, forgetting devices and the operator
    /// (controller) WebSocket need this token: as the `X-Operator-Token`
    /// header for HTTP (a header, so another site cannot forge the request
    /// from the operator's browser) and as `?token=` for the WebSocket.
    /// Unset: open, as a hub on a trusted LAN has always been.
    pub operator_token: Option<String>,
}

/// A fresh operator token: 128 random bits as hex.
pub fn generate_operator_token() -> String {
    crate::hub::random_token()
}

/// Header that carries [`ServerOptions::operator_token`].
pub const OPERATOR_TOKEN_HEADER: &str = "x-operator-token";

#[derive(Clone)]
struct AppState {
    hub: Arc<Hub>,
    provisioning: Arc<Provisioning>,
    operator_token: Option<Arc<str>>,
}

impl AppState {
    fn operator_ok(&self, given: Option<&str>) -> bool {
        match (&self.operator_token, given) {
            (None, _) => true,
            (Some(want), Some(got)) => constant_time_eq(want.as_bytes(), got.as_bytes()),
            (Some(_), None) => false,
        }
    }

    fn operator_header_ok(&self, headers: &HeaderMap) -> bool {
        self.operator_ok(
            headers
                .get(OPERATOR_TOKEN_HEADER)
                .and_then(|v| v.to_str().ok()),
        )
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl FromRef<AppState> for Arc<Hub> {
    fn from_ref(s: &AppState) -> Self {
        s.hub.clone()
    }
}

impl FromRef<AppState> for Arc<Provisioning> {
    fn from_ref(s: &AppState) -> Self {
        s.provisioning.clone()
    }
}

pub fn router(hub: Arc<Hub>, provisioning: Provisioning) -> Router {
    router_with(hub, provisioning, ServerOptions::default())
}

pub fn router_with(hub: Arc<Hub>, provisioning: Provisioning, options: ServerOptions) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/clapper", get(clapper))
        .route("/join", get(join))
        .route("/api/info", get(api_info))
        .route("/api/connect.svg", get(api_connect_svg))
        .route("/api/invite", post(api_invite))
        .route("/api/devices/{id}", delete(api_forget_device))
        .route(DEVICE_PATH, get(device_ws))
        .route(CONTROL_PATH, get(control_ws))
        .with_state(AppState {
            hub,
            provisioning: Arc::new(provisioning),
            operator_token: options.operator_token.map(Arc::from),
        })
}

async fn clapper() -> impl IntoResponse {
    Html(CLAPPER_HTML)
}

async fn join() -> impl IntoResponse {
    Html(JOIN_HTML)
}

/// The operator page. With invites required, "Add a camera" shows a fresh
/// invite instead of the plain deep link.
async fn index(State(state): State<AppState>) -> impl IntoResponse {
    let mut config = serde_json::json!({ "devicesUrl": "/api/devices" });
    if state.hub.invite_required() {
        config["inviteUrl"] = "/api/invite".into();
    }
    if state.operator_token.is_some() {
        // The page takes the token from its URL fragment (never sent here).
        config["operatorToken"] = true.into();
    }
    Html(operator_page(&config))
}

/// A one-time invite, the shape the operator page expects from `inviteUrl`.
#[derive(Debug, Serialize)]
struct InviteResponse {
    deep_link: String,
    control_url: String,
    srt_url: String,
    qr_svg: String,
    expires_in_secs: u64,
}

/// `POST /api/invite?target=<index>`: a one-time invite for a new phone, for
/// one of the `/api/info` targets (default 0).
async fn api_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ConnectQuery>,
) -> Response {
    if !state.operator_header_ok(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (hub, p) = (&state.hub, &state.provisioning);
    let info = p.info();
    let Some(target) = info.targets.get(q.target) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no such target (no LAN address found?)",
        )
            .into_response();
    };
    let token = hub.create_invite(DEFAULT_INVITE_TTL);
    let control_url = with_invite_token(&target.control_url, &token);
    let link = deep_link(&control_url, &target.srt_url);
    match qr_svg(&link) {
        Ok(svg) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(InviteResponse {
                deep_link: link,
                control_url,
                srt_url: target.srt_url.clone(),
                qr_svg: svg,
                expires_in_secs: DEFAULT_INVITE_TTL.as_secs(),
            }),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// `DELETE /api/devices/{id}`: forget (revoke) a device, see [`Hub::forget`].
async fn api_forget_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> StatusCode {
    if !state.operator_header_ok(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    if state.hub.forget(&id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn api_info(State(p): State<Arc<Provisioning>>) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Json(p.info()))
}

#[derive(Deserialize)]
struct ConnectQuery {
    /// Index into `/api/info` `targets`; default 0.
    #[serde(default)]
    target: usize,
}

/// The deep link of one target as a QR code.
async fn api_connect_svg(
    State(p): State<Arc<Provisioning>>,
    Query(q): Query<ConnectQuery>,
) -> Response {
    let info = p.info();
    let Some(target) = info.targets.get(q.target) else {
        return (
            StatusCode::NOT_FOUND,
            "no such target (no LAN address found?)",
        )
            .into_response();
    };
    match qr_svg(&target.deep_link) {
        Ok(svg) => (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            svg,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// Query of the device WebSocket URL. `token` is the invite token from the
/// provisioning link (only checked when the hub requires invites); the name
/// is [`crate::provision::INVITE_TOKEN_PARAM`].
#[derive(Debug, Default, Deserialize)]
pub struct DeviceQuery {
    #[serde(default)]
    pub token: Option<String>,
}

/// How a phone reached the device endpoint.
#[derive(Debug, Clone)]
pub struct DeviceConnection {
    pub addr: SocketAddr,
    /// The `token` query parameter, if any.
    pub token: Option<String>,
    /// The `Host` header of the upgrade request: the name or address the
    /// phone reaches this server by (see [`Hub::device_host`]).
    pub host: Option<String>,
}

async fn device_ws(
    ws: WebSocketUpgrade,
    State(hub): State<Arc<Hub>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<DeviceQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    let conn = DeviceConnection {
        addr,
        token: q.token,
        host,
    };
    ws.on_upgrade(move |socket| device_session_for(socket, hub, conn))
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

async fn control_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(q): Query<TokenQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !state.operator_ok(q.token.as_deref()) {
        warn!("Controller from {addr} refused: no or wrong operator token");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let hub = state.hub;
    ws.on_upgrade(move |socket| controller_session(socket, hub, addr))
        .into_response()
}

/// Runs one phone connection until it closes or goes silent, with no invite
/// token (admitted only while the hub does not require invites, or when the
/// device is known).
pub async fn device_session(socket: WebSocket, hub: Arc<Hub>, addr: SocketAddr) {
    let connection = DeviceConnection {
        addr,
        token: None,
        host: None,
    };
    device_session_for(socket, hub, connection).await
}

/// Where a phone connection is in the handshake.
enum Phase {
    /// Waiting for the first `hello`.
    Hello,
    /// A `hello` with a public key came; the phone was sent `challenge` and
    /// must answer with `auth` by `deadline`.
    Challenged {
        hello: Box<Hello>,
        challenge: Challenge,
        deadline: tokio::time::Instant,
    },
    /// Admitted as this device id, with this public key (`None`: an app
    /// before device keys). A later `hello` must carry the same key.
    Admitted { id: String, key: Option<String> },
}

/// Runs one phone connection described by `connection`.
///
/// The first `hello` decides how the phone is admitted ([`Hub::admit`]):
/// one with a public key is sent a `challenge` and admitted only after a
/// valid `auth`; one without (an app before device keys) is admitted or
/// refused right away. A refused phone gets a `message` with the reason and
/// the socket is closed.
pub async fn device_session_for(socket: WebSocket, hub: Arc<Hub>, connection: DeviceConnection) {
    let DeviceConnection { addr, token, host } = connection;
    let conn = hub.new_connection_id();
    debug!("Device socket {conn} opened from {addr}");
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel();
    // The hub holds the only strong sender once the phone is admitted. When
    // a newer connection replaces this one (or the device is forgotten), the
    // hub drops it, `rx` ends, and the writer closes the socket.
    let mut tx = Some(tx);

    let mut writer = tokio::spawn(async move {
        let mut ping = interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    let Ok(json) = serde_json::to_string(&msg) else { continue };
                    if sink.send(Message::Text(json.into())).await.is_err() {
                        return;
                    }
                }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    });

    let mut phase = Phase::Hello;
    loop {
        let wait = match &phase {
            Phase::Challenged { deadline, .. } => DEVICE_DEAD_AFTER
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
            _ => DEVICE_DEAD_AFTER,
        };
        let next = tokio::select! {
            _ = &mut writer => break,
            next = timeout(wait, stream.next()) => next,
        };
        let frame = match next {
            Err(_) if matches!(phase, Phase::Challenged { .. }) => {
                info!("Device socket {conn} from {addr}: no auth within {AUTH_TIMEOUT:?}, closing");
                break;
            }
            Err(_) => {
                info!("Device socket {conn} silent for {DEVICE_DEAD_AFTER:?}, dropping it");
                break;
            }
            Ok(None) | Ok(Some(Err(_))) => break,
            Ok(Some(Ok(frame))) => frame,
        };
        let text = match frame {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue, // ping/pong/binary: only liveness
        };
        let msg: DeviceMessage = match serde_json::from_str(text.as_str()) {
            Ok(m) => m,
            Err(e) => {
                warn!("Device socket {conn}: bad message: {e}");
                continue;
            }
        };
        let (hello, proof) = match (msg, &mut phase) {
            (DeviceMessage::Hello(hello), Phase::Hello) => {
                if hello.device.id.is_empty() || hello.device.id.contains('\n') {
                    warn!("Device socket {conn}: hello without a usable device.id, closing");
                    break;
                }
                if let Err(e) = check_hello_key(&hello) {
                    refuse(conn, &hello.device.id, addr, e, tx.take(), &mut writer).await;
                    break;
                }
                if hello.public_key.is_some() {
                    let challenge = hub.challenge();
                    if let Some(tx) = &tx {
                        let _ = tx.send(challenge.message());
                    }
                    phase = Phase::Challenged {
                        hello: Box::new(hello),
                        challenge,
                        deadline: tokio::time::Instant::now() + AUTH_TIMEOUT,
                    };
                    continue;
                }
                (hello, None)
            }
            (DeviceMessage::Hello(new), Phase::Challenged { hello, .. }) => {
                // Controls changed before the phone was admitted: keep the
                // newest hello, for the same device and key only.
                if new.device.id != hello.device.id || new.public_key != hello.public_key {
                    warn!("Device socket {conn}: hello for another device during the challenge, closing");
                    break;
                }
                **hello = new;
                continue;
            }
            (DeviceMessage::Auth { signature }, Phase::Challenged { .. }) => {
                let Phase::Challenged {
                    hello, challenge, ..
                } = std::mem::replace(&mut phase, Phase::Hello)
                else {
                    unreachable!()
                };
                (*hello, Some((challenge, signature)))
            }
            (DeviceMessage::Hello(hello), Phase::Admitted { id, key }) => {
                if &hello.device.id != id {
                    warn!(
                        "Device socket {conn}: hello for {} on a connection for {id}, ignored",
                        hello.device.id
                    );
                    continue;
                }
                if &hello.public_key != key {
                    // A key is bound only through a challenge.
                    warn!("Device socket {conn}: hello for {id} with another key, closing");
                    break;
                }
                hub.device_hello(conn, hello);
                continue;
            }
            (DeviceMessage::Auth { .. }, Phase::Admitted { .. }) => continue,
            (_, Phase::Hello | Phase::Challenged { .. }) => {
                debug!("Device socket {conn}: message before admission, ignored");
                continue;
            }
            (msg, Phase::Admitted { id, .. }) => {
                hub.device_message(conn, id, msg);
                continue;
            }
        };
        let proof = proof.as_ref().map(|(challenge, signature)| Proof {
            challenge,
            signature: signature.as_str(),
        });
        let admission = match hub.admit(&hello, proof, token.as_deref()) {
            Ok(admission) => admission,
            Err(e) => {
                refuse(conn, &hello.device.id, addr, e, tx.take(), &mut writer).await;
                break;
            }
        };
        let (id, key) = (hello.device.id.clone(), hello.public_key.clone());
        let Some(sender) = tx.clone() else { break };
        if let Err(e) = hub.device_join(conn, hello, admission, sender, host.clone()) {
            refuse(conn, &id, addr, e, tx.take(), &mut writer).await;
            break;
        }
        // The hub now holds the only strong sender: when a newer connection
        // replaces this one, the channel ends and the writer closes.
        tx = None;
        phase = Phase::Admitted { id, key };
    }

    writer.abort();
    if let Phase::Admitted { id, .. } = &phase {
        hub.device_disconnected(conn, id);
    }
    debug!("Device socket {conn} closed");
}

/// Tell a refused phone why (a banner on the phone), then let the writer send
/// it and close the socket.
async fn refuse(
    conn: u64,
    device: &str,
    addr: SocketAddr,
    reason: AdmitError,
    tx: Option<mpsc::UnboundedSender<ServerToDevice>>,
    writer: &mut tokio::task::JoinHandle<()>,
) {
    warn!("Device socket {conn}: refused {device} from {addr}: {reason}");
    if let Some(tx) = tx {
        let _ = tx.send(ServerToDevice::Message {
            text: reason.to_string(),
        });
        // Dropping the last sender ends the writer, which sends Close.
        drop(tx);
        let _ = timeout(Duration::from_secs(1), writer).await;
    }
}

/// Runs one operator UI connection: snapshot, then events, and requests in.
pub async fn controller_session(socket: WebSocket, hub: Arc<Hub>, addr: SocketAddr) {
    info!("Controller connected from {addr}");
    let (mut sink, mut stream) = socket.split();
    let (snapshot, mut events, _guard) = hub.controller_joined();
    if sink.send(Message::Text(snapshot.into())).await.is_err() {
        return;
    }
    let mut ping = interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_seen = tokio::time::Instant::now();

    loop {
        let out: Option<String> = tokio::select! {
            ev = events.recv() => match ev {
                Ok(frame) => Some(frame.json.to_string()),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Controller {addr} lagged by {n} events, resending snapshot");
                    let (snapshot, rx) = hub.resync();
                    events = rx;
                    Some(snapshot)
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            frame = stream.next() => {
                let Some(Ok(frame)) = frame else { break };
                last_seen = tokio::time::Instant::now();
                match frame {
                    Message::Text(text) => handle_controller_text(&hub, text.as_str()),
                    Message::Close(_) => break,
                    _ => None,
                }
            }
            _ = ping.tick() => {
                if last_seen.elapsed() > CONTROLLER_DEAD_AFTER {
                    info!("Controller {addr} silent, dropping it");
                    break;
                }
                if sink.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
                None
            }
        };
        if let Some(out) = out {
            if sink.send(Message::Text(out.into())).await.is_err() {
                break;
            }
        }
    }
    info!("Controller {addr} disconnected");
}

/// Returns a reply for this controller only, if any.
fn handle_controller_text(hub: &Hub, text: &str) -> Option<String> {
    let reply = |device: Option<String>, message: String| {
        serde_json::to_string(&ControllerEvent::Error { device, message }).ok()
    };
    // Application-level ping from the UI.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if v.get("type").and_then(|t| t.as_str()) == Some("ping") {
            let mut pong = serde_json::json!({ "type": "pong" });
            if let Some(id) = v.get("id") {
                pong["id"] = id.clone();
            }
            return Some(pong.to_string());
        }
    }
    match ControllerInput::parse(text) {
        Ok(Some(ControllerInput::Device(req))) => {
            let device = req.device.clone();
            match hub.controller_request(req) {
                Ok(()) => None,
                Err(e) => reply(Some(device), e),
            }
        }
        Ok(Some(ControllerInput::Production(production))) => match hub.set_production(production) {
            Ok(()) => None,
            Err(e) => reply(None, format!("production: {e}")),
        },
        Ok(None) => None,
        Err(e) => reply(None, format!("bad request: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_page_carries_the_config() {
        assert!(INDEX_HTML.contains(CONFIG_MARKER));
        let page = operator_page(&serde_json::json!({
            "controlPath": "/api/cameras/control",
            "title": "</script><b>x",
        }));
        assert!(!page.contains(CONFIG_MARKER));
        assert!(page.contains(r#"window.TALLYCAM_CONFIG = {"controlPath":"/api/cameras/control""#));
        assert!(
            !page.contains("</script><b>"),
            "config cannot close the script"
        );
    }
}
