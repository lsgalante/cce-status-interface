use std::sync::Arc;
use std::collections::HashMap;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowAttributes};
use winit::platform::wayland::WindowAttributesExtWayland;
use glyphon::{
    Attrs, Buffer, Cache, FontSystem, Metrics, Resolution, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport,
};
use clear_ui::color;
// zbus D-Bus Integration




#[derive(Debug, Clone)]
struct TrayPixmap {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct TrayItem {
    id: String,
    icon_name: Option<String>,
    icon_theme_path: Option<String>,
    pixmaps: Option<Vec<TrayPixmap>>,
    title: Option<String>,
}


#[derive(Debug, Clone)]
struct TrayIconBounds {
    id: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: Option<String>,
}

#[derive(Debug, Clone)]
struct SystemStats {
    clock: String,
    memory: String,
    cpu: String,
    battery: String,
}

#[derive(Debug, Clone)]
enum CustomEvent {
    TagsUpdated(String),
    LayoutUpdated(String),
    TitleUpdated(String),
    SystemStatsUpdated(SystemStats),
    TrayUpdated(TrayItem),
    TrayRemoved(String),
}


#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x4,
    ];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

fn quad_vertices(x: f32, y: f32, w: f32, h: f32, sw: f32, sh: f32, c: [f32; 4]) -> [Vertex; 6] {
    let x0 = (x / sw) * 2.0 - 1.0;
    let y0 = 1.0 - (y / sh) * 2.0;
    let x1 = ((x + w) / sw) * 2.0 - 1.0;
    let y1 = 1.0 - ((y + h) / sh) * 2.0;
    [
        Vertex { position: [x0, y0], color: c },
        Vertex { position: [x1, y0], color: c },
        Vertex { position: [x0, y1], color: c },
        Vertex { position: [x1, y0], color: c },
        Vertex { position: [x1, y1], color: c },
        Vertex { position: [x0, y1], color: c },
    ]
}

fn make_text_buffer(fs: &mut FontSystem, text: &str, size: f32) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buf = Buffer::new(fs, metrics);
    buf.set_text(fs, text, Attrs::new(), glyphon::Shaping::Advanced);
    buf.shape_until_scroll(fs, true);
    buf
}

fn parse_hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    let s = hex.trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
        Some([r, g, b, 1.0])
    } else {
        None
    }
}

fn parse_tags(pango: &str) -> Vec<([f32; 4], String)> {
    let mut result = Vec::new();
    let mut remaining = pango;
    while let Some(start_span) = remaining.find("<span color='") {
        let color_start = start_span + "<span color='".len();
        if let Some(color_end) = remaining[color_start..].find("'") {
            let hex_color = &remaining[color_start..color_start + color_end];
            let tag_start = color_start + color_end + "'>".len();
            if let Some(tag_end) = remaining[tag_start..].find("</span>") {
                let tag_text = &remaining[tag_start..tag_start + tag_end];
                let color = parse_hex_to_rgba(hex_color).unwrap_or([0.8, 0.8, 0.8, 1.0]);
                result.push((color, tag_text.to_string()));
                remaining = &remaining[tag_start + tag_end + "</span>".len()..];
            } else {
                break;
            }
        } else {
            break;
        }
    }
    if result.is_empty() && !pango.is_empty() {
        // Fallback for plain text
        result.push(([0.8, 0.8, 0.8, 1.0], pango.to_string()));
    }
    result
}

fn read_cpu_ticks() -> Option<(u64, u64)> {
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

fn read_memory_usage() -> Option<String> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0.0;
    let mut free = 0.0;
    let mut buffers = 0.0;
    let mut cached = 0.0;
    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            total = line.split_whitespace().nth(1)?.parse::<f32>().ok()? / 1024.0 / 1024.0;
        } else if line.starts_with("MemFree:") {
            free = line.split_whitespace().nth(1)?.parse::<f32>().ok()? / 1024.0 / 1024.0;
        } else if line.starts_with("Buffers:") {
            buffers = line.split_whitespace().nth(1)?.parse::<f32>().ok()? / 1024.0 / 1024.0;
        } else if line.starts_with("Cached:") {
            cached = line.split_whitespace().nth(1)?.parse::<f32>().ok()? / 1024.0 / 1024.0;
        }
    }
    if total > 0.0 {
        let used = total - free - buffers - cached;
        Some(format!("Mem {:.1}G/{:.1}G", used, total))
    } else {
        None
    }
}

fn read_battery() -> Option<String> {
    for bat in &["BAT0", "BAT1"] {
        let cap_path = format!("/sys/class/power_supply/{}/capacity", bat);
        let status_path = format!("/sys/class/power_supply/{}/status", bat);
        if let Ok(cap_str) = std::fs::read_to_string(&cap_path) {
            let cap = cap_str.trim();
            let status = std::fs::read_to_string(&status_path).unwrap_or_default();
            let charge_symbol = if status.trim() == "Charging" { "⚡" } else { "Bat" };
            return Some(format!("{} {}%", charge_symbol, cap));
        }
    }
    None
}

struct RectWidget {
    x: f32, y: f32, w: f32, h: f32,
    color: [f32; 4],
}

struct TextItem {
    buffer: Buffer,
    x: f32, y: f32,
    color: glyphon::Color,
}

struct StatusApp {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,

    // Status State
    tags: String,
    layout: String,
    title: String,
    stats: Option<SystemStats>,
    tray_items: HashMap<String, TrayItem>,
    cursor_pos: (f64, f64),
    hovered_tray_item: Option<String>,
    tray_item_bounds: Vec<TrayIconBounds>,

    font_system: FontSystem,
    swash_cache: SwashCache,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_viewport: Viewport,

    rects: Vec<RectWidget>,
    text_items: Vec<TextItem>,

    scale_factor: f64,
    width: u32,
    height: u32,
    needs_rebuild: bool,
}

impl StatusApp {
    async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.expect("adapter");
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("GPU Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
        }, None).await.expect("device");
        let config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("config");
        surface.configure(&device, &config);

        let shader_code = clear_ui::SHADER;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(shader_code)),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
                strip_index_format: None,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });

        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let mut text_atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let text_renderer = TextRenderer::new(&mut text_atlas, &device, wgpu::MultisampleState::default(), None);
        let mut text_viewport = Viewport::new(&device, &cache);
        text_viewport.update(&queue, Resolution { width: size.width, height: size.height });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vertex Buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let scale_factor = (window.scale_factor() as f32).max(2.0) as f64;

        let mut app = Self {
            window, surface, device, queue, config, render_pipeline,
            vertex_buffer, vertex_count: 0,
            tags: String::new(),
            layout: String::new(),
            title: String::new(),
            stats: None,
            tray_items: HashMap::new(),
            cursor_pos: (0.0, 0.0),
            hovered_tray_item: None,
            tray_item_bounds: Vec::new(),
            font_system, swash_cache, text_atlas, text_renderer, text_viewport,
            rects: Vec::new(), text_items: Vec::new(),
            scale_factor,
            width: size.width, height: size.height,
            needs_rebuild: true,
        };

        app.rebuild_layout();
        app
    }

    fn rebuild_layout(&mut self) {
        let sw = self.width as f32;
        let sh = self.height as f32;
        let s = self.scale_factor as f32;
        let bar_h = 28.0 * s;

        self.rects.clear();
        self.text_items.clear();

        // 1. Background (spans the entire fullscreen area)
        self.rects.push(RectWidget {
            x: 0.0, y: 0.0, w: sw, h: sh,
            color: color::STATUS_BG,
        });
        // 1b. Accent border at bottom of the status bar area
        self.rects.push(RectWidget {
            x: 0.0, y: bar_h - 2.0 * s, w: sw, h: 2.0 * s,
            color: color::STATUS_ACCENT,
        });

        let mut left_x = 12.0 * s;

        // 2. Tags
        let tags = parse_tags(&self.tags);
        for (col, text) in tags {
            let label = format!(" {} ", text);
            let buf = make_text_buffer(&mut self.font_system, &label, 11.0 * s);
            let line_w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
            self.text_items.push(TextItem {
                buffer: buf,
                x: left_x,
                y: (bar_h - 11.0 * s * 1.4) / 2.0,
                color: glyphon::Color::rgb(
                    (col[0] * 255.0) as u8,
                    (col[1] * 255.0) as u8,
                    (col[2] * 255.0) as u8,
                ),
            });
            left_x += line_w + 4.0 * s;
        }

        // Add padding before Layout
        left_x += 8.0 * s;

        // 3. Layout Mode
        if !self.layout.is_empty() {
            let label = format!("[{}]", self.layout);
            let buf = make_text_buffer(&mut self.font_system, &label, 11.0 * s);
            let line_w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
            self.text_items.push(TextItem {
                buffer: buf,
                x: left_x,
                y: (bar_h - 11.0 * s * 1.4) / 2.0,
                color: glyphon::Color::rgb(
                    (color::TEXT_ACCENT[0] * 255.0) as u8,
                    (color::TEXT_ACCENT[1] * 255.0) as u8,
                    (color::TEXT_ACCENT[2] * 255.0) as u8,
                ),
            });
            left_x += line_w + 16.0 * s;
        }

        // 4. Focused Title
        if !self.title.is_empty() && self.title != "(none)" {
            let mut display_title = self.title.clone();
            if display_title.chars().count() > 40 {
                display_title = display_title.chars().take(37).collect::<String>() + "...";
            }
            let buf = make_text_buffer(&mut self.font_system, &display_title, 11.0 * s);
            self.text_items.push(TextItem {
                buffer: buf,
                x: left_x,
                y: (bar_h - 11.0 * s * 1.4) / 2.0,
                color: glyphon::Color::rgb(
                    (color::TEXT_FG[0] * 255.0) as u8,
                    (color::TEXT_FG[1] * 255.0) as u8,
                    (color::TEXT_FG[2] * 255.0) as u8,
                ),
            });
        }

        // 5. Right Side Stats (CPU, Mem, Bat, Clock)
        let mut right_x = sw - 12.0 * s;
        if let Some(ref stats) = self.stats {
            // Clock
            let buf = make_text_buffer(&mut self.font_system, &stats.clock, 11.0 * s);
            let w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
            right_x -= w;
            self.text_items.push(TextItem {
                buffer: buf,
                x: right_x,
                y: (bar_h - 11.0 * s * 1.4) / 2.0,
                color: glyphon::Color::rgb(
                    (color::TEXT_FG[0] * 255.0) as u8,
                    (color::TEXT_FG[1] * 255.0) as u8,
                    (color::TEXT_FG[2] * 255.0) as u8,
                ),
            });

            // Battery
            if !stats.battery.is_empty() {
                right_x -= 16.0 * s;
                let buf = make_text_buffer(&mut self.font_system, &stats.battery, 11.0 * s);
                let w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
                right_x -= w;
                self.text_items.push(TextItem {
                    buffer: buf,
                    x: right_x,
                    y: (bar_h - 11.0 * s * 1.4) / 2.0,
                    color: glyphon::Color::rgb(
                        (color::TEXT_ACCENT[0] * 255.0) as u8,
                        (color::TEXT_ACCENT[1] * 255.0) as u8,
                        (color::TEXT_ACCENT[2] * 255.0) as u8,
                    ),
                });
            }

            // Memory
            right_x -= 16.0 * s;
            let buf = make_text_buffer(&mut self.font_system, &stats.memory, 11.0 * s);
            let w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
            right_x -= w;
            self.text_items.push(TextItem {
                buffer: buf,
                x: right_x,
                y: (bar_h - 11.0 * s * 1.4) / 2.0,
                color: glyphon::Color::rgb(
                    (color::TEXT_DIM[0] * 255.0) as u8,
                    (color::TEXT_DIM[1] * 255.0) as u8,
                    (color::TEXT_DIM[2] * 255.0) as u8,
                ),
            });

            // CPU
            right_x -= 16.0 * s;
            let buf = make_text_buffer(&mut self.font_system, &stats.cpu, 11.0 * s);
            let w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
            right_x -= w;
            self.text_items.push(TextItem {
                buffer: buf,
                x: right_x,
                y: (bar_h - 11.0 * s * 1.4) / 2.0,
                color: glyphon::Color::rgb(
                    (color::TEXT_DIM[0] * 255.0) as u8,
                    (color::TEXT_DIM[1] * 255.0) as u8,
                    (color::TEXT_DIM[2] * 255.0) as u8,
                ),
            });
        }

        // 5b. System Tray Icons (render to the left of the CPU/stats block)
        self.tray_item_bounds.clear();
        if !self.tray_items.is_empty() {
            println!("[status-tray-render] Rendering {} tray items (screen size: {}x{})", self.tray_items.len(), sw, sh);
            right_x -= 16.0 * s; // Separator padding
            let mut sorted_tray: Vec<&TrayItem> = self.tray_items.values().collect();
            sorted_tray.sort_by_key(|item| &item.id);

            for item in sorted_tray.iter().rev() {
                println!("[status-tray-render] Item ID: '{}', icon_name: {:?}, pixmaps is Some: {}", item.id, item.icon_name, item.pixmaps.is_some());
                let icon_size = 16.0 * s;
                right_x -= icon_size;
                let x = right_x;
                let y = (bar_h - icon_size) / 2.0;

                // Record bounds for hit-testing
                self.tray_item_bounds.push(TrayIconBounds {
                    id: item.id.clone(),
                    x,
                    y,
                    w: icon_size,
                    h: icon_size,
                    title: item.title.clone(),
                });

                // Attempt to draw pixmap
                let mut drawn_pixmap = false;
                if let Some(ref pixmaps) = item.pixmaps {
                    if !pixmaps.is_empty() {
                        // Find pixmap closest to 16 pixels wide
                        if let Some(pixmap) = pixmaps.iter().min_by_key(|p| (p.width - 16).abs()) {
                            if pixmap.width > 0 && pixmap.height > 0 {
                                // Calculate average brightness of visible pixels to see if we need to recolor
                                let mut total_brightness = 0.0;
                                let mut visible_pixel_count = 0;
                                for row in 0..pixmap.height {
                                    for col in 0..pixmap.width {
                                        let idx = ((row * pixmap.width + col) * 4) as usize;
                                        if idx + 3 < pixmap.pixels.len() {
                                            let a = pixmap.pixels[idx] as f32 / 255.0;
                                            if a > 0.1 {
                                                let r = pixmap.pixels[idx + 1] as f32 / 255.0;
                                                let g = pixmap.pixels[idx + 2] as f32 / 255.0;
                                                let b = pixmap.pixels[idx + 3] as f32 / 255.0;
                                                total_brightness += (r + g + b) / 3.0;
                                                visible_pixel_count += 1;
                                            }
                                        }
                                    }
                                }
                                
                                let avg_brightness = if visible_pixel_count > 0 {
                                    total_brightness / visible_pixel_count as f32
                                } else {
                                    0.5
                                };
                                                               // If the average brightness is dark, recolor it to be light
                                let recolor_light = avg_brightness < 0.35;
 
                                // Downsample to 16x16 quads to optimize rendering
                                let draw_w = 16;
                                let draw_h = 16;
                                let pixel_w = icon_size / draw_w as f32;
                                let pixel_h = icon_size / draw_h as f32;
                                let mut pushed_pixels = 0;
                                for row in 0..draw_h {
                                    for col in 0..draw_w {
                                        let src_row = row * pixmap.height / draw_h;
                                        let src_col = col * pixmap.width / draw_w;
                                        let idx = ((src_row * pixmap.width + src_col) * 4) as usize;
                                        if idx + 3 < pixmap.pixels.len() {
                                            let a = pixmap.pixels[idx] as f32 / 255.0;
                                            if a > 0.0 {
                                                let mut r = pixmap.pixels[idx + 1] as f32 / 255.0;
                                                let mut g = pixmap.pixels[idx + 2] as f32 / 255.0;
                                                let mut b = pixmap.pixels[idx + 3] as f32 / 255.0;
                                                
                                                if recolor_light {
                                                    let l = (r + g + b) / 3.0;
                                                    // Map 0.0 (black) to 0.85 (light grey), 1.0 stays 1.0
                                                    let new_l = 0.85 + (1.0 - 0.85) * l;
                                                    r = new_l;
                                                    g = new_l;
                                                    b = new_l;
                                                }
                                                
                                                self.rects.push(RectWidget {
                                                    x: x + col as f32 * pixel_w,
                                                    y: y + row as f32 * pixel_h,
                                                    w: pixel_w,
                                                    h: pixel_h,
                                                    color: [r, g, b, a],
                                                });
                                                pushed_pixels += 1;
                                            }
                                        }
                                    }
                                }
                                println!("[status-tray-render] Item '{}' drawing downsampled pixmap width={}, height={}, icon_size={}, x={}, y={}, pixel_size={}x{}, pushed_rects={}", 
                                    item.id, pixmap.width, pixmap.height, icon_size, x, y, pixel_w, pixel_h, pushed_pixels);
                                drawn_pixmap = true;
                            }
                        }
                    }
                }

                println!("[status-tray-render] Item '{}' drawn_pixmap: {}", item.id, drawn_pixmap);
                // Fallback to text icon symbol
                if !drawn_pixmap {
                    let symbol = if let Some(ref name) = item.icon_name {
                        let name_lower = name.to_lowercase();
                        if name_lower.contains("volume") || name_lower.contains("sound") || name_lower.contains("audio") {
                            if name_lower.contains("mute") { "🔇" } else { "🔊" }
                        } else if name_lower.contains("wifi") || name_lower.contains("network") || name_lower.contains("ethernet") {
                            "📶"
                        } else if name_lower.contains("battery") {
                            "🔋"
                        } else if name_lower.contains("bluetooth") {
                            "ᛒ"
                        } else if name_lower.contains("mail") || name_lower.contains("envelope") {
                            "✉"
                        } else if name_lower.contains("chat") || name_lower.contains("messenger") || name_lower.contains("discord") || name_lower.contains("slack") || name_lower.contains("telegram") {
                            "💬"
                        } else if name_lower.contains("steam") || name_lower.contains("game") {
                            "🎮"
                        } else if name_lower.contains("dropbox") {
                            "📦"
                        } else {
                            "⚙"
                        }
                    } else {
                        "⚙"
                    };

                    let buf = make_text_buffer(&mut self.font_system, symbol, 11.0 * s);
                    let tw = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
                    let tx = x + (icon_size - tw) / 2.0;
                    let ty = y + (icon_size - 11.0 * s * 1.4) / 2.0;
                    self.text_items.push(TextItem {
                        buffer: buf,
                        x: tx,
                        y: ty,
                        color: glyphon::Color::rgb(
                            (color::TEXT_ACCENT[0] * 255.0) as u8,
                            (color::TEXT_ACCENT[1] * 255.0) as u8,
                            (color::TEXT_ACCENT[2] * 255.0) as u8,
                        ),
                    });
                }

                right_x -= 8.0 * s; // Gap between icons
            }
        }

        // 5c. Tooltip Rendering (if hovered)
        if let Some(ref hovered_id) = self.hovered_tray_item {
            if let Some(bound) = self.tray_item_bounds.iter().find(|b| &b.id == hovered_id) {
                let tooltip_text = bound.title.as_deref().unwrap_or(bound.id.as_str());
                let font_size = 10.0 * s;
                let buf = make_text_buffer(&mut self.font_system, tooltip_text, font_size);
                let text_w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
                let padding = 6.0 * s;

                let tooltip_w = text_w + padding * 2.0;
                let tooltip_h = font_size * 1.4 + padding * 2.0;
                let tx = bound.x + (bound.w - tooltip_w) / 2.0;
                let ty = bar_h + 4.0 * s;

                // Tooltip background
                self.rects.push(RectWidget {
                    x: tx,
                    y: ty,
                    w: tooltip_w,
                    h: tooltip_h,
                    color: [0.08, 0.08, 0.12, 0.95],
                });

                // Tooltip text
                self.text_items.push(TextItem {
                    buffer: buf,
                    x: tx + padding,
                    y: ty + padding,
                    color: glyphon::Color::rgb(
                        (color::TEXT_FG[0] * 255.0) as u8,
                        (color::TEXT_FG[1] * 255.0) as u8,
                        (color::TEXT_FG[2] * 255.0) as u8,
                    ),
                });
            }
        }

        self.needs_rebuild = false;
    }

    fn collect_vertices(&self) -> Vec<Vertex> {
        let sw = self.width as f32;
        let sh = self.height as f32;
        let mut verts = Vec::new();
        for r in &self.rects {
            verts.extend(quad_vertices(r.x, r.y, r.w, r.h, sw, sh, r.color));
        }
        verts
    }

    fn upload_vertices(&mut self) {
        let verts = self.collect_vertices();
        self.vertex_count = verts.len() as u32;
        let data = bytemuck::cast_slice(&verts);
        let needed = data.len() as wgpu::BufferAddress;
        if needed > self.vertex_buffer.size() {
            self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Vertex Buffer"),
                size: needed,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(&self.vertex_buffer, 0, data);
    }

    fn prepare_text(&mut self) {
        let w = self.width as f32;
        let h = self.height as f32;
        let viewport = Resolution { width: w as u32, height: h as u32 };
        self.text_viewport.update(&self.queue, viewport);
        let bounds = TextBounds { left: 0, top: 0, right: w as i32, bottom: h as i32 };
        let areas: Vec<TextArea> = self.text_items.iter().map(|ti| TextArea {
            buffer: &ti.buffer,
            left: ti.x, top: ti.y, scale: 1.0, bounds,
            default_color: ti.color,
            custom_glyphs: &[],
        }).collect();
        self.text_renderer.prepare(
            &self.device, &self.queue, &mut self.font_system,
            &mut self.text_atlas, &self.text_viewport, areas, &mut self.swash_cache
        ).unwrap();
    }

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.width = size.width;
            self.height = size.height;
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
            self.needs_rebuild = true;
        }
    }

    fn render(&mut self) {
        if self.needs_rebuild {
            self.rebuild_layout();
            self.upload_vertices();
        }
        self.prepare_text();

        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(wgpu::SurfaceError::Timeout) => return,
            Err(e) => { eprintln!("Surface error: {e:?}"); return; }
        };

        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.06, g: 0.06, b: 0.08, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.render_pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.vertex_count, 0..1);

            self.text_renderer.render(&self.text_atlas, &self.text_viewport, &mut pass).unwrap();
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.window.pre_present_notify();
        output.present();
    }
}

struct AppWrapper {
    state: Option<StatusApp>,
}

impl ApplicationHandler<CustomEvent> for AppWrapper {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() { return; }
        let window = Arc::new(event_loop.create_window(
            WindowAttributes::default()
                .with_name("clear-status-interface", "clear-status-interface")
                .with_title("Clear Status Interface")
                .with_fullscreen(Some(Fullscreen::Borderless(None)))
                .with_decorations(false)
        ).unwrap());
        let state = pollster::block_on(StatusApp::new(window));
        self.state = Some(state);
        self.state.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: winit::window::WindowId, event: WindowEvent) {
        let redraw = match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                true
            }
            WindowEvent::Resized(s) => {
                if let Some(st) = &mut self.state {
                    st.resize(s);
                }
                true
            }
            WindowEvent::RedrawRequested => {
                if let Some(st) = &mut self.state {
                    st.render();
                    st.window.request_redraw();
                }
                true
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(st) = &mut self.state {
                    let cx = position.x / st.scale_factor;
                    let cy = position.y / st.scale_factor;
                    st.cursor_pos = (cx, cy);
                    
                    let mut newly_hovered = None;
                    for bound in &st.tray_item_bounds {
                        if cx >= bound.x as f64 && cx <= (bound.x + bound.w) as f64
                            && cy >= bound.y as f64 && cy <= (bound.y + bound.h) as f64 {
                            newly_hovered = Some(bound.id.clone());
                            break;
                        }
                    }
                    if st.hovered_tray_item != newly_hovered {
                        st.hovered_tray_item = newly_hovered;
                        st.needs_rebuild = true;
                        st.window.request_redraw();
                    }
                }
                true
            }
            _ => false,
        };
        if redraw {
            if let Some(st) = &mut self.state {
                st.window.request_redraw();
            }
        }
    }


    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: CustomEvent) {
        if let Some(ref mut st) = self.state {
            match event {
                CustomEvent::TagsUpdated(t) => {
                    st.tags = t;
                }
                CustomEvent::LayoutUpdated(l) => {
                    st.layout = l;
                }
                CustomEvent::TitleUpdated(t) => {
                    st.title = t;
                }
                CustomEvent::SystemStatsUpdated(s) => {
                    st.stats = Some(s);
                }
                CustomEvent::TrayUpdated(item) => {
                    st.tray_items.insert(item.id.clone(), item);
                }
                CustomEvent::TrayRemoved(id) => {
                    st.tray_items.remove(&id);
                }
            }
            st.needs_rebuild = true;
            st.window.request_redraw();
        }
    }

}

async fn spawn_status_listener(sub: &'static str, proxy: EventLoopProxy<CustomEvent>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    loop {
        if let Ok(mut stream) = UnixStream::connect("/tmp/clearwm-status.sock").await {
            if stream.write_all(format!("{}\n", sub).as_bytes()).await.is_ok() {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    let val = line.trim().to_string();
                    if !val.is_empty() {
                        let ev = match sub {
                            "tags" => CustomEvent::TagsUpdated(val),
                            "layout" => CustomEvent::LayoutUpdated(val),
                            "title" => CustomEvent::TitleUpdated(val),
                            _ => unreachable!(),
                        };
                        let _ = proxy.send_event(ev);
                    }
                    line.clear();
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

async fn spawn_system_stats(proxy: EventLoopProxy<CustomEvent>) {
    let mut last_cpu = read_cpu_ticks().unwrap_or((0, 0));
    loop {
        let clock = chrono::Local::now().format("%A, %B %d, %Y %I:%M %p").to_string();
        let memory = read_memory_usage().unwrap_or_else(|| "Mem N/A".to_string());
        
        let cpu_str = if let Some(current_cpu) = read_cpu_ticks() {
            let total_diff = current_cpu.0 - last_cpu.0;
            let idle_diff = current_cpu.1 - last_cpu.1;
            last_cpu = current_cpu;
            if total_diff > 0 {
                let usage = 100.0 - (idle_diff as f32 * 100.0 / total_diff as f32);
                format!("Cpu {:.1}%", usage)
            } else {
                "Cpu 0.0%".to_string()
            }
        } else {
            "Cpu N/A".to_string()
        };

        let battery = read_battery().unwrap_or_default();

        let stats = SystemStats {
            clock,
            memory,
            cpu: cpu_str,
            battery,
        };
        let _ = proxy.send_event(CustomEvent::SystemStatsUpdated(stats));
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[derive(Debug, Clone)]
pub struct NotifierAddress {
    pub destination: String,
    pub path: String,
}

impl NotifierAddress {
    pub fn from_notifier_service(service: &str, sender: &str) -> Result<Self, String> {
        if service.starts_with('/') {
            Ok(NotifierAddress {
                destination: sender.to_string(),
                path: service.to_string(),
            })
        } else if let Some((destination, path)) = service.split_once('/') {
            Ok(NotifierAddress {
                destination: destination.to_string(),
                path: format!("/{}", path),
            })
        } else if service.contains(':') {
            let split = service.split(':').collect::<Vec<&str>>();
            Ok(NotifierAddress {
                destination: format!(":{}", split[1]),
                path: "/StatusNotifierItem".to_string(),
            })
        } else {
            Ok(NotifierAddress {
                destination: service.to_string(),
                path: "/StatusNotifierItem".to_string(),
            })
        }
    }
}

#[zbus::proxy(
    interface = "org.kde.StatusNotifierItem",
    default_path = "/StatusNotifierItem"
)]
trait StatusNotifierItem {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn category(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn title(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_pixmap(&self) -> zbus::Result<Vec<(i32, i32, Vec<u8>)>>;

    #[zbus(signal)]
    fn new_icon(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn new_title(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_status(&self) -> zbus::Result<()>;
}

fn find_icon_file(dir: &std::path::Path, icon_name: &str) -> Option<std::path::PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            if let Ok(file_type) = entry.file_type() {
                let path = entry.path();
                if file_type.is_dir() {
                    if !file_type.is_symlink() {
                        if let Some(found) = find_icon_file(&path, icon_name) {
                            return Some(found);
                        }
                    }
                } else if file_type.is_file() {
                    if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
                        if file_name == format!("{}.png", icon_name) {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

fn load_png_as_pixmap(path: &std::path::Path) -> Option<TrayPixmap> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    
    let width = info.width as i32;
    let height = info.height as i32;
    println!("[status-tray] PNG info: width={}, height={}, color_type={:?}, buffer_size={}", width, height, info.color_type, info.buffer_size());
    let mut argb_pixels = Vec::with_capacity((width * height * 4) as usize);
    
    let actual_bytes = &buf[..info.buffer_size()];
    let mut printed = 0;
    match info.color_type {
        png::ColorType::Rgba => {
            for (i, chunk) in actual_bytes.chunks_exact(4).enumerate() {
                if chunk[3] > 10 && printed < 5 {
                    println!("[status-tray] Pixel {}: original RGBA=[{}, {}, {}, {}]", i, chunk[0], chunk[1], chunk[2], chunk[3]);
                    printed += 1;
                }
                argb_pixels.push(chunk[3]); // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[1]); // G
                argb_pixels.push(chunk[2]); // B
            }
        }
        png::ColorType::Rgb => {
            for chunk in actual_bytes.chunks_exact(3) {
                argb_pixels.push(255);      // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[1]); // G
                argb_pixels.push(chunk[2]); // B
            }
        }
        png::ColorType::Grayscale => {
            for &g in actual_bytes {
                argb_pixels.push(255); // A
                argb_pixels.push(g);   // R
                argb_pixels.push(g);   // G
                argb_pixels.push(g);   // B
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in actual_bytes.chunks_exact(2) {
                argb_pixels.push(chunk[1]); // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[0]); // G
                argb_pixels.push(chunk[0]); // B
            }
        }
        _ => return None,
    }
    
    Some(TrayPixmap {
        width,
        height,
        pixels: argb_pixels,
    })
}

fn resolve_icon_path(theme_path: Option<&str>, icon_name: &str) -> Option<std::path::PathBuf> {
    if icon_name.is_empty() {
        return None;
    }

    let icon_name = if icon_name == "dropbox" { "dropboxstatus-idle" } else { icon_name };

    if let Some(path_str) = theme_path {
        if !path_str.is_empty() {
            let path = std::path::Path::new(path_str);
            if path.exists() {
                if let Some(found) = find_icon_file(path, icon_name) {
                    return Some(found);
                }
            }
        }
    }

    let mut search_dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        search_dirs.push(format!("{}/.local/share/icons", home));
        search_dirs.push(format!("{}/.icons", home));
    }
    search_dirs.push("/usr/share/icons".to_string());
    search_dirs.push("/usr/share/pixmaps".to_string());

    let sub_paths = [
        "hicolor/16x16/status",
        "hicolor/22x22/status",
        "hicolor/24x24/status",
        "hicolor/32x32/status",
        "hicolor/48x48/status",
        "hicolor/16x16/apps",
        "hicolor/22x22/apps",
        "hicolor/24x24/apps",
        "hicolor/32x32/apps",
        "hicolor/48x48/apps",
        "gnome/16x16/status",
        "gnome/22x22/status",
        "gnome/24x24/status",
        "gnome/32x32/status",
        "gnome/48x48/status",
        "gnome/16x16/apps",
        "gnome/22x22/apps",
        "gnome/24x24/apps",
        "gnome/32x32/apps",
        "gnome/48x48/apps",
    ];

    for base in &search_dirs {
        for sub in &sub_paths {
            let path = std::path::Path::new(base).join(sub).join(format!("{}.png", icon_name));
            if path.exists() && path.is_file() {
                return Some(path);
            }
        }
        let base_path = std::path::Path::new(base);
        if base_path.exists() {
            if let Some(found) = find_icon_file(base_path, icon_name) {
                return Some(found);
            }
        }
    }

    None
}

async fn fetch_tray_item(conn: &zbus::Connection, addr: &NotifierAddress) -> Result<TrayItem, zbus::Error> {
    let proxy = StatusNotifierItemProxy::builder(conn)
        .destination(addr.destination.clone())?
        .path(addr.path.clone())?
        .build()
        .await?;

    let id = format!("{}/{}", addr.destination, addr.path.trim_start_matches('/'));
    let icon_name = proxy.icon_name().await.ok();
    let icon_theme_path = proxy.icon_theme_path().await.ok();
    let title = proxy.title().await.ok();

    let mut pixmaps = proxy.icon_pixmap().await.ok().and_then(|v| {
        if v.is_empty() || (v.len() == 1 && v[0].0 == 0 && v[0].1 == 0) {
            None
        } else {
            Some(v.into_iter()
                .map(|(w, h, pixels)| TrayPixmap {
                    width: w,
                    height: h,
                    pixels,
                })
                .collect::<Vec<_>>())
        }
    });

    if pixmaps.is_none() {
        if let Some(ref name) = icon_name {
            println!("[status-tray] Trying to resolve theme icon for '{}' (theme path: {:?})", name, icon_theme_path);
            if let Some(icon_path) = resolve_icon_path(icon_theme_path.as_deref(), name) {
                println!("[status-tray] Found icon file at {:?}", icon_path);
                if let Some(pixmap) = load_png_as_pixmap(&icon_path) {
                    println!("[status-tray] Successfully decoded icon file to pixmap (size: {}x{})", pixmap.width, pixmap.height);
                    pixmaps = Some(vec![pixmap]);
                } else {
                    println!("[status-tray] Failed to decode icon file");
                }
            } else {
                println!("[status-tray] Could not find icon file on system or theme path");
            }
        }
    } else {
        println!("[status-tray] Loaded raw D-Bus pixmap for '{}'", id);
    }

    Ok(TrayItem {
        id,
        icon_name,
        icon_theme_path,
        pixmaps,
        title,
    })
}

struct Watcher {
    registered_items: Arc<tokio::sync::Mutex<HashMap<String, NotifierAddress>>>,
    proxy_events: EventLoopProxy<CustomEvent>,
    tokio_handle: tokio::runtime::Handle,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .unwrap_or_else(|| service.to_string());
        
        if let Ok(addr) = NotifierAddress::from_notifier_service(service, &sender) {
            let mut items = self.registered_items.lock().await;
            let full_address = format!("{}/{}", addr.destination, addr.path.trim_start_matches('/'));
            if !items.contains_key(&full_address) {
                items.insert(full_address.clone(), addr.clone());
                
                let conn = conn.clone();
                let addr_clone = addr.clone();
                let proxy_events_clone = self.proxy_events.clone();
                
                self.tokio_handle.spawn(async move {
                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                        let _ = proxy_events_clone.send_event(CustomEvent::TrayUpdated(item));
                    }
                    
                    // Listen for updates
                    if let Ok(proxy) = StatusNotifierItemProxy::builder(&conn)
                        .destination(addr_clone.destination.clone())
                        .unwrap()
                        .path(addr_clone.path.clone())
                        .unwrap()
                        .build()
                        .await
                    {
                        let mut new_icon_stream = proxy.receive_new_icon().await.ok();
                        let mut new_title_stream = proxy.receive_new_title().await.ok();
                        let mut new_status_stream = proxy.receive_new_status().await.ok();
                        
                        use tokio_stream::StreamExt;
                        loop {
                            tokio::select! {
                                Some(_) = async {
                                    if let Some(ref mut s) = new_icon_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = proxy_events_clone.send_event(CustomEvent::TrayUpdated(item));
                                    }
                                }
                                Some(_) = async {
                                    if let Some(ref mut s) = new_title_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = proxy_events_clone.send_event(CustomEvent::TrayUpdated(item));
                                    }
                                }
                                Some(_) = async {
                                    if let Some(ref mut s) = new_status_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = proxy_events_clone.send_event(CustomEvent::TrayUpdated(item));
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }
    }

    async fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    async fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        let items = self.registered_items.lock().await;
        items.keys().cloned().collect()
    }
}

async fn spawn_status_tray(proxy: EventLoopProxy<CustomEvent>) {
    let registered_items = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let tokio_handle = tokio::runtime::Handle::current();

    let watcher = Watcher {
        registered_items: registered_items.clone(),
        proxy_events: proxy.clone(),
        tokio_handle,
    };

    let conn = match zbus::ConnectionBuilder::session() {
        Ok(builder) => {
            match builder
                .name("org.kde.StatusNotifierWatcher")
                .unwrap()
                .serve_at("/StatusNotifierWatcher", watcher)
                .unwrap()
                .build()
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to build D-Bus connection: {:?}", e);
                    return;
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to initialize D-Bus session: {:?}", e);
            return;
        }
    };

    println!("StatusNotifierWatcher running successfully on D-Bus!");

    // Start NameOwnerChanged listener to detect when tray apps disconnect
    let dbus_proxy = match zbus::fdo::DBusProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to create DBusProxy: {:?}", e);
            return;
        }
    };
    let mut owner_changes = match dbus_proxy.receive_name_owner_changed().await {
        Ok(oc) => oc,
        Err(e) => {
            eprintln!("Failed to receive name owner changed: {:?}", e);
            return;
        }
    };

    let registered_items_clone = registered_items.clone();
    let proxy_events_clone = proxy.clone();
    
    tokio::spawn(async move {
        use tokio_stream::StreamExt;
        while let Some(signal) = owner_changes.next().await {
            if let Ok(args) = signal.args() {
                let old = args.old_owner;
                let new = args.new_owner;
                let old_opt: &Option<_> = &*old;
                if let Some(ref old_owner) = old_opt {
                    if new.is_none() {
                        let mut items = registered_items_clone.lock().await;
                        let mut to_remove = Vec::new();
                        for (key, addr) in items.iter() {
                            if addr.destination == old_owner.as_str() {
                                to_remove.push(key.clone());
                            }
                        }
                        for key in to_remove {
                            items.remove(&key);
                            let _ = proxy_events_clone.send_event(CustomEvent::TrayRemoved(key));
                        }
                    }
                }
            }
        }
    });

    // Keep the task alive
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

fn main() {
    let event_loop = EventLoop::<CustomEvent>::with_user_event().build().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy_tags = event_loop.create_proxy();
    let proxy_layout = event_loop.create_proxy();
    let proxy_title = event_loop.create_proxy();
    let proxy_stats = event_loop.create_proxy();
    let proxy_tray = event_loop.create_proxy();

    // Spawn Tokio Runtime for async listeners
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            tokio::spawn(spawn_status_listener("tags", proxy_tags));
            tokio::spawn(spawn_status_listener("layout", proxy_layout));
            tokio::spawn(spawn_status_listener("title", proxy_title));
            tokio::spawn(spawn_system_stats(proxy_stats));
            tokio::spawn(spawn_status_tray(proxy_tray));
            
            // Keep the runtime thread alive
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
    });

    let mut wrapper = AppWrapper { state: None };
    event_loop.run_app(&mut wrapper).unwrap();
}





















