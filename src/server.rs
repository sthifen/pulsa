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
//!
//! Al PC le llegan: `{"t":"hello",…}` al conectar y `{"t":"leds",…}` cuando
//! cambia Bloq Mayús o Bloq Num.
//!
//! Una tecla que se queda pulsada se repite sin fin, así que lo pulsado se
//! suelta si la tablet se desconecta o deja de dar señales (la WiFi puede
//! caerse sin cerrar la conexión).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
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
}

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
}

pub async fn serve(state: Arc<AppState>, listener: tokio::net::TcpListener) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/info", get(info))
        .route("/ws", get(ws))
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
}

async fn ws(
    ws: WebSocketUpgrade,
    State(s): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(params): Query<WsParams>,
) -> impl IntoResponse {
    let who = if params.name.is_empty() { peer.to_string() } else { format!("{} ({peer})", params.name) };
    ws.on_upgrade(move |socket| session(socket, s, who))
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

async fn session(mut socket: WebSocket, s: Arc<AppState>, who: String) {
    tracing::info!("conectada: {who}");
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
        ClientMsg::Ping => {}
    }
    Ok(())
}
