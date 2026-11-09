//! Servidor HTTP/WebSocket: sirve el teclado (`web/index.html`) a la tablet
//! y recibe por `/ws` lo que se pulsa.
//!
//! Mensajes de la tablet (JSON, uno por evento):
//!
//! * `{"t":"k","c":30,"d":true}`: tecla (código evdev) pulsada o soltada.
//! * `{"t":"b","b":0,"d":true}`: botón del trackpad (0 izq., 1 central, 2 der.).
//! * `{"t":"m","x":1.5,"y":-2}`: movimiento relativo del puntero, en píxeles.
//! * `{"t":"s","x":0,"y":120}`: desplazamiento (120 = una muesca; `y` > 0 sube).
//! * `{"t":"p"}`: sigo aquí (cada segundo).
//! * `{"t":"code","c":"123456"}`: código de emparejamiento (sólo por WiFi,
//!   la primera vez; ver `pairing.rs`).
//!
//! A la tablet le llegan: `{"t":"pair"}` si tiene que escribir el código,
//! `{"t":"badcode"}` si no era, `{"t":"hello",…}` cuando ya puede escribir y
//! `{"t":"leds",…}` cuando cambia Bloq Mayús o Bloq Num.
//!
//! Una tecla que se queda pulsada se repite sin fin, así que lo pulsado se
//! suelta si la tablet se desconecta o deja de dar señales (la WiFi puede
//! caerse sin cerrar la conexión).

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::watch;

use crate::inject::{Injector, Leds, MAX_KEY, button_code};

const INDEX_HTML: &str = include_str!("../web/index.html");

/// Sin mensajes durante este tiempo, se suelta lo pulsado.
const QUIET_RELEASE: Duration = Duration::from_secs(3);
/// Y durante este, se da la tablet por perdida.
const QUIET_CLOSE: Duration = Duration::from_secs(15);

pub struct AppState {
    pub injector: Injector,
    pub leds: watch::Receiver<Leds>,
    pub pairing: crate::pairing::Pairing,
    pub port: u16,
    /// Tablets conectadas ahora mismo, para la página de estado.
    pub online: Mutex<BTreeMap<u64, String>>,
    /// La última tablet que pidió el código (nombre, cuándo), para avisar en la ventana.
    pub pair_request: Mutex<Option<(String, std::time::Instant)>>,
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Deserialize)]
#[serde(tag = "t")]
enum ClientMsg {
    #[serde(rename = "k")]
    Key { c: u16, d: bool },
    #[serde(rename = "b")]
    Button { b: u8, d: bool },
    #[serde(rename = "m")]
    Move { x: f64, y: f64 },
    #[serde(rename = "s")]
    Scroll {
        #[serde(default)]
        x: i32,
        #[serde(default)]
        y: i32,
    },
    #[serde(rename = "p")]
    Ping,
    #[serde(rename = "code")]
    Code { c: String },
}

pub async fn serve(state: Arc<AppState>, listener: tokio::net::TcpListener) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/info", get(info))
        .route("/ws", get(ws))
        .route("/estado", get(crate::status::page))
        .route("/estado/qr.svg", get(crate::status::qr))
        .route("/estado/info", get(crate::status::info))
        .route("/estado/accion", post(crate::status::action))
        .with_state(state);
    // Sin Nagle: cada tecla es un paquete pequeño que TCP retendría
    // esperando juntar más.
    let listener = axum::serve::ListenerExt::tap_io(listener, |tcp| {
        let _ = tcp.set_nodelay(true);
    });
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
        INDEX_HTML,
    )
}

/// Para que la app encuentre este PC. CORS abierto: la app pregunta desde file://.
async fn info(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        axum::Json(json!({
            "app": "pulsa",
            "version": env!("CARGO_PKG_VERSION"),
            "name": crate::lan::hostname(),
            "id": crate::lan::server_id(),
            "via": s.injector.name(),
        })),
    )
}

#[derive(Deserialize)]
struct WsParams {
    #[serde(default)]
    name: String,
    /// Identificador fijo de la tablet (lo genera la app o la página).
    #[serde(default)]
    id: String,
}

async fn ws(
    ws: WebSocketUpgrade,
    State(s): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(params): Query<WsParams>,
) -> impl IntoResponse {
    let who = if params.name.is_empty() { peer.to_string() } else { format!("{} ({peer})", params.name) };
    let trusted = s.pairing.trusted(peer.ip(), &params.id);
    ws.on_upgrade(move |socket| async move {
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let label = if params.name.is_empty() { peer.ip().to_string() } else { params.name.clone() };
        let via = if peer.ip().is_loopback() { "USB" } else { "WiFi" };
        s.online.lock().unwrap().insert(id, format!("{label} · {via}"));
        session(socket, s.clone(), who, trusted, params).await;
        s.online.lock().unwrap().remove(&id);
    })
}

/// Lo que esta tablet tiene pulsado ahora mismo.
#[derive(Default)]
struct Held {
    keys: HashSet<u16>,
    buttons: HashSet<u16>,
}

impl Held {
    async fn release(&mut self, inj: &Injector) {
        for code in self.keys.drain() {
            let _ = inj.key(code, false).await;
        }
        for code in self.buttons.drain() {
            let _ = inj.button(code, false).await;
        }
    }
}

/// Una tablet nueva por WiFi: hasta que escriba el código no hace nada más.
/// `false` si se fue antes.
async fn pair(socket: &mut WebSocket, s: &AppState, who: &str, params: &WsParams) -> bool {
    tracing::info!("{who} quiere emparejarse por WiFi: código {}", s.pairing.code());
    let label = if params.name.is_empty() { who.to_string() } else { params.name.clone() };
    *s.pair_request.lock().unwrap() = Some((label, std::time::Instant::now()));
    if socket.send(Message::Text(json!({"t": "pair"}).to_string().into())).await.is_err() {
        return false;
    }
    loop {
        let msg = match tokio::time::timeout(Duration::from_secs(300), socket.recv()).await {
            Ok(Some(Ok(Message::Text(t)))) => t,
            Ok(Some(Ok(_))) => continue,
            _ => return false,
        };
        if let Ok(ClientMsg::Code { c }) = serde_json::from_str::<ClientMsg>(&msg) {
            if s.pairing.try_code(&params.id, &params.name, &c) {
                tracing::info!("{who} emparejada");
                return true;
            }
            tracing::warn!("{who}: código equivocado");
            if socket.send(Message::Text(json!({"t": "badcode"}).to_string().into())).await.is_err() {
                return false;
            }
        }
    }
}

async fn session(mut socket: WebSocket, s: Arc<AppState>, who: String, trusted: bool, params: WsParams) {
    tracing::info!("conectada: {who}");
    if !trusted && !pair(&mut socket, &s, &who, &params).await {
        tracing::info!("desconectada sin emparejar: {who}");
        return;
    }
    let hello = json!({ "t": "hello", "name": crate::lan::hostname(), "via": s.injector.name() });
    let mut leds = s.leds.clone();
    let mut sent_leds = *leds.borrow_and_update();
    if socket.send(Message::Text(hello.to_string().into())).await.is_err()
        || socket.send(leds_msg(sent_leds)).await.is_err()
    {
        return;
    }

    let mut held = Held::default();
    let mut quiet = Duration::ZERO;
    loop {
        let wait = if held.keys.is_empty() && held.buttons.is_empty() { QUIET_CLOSE } else { QUIET_RELEASE };
        tokio::select! {
            msg = tokio::time::timeout(wait.saturating_sub(quiet), socket.recv()) => match msg {
                Err(_) if wait == QUIET_RELEASE => {
                    tracing::warn!("{who} dejó de responder con teclas pulsadas: se sueltan");
                    held.release(&s.injector).await;
                    quiet = QUIET_RELEASE;
                }
                Err(_) => {
                    tracing::warn!("{who} dejó de responder");
                    break;
                }
                Ok(None | Some(Err(_)) | Some(Ok(Message::Close(_)))) => break,
                Ok(Some(Ok(Message::Text(text)))) => {
                    quiet = Duration::ZERO;
                    match serde_json::from_str::<ClientMsg>(&text) {
                        Ok(m) => {
                            if let Err(e) = apply(&s.injector, &mut held, m).await {
                                tracing::warn!("no se pudo meter la entrada: {e:#}");
                            }
                        }
                        Err(e) => tracing::debug!("mensaje no válido de {who}: {e}"),
                    }
                }
                Ok(Some(Ok(_))) => quiet = Duration::ZERO,
            },
            Ok(()) = leds.changed() => {
                let now = *leds.borrow_and_update();
                if now != sent_leds {
                    sent_leds = now;
                    if socket.send(leds_msg(now)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
    held.release(&s.injector).await;
    tracing::info!("desconectada: {who}");
}

fn leds_msg(l: Leds) -> Message {
    Message::Text(json!({ "t": "leds", "caps": l.caps, "num": l.num }).to_string().into())
}

async fn apply(inj: &Injector, held: &mut Held, m: ClientMsg) -> anyhow::Result<()> {
    match m {
        ClientMsg::Key { c, d } => {
            if c == 0 || c > MAX_KEY {
                return Ok(());
            }
            // Una tecla ya pulsada no se vuelve a pulsar (y una suelta, no se
            // suelta): así un mensaje repetido no deja nada a medias.
            let changed = if d { held.keys.insert(c) } else { held.keys.remove(&c) };
            if changed {
                inj.key(c, d).await?;
            }
        }
        ClientMsg::Button { b, d } => {
            let code = button_code(b);
            let changed = if d { held.buttons.insert(code) } else { held.buttons.remove(&code) };
            if changed {
                inj.button(code, d).await?;
            }
        }
        ClientMsg::Move { x, y } => {
            if x.is_finite() && y.is_finite() {
                inj.motion(x.clamp(-2000.0, 2000.0), y.clamp(-2000.0, 2000.0)).await?;
            }
        }
        ClientMsg::Scroll { x, y } => inj.scroll(x.clamp(-2400, 2400), y.clamp(-2400, 2400)).await?,
        ClientMsg::Ping | ClientMsg::Code { .. } => {}
    }
    Ok(())
}
