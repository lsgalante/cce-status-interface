//! StatusNotifierItem tray host: the SNI watcher/host D-Bus interfaces,
//! item fetching, and icon loading.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{CustomEvent, TrayItem, TrayPixmap};

#[derive(Debug, Clone)]
pub struct NotifierAddress {
    pub destination: String,
    pub path: String,
}

impl NotifierAddress {
    pub fn from_notifier_service(service: &str, sender: &str) -> Result<Self, String> {
        if service.starts_with('/') {
            Ok(NotifierAddress {
                destination: sender.to_string(),
                path: service.to_string(),
            })
        } else if let Some((destination, path)) = service.split_once('/') {
            Ok(NotifierAddress {
                destination: destination.to_string(),
                path: format!("/{}", path),
            })
        } else if service.contains(':') {
            let split = service.split(':').collect::<Vec<&str>>();
            Ok(NotifierAddress {
                destination: format!(":{}", split[1]),
                path: "/StatusNotifierItem".to_string(),
            })
        } else {
            Ok(NotifierAddress {
                destination: service.to_string(),
                path: "/StatusNotifierItem".to_string(),
            })
        }
    }
}

#[zbus::proxy(
    interface = "org.kde.StatusNotifierItem",
    default_path = "/StatusNotifierItem"
)]
pub(crate) trait StatusNotifierItem {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn category(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn title(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_pixmap(&self) -> zbus::Result<Vec<(i32, i32, Vec<u8>)>>;

    #[zbus(signal)]
    fn new_icon(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn new_title(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_status(&self) -> zbus::Result<()>;

    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;
    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;

    #[zbus(property)]
    fn item_is_menu(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn menu(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

pub(crate) fn find_icon_file(dir: &std::path::Path, icon_name: &str) -> Option<std::path::PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            if let Ok(file_type) = entry.file_type() {
                let path = entry.path();
                if file_type.is_dir() {
                    if !file_type.is_symlink() {
                        if let Some(found) = find_icon_file(&path, icon_name) {
                            return Some(found);
                        }
                    }
                } else if file_type.is_file() {
                    if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
                        if file_name == format!("{}.png", icon_name) || file_name == format!("{}.svg", icon_name) {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn load_png_as_pixmap(path: &std::path::Path) -> Option<TrayPixmap> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    
    let width = info.width as i32;
    let height = info.height as i32;
    let mut argb_pixels = Vec::with_capacity((width * height * 4) as usize);
    
    let actual_bytes = &buf[..info.buffer_size()];
    match info.color_type {
        png::ColorType::Rgba => {
            for chunk in actual_bytes.chunks_exact(4) {
                argb_pixels.push(chunk[3]); // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[1]); // G
                argb_pixels.push(chunk[2]); // B
            }
        }
        png::ColorType::Rgb => {
            for chunk in actual_bytes.chunks_exact(3) {
                argb_pixels.push(255);      // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[1]); // G
                argb_pixels.push(chunk[2]); // B
            }
        }
        png::ColorType::Grayscale => {
            for &g in actual_bytes {
                argb_pixels.push(255); // A
                argb_pixels.push(g);   // R
                argb_pixels.push(g);   // G
                argb_pixels.push(g);   // B
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in actual_bytes.chunks_exact(2) {
                argb_pixels.push(chunk[1]); // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[0]); // G
                argb_pixels.push(chunk[0]); // B
            }
        }
        _ => return None,
    }
    
    Some(TrayPixmap {
        width,
        height,
        pixels: argb_pixels,
    })
}

pub(crate) fn load_svg_as_pixmap(path: &std::path::Path) -> Option<TrayPixmap> {
    let svg_data = std::fs::read(path).ok()?;
    let opt = resvg::usvg::Options::default();
    let fontdb = resvg::usvg::fontdb::Database::new();
    let tree = resvg::usvg::Tree::from_data(&svg_data, &opt, &fontdb).ok()?;
    
    let target_w = 48;
    let target_h = 48;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(target_w, target_h)?;
    
    let orig_w = tree.size().width();
    let orig_h = tree.size().height();
    let sx = target_w as f32 / orig_w;
    let sy = target_h as f32 / orig_h;
    let transform = resvg::tiny_skia::Transform::from_scale(sx, sy);
    
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    
    let raw_pixels = pixmap.data();
    let mut argb_pixels = Vec::with_capacity((target_w * target_h * 4) as usize);
    for chunk in raw_pixels.chunks_exact(4) {
        argb_pixels.push(chunk[3]); // A
        argb_pixels.push(chunk[0]); // R
        argb_pixels.push(chunk[1]); // G
        argb_pixels.push(chunk[2]); // B
    }
    
    Some(TrayPixmap {
        width: target_w as i32,
        height: target_h as i32,
        pixels: argb_pixels,
    })
}

pub(crate) fn resolve_icon_path(theme_path: Option<&str>, icon_name: &str) -> Option<std::path::PathBuf> {
    if icon_name.is_empty() {
        return None;
    }

    // Dropbox registers plain "dropbox" but ships only dropboxstatus-* icons.
    let icon_name = if icon_name == "dropbox" { "dropboxstatus-idle" } else { icon_name };

    // SNI IconThemePath: the item may point at its own icon directory, which
    // outranks any installed theme. Layout inside it is unspecified, hence the
    // recursive scan.
    if let Some(path_str) = theme_path {
        if !path_str.is_empty() {
            let path = std::path::Path::new(path_str);
            if path.exists() {
                if let Some(found) = find_icon_file(path, icon_name) {
                    return Some(found);
                }
            }
        }
    }

    cce_ui::icon::lookup_in(icon_name, &["status", "apps"])
}

pub(crate) async fn fetch_tray_item(conn: &zbus::Connection, addr: &NotifierAddress) -> Result<TrayItem, zbus::Error> {
    let proxy = StatusNotifierItemProxy::builder(conn)
        .destination(addr.destination.clone())?
        .path(addr.path.clone())?
        .build()
        .await?;

    let id = format!("{}/{}", addr.destination, addr.path.trim_start_matches('/'));
    let icon_name = proxy.icon_name().await.ok();
    let icon_theme_path = proxy.icon_theme_path().await.ok();
    let title = proxy.title().await.ok();
    let dbus_id = proxy.id().await.ok();

    let mut pixmaps = proxy.icon_pixmap().await.ok().and_then(|v| {
        if v.is_empty() || (v.len() == 1 && v[0].0 == 0 && v[0].1 == 0) {
            None
        } else {
            Some(v.into_iter()
                .map(|(w, h, pixels)| TrayPixmap {
                    width: w,
                    height: h,
                    pixels,
                })
                .collect::<Vec<_>>())
        }
    });

    if pixmaps.is_none() {
        if let Some(ref name) = icon_name {
            if let Some(icon_path) = resolve_icon_path(icon_theme_path.as_deref(), name) {
                let ext = icon_path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let pixmap = if ext.eq_ignore_ascii_case("svg") {
                    load_svg_as_pixmap(&icon_path)
                } else {
                    load_png_as_pixmap(&icon_path)
                };
                if let Some(pixmap) = pixmap {
                    pixmaps = Some(vec![pixmap]);
                }
            }
        }
    }

    Ok(TrayItem {
        id,
        icon_name,
        icon_theme_path,
        pixmaps,
        title,
        dbus_id,
    })
}

struct Watcher {
    registered_items: Arc<tokio::sync::Mutex<HashMap<String, NotifierAddress>>>,
    sender: calloop::channel::Sender<CustomEvent>,
    tokio_handle: tokio::runtime::Handle,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .unwrap_or_else(|| service.to_string());
        
        if let Ok(addr) = NotifierAddress::from_notifier_service(service, &sender) {
            let mut items = self.registered_items.lock().await;
            let full_address = format!("{}/{}", addr.destination, addr.path.trim_start_matches('/'));
            if !items.contains_key(&full_address) {
                items.insert(full_address.clone(), addr.clone());
                
                let conn = conn.clone();
                let addr_clone = addr.clone();
                let sender_clone = self.sender.clone();
                
                self.tokio_handle.spawn(async move {
                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                        let _ = sender_clone.send(CustomEvent::TrayUpdated(item));
                    }
                    
                    // Listen for updates
                    if let Ok(proxy) = StatusNotifierItemProxy::builder(&conn)
                        .destination(addr_clone.destination.clone())
                        .unwrap()
                        .path(addr_clone.path.clone())
                        .unwrap()
                        .build()
                        .await
                    {
                        let mut new_icon_stream = proxy.receive_new_icon().await.ok();
                        let mut new_title_stream = proxy.receive_new_title().await.ok();
                        let mut new_status_stream = proxy.receive_new_status().await.ok();
                        
                        use tokio_stream::StreamExt;
                        loop {
                            tokio::select! {
                                Some(_) = async {
                                    if let Some(ref mut s) = new_icon_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = sender_clone.send(CustomEvent::TrayUpdated(item));
                                    }
                                }
                                Some(_) = async {
                                    if let Some(ref mut s) = new_title_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = sender_clone.send(CustomEvent::TrayUpdated(item));
                                    }
                                }
                                Some(_) = async {
                                    if let Some(ref mut s) = new_status_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = sender_clone.send(CustomEvent::TrayUpdated(item));
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }
    }

    async fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    async fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        let items = self.registered_items.lock().await;
        items.keys().cloned().collect()
    }
}

struct StatusInterface;

#[zbus::interface(name = "org.clear.StatusInterface")]
impl StatusInterface {
    async fn notify_attention(&self, app_id: String, title: String) {
        log::debug!("[status-interface] Received NotifyAttention: app_id={}, title={}", app_id, title);
        let title_escaped = title.replace('\'', "'\\''");
        let app_id_escaped = app_id.replace('\'', "'\\''");
        let cmd = format!(
            "notify-send -a '{}' '{} needs attention' 'This window has requested activation.'",
            app_id_escaped, title_escaped
        );
        std::process::Command::new("sh")
            .args(["-c", &cmd])
            .spawn()
            .ok();
    }
}

pub(crate) async fn spawn_status_tray(sender: calloop::channel::Sender<CustomEvent>) {
    let registered_items = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let tokio_handle = tokio::runtime::Handle::current();

    // Across a service restart the outgoing tray process can still own the
    // well-known name for a moment, so NameTaken here is normally transient.
    // Without the retry the new process gave up for good and the tray hosted
    // no icons until the next restart.
    let mut conn = None;
    for attempt in 1..=10 {
        if attempt > 1 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let watcher = Watcher {
            registered_items: registered_items.clone(),
            sender: sender.clone(),
            tokio_handle: tokio_handle.clone(),
        };
        let builder = match zbus::ConnectionBuilder::session() {
            Ok(b) => b,
            Err(e) => {
                log::warn!("Failed to initialize D-Bus session: {:?}", e);
                return;
            }
        };
        match builder
            .name("org.kde.StatusNotifierWatcher")
            .unwrap()
            .serve_at("/StatusNotifierWatcher", watcher)
            .unwrap()
            .serve_at("/StatusInterface", StatusInterface)
            .unwrap()
            .build()
            .await
        {
            Ok(c) => {
                conn = Some(c);
                break;
            }
            Err(e) => log::warn!("Failed to build D-Bus connection (attempt {}/10): {:?}", attempt, e),
        }
    }
    let Some(conn) = conn else {
        log::warn!("StatusNotifierWatcher name never became available; tray disabled");
        return;
    };

    log::info!("StatusNotifierWatcher running successfully on D-Bus!");

    // Start NameOwnerChanged listener to detect when tray apps disconnect
    let dbus_proxy = match zbus::fdo::DBusProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Failed to create DBusProxy: {:?}", e);
            return;
        }
    };
    let mut owner_changes = match dbus_proxy.receive_name_owner_changed().await {
        Ok(oc) => oc,
        Err(e) => {
            log::warn!("Failed to receive name owner changed: {:?}", e);
            return;
        }
    };

    let registered_items_clone = registered_items.clone();
    let sender_clone = sender.clone();
    
    tokio::spawn(async move {
        use tokio_stream::StreamExt;
        while let Some(signal) = owner_changes.next().await {
            if let Ok(args) = signal.args() {
                let old = args.old_owner;
                let new = args.new_owner;
                let old_opt: &Option<_> = &*old;
                if let Some(ref old_owner) = old_opt {
                    if new.is_none() {
                        let mut items = registered_items_clone.lock().await;
                        let mut to_remove = Vec::new();
                        for (key, addr) in items.iter() {
                            if addr.destination == old_owner.as_str() {
                                to_remove.push(key.clone());
                            }
                        }
                        for key in to_remove {
                            items.remove(&key);
                            let _ = sender_clone.send(CustomEvent::TrayRemoved(key));
                        }
                    }
                }
            }
        }
    });

    // Keep the task alive
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_icon_path;

    /// The two tray-specific pieces kept local when the theme search moved to
    /// `cce_ui::icon::lookup_in`: the dropbox alias, and the SNI IconThemePath
    /// directory outranking the theme search (recursively — Dropbox nests its
    /// icons under images/hicolor/<size>/status/).
    #[test]
    fn theme_path_override_and_dropbox_alias() {
        let root = std::env::temp_dir().join("cce-tray-icon-test");
        let status = root.join("hicolor/16x16/status");
        std::fs::create_dir_all(&status).unwrap();
        let icon = status.join("dropboxstatus-idle.png");
        std::fs::write(&icon, b"x").unwrap();

        let theme_path = root.to_str().unwrap();
        assert_eq!(resolve_icon_path(Some(theme_path), "dropbox"), Some(icon.clone()));
        assert_eq!(resolve_icon_path(Some(theme_path), "dropboxstatus-idle"), Some(icon));
        assert_eq!(resolve_icon_path(Some(theme_path), ""), None);

        std::fs::remove_dir_all(&root).ok();
    }
}
