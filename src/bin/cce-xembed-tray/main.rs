//! `cce-xembed-tray` — the legacy X11 system tray, bridged into the bar.
//!
//! X11 apps that predate StatusNotifierItem (Wine and Proton programs above
//! all) put their tray icons in an XEmbed tray: whichever X client owns the
//! `_NET_SYSTEM_TRAY_S0` selection. With no owner, Wine falls back to a
//! window of its own holding the icons, which on this desktop showed up as
//! a blank white window beside Ubisoft Connect. This process owns that
//! selection, adopts each icon window into a container the compositor never
//! shows, and republishes it as an SNI item the status bar's tray hosts:
//! pixels from the icon window as it redraws, clicks forwarded as X button
//! events (left = Activate, right = ContextMenu, so the app draws its own
//! menu).

mod sni;
mod title;
mod x11;

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

fn connect() -> x11::Res<(Arc<x11rb::rust_connection::RustConnection>, usize)> {
    // Xwayland comes up with the compositor, which may still be starting
    // when the session's units are.
    let mut last = None;
    for attempt in 0..10 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        match x11rb::connect(None) {
            Ok((conn, screen)) => return Ok((Arc::new(conn), screen)),
            Err(e) => last = Some(e),
        }
    }
    Err(format!("cannot connect to X: {}", last.map(|e| e.to_string()).unwrap_or_default()).into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let (conn, screen) = match connect() {
        Ok(c) => c,
        Err(e) => {
            log::error!("{e}");
            std::process::exit(1);
        }
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    // The D-Bus side's own way into the same queue: late title lookups.
    let retry_tx = tx.downgrade();
    let mut tray = match x11::Tray::new(conn, screen, tx) {
        Ok(t) => t,
        Err(e) => {
            log::error!("X setup failed: {e}");
            std::process::exit(1);
        }
    };
    let handle = Arc::new(tray.handle());

    // 0 once another tray took over cleanly; 1 when X failed, so systemd
    // restarts us.
    let code = Arc::new(AtomicI32::new(0));
    let x_code = code.clone();
    std::thread::spawn(move || {
        let result = tray.acquire().and_then(|()| tray.run());
        if let Err(e) = result {
            log::error!("X side stopped: {e}");
            x_code.store(1, Ordering::SeqCst);
        }
        // Dropping `tray` closes the channel, which ends `sni::run`.
    });

    if let Err(e) = sni::run(handle, rx, retry_tx).await {
        log::error!("D-Bus side failed: {e}");
        code.store(1, Ordering::SeqCst);
    }
    std::process::exit(code.load(Ordering::SeqCst));
}
