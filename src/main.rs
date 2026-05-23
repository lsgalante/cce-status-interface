use std::sync::Arc;
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
        if let Some(ref stats) = self.stats {
            let mut right_x = sw - 12.0 * s;

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

fn main() {
    let event_loop = EventLoop::<CustomEvent>::with_user_event().build().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy_tags = event_loop.create_proxy();
    let proxy_layout = event_loop.create_proxy();
    let proxy_title = event_loop.create_proxy();
    let proxy_stats = event_loop.create_proxy();

    // Spawn Tokio Runtime for async listeners
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            tokio::spawn(spawn_status_listener("tags", proxy_tags));
            tokio::spawn(spawn_status_listener("layout", proxy_layout));
            tokio::spawn(spawn_status_listener("title", proxy_title));
            tokio::spawn(spawn_system_stats(proxy_stats));
            
            // Keep the runtime thread alive
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
    });

    let mut wrapper = AppWrapper { state: None };
    event_loop.run_app(&mut wrapper).unwrap();
}
