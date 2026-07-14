//! cce-cloud popups: the window picker and D-Bus menus rendered by spawning
//! a `cce-cloud` process fed JSON pages on stdin.

use crate::{parse_ccectl_windows, CustomEvent};
use crate::config::{get_cce_cloud_cmd, get_ccectl_cmd};

#[zbus::proxy(
    interface = "com.canonical.dbusmenu",
    default_path = "/StatusNotifierItem/menu"
)]
pub(crate) trait DBusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::Result<(u32, (i32, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>))>;

    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &zbus::zvariant::Value<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;
}

pub(crate) struct MenuItem {
    id: i32,
    label: String,
    enabled: bool,
    is_separator: bool,
    toggle_state: i32, // -1 if not toggleable, 0 if unchecked, 1 if checked
    children: Vec<MenuItem>,
}

pub(crate) fn parse_menu_item(
    id: i32,
    mut properties: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    children_vals: Vec<zbus::zvariant::OwnedValue>,
) -> Option<MenuItem> {
    let type_: String = properties.remove("type")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();
    let is_separator = type_ == "separator";

    let label: String = properties.remove("label")
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

    let toggle_state: i32 = properties.remove("toggle-state")
        .and_then(|v| {
            let i: Result<i32, _> = v.try_into();
            i.ok()
        })
        .unwrap_or(-1);

    let mut children = Vec::new();
    for child_val in children_vals {
        let child_val_inner = zbus::zvariant::Value::from(child_val);
        if let Ok(child) = <(i32, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>)>::try_from(child_val_inner) {
            if let Some(parsed) = parse_menu_item(child.0, child.1, child.2) {
                children.push(parsed);
            }
        }
    }

    Some(MenuItem {
        id,
        label,
        enabled,
        is_separator,
        toggle_state,
        children,
    })
}

pub(crate) fn get_currently_focused_window() -> Option<String> {
    let output = std::process::Command::new(get_ccectl_cmd())
        .arg("windows")
        .output();
    if let Ok(out) = output {
        let stdout_str = String::from_utf8_lossy(&out.stdout);
        for line in stdout_str.lines() {
            let focused = if let Some(idx) = line.find("focused=") {
                let rest = &line[idx + 8..];
                let end = rest.find(' ').unwrap_or(rest.len());
                rest[..end].trim() == "true"
            } else {
                false
            };

            if focused {
                let app_id = if let Some(idx) = line.find("app_id=") {
                    let rest = &line[idx + 7..];
                    let end = rest.find(' ').unwrap_or(rest.len());
                    rest[..end].to_string()
                } else {
                    continue;
                };
                if app_id == "cce-status" || app_id == "cce-cloud" {
                    continue;
                }
                
                // Return the unique window ID if present, otherwise fall back to app_id
                let id = if let Some(idx) = line.find("window id=") {
                    let rest = &line[idx + 10..];
                    let end = rest.find(' ').unwrap_or(rest.len());
                    rest[..end].to_string()
                } else {
                    app_id
                };
                return Some(id);
            }
        }
    }
    None
}

pub(crate) async fn show_cce_cloud_menu(
    conn: &zbus::Connection,
    destination: &str,
    menu_path: &str,
    x_pos: i32,
    y_pos: i32,
    align_right: bool,
    thread_sender: calloop::channel::Sender<CustomEvent>,
    source: String,
    parent_app_id: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut last_spawned_pid = 0;

    let res = async {
        let menu_proxy = DBusMenuProxy::builder(conn)
            .destination(destination)?
            .path(menu_path)?
            .build()
            .await?;

        let _ = menu_proxy.about_to_show(0).await;
        let (_, layout) = menu_proxy.get_layout(0, 5, vec![]).await?;

        let root_item = match parse_menu_item(layout.0, layout.1, layout.2) {
            Some(item) => item,
            None => return Ok(()),
        };

        // Assign page indices to submenus.
        let mut page_indices = std::collections::HashMap::new();
        page_indices.insert(root_item.id, 0);
        let mut parent_pages = std::collections::HashMap::new();
        let mut next_page = 1;

        fn assign_pages(
            item: &MenuItem,
            current_page: usize,
            page_indices: &mut std::collections::HashMap<i32, usize>,
            parent_pages: &mut std::collections::HashMap<usize, usize>,
            next_page: &mut usize,
        ) {
            for child in &item.children {
                if child.is_separator || !child.enabled {
                    continue;
                }
                if !child.children.is_empty() && *next_page < 16 {
                    let child_page = *next_page;
                    page_indices.insert(child.id, child_page);
                    parent_pages.insert(child_page, current_page);
                    *next_page += 1;
                    assign_pages(child, child_page, page_indices, parent_pages, next_page);
                }
            }
        }

        assign_pages(&root_item, 0, &mut page_indices, &mut parent_pages, &mut next_page);

        #[derive(Debug, Clone)]
        struct LocalWidget {
            widget_type: String,
            text: String,
            id: Option<String>,
            target_page: Option<usize>,
        }

        #[derive(Debug, Clone)]
        struct LocalPage {
            title: String,
            widgets: Vec<LocalWidget>,
        }

        let mut pages = vec![LocalPage {
            title: "".to_string(),
            widgets: Vec::new(),
        }; next_page];

        fn build_pages(
            item: &MenuItem,
            current_page: usize,
            page_indices: &std::collections::HashMap<i32, usize>,
            parent_pages: &std::collections::HashMap<usize, usize>,
            pages: &mut [LocalPage],
        ) {
            let mut widgets = Vec::new();

            if current_page > 0 {
                if let Some(&parent_page) = parent_pages.get(&current_page) {
                    widgets.push(LocalWidget {
                        widget_type: "button".to_string(),
                        text: "< Back".to_string(),
                        id: Some(format!("back_to_{}", parent_page)),
                        target_page: Some(parent_page),
                    });
                }
            }

            for child in &item.children {
                if child.is_separator || !child.enabled {
                    continue;
                }

                let mut display_label = if child.toggle_state == 1 {
                    format!("[x] {}", child.label)
                } else if child.toggle_state == 0 {
                    format!("[ ] {}", child.label)
                } else {
                    child.label.clone()
                };

                if !child.children.is_empty() {
                    if let Some(&target_page) = page_indices.get(&child.id) {
                        display_label = format!("{} >", display_label);

                        widgets.push(LocalWidget {
                            widget_type: "button".to_string(),
                            text: display_label,
                            id: Some(format!("submenu_{}", child.id)),
                            target_page: Some(target_page),
                        });

                        build_pages(child, target_page, page_indices, parent_pages, pages);
                    } else {
                        widgets.push(LocalWidget {
                            widget_type: "button".to_string(),
                            text: display_label,
                            id: Some(format!("item_{}", child.id)),
                            target_page: None,
                        });
                    }
                } else {
                    widgets.push(LocalWidget {
                        widget_type: "button".to_string(),
                        text: display_label,
                        id: Some(format!("item_{}", child.id)),
                        target_page: None,
                    });
                }
            }

            let title = if item.label.is_empty() {
                if current_page == 0 {
                    "Tray Menu".to_string()
                } else {
                    "".to_string()
                }
            } else {
                item.label.clone()
            };

            pages[current_page] = LocalPage {
                title,
                widgets,
            };
        }

        build_pages(&root_item, 0, &page_indices, &parent_pages, &mut pages);

        // Serialize to JSON value
        let mut pages_json = Vec::new();
        for page in pages {
            let mut widgets_json = Vec::new();
            for w in page.widgets {
                let mut w_val = serde_json::json!({
                    "type": w.widget_type,
                    "text": w.text,
                });
                if let Some(id) = w.id {
                    w_val["id"] = serde_json::Value::String(id);
                }
                if let Some(tp) = w.target_page {
                    w_val["target_page"] = serde_json::Value::Number(tp.into());
                }
                widgets_json.push(w_val);
            }
            pages_json.push(serde_json::json!({
                "title": page.title,
                "widgets": widgets_json,
            }));
        }

        let layout_json = serde_json::json!({
            "width": 260,
            "pages": pages_json,
        });
        let layout_str = layout_json.to_string();

        let mut cmd_args = vec![
            "--json".to_string(),
            "-x".to_string(),
            x_pos.to_string(),
            "-y".to_string(),
            y_pos.to_string(),
            "--parent-app-id".to_string(),
            parent_app_id,
        ];
        if align_right {
            cmd_args.push("--align-right".to_string());
        }

        let mut child = std::process::Command::new(get_cce_cloud_cmd())
            .args(&cmd_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;

        let pid = child.id();
        last_spawned_pid = pid;
        let _ = thread_sender.send(CustomEvent::CloudSpawned { pid, source: source.clone() });

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(layout_str.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            if let Ok(parsed_json) = serde_json::from_str::<serde_json::Value>(stdout_str.trim()) {
                if let Some(btn_id) = parsed_json.get("button").and_then(|v| v.as_str()) {
                    if btn_id.starts_with("item_") {
                        if let Ok(item_id) = btn_id["item_".len()..].parse::<i32>() {
                            let timestamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as u32;
                            let val = zbus::zvariant::Value::from("");
                            let _ = menu_proxy.event(item_id, "clicked", &val, timestamp).await;
                        }
                    }
                }
            }
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }.await;

    let _ = thread_sender.send(CustomEvent::CloudClosed { pid: last_spawned_pid, source });
    res
}


/// Spawn the click-to-pick window list as a cce-cloud dmenu process, report
/// lifecycle via CloudSpawned/CloudClosed, and focus the picked window.
pub(crate) fn spawn_window_picker(
    x_pos: i32,
    y_pos: i32,
    thread_sender: calloop::channel::Sender<CustomEvent>,
    source: String,
) {
    std::thread::spawn(move || {
        // Run "ccectl windows" to fetch the windows list
        let output = std::process::Command::new(get_ccectl_cmd())
            .arg("windows")
            .output();

        let windows = if let Ok(out) = output {
            parse_ccectl_windows(&String::from_utf8_lossy(&out.stdout))
        } else {
            Vec::new()
        };

        if windows.is_empty() {
            // If there are no windows, don't open a switcher and clear state
            let _ = thread_sender.send(CustomEvent::CloudClosed { pid: 0, source: source.clone() });
            return;
        }

        // Format items for dmenu, keeping the stable order returned by ccectl
        let mut input_str = String::new();
        for (_, app_id, title, _) in &windows {
            let display = if title.is_empty() {
                app_id.clone()
            } else {
                format!("{} ({})", title, app_id)
            };
            input_str.push_str(&display);
            input_str.push('\n');
        }

        let cmd_args = vec![
            "--dmenu".to_string(),
            "-p".to_string(),
            "Windows:".to_string(),
            "-x".to_string(),
            x_pos.to_string(),
            "-y".to_string(),
            y_pos.to_string(),
        ];

        let mut child = match std::process::Command::new(get_cce_cloud_cmd())
            .args(&cmd_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[switcher] Failed to spawn cce-cloud: {:?}", e);
                let _ = thread_sender.send(CustomEvent::CloudClosed { pid: 0, source: source.clone() });
                return;
            }
        };

        let pid = child.id();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();

        // Write the item list, then drop stdin so cce-cloud sees EOF.
        use std::io::Write;
        let _ = stdin.write_all(input_str.as_bytes());
        let _ = stdin.flush();
        drop(stdin);

        // Spawn stdout reader
        let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out_str = String::new();
            use std::io::Read;
            let _ = stdout.read_to_string(&mut out_str);
            let _ = stdout_tx.send(out_str);
        });

        let _ = thread_sender.send(CustomEvent::CloudSpawned { pid, source: source.clone() });

        let _ = child.wait();
        let stdout_str = stdout_rx.recv().unwrap_or_default();

        let selected = stdout_str.trim().to_string();
        if !selected.is_empty() {
            // Find the matched window
            for (id, app_id, title, _) in windows {
                let display = if title.is_empty() {
                    app_id.clone()
                } else {
                    format!("{} ({})", title, app_id)
                };
                if display == selected {
                    log::debug!("[switcher] Selecting window title: {}, app_id: {}, id: {}", title, app_id, id);
                    let _ = std::process::Command::new(get_ccectl_cmd())
                        .args(["focus-window", &id])
                        .spawn();
                    break;
                }
            }
        }

        let _ = thread_sender.send(CustomEvent::CloudClosed { pid, source: source.clone() });
    });
}
