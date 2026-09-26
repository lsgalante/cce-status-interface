//! The X11 half: own the XEmbed system tray selection, adopt each docked
//! icon into a hidden container, and report its pixels as they change.
//!
//! Everything here runs on one blocking thread that reads X events. The
//! D-Bus side only ever *sends* requests (clicks, the release on exit)
//! through an [`XHandle`]; `RustConnection` is thread-safe for that.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// One SNI `IconPixmap` entry: width, height, ARGB32 in network byte order
/// with straight (not premultiplied) alpha.
pub type SniImage = (i32, i32, Vec<u8>);

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        MANAGER,
        _NET_SYSTEM_TRAY_OPCODE,
        _NET_SYSTEM_TRAY_ORIENTATION,
        _NET_SYSTEM_TRAY_VISUAL,
        _XEMBED,
        _XEMBED_INFO,
        _NET_WM_NAME,
        _NET_WM_PID,
        UTF8_STRING,
        _CCE_XEMBED_TRAY_TIME,
    }
}

const SYSTEM_TRAY_REQUEST_DOCK: u32 = 0;
const XEMBED_EMBEDDED_NOTIFY: u32 = 0;
const XEMBED_MAPPED: u32 = 1;

/// WM_CLASS (instance, class) of every container. The compositor matches
/// the class and never shows these windows (`xwayland_override_redirect.rs`,
/// `is_xembed_tray_container`) — keep the two in step.
const CONTAINER_CLASS: &[u8] = b"cce-xembed-tray\0cce-xembed-tray\0";

/// Icon windows are kept square, at their own size clamped to this range.
const MIN_ICON: u16 = 16;
const MAX_ICON: u16 = 64;

pub enum TrayEvent {
    /// A new icon. `image` is `None` until it has drawn something;
    /// `title_settled` is false while the title is only a fallback
    /// (`title::resolve`), so the D-Bus side asks again later.
    Docked { icon: Window, title: String, title_settled: bool, image: Option<SniImage>, at: ClickPoint },
    Image { icon: Window, image: SniImage },
    Title { icon: Window, title: String },
    Undocked { icon: Window },
}

/// Where a forwarded click lands when the host gives no screen point: in
/// root coordinates (what the app reads back as the cursor position, so
/// where its menu opens) and in the icon's. Also the icon's container and
/// size, which a click at a host-given point moves (`XHandle::click`).
#[derive(Clone, Copy, Debug)]
pub struct ClickPoint {
    pub root: (i16, i16),
    pub local: (i16, i16),
    pub container: Window,
    pub size: u16,
}

struct Icon {
    container: Window,
    damage: damage::Damage,
    slot: u16,
    size: u16,
    last: Option<Vec<u8>>,
}

/// How long after a forwarded click a newly mapped override-redirect
/// window counts as the popup that click opened.
const POPUP_ARM: Duration = Duration::from_millis(1500);

/// The popup an app opened in answer to a forwarded click, shared between
/// the X thread (which sees it map and go) and the D-Bus side (which arms
/// the watch on a click and closes the popup on a click-away).
#[derive(Default)]
struct PopupWatch {
    /// When the last click was forwarded, and the root y (X pixels) the
    /// popup should not reach above: the bar's bottom edge, when the host
    /// gave a screen point.
    armed: Option<(Instant, Option<i32>)>,
    popup: Option<Window>,
}

/// The request-only view of the connection the D-Bus side holds.
pub struct XHandle {
    conn: Arc<RustConnection>,
    root: Window,
    atoms: Atoms,
    watch: Arc<Mutex<PopupWatch>>,
}

impl XHandle {
    /// The icon's name, as `Tray` resolves it at dock time. Blocking.
    pub fn title(&self, icon: Window) -> crate::title::Title {
        crate::title::resolve(&self.conn, self.root, &self.atoms, icon)
    }

    /// Press and release `button` on `icon`. Sent with an empty event mask,
    /// which delivers it to the client that created the window — the app
    /// that docked the icon — whatever it selected.
    ///
    /// `screen` is the host's click point in screen (logical) coordinates,
    /// as SNI's Activate/ContextMenu carry it; cce's bar sends the pointer's
    /// x at the bar's bottom edge. An app opens its menu at the root
    /// position the event reports, so that point — scaled into X's pixels —
    /// is where the click is placed, and the container is moved under it
    /// first so the icon's real position agrees for an app that asks X
    /// instead. `None` (or the 0,0 a host with no idea sends) keeps the
    /// container where it docked, at the top-right of the X screen.
    pub fn click(&self, icon: Window, button: u8, at: ClickPoint, screen: Option<(i32, i32)>) -> Res<()> {
        let placed = screen.filter(|&(x, y)| x > 0 || y > 0);
        let at = match placed {
            Some(point) => self.move_under(at, point, x11_scale(&self.conn, self.root)),
            None => at,
        };
        // Whatever override-redirect window maps next is this click's
        // popup: armed before the press goes out, so the map cannot win.
        if let Ok(mut w) = self.watch.lock() {
            w.armed = Some((Instant::now(), placed.map(|_| i32::from(at.root.1))));
        }
        let mask = 1u16 << (7 + button.min(5));
        for (kind, state) in [(BUTTON_PRESS_EVENT, 0u16), (BUTTON_RELEASE_EVENT, mask)] {
            let ev = ButtonPressEvent {
                response_type: kind,
                detail: button,
                sequence: 0,
                time: CURRENT_TIME,
                root: self.root,
                event: icon,
                child: NONE,
                root_x: at.root.0,
                root_y: at.root.1,
                event_x: at.local.0,
                event_y: at.local.1,
                state: KeyButMask::from(state),
                same_screen: true,
            };
            self.conn.send_event(false, icon, EventMask::NO_EVENT, ev)?;
        }
        self.conn.flush()?;
        Ok(())
    }

    /// Put `at`'s container so the icon's bottom-centre is the screen point
    /// (in X pixels), and return the click point to match.
    fn move_under(&self, at: ClickPoint, (sx, sy): (i32, i32), scale: f64) -> ClickPoint {
        let (rx, ry) = to_x11_point(sx, sy, scale);
        let size = i32::from(at.size);
        let half = size / 2;
        let cx = (rx - half).max(0);
        let cy = (ry - (size - 1)).max(0);
        let _ = self
            .conn
            .configure_window(at.container, &ConfigureWindowAux::new().x(cx).y(cy));
        let local = ((rx - cx).clamp(0, size - 1), (ry - cy).clamp(0, size - 1));
        ClickPoint {
            root: (clamp_i16(cx + local.0), clamp_i16(cy + local.1)),
            local: (clamp_i16(local.0), clamp_i16(local.1)),
            ..at
        }
    }

    /// Close the popup a forwarded click opened, if it is still up: press
    /// and release at a point just outside it, addressed to the popup. The
    /// app holds the mouse capture while its menu is open, so it reads that
    /// as a click outside the menu and closes it — what a click on any
    /// other window would do on Windows, and what one on a Wayland window
    /// cannot, since Xwayland never delivers it (see the compositor's
    /// `clickaway` status topic, which is what calls this).
    pub fn dismiss_popup(&self) {
        let Some(popup) = self.watch.lock().ok().and_then(|w| w.popup) else {
            return;
        };
        let Some(geom) = self.conn.get_geometry(popup).ok().and_then(|c| c.reply().ok()) else {
            return;
        };
        let (ex, ey) = (-8i16, -8i16);
        for (kind, state) in [(BUTTON_PRESS_EVENT, 0u16), (BUTTON_RELEASE_EVENT, 1u16 << 8)] {
            let ev = ButtonPressEvent {
                response_type: kind,
                detail: 1,
                sequence: 0,
                time: CURRENT_TIME,
                root: self.root,
                event: popup,
                child: NONE,
                root_x: geom.x.saturating_add(ex),
                root_y: geom.y.saturating_add(ey),
                event_x: ex,
                event_y: ey,
                state: KeyButMask::from(state),
                same_screen: true,
            };
            let _ = self.conn.send_event(false, popup, EventMask::NO_EVENT, ev);
        }
        let _ = self.conn.flush();
        log::info!("click-away: closing popup {popup:#x}");
    }

    /// Hand every icon back on the way out: unmapped, on the root window,
    /// which the XEmbed client reads as "the tray is gone" and so goes
    /// looking for the next one (or its own fallback).
    pub fn release(&self, icons: impl IntoIterator<Item = Window>) {
        for icon in icons {
            let _ = self.conn.unmap_window(icon);
            let _ = self.conn.reparent_window(icon, self.root, 0, 0);
        }
        let _ = self.conn.flush();
    }
}

pub struct Tray {
    conn: Arc<RustConnection>,
    root: Window,
    root_width: u16,
    atoms: Atoms,
    selection: Atom,
    manager: Window,
    depth: u8,
    visual: Visualid,
    colormap: Colormap,
    icons: HashMap<Window, Icon>,
    tx: UnboundedSender<TrayEvent>,
    watch: Arc<Mutex<PopupWatch>>,
}

impl Tray {
    pub fn new(conn: Arc<RustConnection>, screen_num: usize, tx: UnboundedSender<TrayEvent>) -> Res<Self> {
        let screen = conn.setup().roots[screen_num].clone();
        let atoms = Atoms::new(&*conn)?.reply()?;
        let selection = conn
            .intern_atom(false, format!("_NET_SYSTEM_TRAY_S{screen_num}").as_bytes())?
            .reply()?
            .atom;
        // Damage needs both versions negotiated before its first request.
        conn.xfixes_query_version(5, 0)?.reply()?;
        conn.damage_query_version(1, 1)?.reply()?;

        // An ARGB visual, advertised through _NET_SYSTEM_TRAY_VISUAL, lets
        // an icon draw with real transparency instead of over a solid box.
        let (depth, visual) = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| {
                d.visuals
                    .iter()
                    .filter(|v| v.class == VisualClass::TRUE_COLOR)
                    .map(move |v| (d.depth, v.visual_id))
            })
            .next()
            .unwrap_or((screen.root_depth, screen.root_visual));
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual)?;

        let manager = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            manager,
            screen.root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().override_redirect(1).event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            manager,
            atoms._NET_SYSTEM_TRAY_ORIENTATION,
            AtomEnum::CARDINAL,
            &[0], // horizontal
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            manager,
            atoms._NET_SYSTEM_TRAY_VISUAL,
            AtomEnum::VISUALID,
            &[visual],
        )?;
        // Top-level map/unmap events, to catch the popup a forwarded click
        // opens (`PopupWatch`). SubstructureNotify is shareable; only the
        // redirect mask is the window manager's alone.
        conn.change_window_attributes(
            screen.root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY),
        )?;
        conn.flush()?;
        log::info!("tray visual {visual:#x} (depth {depth})");

        Ok(Tray {
            conn,
            root: screen.root,
            root_width: screen.width_in_pixels,
            atoms,
            selection,
            manager,
            depth,
            visual,
            colormap,
            icons: HashMap::new(),
            tx,
            watch: Arc::new(Mutex::new(PopupWatch::default())),
        })
    }

    pub fn handle(&self) -> XHandle {
        XHandle { conn: self.conn.clone(), root: self.root, atoms: self.atoms, watch: self.watch.clone() }
    }

    /// Take the tray selection and announce it. Another tray already
    /// holding it is left alone: this waits for that owner to go away
    /// rather than stealing it, so two trays never fight over the icons.
    pub fn acquire(&mut self) -> Res<()> {
        loop {
            let owner = self.conn.get_selection_owner(self.selection)?.reply()?.owner;
            if owner == NONE {
                break;
            }
            log::info!("tray selection held by window {owner:#x}; waiting for it to be released");
            let watch = ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY);
            if self.conn.change_window_attributes(owner, &watch)?.check().is_err() {
                continue; // gone already
            }
            if self.conn.get_selection_owner(self.selection)?.reply()?.owner != owner {
                continue;
            }
            loop {
                if let Event::DestroyNotify(e) = self.conn.wait_for_event()? {
                    if e.window == owner {
                        break;
                    }
                }
            }
        }
        let time = self.server_time()?;
        self.conn.set_selection_owner(self.manager, self.selection, time)?;
        if self.conn.get_selection_owner(self.selection)?.reply()?.owner != self.manager {
            return Err("lost the race for the tray selection".into());
        }
        let announce = ClientMessageEvent::new(
            32,
            self.root,
            self.atoms.MANAGER,
            [time, self.selection, self.manager, 0, 0],
        );
        self.conn.send_event(false, self.root, EventMask::STRUCTURE_NOTIFY, announce)?;
        self.conn.flush()?;
        log::info!("own the XEmbed system tray selection");
        Ok(())
    }

    /// A real server timestamp, which ICCCM asks selection owners to use:
    /// touch a property on our own window and read the notify's time.
    fn server_time(&self) -> Res<Timestamp> {
        self.conn.change_property8(
            PropMode::APPEND,
            self.manager,
            self.atoms._CCE_XEMBED_TRAY_TIME,
            AtomEnum::STRING,
            &[],
        )?;
        self.conn.flush()?;
        loop {
            if let Event::PropertyNotify(e) = self.conn.wait_for_event()? {
                if e.window == self.manager {
                    return Ok(e.time);
                }
            }
        }
    }

    /// Serve until the selection is taken by another tray (`Ok`) or the X
    /// connection fails (`Err`).
    pub fn run(&mut self) -> Res<()> {
        loop {
            let event = self.conn.wait_for_event()?;
            match event {
                Event::ClientMessage(e)
                    if e.window == self.manager && e.type_ == self.atoms._NET_SYSTEM_TRAY_OPCODE =>
                {
                    let data = e.data.as_data32();
                    // BEGIN_MESSAGE / CANCEL_MESSAGE (balloons) are ignored.
                    if data[1] == SYSTEM_TRAY_REQUEST_DOCK {
                        if let Err(err) = self.dock(data[2]) {
                            log::warn!("dock of {:#x} failed: {err}", data[2]);
                        }
                    }
                }
                Event::SelectionClear(e) if e.selection == self.selection => {
                    log::info!("another tray took the selection; handing over");
                    return Ok(());
                }
                Event::DestroyNotify(e) => {
                    self.popup_gone(e.window);
                    self.undock(e.window, true)
                }
                Event::UnmapNotify(e) => self.popup_gone(e.window),
                Event::MapNotify(e) if e.override_redirect => self.popup_mapped(e.window)?,
                Event::ReparentNotify(e) => {
                    if self.icons.get(&e.window).is_some_and(|i| i.container != e.parent) {
                        self.undock(e.window, false);
                    }
                }
                Event::ConfigureNotify(e) => {
                    // The embedder decides an icon's size; an icon that
                    // resizes itself is put back.
                    if let Some(icon) = self.icons.get(&e.window) {
                        if e.width != icon.size || e.height != icon.size || e.x != 0 || e.y != 0 {
                            let size = u32::from(icon.size);
                            let aux = ConfigureWindowAux::new().x(0).y(0).width(size).height(size);
                            self.conn.configure_window(e.window, &aux)?;
                        }
                    }
                }
                Event::PropertyNotify(e) if self.icons.contains_key(&e.window) => {
                    if e.atom == self.atoms._XEMBED_INFO {
                        self.apply_xembed_info(e.window)?;
                    } else if e.atom == self.atoms._NET_WM_NAME || e.atom == u32::from(AtomEnum::WM_NAME) {
                        let title = crate::title::resolve(&self.conn, self.root, &self.atoms, e.window);
                        let _ = self.tx.send(TrayEvent::Title { icon: e.window, title: title.text });
                    }
                }
                Event::DamageNotify(e) => {
                    self.conn.damage_subtract(e.damage, NONE, NONE)?;
                    if let Some(image) = self.capture_changed(e.drawable) {
                        let _ = self.tx.send(TrayEvent::Image { icon: e.drawable, image });
                    }
                }
                // Requests on a window that vanished under us (an icon's
                // app exiting mid-dock) fail harmlessly.
                Event::Error(e) => log::debug!("X error: {e:?}"),
                _ => {}
            }
            self.conn.flush()?;
        }
    }

    /// An override-redirect window mapped: if a forwarded click is still
    /// armed, it is that click's popup. Remember it for the click-away, and
    /// if it covers the bar, move it down to the bar's bottom edge. Windows
    /// tray apps open their menu UPWARD from the pointer, the taskbar being
    /// at the bottom there; with the pointer on a top bar there is no room
    /// above, and the app clamps the menu to the screen top — over the very
    /// icon that opened it. The app keeps working in the moved window:
    /// X reports pointer positions relative to the window, which is what
    /// the app hit-tests with.
    fn popup_mapped(&mut self, window: Window) -> Res<()> {
        if self.icons.values().any(|i| i.container == window) {
            return Ok(());
        }
        let anchor = {
            let Ok(mut w) = self.watch.lock() else { return Ok(()) };
            match w.armed {
                Some((at, anchor)) if at.elapsed() < POPUP_ARM => {
                    w.popup = Some(window);
                    anchor
                }
                _ => return Ok(()),
            }
        };
        let geom = self.conn.get_geometry(window)?.reply()?;
        log::info!("popup {window:#x} opened at {},{} {}x{}", geom.x, geom.y, geom.width, geom.height);
        if let Some(bar_bottom) = anchor {
            if i32::from(geom.y) < bar_bottom {
                self.conn.configure_window(window, &ConfigureWindowAux::new().y(bar_bottom))?;
                log::info!("popup {window:#x} moved below the bar to y={bar_bottom}");
            }
        }
        Ok(())
    }

    fn popup_gone(&self, window: Window) {
        if let Ok(mut w) = self.watch.lock() {
            if w.popup == Some(window) {
                w.popup = None;
            }
        }
    }

    fn dock(&mut self, icon: Window) -> Res<()> {
        if icon == NONE || self.icons.contains_key(&icon) {
            return Ok(());
        }
        let geom = self.conn.get_geometry(icon)?.reply()?;
        let size = geom.width.max(geom.height).clamp(MIN_ICON, MAX_ICON);
        let slot = (0u16..).find(|s| !self.icons.values().any(|i| i.slot == *s)).unwrap_or(0);

        // Containers sit along the top-right of the X screen, one slot per
        // icon. Nobody sees them — the compositor never draws them — but
        // an app opens its menu at the click's root position, and the
        // status bar's tray is up there.
        let x = (i32::from(self.root_width) - i32::from(slot + 1) * i32::from(size)).max(0) as i16;
        let container = self.conn.generate_id()?;
        self.conn.create_window(
            self.depth,
            container,
            self.root,
            x,
            0,
            size,
            size,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .colormap(self.colormap)
                .override_redirect(1),
        )?;
        self.conn.change_property8(
            PropMode::REPLACE,
            container,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            CONTAINER_CLASS,
        )?;
        // An empty input region: X never routes the pointer into a
        // container, so one can never swallow a click meant for a real X
        // window it happens to sit above. Clicks reach icons only as the
        // events `XHandle::click` sends.
        self.conn.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            ClipOrdering::UNSORTED,
            container,
            0,
            0,
            &[],
        )?;

        self.conn.change_window_attributes(
            icon,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
        )?;
        // If this process dies, X hands the icon back to the root window
        // instead of destroying it with the container.
        self.conn.change_save_set(SetMode::INSERT, icon)?;
        self.conn.reparent_window(icon, container, 0, 0)?;
        let s = u32::from(size);
        self.conn
            .configure_window(icon, &ConfigureWindowAux::new().x(0).y(0).width(s).height(s))?;

        let damage = self.conn.generate_id()?;
        self.conn.damage_create(damage, icon, damage::ReportLevel::NON_EMPTY)?;
        self.icons.insert(icon, Icon { container, damage, slot, size, last: None });

        self.apply_xembed_info(icon)?;
        self.conn
            .configure_window(container, &ConfigureWindowAux::new().stack_mode(StackMode::BELOW))?;
        self.conn.map_window(container)?;
        let notify = ClientMessageEvent::new(
            32,
            icon,
            self.atoms._XEMBED,
            [CURRENT_TIME, XEMBED_EMBEDDED_NOTIFY, 0, container, 0],
        );
        self.conn.send_event(false, icon, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;

        let title = crate::title::resolve(&self.conn, self.root, &self.atoms, icon);
        let half = (size / 2) as i16;
        let at = ClickPoint { root: (x + half, half), local: (half, half), container, size };
        let image = self.capture_changed(icon);
        log::info!("docked {icon:#x} ({:?}, {size}px, depth {}) in slot {slot}", title.text, geom.depth);
        let _ = self.tx.send(TrayEvent::Docked {
            icon,
            title: title.text,
            title_settled: title.settled,
            image,
            at,
        });
        Ok(())
    }

    /// Map or unmap the icon as its `_XEMBED_INFO` flags say. An icon
    /// without the property is mapped: that is what trays do in practice.
    fn apply_xembed_info(&self, icon: Window) -> Res<()> {
        let reply = self
            .conn
            .get_property(false, icon, self.atoms._XEMBED_INFO, AtomEnum::ANY, 0, 2)?
            .reply()?;
        let flags = reply.value32().and_then(|mut v| v.nth(1));
        if flags.map_or(true, |f| f & XEMBED_MAPPED != 0) {
            self.conn.map_window(icon)?;
        } else {
            self.conn.unmap_window(icon)?;
        }
        Ok(())
    }

    fn undock(&mut self, icon: Window, destroyed: bool) {
        let Some(entry) = self.icons.remove(&icon) else {
            return;
        };
        // A destroyed drawable takes its damage object with it.
        if !destroyed {
            let _ = self.conn.damage_destroy(entry.damage);
        }
        let _ = self.conn.destroy_window(entry.container);
        let _ = self.conn.flush();
        log::info!("undocked {icon:#x}");
        let _ = self.tx.send(TrayEvent::Undocked { icon });
    }

    /// Read the icon's pixels; `Some` only when they differ from what was
    /// last reported and are not entirely transparent (not drawn yet).
    fn capture_changed(&mut self, icon: Window) -> Option<SniImage> {
        let size = self.icons.get(&icon)?.size;
        let reply = match self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, icon, 0, 0, size, size, !0)
            .ok()?
            .reply()
        {
            Ok(r) => r,
            Err(e) => {
                log::debug!("capture of {icon:#x} failed: {e}");
                return None;
            }
        };
        let lsb = self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        let pixels = to_sni_argb(&reply.data, reply.depth, lsb)?;
        if pixels.chunks_exact(4).all(|p| p[0] == 0) {
            return None;
        }
        let entry = self.icons.get_mut(&icon)?;
        if entry.last.as_ref() == Some(&pixels) {
            return None;
        }
        entry.last = Some(pixels.clone());
        Some((i32::from(size), i32::from(size), pixels))
    }
}

/// How many X pixels one logical pixel is: cce writes `Xft.dpi` into the
/// root window's resources for its X11 scale (192 at scale 2), so it is
/// read back from there. 1 when absent.
fn x11_scale(conn: &RustConnection, root: Window) -> f64 {
    let dpi = conn
        .get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| xft_dpi(&String::from_utf8_lossy(&r.value)));
    dpi.map_or(1.0, |d| d / 96.0).max(1.0)
}

/// `Xft.dpi` from an X resource database string.
pub fn xft_dpi(resources: &str) -> Option<f64> {
    resources.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == "Xft.dpi").then(|| v.trim().parse().ok()).flatten()
    })
}

/// A logical screen point in X root pixels.
pub fn to_x11_point(x: i32, y: i32, scale: f64) -> (i32, i32) {
    ((f64::from(x) * scale).round() as i32, (f64::from(y) * scale).round() as i32)
}

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Convert a 32-bits-per-pixel ZPixmap to SNI's ARGB32: network byte
/// order, straight alpha. X's ARGB visuals are premultiplied; a depth-24
/// image has no alpha and is opaque. `None` for any other layout.
pub fn to_sni_argb(data: &[u8], depth: u8, lsb_first: bool) -> Option<Vec<u8>> {
    if data.len() % 4 != 0 || !(depth == 24 || depth == 32) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    for px in data.chunks_exact(4) {
        let bytes = [px[0], px[1], px[2], px[3]];
        let v = if lsb_first { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) };
        let a = if depth == 32 { (v >> 24) as u8 } else { 255 };
        let unpremultiply = |c: u32| -> u8 {
            match a {
                0 => 0,
                255 => c as u8,
                a => ((c * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8,
            }
        };
        out.extend_from_slice(&[
            a,
            unpremultiply((v >> 16) & 0xff),
            unpremultiply((v >> 8) & 0xff),
            unpremultiply(v & 0xff),
        ]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{to_sni_argb, to_x11_point, xft_dpi};

    #[test]
    fn scale_comes_from_xft_dpi() {
        assert_eq!(xft_dpi("Xcursor.size:\t48\nXft.dpi:\t192\n"), Some(192.0));
        assert_eq!(xft_dpi("Xcursor.size: 48\n"), None);
        assert_eq!(to_x11_point(1167, 27, 2.0), (2334, 54));
        assert_eq!(to_x11_point(1167, 27, 1.0), (1167, 27));
    }

    #[test]
    fn premultiplied_argb_becomes_straight_network_order() {
        // 50% alpha, premultiplied red 0x80: straight red is 0xff.
        let px = 0x8080_0000u32.to_le_bytes();
        assert_eq!(to_sni_argb(&px, 32, true).unwrap(), vec![0x80, 0xff, 0x00, 0x00]);
        assert_eq!(to_sni_argb(&0x8080_0000u32.to_be_bytes(), 32, false).unwrap(), vec![0x80, 0xff, 0, 0]);
    }

    #[test]
    fn depth_24_is_opaque_and_transparent_stays_zero() {
        let px = 0x0012_3456u32.to_le_bytes();
        assert_eq!(to_sni_argb(&px, 24, true).unwrap(), vec![0xff, 0x12, 0x34, 0x56]);
        assert_eq!(to_sni_argb(&[0, 0, 0, 0], 32, true).unwrap(), vec![0, 0, 0, 0]);
    }

    #[test]
    fn other_layouts_are_refused() {
        assert!(to_sni_argb(&[0, 0, 0], 32, true).is_none());
        assert!(to_sni_argb(&[0, 0, 0, 0], 16, true).is_none());
    }
}
