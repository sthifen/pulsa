//! Ventana de Pulsa (GTK4 + libadwaita), para que se sienta parte del
//! escritorio: estado, QR de la app, código para la WiFi, tablets, e
//! «Iniciar con el sistema».
//!
//! El servidor corre en un hilo aparte con su propio runtime de tokio. Al
//! cerrar la ventana Pulsa sigue en segundo plano (como las apps del
//! sistema); abrirlo otra vez desde el menú vuelve a mostrar la misma ventana,
//! porque GApplication deja una sola instancia. «Salir de Pulsa» lo cierra.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use anyhow::Result;
use gtk::{gdk, gio, glib};

use crate::server::AppState;
use crate::{Args, lan, status};

const APP_ID: &str = "dev.pulsa.Pulsa";
const ICON_SVG: &[u8] = include_bytes!("../packaging/pulsa.svg");
/// También en PNG: no todos los sistemas tienen el cargador de SVG de GTK.
const ICON_PNG: &[u8] = include_bytes!("../packaging/pulsa-256.png");

pub fn run(args: Args) -> Result<()> {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let fondo = args.fondo;
    let ui: Rc<RefCell<Option<Ui>>> = Rc::default();

    app.connect_startup({
        let ui = ui.clone();
        move |app| {
            install_desktop_integration();
            // Sigue vivo aunque no haya ventana: el servidor no se para al cerrarla.
            std::mem::forget(app.hold());
            let rx = start_server(args.clone());
            *ui.borrow_mut() = Some(Ui::build(app, rx));
            add_actions(app, ui.clone());
        }
    });

    let first = Cell::new(true);
    app.connect_activate(move |_| {
        // Arranque con el sistema: la primera vez, sin ventana.
        if first.replace(false) && fondo {
            return;
        }
        if let Some(ui) = ui.borrow().as_ref() {
            ui.window.present();
        }
    });

    // GTK no debe ver nuestras opciones (--ventana, --fondo…): ya las leyó clap.
    app.run_with_args::<&str>(&[]);
    Ok(())
}

/// Arranca el servidor en otro hilo y devuelve por dónde llegará su estado.
fn start_server(args: Args) -> mpsc::Receiver<Result<Arc<AppState>, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("pulsa-servidor".into())
        .spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };
            rt.block_on(async {
                match crate::start(&args).await {
                    Ok(state) => {
                        let _ = tx.send(Ok(state));
                        std::future::pending::<()>().await;
                    }
                    Err(e) => {
                        let _ = tx.send(Err(friendly_error(&e)));
                    }
                }
            });
        })
        .expect("hilo del servidor");
    rx
}

fn friendly_error(e: &anyhow::Error) -> String {
    let text = format!("{e:#}");
    if text.contains("no se pudo escuchar") {
        "Ya hay otro Pulsa en marcha en este equipo (quizá como servicio). Ciérralo y vuelve a abrir Pulsa.".into()
    } else if text.contains("no hay forma de meter teclas") {
        "Pulsa no tiene permiso para crear el teclado virtual. Instálalo con el instalador de una línea, que lo configura.".into()
    } else {
        text
    }
}

/// Icono y entrada del menú con el id de la app: así GNOME pone el icono a
/// la ventana, la agrupa en el dock y deja mostrar avisos.
fn install_desktop_integration() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let data = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).unwrap_or(home.join(".local/share"));
    for (rel, bytes) in [
        ("icons/hicolor/scalable/apps/dev.pulsa.Pulsa.svg", ICON_SVG),
        ("icons/hicolor/256x256/apps/dev.pulsa.Pulsa.png", ICON_PNG),
    ] {
        let icon = data.join(rel);
        if std::fs::read(&icon).ok().as_deref() != Some(bytes) {
            if let Some(dir) = icon.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&icon, bytes);
        }
    }
    if !status::menu_path().is_file() {
        let _ = status::desktop_file(&status::menu_path(), true, false);
    }
}

fn add_actions(app: &adw::Application, ui: Rc<RefCell<Option<Ui>>>) {
    let show = gio::SimpleAction::new("show", None);
    show.connect_activate({
        let ui = ui.clone();
        move |_, _| {
            if let Some(ui) = ui.borrow().as_ref() {
                ui.window.present();
            }
        }
    });
    app.add_action(&show);

    let about = gio::SimpleAction::new("about", None);
    about.connect_activate({
        let ui = ui.clone();
        move |_, _| {
            let Some(ui) = ui.borrow().as_ref().map(|u| u.window.clone()) else { return };
            adw::AboutDialog::builder()
                .application_name("Pulsa")
                .application_icon(APP_ID)
                .version(env!("CARGO_PKG_VERSION"))
                .developer_name("Steven")
                .comments("Tu tablet como teclado mecánico y trackpad.")
                .website("https://github.com/sthifen/pulsa")
                .issue_url("https://github.com/sthifen/pulsa/issues")
                .license_type(gtk::License::MitX11)
                .build()
                .present(Some(&ui));
        }
    });
    app.add_action(&about);

    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate({
        let app = app.clone();
        move |_, _| app.quit()
    });
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
    app.set_accels_for_action("window.close", &["<Ctrl>w"]);
}

// ── La ventana ────────────────────────────────────────────────────────────

struct Ui {
    window: adw::ApplicationWindow,
}

/// Lo que cambia y hay que redibujar.
struct Live {
    title: gtk::Label,
    subtitle: gtk::Label,
    code: gtk::Label,
    tablets: adw::PreferencesGroup,
    rows: Vec<gtk::Widget>,
    shown: Vec<String>,
    wifi_row: adw::ActionRow,
    via_row: adw::ActionRow,
    connect_group: adw::PreferencesGroup,
    online: BTreeMap<u64, String>,
    pair_seen: Option<Instant>,
}

impl Ui {
    fn build(app: &adw::Application, rx: mpsc::Receiver<Result<Arc<AppState>, String>>) -> Self {
        let menu = gio::Menu::new();
        menu.append(Some("Acerca de Pulsa"), Some("app.about"));
        menu.append(Some("Salir de Pulsa"), Some("app.quit"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Menú")
            .build();
        let header = adw::HeaderBar::new();
        header.pack_end(&menu_button);

        // Cabecera: icono, estado en una frase.
        let icon = gtk::Image::builder().pixel_size(96).build();
        match gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON_PNG)) {
            Ok(t) => icon.set_paintable(Some(&t)),
            Err(_) => icon.set_icon_name(Some(APP_ID)),
        }
        let title = gtk::Label::builder().label("Arrancando…").css_classes(["title-1"]).wrap(true).justify(gtk::Justification::Center).build();
        let subtitle = gtk::Label::builder()
            .label("Un momento.")
            .css_classes(["dim-label"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let hero = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(12).build();
        hero.append(&icon);
        hero.append(&title);
        hero.append(&subtitle);

        // Conectar una tablet: QR de la app + pasos + código.
        let qr = gtk::Picture::for_paintable(&qr_texture(status::APK_URL));
        qr.set_can_shrink(false);
        qr.set_halign(gtk::Align::Start);
        qr.set_valign(gtk::Align::Start);
        qr.add_css_class("card");
        let step = |n: &str, text: &str| {
            let l = gtk::Label::builder().label(format!("<b>{n}</b>  {text}")).use_markup(true).wrap(true).xalign(0.0).build();
            l
        };
        let code = gtk::Label::builder().label("······").use_markup(true).xalign(0.0).build();
        let steps = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).hexpand(true).valign(gtk::Align::Center).build();
        steps.append(&step("1", "Escanea el código con la tablet e instala la app."));
        steps.append(&step("2", "Enchúfala con un cable, o tócala en la app si estáis en la misma WiFi."));
        steps.append(&step("3", "Por WiFi, la primera vez escribe este código:"));
        steps.append(&code);
        let connect_box = gtk::Box::builder().spacing(18).margin_top(6).margin_bottom(6).build();
        connect_box.append(&qr);
        connect_box.append(&steps);
        let connect_group = adw::PreferencesGroup::builder().title("Conectar una tablet").build();
        connect_group.add(&connect_box);

        // Tablets
        let tablets = adw::PreferencesGroup::builder().title("Tablets").build();

        // General
        let autostart = adw::SwitchRow::builder()
            .title("Iniciar con el sistema")
            .subtitle("Pulsa se abre en segundo plano al encender el ordenador")
            .active(status::autostart_path().is_file())
            .build();
        autostart.connect_active_notify(|row| {
            if let Err(e) = status::desktop_file(&status::autostart_path(), row.is_active(), true) {
                tracing::warn!("no se pudo cambiar el arranque automático: {e}");
            }
        });
        let general = adw::PreferencesGroup::builder()
            .title("General")
            .description("Al cerrar la ventana, Pulsa sigue funcionando. Para cerrarlo del todo: menú → Salir de Pulsa.")
            .build();
        general.add(&autostart);

        // Detalles (escondidos)
        let wifi_row = adw::ActionRow::builder().title("Direcciones en la WiFi").subtitle("—").subtitle_selectable(true).build();
        let via_row = adw::ActionRow::builder().title("Teclado virtual").subtitle("—").build();
        let version_row = adw::ActionRow::builder().title("Versión").subtitle(env!("CARGO_PKG_VERSION")).build();
        let details = adw::ExpanderRow::builder().title("Detalles").subtitle("Para cuando algo no conecta").build();
        details.add_row(&wifi_row);
        details.add_row(&via_row);
        details.add_row(&version_row);
        let details_group = adw::PreferencesGroup::new();
        details_group.add(&details);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&hero);
        content.append(&connect_group);
        content.append(&tablets);
        content.append(&general);
        content.append(&details_group);
        let clamp = adw::Clamp::builder().maximum_size(560).child(&content).build();
        let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroll));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Pulsa")
            .icon_name(APP_ID)
            .default_width(520)
            .default_height(780)
            .hide_on_close(true)
            .content(&toolbar)
            .build();

        let live = Rc::new(RefCell::new(Live {
            title,
            subtitle,
            code,
            tablets,
            rows: Vec::new(),
            shown: vec!["<sin datos>".into()],
            wifi_row,
            via_row,
            connect_group,
            online: BTreeMap::new(),
            pair_seen: None,
        }));

        // Esperar al servidor y luego refrescar cada medio segundo.
        let state: Rc<RefCell<Option<Arc<AppState>>>> = Rc::default();
        let app_weak = app.downgrade();
        glib::timeout_add_local(Duration::from_millis(500), move || {
            let Some(app) = app_weak.upgrade() else { return glib::ControlFlow::Break };
            if state.borrow().is_none() {
                match rx.try_recv() {
                    Ok(Ok(s)) => *state.borrow_mut() = Some(s),
                    Ok(Err(e)) => {
                        let l = live.borrow();
                        l.title.set_label("Pulsa no pudo arrancar");
                        l.subtitle.set_label(&e);
                        l.connect_group.set_sensitive(false);
                        return glib::ControlFlow::Break;
                    }
                    Err(_) => return glib::ControlFlow::Continue,
                }
            }
            if let Some(s) = state.borrow().as_ref() {
                refresh(&app, &mut live.borrow_mut(), s);
            }
            glib::ControlFlow::Continue
        });

        Ui { window }
    }
}

fn refresh(app: &adw::Application, l: &mut Live, s: &Arc<AppState>) {
    let online = s.online.lock().unwrap().clone();
    let paired = s.pairing.tablets();

    // Avisos del sistema: tablets que entran y salen.
    for (id, name) in &online {
        if !l.online.contains_key(id) {
            notify(app, "tablet", "Tablet conectada", name);
        }
    }
    for (id, name) in &l.online {
        if !online.contains_key(id) {
            notify(app, "tablet", "Tablet desconectada", name);
        }
    }
    l.online = online.clone();

    let code = s.pairing.code();
    // Una tablet nueva pide el código: aviso con el número, para no tener que buscarlo.
    let req = s.pair_request.lock().unwrap().clone();
    if let Some((name, when)) = req {
        if l.pair_seen != Some(when) {
            l.pair_seen = Some(when);
            notify(app, "pair", "Una tablet quiere conectarse", &format!("Escribe {code} en {name}"));
        }
    }

    let n = online.len();
    l.title.set_label(if n == 0 { "Pulsa está listo" } else { "Escribiendo desde la tablet" });
    l.subtitle.set_label(&match n {
        0 => "Abre la app en la tablet para empezar a escribir.".to_string(),
        1 => format!("Conectada: {}", online.values().next().cloned().unwrap_or_default()),
        _ => format!("{n} tablets conectadas"),
    });
    l.code.set_markup(&format!(
        "<span font_family=\"monospace\" size=\"300%\" weight=\"bold\" letter_spacing=\"6000\">{code}</span>"
    ));
    let port = s.port;
    let wifi: Vec<String> = lan::addresses().iter().map(|ip| format!("{ip}:{port}")).collect();
    l.wifi_row.set_subtitle(&if wifi.is_empty() { "Este equipo no está en ninguna WiFi".into() } else { wifi.join("  ·  ") });
    l.via_row.set_subtitle(match s.injector.name() {
        "uinput" => "Teclado y ratón virtuales del sistema (uinput)",
        _ => "Escritorio remoto de GNOME",
    });

    // Lista de tablets: sólo se rehace si cambió.
    let mut key: Vec<String> = online.values().map(|v| format!("on:{v}")).collect();
    key.extend(paired.iter().map(|(id, name)| format!("pa:{id}:{name}")));
    if key == l.shown {
        return;
    }
    l.shown = key;
    for r in l.rows.drain(..) {
        l.tablets.remove(&r);
    }
    for name in online.values() {
        let (label, via) = name.rsplit_once(" · ").unwrap_or((name.as_str(), ""));
        let row = adw::ActionRow::builder()
            .title(label)
            .subtitle(if via == "USB" { "Conectada por cable" } else { "Conectada por WiFi" })
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("input-tablet-symbolic"));
        let dot = gtk::Label::builder().label("Conectada").css_classes(["success"]).build();
        row.add_suffix(&dot);
        l.tablets.add(&row);
        l.rows.push(row.upcast());
    }
    for (id, name) in paired {
        let row = adw::ActionRow::builder()
            .title(if name.is_empty() { "Tablet" } else { name.as_str() })
            .subtitle("Puede conectarse por WiFi sin código")
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("network-wireless-symbolic"));
        let forget = gtk::Button::builder().label("Olvidar").valign(gtk::Align::Center).css_classes(["flat"]).build();
        let s = s.clone();
        forget.connect_clicked(move |_| s.pairing.forget(&id));
        row.add_suffix(&forget);
        l.tablets.add(&row);
        l.rows.push(row.upcast());
    }
    if l.rows.is_empty() {
        let row = adw::ActionRow::builder()
            .title("Ninguna tablet todavía")
            .subtitle("Aparecerán aquí al conectarse")
            .build();
        l.tablets.add(&row);
        l.rows.push(row.upcast());
    }
}

fn notify(app: &adw::Application, id: &str, title: &str, body: &str) {
    let n = gio::Notification::new(title);
    n.set_body(Some(body));
    n.set_default_action("app.show");
    app.send_notification(Some(id), &n);
}

/// QR con el enlace de la app, como textura (sin depender de cargadores SVG).
fn qr_texture(text: &str) -> gdk::MemoryTexture {
    let code = qrcode::QrCode::new(text).expect("QR");
    let modules = code.width();
    let quiet = 3;
    let scale = 4;
    let size = (modules + quiet * 2) * scale;
    let mut buf = vec![255u8; size * size * 4];
    for y in 0..modules {
        for x in 0..modules {
            if code[(x, y)] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = (x + quiet) * scale + dx;
                        let py = (y + quiet) * scale + dy;
                        let i = (py * size + px) * 4;
                        buf[i..i + 3].copy_from_slice(&[0x15, 0x17, 0x1c]);
                    }
                }
            }
        }
    }
    gdk::MemoryTexture::new(
        size as i32,
        size as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(buf),
        size * 4,
    )
}
