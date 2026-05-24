#[tokio::main]
async fn main() {
    println!("Starting read_volume diagnostics...");
    
    let vol_res = tokio::process::Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .output()
        .await;
        
    match &vol_res {
        Ok(output) => {
            println!("pactl get-sink-volume succeeded.");
            println!("stdout: {:?}", String::from_utf8_lossy(&output.stdout));
            println!("stderr: {:?}", String::from_utf8_lossy(&output.stderr));
        }
        Err(e) => {
            println!("pactl get-sink-volume failed: {:?}", e);
        }
    }

    let mute_res = tokio::process::Command::new("pactl")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .output()
        .await;

    match &mute_res {
        Ok(output) => {
            println!("pactl get-sink-mute succeeded.");
            println!("stdout: {:?}", String::from_utf8_lossy(&output.stdout));
            println!("stderr: {:?}", String::from_utf8_lossy(&output.stderr));
        }
        Err(e) => {
            println!("pactl get-sink-mute failed: {:?}", e);
        }
    }

    // Try parsing
    if let Ok(vol_output) = vol_res {
        let vol_str = String::from_utf8_lossy(&vol_output.stdout);
        let mut pct = None;
        if let Some(pos) = vol_str.find('%') {
            let start = vol_str[..pos].rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(0);
            println!("pos: {}, start: {}, substring: {:?}", pos, start, &vol_str[start..pos]);
            if let Ok(num) = vol_str[start..pos].parse::<u32>() {
                pct = Some(num);
            }
        }
        println!("Parsed pct: {:?}", pct);
    }
}
