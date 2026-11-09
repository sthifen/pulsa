//! Cómo llegan al PC las teclas y el trackpad de la tablet.
//!
//! * `/dev/uinput` (lo normal): un teclado y un ratón virtuales que el
//!   kernel trata como si estuvieran enchufados. Funciona en cualquier
//!   escritorio (GNOME, KDE, Niri, incluso X11) y la distribución del teclado
//!   la pone el PC, porque se mandan teclas físicas y no letras. Hace falta
//!   permiso de escritura en `/dev/uinput` (regla udev con `uaccess`).
//! * GNOME sin ese permiso: la sesión de escritorio remoto de Mutter
//!   (`org.gnome.Mutter.RemoteDesktop`), la misma que usa Encaje. Tampoco
//!   necesita un monitor: sólo teclas y movimiento relativo.
//!
//! El teclado virtual de uinput además recibe los LED que pone el
//! escritorio (Bloq Mayús, Bloq Num), y la tablet los enciende en sus teclas.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use tokio::sync::watch;

// ── Códigos de linux/input-event-codes.h y linux/uinput.h ─────────────────

const UI_DEV_CREATE: libc::Ioctl = 0x5501 as libc::Ioctl;
const UI_DEV_DESTROY: libc::Ioctl = 0x5502 as libc::Ioctl;
const UI_DEV_SETUP: libc::Ioctl = 0x405c5503 as libc::Ioctl;
const UI_SET_EVBIT: libc::Ioctl = 0x40045564 as libc::Ioctl;
const UI_SET_KEYBIT: libc::Ioctl = 0x40045565 as libc::Ioctl;
const UI_SET_RELBIT: libc::Ioctl = 0x40045566 as libc::Ioctl;
const UI_SET_LEDBIT: libc::Ioctl = 0x40045569 as libc::Ioctl;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_LED: u16 = 0x11;
const EV_REP: u16 = 0x14;
const SYN_REPORT: u16 = 0x00;

const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;
const REL_WHEEL_HI_RES: u16 = 0x0b;
const REL_HWHEEL_HI_RES: u16 = 0x0c;

const LED_NUML: u16 = 0x00;
const LED_CAPSL: u16 = 0x01;

pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

/// Una muesca de rueda en unidades de alta resolución.
pub const WHEEL_NOTCH: i32 = 120;

/// Teclas que acepta el teclado virtual (todas las de un teclado de PC).
pub const MAX_KEY: u16 = 248;

/// Botón del cliente (0 izquierdo, 1 central, 2 derecho) → código evdev.
pub fn button_code(b: u8) -> u16 {
    match b {
        1 => BTN_MIDDLE,
        2 => BTN_RIGHT,
        _ => BTN_LEFT,
    }
}

/// LED del teclado que enciende el escritorio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Leds {
    pub caps: bool,
    pub num: bool,
}

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

// time_t/suseconds_t: `long` del kernel, que es lo que pide input_event.
#[allow(deprecated)]
#[repr(C)]
struct RawInputEvent {
    tv_sec: libc::time_t,
    tv_usec: libc::suseconds_t,
    type_: u16,
    code: u16,
    value: i32,
}

const EVENT_SIZE: usize = std::mem::size_of::<RawInputEvent>();

/// Un dispositivo de `/dev/uinput`. Se quita al soltarlo.
struct Uinput {
    file: File,
}

impl Uinput {
    /// `setup` declara qué eventos produce el dispositivo (ioctl UI_SET_*).
    fn create(name: &str, product: u16, setup: impl FnOnce(i32) -> Result<()>) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
            .context("no se pudo abrir /dev/uinput")?;
        let fd = file.as_raw_fd();
        setup(fd)?;
        let mut label = [0u8; 80];
        label[..name.len()].copy_from_slice(name.as_bytes());
        let id = UinputSetup {
            id: InputId { bustype: 0x06 /* BUS_VIRTUAL */, vendor: 0x1d6b, product, version: 1 },
            name: label,
            ff_effects_max: 0,
        };
        // SAFETY: `id` es un `struct uinput_setup` con la disposición de C.
        unsafe {
            if libc::ioctl(fd, UI_DEV_SETUP, &id) < 0 {
                bail!("ioctl UI_DEV_SETUP falló: {}", std::io::Error::last_os_error());
            }
            if libc::ioctl(fd, UI_DEV_CREATE) < 0 {
                bail!("ioctl UI_DEV_CREATE falló: {}", std::io::Error::last_os_error());
            }
        }
        Ok(Self { file })
    }

    /// Escribe los eventos y un SYN_REPORT, que los entrega juntos.
    fn emit(&self, events: &[(u16, u16, i32)]) -> Result<()> {
        let mut buf = Vec::with_capacity((events.len() + 1) * EVENT_SIZE);
        for &(type_, code, value) in events.iter().chain(&[(EV_SYN, SYN_REPORT, 0)]) {
            let ev = RawInputEvent { tv_sec: 0, tv_usec: 0, type_, code, value };
            // SAFETY: `RawInputEvent` es un `struct input_event` sin relleno.
            buf.extend_from_slice(unsafe {
                std::slice::from_raw_parts((&ev as *const RawInputEvent).cast::<u8>(), EVENT_SIZE)
            });
        }
        (&self.file).write_all(&buf)?;
        Ok(())
    }
}

impl Drop for Uinput {
    fn drop(&mut self) {
        // SAFETY: el descriptor sigue abierto hasta que se suelte `file`.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
    }
}

fn ioctl_int(fd: i32, req: libc::Ioctl, value: u16) -> Result<()> {
    // SAFETY: las peticiones UI_SET_* reciben un entero por valor.
    if unsafe { libc::ioctl(fd, req, value as libc::c_int) } < 0 {
        bail!("ioctl {req:#x} falló: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

pub struct UinputPair {
    keyboard: Uinput,
    mouse: Uinput,
    /// Lo que queda de los movimientos con decimales, para no perderlo.
    rest: Mutex<(f64, f64, i32, i32)>,
}

impl UinputPair {
    fn create(leds: watch::Sender<Leds>) -> Result<Self> {
        let keyboard = Uinput::create("Pulsa teclado", 0x5001, |fd| {
            ioctl_int(fd, UI_SET_EVBIT, EV_KEY)?;
            ioctl_int(fd, UI_SET_EVBIT, EV_LED)?;
            ioctl_int(fd, UI_SET_EVBIT, EV_REP)?;
            for key in 1..=MAX_KEY {
                ioctl_int(fd, UI_SET_KEYBIT, key)?;
            }
            ioctl_int(fd, UI_SET_LEDBIT, LED_NUML)?;
            ioctl_int(fd, UI_SET_LEDBIT, LED_CAPSL)
        })?;
        let mouse = Uinput::create("Pulsa trackpad", 0x5002, |fd| {
            ioctl_int(fd, UI_SET_EVBIT, EV_KEY)?;
            ioctl_int(fd, UI_SET_EVBIT, EV_REL)?;
            for b in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
                ioctl_int(fd, UI_SET_KEYBIT, b)?;
            }
            for r in [REL_X, REL_Y, REL_WHEEL, REL_HWHEEL, REL_WHEEL_HI_RES, REL_HWHEEL_HI_RES] {
                ioctl_int(fd, UI_SET_RELBIT, r)?;
            }
            Ok(())
        })?;
        let reader = keyboard.file.try_clone()?;
        std::thread::Builder::new()
            .name("pulsa-leds".into())
            .spawn(move || read_leds(reader, leds))?;
        Ok(Self { keyboard, mouse, rest: Mutex::new((0.0, 0.0, 0, 0)) })
    }

    fn scroll(&self, dx: i32, dy: i32) -> Result<()> {
        let mut rest = self.rest.lock().unwrap();
        // Rueda clásica: una muesca cada 120 unidades, para las apps que
        // no entienden la de alta resolución.
        rest.2 += dy;
        rest.3 += dx;
        let (notch_y, notch_x) = (rest.2 / WHEEL_NOTCH, rest.3 / WHEEL_NOTCH);
        rest.2 -= notch_y * WHEEL_NOTCH;
        rest.3 -= notch_x * WHEEL_NOTCH;
        let mut ev = Vec::with_capacity(4);
        if dy != 0 {
            ev.push((EV_REL, REL_WHEEL_HI_RES, dy));
        }
        if notch_y != 0 {
            ev.push((EV_REL, REL_WHEEL, notch_y));
        }
        if dx != 0 {
            ev.push((EV_REL, REL_HWHEEL_HI_RES, dx));
        }
        if notch_x != 0 {
            ev.push((EV_REL, REL_HWHEEL, notch_x));
        }
        if ev.is_empty() { Ok(()) } else { self.mouse.emit(&ev) }
    }

    fn motion(&self, dx: f64, dy: f64) -> Result<()> {
        let mut rest = self.rest.lock().unwrap();
        let (x, y) = (rest.0 + dx, rest.1 + dy);
        let (ix, iy) = (x.trunc(), y.trunc());
        rest.0 = x - ix;
        rest.1 = y - iy;
        drop(rest);
        if ix == 0.0 && iy == 0.0 {
            return Ok(());
        }
        self.mouse.emit(&[(EV_REL, REL_X, ix as i32), (EV_REL, REL_Y, iy as i32)])
    }
}

/// El escritorio escribe en el teclado virtual los LED que quiere encendidos.
fn read_leds(mut file: File, leds: watch::Sender<Leds>) {
    let mut buf = [0u8; EVENT_SIZE * 16];
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) | Err(_) => return, // el dispositivo se cerró
            Ok(n) => n,
        };
        for chunk in buf[..n].chunks_exact(EVENT_SIZE) {
            // SAFETY: cada trozo es un `struct input_event` completo.
            let ev = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<RawInputEvent>()) };
            if ev.type_ != EV_LED {
                continue;
            }
            leds.send_if_modified(|l| {
                let before = *l;
                match ev.code {
                    LED_CAPSL => l.caps = ev.value != 0,
                    LED_NUML => l.num = ev.value != 0,
                    _ => {}
                }
                *l != before
            });
        }
    }
}

// ── GNOME: escritorio remoto de Mutter ─────────────────────────────────────

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop",
    default_service = "org.gnome.Mutter.RemoteDesktop",
    default_path = "/org/gnome/Mutter/RemoteDesktop"
)]
trait RemoteDesktop {
    fn create_session(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop.Session",
    default_service = "org.gnome.Mutter.RemoteDesktop"
)]
pub trait RemoteDesktopSession {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn notify_pointer_motion_relative(&self, dx: f64, dy: f64) -> zbus::Result<()>;
    fn notify_pointer_button(&self, button: i32, state: bool) -> zbus::Result<()>;
    fn notify_pointer_axis(&self, dx: f64, dy: f64, flags: u32) -> zbus::Result<()>;
    fn notify_keyboard_keycode(&self, keycode: u32, state: bool) -> zbus::Result<()>;
}

/// `NotifyPointerAxis`: desplazamiento con los dedos (suave, con inercia).
const AXIS_SOURCE_FINGER: u32 = 1 << 2;
/// Píxeles de desplazamiento de Mutter por muesca de rueda.
const MUTTER_PX_PER_NOTCH: f64 = 10.0;

async fn gnome_session() -> Result<RemoteDesktopSessionProxy<'static>> {
    let conn = zbus::Connection::session().await.context("sin bus de sesión D-Bus")?;
    let path = RemoteDesktopProxy::new(&conn)
        .await?
        .create_session()
        .await
        .context("Mutter rechazó CreateSession")?;
    let session = RemoteDesktopSessionProxy::builder(&conn).path(path)?.build().await?;
    session.start().await.context("no se pudo iniciar la sesión de escritorio remoto")?;
    Ok(session)
}

// ── Lo que usa el servidor ────────────────────────────────────────────────

pub enum Injector {
    Uinput(UinputPair),
    Gnome(RemoteDesktopSessionProxy<'static>),
}

impl Injector {
    /// uinput si hay permiso; si no, en GNOME, la sesión de Mutter.
    pub async fn create(leds: watch::Sender<Leds>) -> Result<Self> {
        let uinput_err = match UinputPair::create(leds) {
            Ok(dev) => {
                tracing::info!("entrada por /dev/uinput (teclado y trackpad virtuales)");
                return Ok(Self::Uinput(dev));
            }
            Err(e) => e,
        };
        tracing::warn!("{uinput_err:#}; se prueba la sesión de escritorio remoto de GNOME");
        match gnome_session().await {
            Ok(s) => {
                tracing::info!("entrada por el escritorio remoto de GNOME (Bloq Mayús no se verá en la tablet)");
                Ok(Self::Gnome(s))
            }
            Err(e) => bail!(
                "no hay forma de meter teclas en este PC.\n  · uinput: {uinput_err:#}\n  · GNOME: {e:#}\n\
                 Da permiso a /dev/uinput con una regla udev, por ejemplo en \
                 /etc/udev/rules.d/60-pulsa.rules:\n    \
                 KERNEL==\"uinput\", SUBSYSTEM==\"misc\", TAG+=\"uaccess\", OPTIONS+=\"static_node=uinput\"\n\
                 y luego `sudo udevadm trigger` (o reinicia)."
            ),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Uinput(_) => "uinput",
            Self::Gnome(_) => "gnome",
        }
    }

    pub async fn key(&self, code: u16, down: bool) -> Result<()> {
        match self {
            Self::Uinput(d) => d.keyboard.emit(&[(EV_KEY, code, down as i32)]),
            Self::Gnome(s) => Ok(s.notify_keyboard_keycode(code as u32, down).await?),
        }
    }

    pub async fn button(&self, code: u16, down: bool) -> Result<()> {
        match self {
            Self::Uinput(d) => d.mouse.emit(&[(EV_KEY, code, down as i32)]),
            Self::Gnome(s) => Ok(s.notify_pointer_button(code as i32, down).await?),
        }
    }

    pub async fn motion(&self, dx: f64, dy: f64) -> Result<()> {
        match self {
            Self::Uinput(d) => d.motion(dx, dy),
            Self::Gnome(s) => Ok(s.notify_pointer_motion_relative(dx, dy).await?),
        }
    }

    /// `dx`, `dy` en unidades de rueda de alta resolución (120 = una
    /// muesca); `dy` positivo sube, como la rueda de un ratón.
    pub async fn scroll(&self, dx: i32, dy: i32) -> Result<()> {
        match self {
            Self::Uinput(d) => d.scroll(dx, dy),
            Self::Gnome(s) => {
                let k = MUTTER_PX_PER_NOTCH / WHEEL_NOTCH as f64;
                Ok(s.notify_pointer_axis(dx as f64 * k, -dy as f64 * k, AXIS_SOURCE_FINGER).await?)
            }
        }
    }

    pub async fn stop(&self) {
        if let Self::Gnome(s) = self {
            let _ = s.stop().await;
        }
    }
}
