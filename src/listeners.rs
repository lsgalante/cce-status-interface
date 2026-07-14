//! Compositor push-update listeners: the status-feed subscriptions and the
//! switcher trigger socket.

use crate::CustomEvent;

pub(crate) async fn spawn_status_listener(sub: &'static str, sender: calloop::channel::Sender<CustomEvent>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    loop {
        let socket_path = match std::env::var("WAYLAND_DISPLAY") {
            Ok(display) => {
                let primary = format!("/tmp/cce-status-interface-{}.sock", display);
                if std::path::Path::new(&primary).exists() {
                    primary
                } else {
                    format!("/tmp/cce-status-{}.sock", display)
                }
            }
            Err(_) => {
                let primary = "/tmp/cce-status-interface.sock".to_string();
                if std::path::Path::new(&primary).exists() {
                    primary
                } else {
                    "/tmp/cce-status.sock".to_string()
                }
            }
        };
        if let Ok(mut stream) = UnixStream::connect(&socket_path).await {
            log::info!("[status-listener] connected to {} for sub '{}'", socket_path, sub);
            if stream.write_all(format!("{}\n", sub).as_bytes()).await.is_ok() {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    let val = line.trim().to_string();
                    log::debug!("[status-listener] received '{}' update: '{}'", sub, val);
                    if !val.is_empty() {
                        let ev = match sub {
                            "viewport" => CustomEvent::ViewportUpdated(val.clone()),
                            "layout" => CustomEvent::LayoutUpdated(val.clone()),
                            "title" => CustomEvent::TitleUpdated(val.clone()),
                            "modifiers" => CustomEvent::ModifiersUpdated(val.clone()),
                            _ => unreachable!(),
                        };
                        let _ = sender.send(ev);
                    }
                    line.clear();
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

pub(crate) async fn spawn_switcher_listener(sender: calloop::channel::Sender<CustomEvent>) {
    use tokio::io::AsyncBufReadExt;
    use tokio::net::UnixListener;
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
    let socket_path = format!("/tmp/cce-status-interface-switcher-{}.sock", display);
    let _ = std::fs::remove_file(&socket_path);

    if let Ok(listener) = UnixListener::bind(&socket_path) {
        log::info!("[switcher-listener] Listening on {}", socket_path);
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let mut reader = tokio::io::BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_ok() {
                    let _ = sender.send(CustomEvent::SwitcherTriggered);
                }
            }
        }
    } else {
        log::warn!("[switcher-listener] Failed to bind to {}", socket_path);
    }
}
