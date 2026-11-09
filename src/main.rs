//! Pulsa: la tablet como teclado (completo o 60 % con trackpad) del PC.
//!
//! El PC sirve el teclado como página web; la app de Android lo abre a
//! pantalla completa, por USB (`adb reverse`) o por WiFi. Lo que se pulsa
//! vuelve por WebSocket y entra en el PC como un teclado y un ratón más
//! (`inject.rs`).

mod adb;
mod inject;
mod lan;
mod pairing;
mod server;
mod status;

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::watch;

/// La tablet como teclado y trackpad del PC.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Dirección en la que escuchar.
    #[arg(long, default_value = "0.0.0.0")]
    bind: IpAddr,

    /// Puerto (8787 es de Encaje, 8788 de Sync Tablet y 8790-8792 de los
    /// monitores extra de Encaje).
    #[arg(long, default_value_t = 8795)]
    port: u16,

    /// No configurar `adb reverse` para las tablets conectadas por USB.
    #[arg(long, visible_alias = "sin-usb")]
    no_usb: bool,

    /// Arrancar en silencio (lo usa el arranque automático de la AppImage).
    #[arg(long, hide = true)]
    fondo: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "pulsa=info".into()),
        )
        .with_target(false)
        .init();
    let args = Args::parse();
    let _ = args.fondo; // sólo cambia el lanzador de la AppImage

    let (leds_tx, leds) = watch::channel(Default::default());
    let injector = inject::Injector::create(leds_tx).await?;

    let addr = SocketAddr::new(args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("no se pudo escuchar en {addr} (¿ya hay otro Pulsa abierto?)"))?;
    tracing::info!("Pulsa {} en el puerto {}", env!("CARGO_PKG_VERSION"), args.port);
    for ip in lan::addresses() {
        tracing::info!("  por WiFi: http://{ip}:{}", args.port);
    }
    tracing::info!("  estado y QR de la app: http://localhost:{}/estado", args.port);
    if !args.no_usb {
        tokio::spawn(adb::watch(args.port));
    }

    let state = std::sync::Arc::new(server::AppState {
        injector,
        leds,
        pairing: pairing::Pairing::load(),
        port: args.port,
        online: Default::default(),
    });
    tracing::info!("  código para emparejar tablets por WiFi: {}", state.pairing.code());
    let result = tokio::select! {
        r = server::serve(state.clone(), listener) => r,
        _ = tokio::signal::ctrl_c() => { tracing::info!("cerrando"); Ok(()) }
    };
    state.injector.stop().await;
    result
}
