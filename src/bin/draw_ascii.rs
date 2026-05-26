use std::fs::File;

fn main() {
    let file = File::open("/home/lsgalante/.gemini/antigravity/brain/2aeef5fd-2989-49f9-a513-b732668c813b/current_screen_unified.png").unwrap();
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let width = info.width as usize;
    let height = info.height as usize;
    println!("Image size: {}x{}", width, height);

    // Sample background pixel
    let bg_idx = (20 * width + 100) * 4;
    let bg_r = buf[bg_idx] as i32;
    let bg_g = buf[bg_idx + 1] as i32;
    let bg_b = buf[bg_idx + 2] as i32;
    println!("Background color sampled at (100, 20): R={}, G={}, B={}", bg_r, bg_g, bg_b);

    for y in (10..46).step_by(2) {
        let mut row_chars = String::new();
        for x in (2800..3840).step_by(2) {
            let idx = (y * width + x) * 4;
            let r = buf[idx] as i32;
            let g = buf[idx + 1] as i32;
            let b = buf[idx + 2] as i32;

            let diff = (r - bg_r).abs() + (g - bg_g).abs() + (b - bg_b).abs();
            if diff > 20 {
                row_chars.push('#');
            } else {
                row_chars.push('.');
            }
        }
        println!("{:2}: {}", y, row_chars);
    }
}
