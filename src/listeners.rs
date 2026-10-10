//! Compositor push-update listeners: the status-feed subscriptions and the
//! switcher trigger socket.

use crate::CustomEvent;
use cce_ui::ipc::ctl::StatusTopic;

/// Subscribe to one compositor status topic and forward its pushes as
/// [`CustomEvent`]s, reconnecting every second until the socket is there.
///
/// `topic` is cce-core's `ctl::StatusTopic`, the names the compositor serves.
pub(crate) async fn spawn_status_listener(topic: StatusTopic, sender: calloop::channel::Sender<CustomEvent>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    // Retry delay, doubling while connections keep ending without ever
    // delivering a line and reset the moment one does. A compositor that
    // does not know this topic drops the subscription on sight, so the flat
    // 1s retry turned an unrecognized topic into a permanent once-a-second
    // reconnect from every module process — which is exactly what a bar
    // running ahead of its compositor does with a new topic (the two halves
    // deploy separately, and cce-fx only restarts at login).
    let mut retry_s = 1u64;
    loop {
        let socket_path = cce_ui::ipc::ctl::status_socket();
        if let Ok(mut stream) = UnixStream::connect(&socket_path).await {
            log::info!("[status-listener] connected to {} for topic '{}'", socket_path, topic);
            if stream.write_all(format!("{}\n", topic).as_bytes()).await.is_ok() {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    // Anything at all means the topic is understood.
                    retry_s = 1;
                    let val = line.trim().to_string();
                    log::debug!("[status-listener] received '{}' update: '{}'", topic, val);
                    if !val.is_empty() {
                        let ev = match topic {
                            StatusTopic::Layout => CustomEvent::LayoutUpdated(val.clone()),
                            StatusTopic::Title => CustomEvent::TitleUpdated(val.clone()),
                            // Click-away-close: the payload is the app_id of
                            // the segment the press landed on ("-" for none).
                            StatusTopic::Dismiss => CustomEvent::MenuDismiss(val.clone()),
                            other => unreachable!("the bar does not subscribe to {other}"),
                        };
                        let _ = sender.send(ev);
                    }
                    line.clear();
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(retry_s)).await;
        retry_s = (retry_s * 2).min(30);
    }
}

/// The window-switcher trigger socket, `/tmp/cce-status-interface-switcher-<display>.sock`:
/// `cce-status-interface --trigger-switcher` connects and sends `trigger`.
pub(crate) const SWITCHER_PREFIX: &str = "cce-status-interface-switcher";

pub(crate) async fn spawn_switcher_listener(sender: calloop::channel::Sender<CustomEvent>) {
    use tokio::io::AsyncBufReadExt;
    use tokio::net::UnixListener;
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
    let socket_path = cce_ui::ipc::socket_path_for(SWITCHER_PREFIX, Some(&display));
    let _ = std::fs::remove_file(&socket_path);

    if let Ok(listener) = UnixListener::bind(&socket_path) {
        log::info!("[switcher-listener] Listening on {}", socket_path);
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                // Bounded in size and in TOTAL time: connections are served
                // one at a time here, so one that never finished its line
                // (until 2026-10-02 nothing timed it out) left the Super+Tab
                // switcher dead for the rest of the session.
                use tokio::io::AsyncReadExt;
                let mut reader = tokio::io::BufReader::new(stream.take(4096));
                let mut line = String::new();
                let read = tokio::time::timeout(std::time::Duration::from_secs(2), reader.read_line(&mut line)).await;
                if matches!(read, Ok(Ok(n)) if n > 0) {
                    let _ = sender.send(CustomEvent::SwitcherTriggered);
                }
            }
        }
    } else {
        log::warn!("[switcher-listener] Failed to bind to {}", socket_path);
    }
}
