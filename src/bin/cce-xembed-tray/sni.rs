//! The D-Bus half: one StatusNotifierItem per docked icon.
//!
//! Each item gets its own session-bus connection and registers by object
//! PATH, so the watcher records the connection's unique name as the item's
//! address. That is what the status bar's watcher drops items on
//! (`NameOwnerChanged` for a vanishing unique name), so closing an item's
//! connection is the whole of unregistering it.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio_stream::StreamExt;

use crate::x11::{ClickPoint, SniImage, TrayEvent, XHandle};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";

struct Item {
    icon: u32,
    title: String,
    image: SniImage,
    at: ClickPoint,
    x: Arc<XHandle>,
}

impl Item {
    fn click(&self, button: u8) {
        if let Err(e) = self.x.click(self.icon, button, self.at) {
            log::warn!("click on {:#x} failed: {e}", self.icon);
        }
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> String {
        format!("xembed-{:x}", self.icon)
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        &self.title
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn window_id(&self) -> u32 {
        0
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<SniImage> {
        vec![self.image.clone()]
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<SniImage>, String, String) {
        (String::new(), Vec::new(), self.title.clone(), String::new())
    }

    /// Always false, and there is deliberately no `Menu` property: an
    /// XEmbed icon draws its own menu, which a right-click (ContextMenu)
    /// reaches. The bar fetches a D-Bus menu instead whenever `Menu` exists.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    fn activate(&self, _x: i32, _y: i32) {
        self.click(1);
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {
        self.click(2);
    }

    fn context_menu(&self, _x: i32, _y: i32) {
        self.click(3);
    }

    fn scroll(&self, delta: i32, orientation: &str) {
        let horizontal = orientation.eq_ignore_ascii_case("horizontal");
        let button = match (horizontal, delta > 0) {
            (false, true) => 5,
            (false, false) => 4,
            (true, true) => 7,
            (true, false) => 6,
        };
        self.click(button);
    }

    #[zbus(signal)]
    async fn new_icon(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_title(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_tool_tip(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()>;
}

/// An icon the X side reported. It is published once it has pixels: an
/// icon that has not drawn yet would be an empty slot in the bar.
enum Entry {
    Pending { title: String, at: ClickPoint },
    Published(zbus::Connection),
}

async fn register(conn: &zbus::Connection) {
    let reply = conn
        .call_method(Some(WATCHER), "/StatusNotifierWatcher", Some(WATCHER), "RegisterStatusNotifierItem", &(ITEM_PATH,))
        .await;
    if let Err(e) = reply {
        // No watcher yet: the NameOwnerChanged arm registers everything
        // once one appears.
        log::debug!("register with {WATCHER} failed: {e}");
    }
}

async fn publish(x: &Arc<XHandle>, icon: u32, title: String, image: SniImage, at: ClickPoint) -> zbus::Result<zbus::Connection> {
    let item = Item { icon, title, image, at, x: x.clone() };
    let conn = zbus::ConnectionBuilder::session()?.serve_at(ITEM_PATH, item)?.build().await?;
    register(&conn).await;
    log::info!("published {icon:#x} as {}", conn.unique_name().map(|n| n.to_string()).unwrap_or_default());
    Ok(conn)
}

async fn update(conn: &zbus::Connection, image: Option<SniImage>, title: Option<String>) -> zbus::Result<()> {
    let iface = conn.object_server().interface::<_, Item>(ITEM_PATH).await?;
    {
        let mut item = iface.get_mut().await;
        if let Some(image) = image.clone() {
            item.image = image;
        }
        if let Some(title) = title.clone() {
            item.title = title;
        }
    }
    let ctxt = iface.signal_context();
    if image.is_some() {
        Item::new_icon(ctxt).await?;
    }
    if title.is_some() {
        Item::new_title(ctxt).await?;
        Item::new_tool_tip(ctxt).await?;
    }
    Ok(())
}

/// Mirror the X side's icons onto the bus until it stops (its channel
/// closes) or the process is told to exit; then hand the icons back.
pub async fn run(x: Arc<XHandle>, mut events: UnboundedReceiver<TrayEvent>) -> zbus::Result<()> {
    let control = zbus::Connection::session().await?;
    let dbus = zbus::fdo::DBusProxy::new(&control).await?;
    let mut watcher_owner = dbus.receive_name_owner_changed_with_args(&[(0, WATCHER)]).await?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;

    let mut entries: HashMap<u32, Entry> = HashMap::new();
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                match event {
                    TrayEvent::Docked { icon, title, image, at } => {
                        let entry = match image {
                            Some(image) => match publish(&x, icon, title.clone(), image, at).await {
                                Ok(conn) => Entry::Published(conn),
                                Err(e) => {
                                    log::warn!("publishing {icon:#x} failed: {e}");
                                    Entry::Pending { title, at }
                                }
                            },
                            None => Entry::Pending { title, at },
                        };
                        entries.insert(icon, entry);
                    }
                    TrayEvent::Image { icon, image } => match entries.remove(&icon) {
                        Some(Entry::Pending { title, at }) => {
                            let entry = match publish(&x, icon, title.clone(), image, at).await {
                                Ok(conn) => Entry::Published(conn),
                                Err(e) => {
                                    log::warn!("publishing {icon:#x} failed: {e}");
                                    Entry::Pending { title, at }
                                }
                            };
                            entries.insert(icon, entry);
                        }
                        Some(Entry::Published(conn)) => {
                            if let Err(e) = update(&conn, Some(image), None).await {
                                log::warn!("icon update for {icon:#x} failed: {e}");
                            }
                            entries.insert(icon, Entry::Published(conn));
                        }
                        None => {}
                    },
                    TrayEvent::Title { icon, title } => match entries.get_mut(&icon) {
                        Some(Entry::Pending { title: t, .. }) => *t = title,
                        Some(Entry::Published(conn)) => {
                            if let Err(e) = update(conn, None, Some(title)).await {
                                log::warn!("title update for {icon:#x} failed: {e}");
                            }
                        }
                        None => {}
                    },
                    // Dropping the connection releases its unique name,
                    // which is what removes the item from the bar.
                    TrayEvent::Undocked { icon } => {
                        entries.remove(&icon);
                    }
                }
            }
            Some(change) = watcher_owner.next() => {
                // A (re)started status bar: register every item with it.
                let appeared = change.args().map(|a| a.new_owner.is_some()).unwrap_or(false);
                if appeared {
                    for entry in entries.values() {
                        if let Entry::Published(conn) = entry {
                            register(conn).await;
                        }
                    }
                }
            }
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
        }
    }
    x.release(entries.keys().copied());
    Ok(())
}
