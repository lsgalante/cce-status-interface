use std::fs::File;

fn main() {
    let file = File::open("/home/lsgalante/.gemini/antigravity/brain/2aeef5fd-2989-49f9-a513-b732668c813b/current_screen_unified_2.png").unwrap();
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let width = info.width as usize;

    // Sample background at (100, 20)
    let bg_idx = (20 * width + 100) * 3;
    let bg_r = buf[bg_idx] as i32;
    let bg_g = buf[bg_idx + 1] as i32;
    let bg_b = buf[bg_idx + 2] as i32;
    println!("Background color sampled at (100, 20): R={}, G={}, B={}", bg_r, bg_g, bg_b);

    println!("Background color sampled at (3200, 20): R={}, G={}, B={}", buf[(20 * width + 3200) * 3], buf[(20 * width + 3200) * 3 + 1], buf[(20 * width + 3200) * 3 + 2]);
    println!("Background color sampled at (1500, 20): R={}, G={}, B={}", buf[(20 * width + 1500) * 3], buf[(20 * width + 1500) * 3 + 1], buf[(20 * width + 1500) * 3 + 2]);

    println!("Checking volume text region (x = 3264..3345, y = 12..44):");
    let mut text_pixels = 0;
    for y in 12..44 {
        for x in 3264..3345 {
            let idx = (y * width + x) * 3;
            let r = buf[idx] as i32;
            let g = buf[idx + 1] as i32;
            let b = buf[idx + 2] as i32;

            let diff = (r - bg_r).abs() + (g - bg_g).abs() + (b - bg_b).abs();
            if diff > 15 {
                text_pixels += 1;
            }
        }
    }
    println!("Found {} active pixels in volume region.", text_pixels);

    // Let's also check a region that should be empty (e.g. x = 3200..3250, y = 12..44)
    let mut empty_pixels = 0;
    for y in 12..44 {
        for x in 3200..3250 {
            let idx = (y * width + x) * 3;
            let r = buf[idx] as i32;
            let g = buf[idx + 1] as i32;
            let b = buf[idx + 2] as i32;

            let diff = (r - bg_r).abs() + (g - bg_g).abs() + (b - bg_b).abs();
            if diff > 15 {
                empty_pixels += 1;
            }
        }
    }
    println!("Found {} active pixels in empty region.", empty_pixels);
}
