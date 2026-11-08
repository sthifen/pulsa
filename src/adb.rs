//! Por USB: cada tablet conectada por ADB recibe un
//! `adb reverse tcp:PORT tcp:PORT`, así que en ella `http://localhost:PORT`
//! llega a este PC por el cable (sin WiFi, y con menos latencia).
//!
//! Las reglas de `adb reverse` se pierden cuando la tablet se reconecta, así
//! que en cada vuelta se mira la lista real en lugar de recordar
//! dispositivos. La app se abre sola en la tablet al conectarla.

use std::collections::HashSet;
use std::time::Duration;

use tokio::process::Command;

const ACTIVITY: &str = "dev.pulsa/.MainActivity";

pub async fn watch(port: u16) {
    if Command::new("adb").arg("version").output().await.is_err() {
        tracing::warn!("adb no encontrado: sólo por WiFi (instala android-tools para el USB)");
        return;
    }
    let spec = format!("tcp:{port}");
    let mut warned: HashSet<String> = HashSet::new();
    loop {
        for serial in list_devices().await {
            if has_reverse(&serial, &spec).await {
                continue;
            }
            if adb(&["-s", &serial, "reverse", &spec, &spec]).await.is_none() {
                if warned.insert(serial.clone()) {
                    tracing::warn!("USB: no se pudo configurar adb reverse en {serial} (¿autorizaste la depuración USB?)");
                }
                continue;
            }
            warned.remove(&serial);
            tracing::info!("USB: {serial} conectada (http://localhost:{port})");
            match adb(&["-s", &serial, "shell", "am", "start", "-n", ACTIVITY]).await {
                Some(out) if !out.contains("Error") => tracing::info!("USB: Pulsa abierta en {serial}"),
                _ => tracing::warn!("USB: no se pudo abrir Pulsa en {serial} (¿está instalada?)"),
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn adb(args: &[&str]) -> Option<String> {
    let out = Command::new("adb").args(args).output().await.ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn has_reverse(serial: &str, spec: &str) -> bool {
    adb(&["-s", serial, "reverse", "--list"])
        .await
        .is_some_and(|list| list.lines().any(|l| l.split_whitespace().nth(1) == Some(spec)))
}

async fn list_devices() -> Vec<String> {
    let Some(out) = adb(&["devices"]).await else { return Vec::new() };
    out.lines()
        .skip(1)
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(serial), Some("device")) => Some(serial.to_owned()),
                _ => None,
            }
        })
        .collect()
}
