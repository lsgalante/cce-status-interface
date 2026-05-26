use std::fs::File;
use std::collections::HashMap;

fn main() {
    let file = File::open("/tmp/current_screen.png").unwrap();
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let width = info.width as usize;
    let bytes_per_pixel = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => panic!("Unsupported color type"),
    };

    let mut color_counts = HashMap::new();
    // Scan the window area: x from 12 to 1800 (physical), y from 68 to 2300 (physical)
    for y in (68..2300).step_by(2) {
        for x in (12..1800).step_by(2) {
            let idx = (y * width + x) * bytes_per_pixel;
            let r = buf[idx];
            let g = buf[idx + 1];
            let b = buf[idx + 2];
            *color_counts.entry((r, g, b)).or_insert(0) += 1;
        }
    }

    let mut sorted_colors: Vec<_> = color_counts.into_iter().collect();
    sorted_colors.sort_by(|a, b| b.1.cmp(&a.1));

    println!("Top 30 most common colors in the system interface window area:");
    for (i, ((r, g, b), count)) in sorted_colors.iter().take(30).enumerate() {
        println!("  #{}: R={}, G={}, B={} (count={})", i + 1, r, g, b, count);
    }
}
