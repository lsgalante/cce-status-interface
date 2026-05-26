use std::fs::File;

fn main() {
    let file = File::open("/home/lsgalante/.gemini/antigravity/brain/2aeef5fd-2989-49f9-a513-b732668c813b/current_screen_unified_2.png").unwrap();
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let width = info.width as usize;
    let height = info.height as usize;
    println!("Image size: {}x{}", width, height);

    // Let's print ASCII representation of the region x = 3200..3840, y = 10..46 using horizontal difference
    for y in 10..46 {
        let mut row_chars = String::new();
        for x in (3200..3840).step_by(2) {
            let idx = (y * width + x) * 4;
            let idx_prev = (y * width + (x - 2)) * 4;

            let r = buf[idx] as i32;
            let g = buf[idx + 1] as i32;
            let b = buf[idx + 2] as i32;

            let r_p = buf[idx_prev] as i32;
            let g_p = buf[idx_prev + 1] as i32;
            let b_p = buf[idx_prev + 2] as i32;

            let diff = (r - r_p).abs() + (g - g_p).abs() + (b - b_p).abs();
            if diff > 15 {
                row_chars.push('#');
            } else {
                row_chars.push('.');
            }
        }
        println!("{:2}: {}", y, row_chars);
    }
}
