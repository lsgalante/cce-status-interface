use zbus::{proxy, Connection};
use std::collections::HashMap;
use zbus::zvariant::{OwnedValue, Value};
use std::process::{Command, Stdio};
use std::io::Write;

#[proxy(
    interface = "org.kde.StatusNotifierItem",
    default_path = "/StatusNotifierItem"
)]
trait StatusNotifierItem {
    #[zbus(property)]
    fn item_is_menu(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn menu(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

#[proxy(
    interface = "com.canonical.dbusmenu",
    default_path = "/StatusNotifierItem/menu"
)]
trait DBusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::Result<(u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>))>;

    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &zbus::zvariant::Value<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;
}

fn flatten_menu(
    id: i32,
    mut properties: HashMap<String, OwnedValue>,
    children: Vec<OwnedValue>,
    prefix: &str,
    out: &mut Vec<(i32, String)>
) {
    let label: String = properties.remove("label")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();
    let type_: String = properties.remove("type")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();
    let enabled: bool = properties.remove("enabled")
        .and_then(|v| {
            let b: Result<bool, _> = v.try_into();
            b.ok()
        })
        .unwrap_or(true);

    if type_ == "separator" || !enabled {
        // Skip separator or disabled items
    } else {
        let current_path = if prefix.is_empty() {
            label.clone()
        } else if !label.is_empty() {
            format!("{} > {}", prefix, label)
        } else {
            prefix.to_string()
        };

        if !current_path.is_empty() && children.is_empty() {
            out.push((id, current_path.clone()));
        }

        for child_val in children {
            let child_val_inner = zbus::zvariant::Value::from(child_val);
            if let Ok(child) = <(i32, HashMap<String, OwnedValue>, Vec<OwnedValue>)>::try_from(child_val_inner) {
                flatten_menu(child.0, child.1, child.2, &current_path, out);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::session().await?;
    
    let destination = "org.kde.StatusNotifierItem-997-1";
    let path = "/StatusNotifierItem";
    
    println!("Connecting to StatusNotifierItem at destination='{}', path='{}'...", destination, path);
    let sni_proxy = StatusNotifierItemProxy::builder(&conn)
        .destination(destination)?
        .path(path)?
        .build()
        .await?;
        
    let is_menu = sni_proxy.item_is_menu().await.unwrap_or(false);
    let menu_path = sni_proxy.menu().await?;
    
    println!("is_menu: {}, menu_path: {}", is_menu, menu_path.as_str());
    
    let menu_proxy = DBusMenuProxy::builder(&conn)
        .destination(destination)?
        .path(menu_path.as_str())?
        .build()
        .await?;
        
    // Call about_to_show
    let _ = menu_proxy.about_to_show(0).await;
    
    let (revision, layout) = menu_proxy.get_layout(0, 3, vec![]).await?;
    println!("Menu revision: {}", revision);
    
    let mut items = Vec::new();
    flatten_menu(layout.0, layout.1, layout.2, "", &mut items);
    
    println!("Flattened items count: {}", items.len());
    
    // Format input for fuzzel
    let mut fuzzel_input = String::new();
    for (_, label) in &items {
        fuzzel_input.push_str(label);
        fuzzel_input.push('\n');
    }
    
    // Spawn fuzzel
    println!("Spawning fuzzel...");
    let mut child = Command::new("fuzzel")
        .args(["-dmenu", "-p", "Tray Menu:"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
        
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(fuzzel_input.as_bytes())?;
    }
    
    let output = child.wait_with_output()?;
    if output.status.success() {
        let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
        println!("Selected: '{}'", selected);
        if let Some((id, _)) = items.iter().find(|(_, label)| label == &selected) {
            println!("Triggering event on item ID: {}", id);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;
            let val = Value::from("");
            menu_proxy.event(*id, "clicked", &val, timestamp).await?;
            println!("Event sent successfully!");
        } else {
            println!("Selection match not found in items list!");
        }
    } else {
        println!("Fuzzel was cancelled or failed");
    }
    
    Ok(())
}
