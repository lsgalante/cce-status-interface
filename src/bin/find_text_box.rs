use std::fs::File;

fn main() {
    let file = File::open("/home/lsgalante/.gemini/antigravity/brain/2aeef5fd-2989-49f9-a513-b732668c813b/current_screen_unified_2.png").unwrap();
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let width = info.width as usize;
    let bg_idx = (20 * width + 100) * 3;
    let bg_r = buf[bg_idx] as i32;
    let bg_g = buf[bg_idx + 1] as i32;
    let bg_b = buf[bg_idx + 2] as i32;
    println!("Background color sampled at (100, 20): R={}, G={}, B={}", bg_r, bg_g, bg_b);
    
    // Status bar is in y = 0..80
    // Let's count non-background pixels in each column for y = 10..48
    let mut col_intensity = vec![0; width];
    for x in 0..width {
        for y in 10..48 {
            let idx = (y * width + x) * 3;
            let r = buf[idx] as i32;
            let g = buf[idx + 1] as i32;
            let b = buf[idx + 2] as i32;
            
            let diff = (r - bg_r).abs() + (g - bg_g).abs() + (b - bg_b).abs();
            if diff > 15 {
                col_intensity[x] += 1;
            }
        }
    }
    
    // Group columns into contiguous active segments
    let mut in_segment = false;
    let mut start_x = 0;
    println!("Active segments in status bar (x from left to right):");
    for x in 0..width {
        let active = col_intensity[x] > 2; // threshold of 2 pixels
        if active && !in_segment {
            start_x = x;
            in_segment = true;
        } else if !active && in_segment {
            let end_x = x - 1;
            println!("  Segment at x = {}..{} (width = {})", start_x, end_x, end_x - start_x + 1);
            in_segment = false;
        }
    }
    if in_segment {
        println!("  Segment at x = {}..{} (width = {})", start_x, width - 1, width - start_x);
    }
}
