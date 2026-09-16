//! System statistics: /proc, /sys, and pactl readers plus the polling task
//! that feeds `SystemStats` updates to the bar.

use crate::{CustomEvent, SystemStats};

pub(crate) fn read_cpu_ticks() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let first_line = stat.lines().next()?;
    if first_line.starts_with("cpu ") {
        let parts: Vec<u64> = first_line
            .split_whitespace()
            .skip(1)
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();
        if parts.len() >= 4 {
            let idle = parts[3];
            let total: u64 = parts.iter().sum();
            return Some((total, idle));
        }
    }
    None
}

/// Memory in use as a whole percentage of the total — used being total less
/// free, buffers and page cache, so what an application could not have
/// without the kernel dropping cache first. `None` when /proc/meminfo is
/// unreadable.
pub(crate) fn read_memory_usage() -> Option<u8> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0.0;
    let mut free = 0.0;
    let mut buffers = 0.0;
    let mut cached = 0.0;
    for line in meminfo.lines() {
        let kib = |line: &str| line.split_whitespace().nth(1).and_then(|v| v.parse::<f32>().ok());
        if line.starts_with("MemTotal:") {
            total = kib(line)?;
        } else if line.starts_with("MemFree:") {
            free = kib(line)?;
        } else if line.starts_with("Buffers:") {
            buffers = kib(line)?;
        } else if line.starts_with("Cached:") {
            cached = kib(line)?;
        }
    }
    if total > 0.0 {
        let used = total - free - buffers - cached;
        Some((used / total * 100.0).round().clamp(0.0, 100.0) as u8)
    } else {
        None
    }
}

/// The first battery's `(capacity %, charging)`; `None` on a machine
/// without one.
pub(crate) fn read_battery_details() -> Option<(i32, bool)> {
    for bat in &["BAT0", "BAT1"] {
        let cap_path = format!("/sys/class/power_supply/{}/capacity", bat);
        let status_path = format!("/sys/class/power_supply/{}/status", bat);
        if let Ok(cap_str) = std::fs::read_to_string(&cap_path) {
            let cap = cap_str.trim().parse::<i32>().unwrap_or(0);
            let status = std::fs::read_to_string(&status_path).unwrap_or_default();
            let is_charging = status.trim() == "Charging";
            return Some((cap, is_charging));
        }
    }
    None
}

/// The first backlight's level as a whole percentage; `None` without one.
pub(crate) fn read_brightness() -> Option<i32> {
    let dir = std::fs::read_dir("/sys/class/backlight").ok()?;
    for entry in dir.flatten() {
        let path = entry.path();
        let cur_path = path.join("brightness");
        let max_path = path.join("max_brightness");
        if cur_path.exists() && max_path.exists() {
            let cur_str = std::fs::read_to_string(cur_path).ok()?;
            let max_str = std::fs::read_to_string(max_path).ok()?;
            let cur = cur_str.trim().parse::<f32>().ok()?;
            let max = max_str.trim().parse::<f32>().ok()?;
            if max > 0.0 {
                return Some((cur / max * 100.0).round() as i32);
            }
        }
    }
    None
}

/// The default sink's `(level %, muted)`; `None` when pactl is unavailable
/// or fails. The level is `None` when pactl answered without a percentage.
pub(crate) async fn read_volume() -> Option<(Option<u32>, bool)> {
    let vol_output = match tokio::process::Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            log::warn!("[read_volume] failed to spawn pactl: {:?}", e);
            return None;
        }
    };
    if !vol_output.status.success() {
        log::warn!("[read_volume] pactl get-sink-volume exited with error: {:?}", String::from_utf8_lossy(&vol_output.stderr));
        return None;
    }
    let vol_str = String::from_utf8_lossy(&vol_output.stdout);
    
    let mute_output = match tokio::process::Command::new("pactl")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            log::warn!("[read_volume] failed to spawn pactl mute: {:?}", e);
            return None;
        }
    };
    if !mute_output.status.success() {
        log::warn!("[read_volume] pactl get-sink-mute exited with error: {:?}", String::from_utf8_lossy(&mute_output.stderr));
        return None;
    }
    let mute_str = String::from_utf8_lossy(&mute_output.stdout);
    let muted = mute_str.contains("yes");

    let mut pct = None;
    if let Some(pos) = vol_str.find('%') {
        let start = vol_str[..pos].rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(0);
        if let Ok(num) = vol_str[start..pos].parse::<u32>() {
            pct = Some(num);
        }
    }

    Some((pct, muted))
}

pub(crate) fn get_initial_stats() -> SystemStats {
    let clock = chrono::Local::now().format("%A, %B %d, %Y %I:%M %p").to_string();
    let memory = read_memory_usage();

    SystemStats {
        clock,
        memory,
        cpu_pct: Some(0),
        battery: read_battery_details(),
        volume: pollster::block_on(read_volume()),
        brightness: read_brightness(),
    }
}

pub(crate) async fn spawn_system_stats(sender: calloop::channel::Sender<CustomEvent>) {
    log::info!("[spawn_system_stats] Starting system stats loop!");
    let mut last_cpu = read_cpu_ticks().unwrap_or((0, 0));
    loop {
        log::debug!("[spawn_system_stats] loop iteration start");
        let clock = chrono::Local::now().format("%A, %B %d, %Y %I:%M %p").to_string();
        let memory = read_memory_usage();
        
        let cpu_pct = if let Some(current_cpu) = read_cpu_ticks() {
            let total_diff = current_cpu.0 - last_cpu.0;
            let idle_diff = current_cpu.1 - last_cpu.1;
            last_cpu = current_cpu;
            if total_diff > 0 {
                let usage = 100.0 - (idle_diff as f32 * 100.0 / total_diff as f32);
                Some(usage.round().clamp(0.0, 100.0) as u8)
            } else {
                Some(0)
            }
        } else {
            None
        };

        let stats = SystemStats {
            clock,
            memory,
            cpu_pct,
            battery: read_battery_details(),
            volume: read_volume().await,
            brightness: read_brightness(),
        };
        log::debug!("[spawn_system_stats] stats: {:?}", stats);
        let _ = sender.send(CustomEvent::SystemStatsUpdated(stats));
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
