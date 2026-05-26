use std::fs::File;

fn main() {
    let file = File::open("/tmp/current_screen.png").unwrap();
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let width = info.width as usize;
    let height = info.height as usize;
    let color_type = info.color_type;
    println!("Image size: {}x{}, color_type: {:?}", width, height, color_type);

    let bytes_per_pixel = match color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        _ => panic!("Unsupported color type"),
    };

    // The actual status bar background color is R=69, G=69, B=89
    let bg_r = 69;
    let bg_g = 69;
    let bg_b = 89;

    println!("Strikethrough line pixels at y=30, x=3266..3344 (step by 8):");
    for x in (3266..3344).step_by(8) {
        let idx = (30 * width + x) * bytes_per_pixel;
        println!("  x={}: R={}, G={}, B={}", x, buf[idx], buf[idx+1], buf[idx+2]);
    }
}
