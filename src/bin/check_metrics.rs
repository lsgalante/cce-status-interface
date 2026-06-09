use glyphon::{FontSystem, Buffer, Metrics, Attrs, Shaping};

fn main() {
    let mut font_system = FontSystem::new();
    let font_size = 28.0;
    let metrics = Metrics::new(font_size, font_size * 1.4);
    let mut buffer = Buffer::new(&mut font_system, metrics);
    let attrs = Attrs::new().family(glyphon::Family::Name("Berkeley Mono"));
    buffer.set_text(&mut font_system, "Hello World", attrs, Shaping::Advanced);
    buffer.shape_until_scroll(&mut font_system, true);

    for run in buffer.layout_runs() {
        println!("line_y: {}, line_w: {}", run.line_y, run.line_w);
        for glyph in run.glyphs {
            println!("  glyph at x: {}, w: {}", glyph.x, glyph.w);
        }
    }
}
