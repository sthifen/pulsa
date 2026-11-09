//! Quién puede escribir en este PC.
//!
//! Por USB (`adb reverse`, la conexión llega desde 127.0.0.1) entra
//! cualquiera: hace falta tener la tablet enchufada. Por WiFi, una tablet
//! nueva tiene que escribir una vez el código de 6 dígitos que muestra el PC
//! (en la terminal y en la página de estado); después se recuerda por su id
//! en `~/.config/pulsa/tablets.json`. Sin esto, cualquiera en la misma red
//! (la de la universidad, un bar…) podría escribir en el PC.

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Intentos fallidos seguidos tras los que se cambia el código.
const MAX_TRIES: u32 = 5;
/// Un código sin usar caduca y se cambia.
const CODE_LIFETIME: Duration = Duration::from_secs(10 * 60);

pub struct Pairing {
    path: PathBuf,
    /// id de la tablet → nombre, para mostrarlas.
    approved: Mutex<BTreeMap<String, String>>,
    code: Mutex<Code>,
}

struct Code {
    digits: String,
    made: Instant,
    fails: u32,
}

impl Code {
    fn new() -> Self {
        // Aleatorio sin dependencias: RandomState se siembra del sistema.
        let n = std::collections::hash_map::RandomState::new().build_hasher().finish() % 1_000_000;
        Self { digits: format!("{n:06}"), made: Instant::now(), fails: 0 }
    }
}

impl Pairing {
    pub fn load() -> Self {
        let dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("pulsa");
        let path = dir.join("tablets.json");
        let approved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self { path, approved: Mutex::new(approved), code: Mutex::new(Code::new()) }
    }

    /// ¿Puede escribir sin código?
    pub fn trusted(&self, peer: IpAddr, id: &str) -> bool {
        peer.is_loopback() || (!id.is_empty() && self.approved.lock().unwrap().contains_key(id))
    }

    /// El código vigente (se renueva si caducó).
    pub fn code(&self) -> String {
        let mut c = self.code.lock().unwrap();
        if c.made.elapsed() > CODE_LIFETIME {
            *c = Code::new();
        }
        c.digits.clone()
    }

    /// Comprueba el código; si vale, la tablet queda aprobada para siempre.
    pub fn try_code(&self, id: &str, name: &str, attempt: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let mut c = self.code.lock().unwrap();
        if c.made.elapsed() > CODE_LIFETIME {
            *c = Code::new();
            return false;
        }
        if attempt.trim() != c.digits {
            c.fails += 1;
            if c.fails >= MAX_TRIES {
                tracing::warn!("demasiados códigos equivocados: se cambia el código");
                *c = Code::new();
                tracing::info!("nuevo código para emparejar por WiFi: {}", c.digits);
            }
            return false;
        }
        // Usado: el siguiente emparejamiento tendrá otro.
        *c = Code::new();
        drop(c);
        let mut a = self.approved.lock().unwrap();
        a.insert(id.to_string(), name.to_string());
        self.save(&a);
        true
    }

    pub fn tablets(&self) -> Vec<(String, String)> {
        self.approved.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    pub fn forget(&self, id: &str) {
        let mut a = self.approved.lock().unwrap();
        if a.remove(id).is_some() {
            self.save(&a);
        }
    }

    fn save(&self, a: &BTreeMap<String, String>) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&self.path, serde_json::to_string_pretty(a).unwrap_or_default()) {
            tracing::warn!("no se pudo guardar {}: {e}", self.path.display());
        }
    }
}
