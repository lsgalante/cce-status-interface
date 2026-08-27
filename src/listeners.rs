//! Compositor push-update listeners: the status-feed subscriptions and the
//! switcher trigger socket.

use crate::CustomEvent;

/// Subscribe to one compositor status topic and forward its pushes as
/// [`CustomEvent`]s, reconnecting every second until the socket is there.
///
/// `sub` is the whole subscription line, because one topic takes an argument:
/// `backdrop <app_id>` asks what THAT segment is composited over (every other
/// topic is the same for all subscribers, so it is a bare word).
pub(crate) async fn spawn_status_listener(sub: String, sender: calloop::channel::Sender<CustomEvent>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    // Retry delay, doubling while connections keep ending without ever
    // delivering a line and reset the moment one does. A compositor that
    // does not know this topic drops the subscription on sight, so the flat
    // 1s retry turned an unrecognized topic into a permanent once-a-second
    // reconnect from every module process — which is exactly what a bar
    // running ahead of its compositor does with `backdrop` (the two halves
    // deploy separately, and cce-fx only restarts at login).
    let mut retry_s = 1u64;
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
                    // Anything at all means the topic is understood.
                    retry_s = 1;
                    let val = line.trim().to_string();
                    log::debug!("[status-listener] received '{}' update: '{}'", sub, val);
                    if !val.is_empty() {
                        let topic = sub.split_whitespace().next().unwrap_or("");
                        let ev = match topic {
                            "layout" => CustomEvent::LayoutUpdated(val.clone()),
                            "title" => CustomEvent::TitleUpdated(val.clone()),
                            // Click-away-close: the payload is the app_id of
                            // the segment the press landed on ("-" for none).
                            "dismiss" => CustomEvent::MenuDismiss(val.clone()),
                            // "<luma> <spread>", both 0-100, or "unknown"
                            // when the compositor has no sample for this
                            // segment — treated as the worst case rather
                            // than as no news.
                            "backdrop" => CustomEvent::BackdropUpdated(parse_backdrop(&val)),
                            _ => unreachable!(),
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

/// Parse a `backdrop` line into (luma, spread), both 0-100.
///
/// Anything unreadable — the literal "unknown", a truncated line, a future
/// compositor's extra fields — reports the worst case: mid luminance and full
/// spread, which drives the outline. Guessing "uniform and bright" from a
/// line we failed to understand would silently turn the treatment OFF, and
/// unreadable text is a worse failure than an unnecessary halo.
pub(crate) fn parse_backdrop(line: &str) -> (u8, u8) {
    const UNKNOWN: (u8, u8) = (50, 100);
    let mut parts = line.split_whitespace();
    let (Some(luma), Some(spread)) = (parts.next(), parts.next()) else {
        return UNKNOWN;
    };
    // Parsed wide and range-checked rather than clamped, so that "101" and
    // "300" fail the same way. Clamping would quietly turn an out-of-protocol
    // luma into "bright and uniform" — the one answer that switches the
    // treatment off.
    match (luma.parse::<u16>(), spread.parse::<u16>()) {
        (Ok(l), Ok(s)) if l <= 100 && s <= 100 => (l as u8, s as u8),
        _ => UNKNOWN,
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
