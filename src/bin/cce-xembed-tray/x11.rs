//! The X11 half: own the XEmbed system tray selection, adopt each docked
//! icon into a hidden container, and report its pixels as they change.
//!
//! Everything here runs on one blocking thread that reads X events. The
//! D-Bus side only ever *sends* requests (clicks, the release on exit)
//! through an [`XHandle`]; `RustConnection` is thread-safe for that.

use std::collections::HashMap;
use std::sync::Arc;

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
    /// A new icon. `image` is `None` until it has drawn something.
    Docked { icon: Window, title: String, image: Option<SniImage>, at: ClickPoint },
    Image { icon: Window, image: SniImage },
    Title { icon: Window, title: String },
    Undocked { icon: Window },
}

/// Where a forwarded click lands: in root coordinates (what the app reads
/// back as the cursor position, so where its menu opens) and in the icon's.
#[derive(Clone, Copy, Debug)]
pub struct ClickPoint {
    pub root: (i16, i16),
    pub local: (i16, i16),
}

struct Icon {
    container: Window,
    damage: damage::Damage,
    slot: u16,
    size: u16,
    last: Option<Vec<u8>>,
}

/// The request-only view of the connection the D-Bus side holds.
pub struct XHandle {
    conn: Arc<RustConnection>,
    root: Window,
}

impl XHandle {
    /// Press and release `button` on `icon`. Sent with an empty event mask,
    /// which delivers it to the client that created the window — the app
    /// that docked the icon — whatever it selected.
    pub fn click(&self, icon: Window, button: u8, at: ClickPoint) -> Res<()> {
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
        })
    }

    pub fn handle(&self) -> XHandle {
        XHandle { conn: self.conn.clone(), root: self.root }
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
                Event::DestroyNotify(e) => self.undock(e.window, true),
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
                        let title = self.title_of(e.window);
                        let _ = self.tx.send(TrayEvent::Title { icon: e.window, title });
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

        let title = self.title_of(icon);
        let half = (size / 2) as i16;
        let at = ClickPoint { root: (x + half, half), local: (half, half) };
        let image = self.capture_changed(icon);
        log::info!("docked {icon:#x} ({title:?}, {size}px, depth {}) in slot {slot}", geom.depth);
        let _ = self.tx.send(TrayEvent::Docked { icon, title, image, at });
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

    /// The icon's name for the tray: `_NET_WM_NAME`, then `WM_NAME`, then
    /// its WM_CLASS class, since most icon windows are untitled.
    fn title_of(&self, icon: Window) -> String {
        let text = |atom: Atom, kind: Atom| -> Option<String> {
            let reply = self.conn.get_property(false, icon, atom, kind, 0, 256).ok()?.reply().ok()?;
            let s = String::from_utf8_lossy(&reply.value).trim_end_matches('\0').trim().to_string();
            (!s.is_empty()).then_some(s)
        };
        text(self.atoms._NET_WM_NAME, self.atoms.UTF8_STRING)
            .or_else(|| text(AtomEnum::WM_NAME.into(), AtomEnum::ANY.into()))
            .or_else(|| {
                text(AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())
                    .and_then(|c| c.split('\0').nth(1).map(str::to_string))
                    .filter(|c| !c.is_empty())
            })
            .unwrap_or_else(|| "X11 tray icon".to_string())
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
    use super::to_sni_argb;

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
