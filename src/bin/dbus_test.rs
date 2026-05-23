use zbus::{proxy, Connection};

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

#[derive(Debug, Clone)]
pub struct TrayPixmap {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TrayItem {
    pub id: String,
    pub icon_name: Option<String>,
    pub icon_theme_path: Option<String>,
    pub pixmaps: Option<Vec<TrayPixmap>>,
    pub title: Option<String>,
}

#[proxy(
    interface = "org.kde.StatusNotifierItem",
    default_path = "/StatusNotifierItem"
)]
trait StatusNotifierItem {
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
}

#[proxy(
    interface = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher"
)]
trait StatusNotifierWatcher {
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;
}

async fn fetch_tray_item(conn: &Connection, addr: &NotifierAddress) -> Result<TrayItem, zbus::Error> {
    let proxy = StatusNotifierItemProxy::builder(conn)
        .destination(addr.destination.clone())?
        .path(addr.path.clone())?
        .build()
        .await?;

    println!("Querying properties for destination='{}', path='{}'...", addr.destination, addr.path);
    
    let id_res = proxy.id().await;
    println!("  id result: {:?}", id_res);
    
    let category_res = proxy.category().await;
    println!("  category result: {:?}", category_res);

    let status_res = proxy.status().await;
    println!("  status result: {:?}", status_res);

    let icon_name_res = proxy.icon_name().await;
    println!("  icon_name result: {:?}", icon_name_res);

    let icon_theme_path_res = proxy.icon_theme_path().await;
    println!("  icon_theme_path result: {:?}", icon_theme_path_res);

    let title_res = proxy.title().await;
    println!("  title result: {:?}", title_res);

    let pixmap_res = proxy.icon_pixmap().await;
    println!("  pixmap result: ({} pixmaps or error: {:?})", 
        pixmap_res.as_ref().map(|v| v.len()).unwrap_or(0),
        pixmap_res.as_ref().err()
    );

    let id = format!("{}/{}", addr.destination, addr.path.trim_start_matches('/'));
    let icon_name = icon_name_res.ok();
    let icon_theme_path = icon_theme_path_res.ok();
    let title = title_res.ok();

    let pixmaps = pixmap_res.ok().map(|v| {
        v.into_iter()
            .map(|(w, h, pixels)| TrayPixmap {
                width: w,
                height: h,
                pixels,
            })
            .collect()
    });

    Ok(TrayItem {
        id,
        icon_name,
        icon_theme_path,
        pixmaps,
        title,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::session().await?;
    
    let watcher_proxy = StatusNotifierWatcherProxy::builder(&conn)
        .destination("org.kde.StatusNotifierWatcher")?
        .path("/StatusNotifierWatcher")?
        .build()
        .await?;
        
    let items = watcher_proxy.registered_status_notifier_items().await?;
    println!("Registered status notifier items from watcher: {:?}", items);

    for item in items {
        println!("======================================");
        // An item string can be like ":1.5631/org/ayatana/NotificationItem/dropbox_client_1112673"
        // Let's parse it using split_once('/')
        let addr = if let Some((destination, path)) = item.split_once('/') {
            NotifierAddress {
                destination: destination.to_string(),
                path: format!("/{}", path),
            }
        } else {
            NotifierAddress {
                destination: item.clone(),
                path: "/StatusNotifierItem".to_string(),
            }
        };
        
        match fetch_tray_item(&conn, &addr).await {
            Ok(item) => println!("SUCCESS: {:?}", item.id),
            Err(e) => println!("ERROR for {}: {:?}", item, e),
        }
    }

    Ok(())
}
