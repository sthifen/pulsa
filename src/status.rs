//! Página de estado en el navegador del PC (`/estado`): lo que se abre al
//! lanzar la AppImage con doble clic, porque Pulsa no tiene ventana propia.
//! Muestra cómo conectar la tablet (QR de la app, direcciones, código de
//! emparejamiento), qué tablets hay, y deja arrancar con el ordenador,
//! añadir Pulsa al menú o cerrarlo.
//!
//! Las acciones sólo se aceptan desde este mismo equipo y con la cabecera
//! `X-Pulsa`, que una web ajena no puede poner sin permiso de CORS: así
//! ninguna página que visites puede cerrar Pulsa ni tocar el arranque.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use crate::server::AppState;

const PAGE: &str = include_str!("../web/estado.html");
pub const APK_URL: &str = "https://github.com/sthifen/pulsa/releases/latest/download/pulsa.apk";

pub async fn page() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], PAGE)
}

/// QR (SVG) con el enlace de descarga de la app.
pub async fn qr() -> impl IntoResponse {
    let svg = qrcode::QrCode::new(APK_URL)
        .map(|c| {
            c.render::<qrcode::render::svg::Color>()
                .min_dimensions(200, 200)
                .quiet_zone(true)
                .dark_color(qrcode::render::svg::Color("#15171c"))
                .light_color(qrcode::render::svg::Color("#ffffff"))
                .build()
        })
        .unwrap_or_default();
    ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "max-age=3600")], svg)
}

fn local(peer: &SocketAddr, headers: &HeaderMap) -> bool {
    peer.ip().is_loopback() && headers.contains_key("x-pulsa")
}

pub async fn info(
    State(s): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !local(&peer, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let port = s.port;
    let online: Vec<_> = s.online.lock().unwrap().values().cloned().collect();
    let paired: Vec<_> = s.pairing.tablets().into_iter().map(|(id, name)| json!({"id": id, "name": name})).collect();
    axum::Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "name": crate::lan::hostname(),
        "via": s.injector.name(),
        "port": port,
        "wifi": crate::lan::addresses().iter().map(|ip| format!("{ip}:{port}")).collect::<Vec<_>>(),
        "code": s.pairing.code(),
        "online": online,
        "paired": paired,
        "autostart": autostart_path().is_file(),
        "menu": menu_path().is_file(),
        "appimage": std::env::var_os("APPIMAGE").is_some(),
        "apk": APK_URL,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct Action {
    what: String,
    #[serde(default)]
    on: bool,
    #[serde(default)]
    id: String,
}

pub async fn action(
    State(s): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(a): Query<Action>,
) -> impl IntoResponse {
    if !local(&peer, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let result = match a.what.as_str() {
        "autostart" => desktop_file(&autostart_path(), a.on, true),
        "menu" => desktop_file(&menu_path(), a.on, false),
        "forget" => {
            s.pairing.forget(&a.id);
            Ok(())
        }
        "quit" => {
            tracing::info!("cerrado desde la página de estado");
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                std::process::exit(0);
            });
            Ok(())
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

pub fn autostart_path() -> PathBuf {
    config_home().join("autostart/pulsa.desktop")
}

pub fn menu_path() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    // Con el id de la app: así GNOME asocia la ventana a su icono del menú.
    data.join("applications/dev.pulsa.Pulsa.desktop")
}

/// Crea o borra un acceso directo que lanza esta misma AppImage (o binario).
pub fn desktop_file(path: &PathBuf, on: bool, background: bool) -> std::io::Result<()> {
    if !on {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let exe = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| std::io::Error::other("no se sabe dónde está Pulsa"))?;
    // Al iniciar sesión, en segundo plano; desde el menú, con la ventana.
    let gui = cfg!(feature = "gui");
    let arg = match (background, gui) {
        (true, true) => " --ventana --fondo",
        (false, true) => " --ventana",
        _ => "",
    };
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Pulsa\nComment=La tablet como teclado y trackpad\n\
         Exec=\"{}\"{arg}\nIcon=dev.pulsa.Pulsa\nTerminal=false\nCategories=Utility;\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}
