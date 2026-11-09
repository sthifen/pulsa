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
#[cfg(feature = "gui")]
mod gui;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::watch;

/// La tablet como teclado y trackpad del PC.
#[derive(Parser, Clone)]
#[command(version, about)]
pub struct Args {
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

    /// Abrir la ventana de Pulsa (la AppImage lo hace al abrirla desde el menú).
    #[arg(long, visible_alias = "window")]
    ventana: bool,

    /// Con --ventana: arrancar en segundo plano, sin mostrar la ventana
    /// (lo usa «Iniciar con el sistema»).
    #[arg(long)]
    fondo: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "pulsa=info".into()),
        )
        .with_target(false)
        .init();
    let args = Args::parse();

    #[cfg(feature = "gui")]
    if args.ventana {
        return gui::run(args);
    }
    #[cfg(not(feature = "gui"))]
    if args.ventana {
        tracing::warn!("esta versión de Pulsa no tiene ventana; la página de estado está en http://localhost:{}/estado", args.port);
    }

    tokio::runtime::Runtime::new()?.block_on(async {
        let state = start(&args).await?;
        tokio::signal::ctrl_c().await?;
        tracing::info!("cerrando");
        state.injector.stop().await;
        Ok(())
    })
}

/// Crea el teclado virtual, empieza a escuchar y deja el servidor corriendo
/// en segundo plano (en el runtime de tokio actual).
pub async fn start(args: &Args) -> Result<Arc<server::AppState>> {
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

    let state = Arc::new(server::AppState {
        injector,
        leds,
        pairing: pairing::Pairing::load(),
        port: args.port,
        online: Default::default(),
        pair_request: Default::default(),
    });
    tracing::info!("  código para emparejar tablets por WiFi: {}", state.pairing.code());
    let serving = state.clone();
    tokio::spawn(async move {
        if let Err(e) = server::serve(serving, listener).await {
            tracing::error!("el servidor se detuvo: {e:#}");
        }
    });
    Ok(state)
}
