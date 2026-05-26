use std::fs::File;

fn main() {
    let file = File::open("/tmp/current_screen.png").expect("Failed to open screenshot");
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().expect("Failed to read PNG info");
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("Failed to decode PNG frame");
    let width = info.width as usize;
    let height = info.height as usize;
    let bytes_per_pixel = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => panic!("Unsupported color type"),
    };

    let mut found = 0;
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * bytes_per_pixel;
            let r = buf[idx];
            let g = buf[idx + 1];
            let b = buf[idx + 2];
            if r < 30 && g < 30 && b > 50 {
                found += 1;
                if found <= 20 {
                    println!("Pixel at ({}, {}): R={}, G={}, B={}", x, y, r, g, b);
                }
            }
        }
    }
    println!("Total pixels matching: {}", found);
}
