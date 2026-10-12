// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The window: Dear ImGui over winit and OpenGL (glutin + glow), so it runs on Windows, macOS
//! and Linux alike. A mod built here can be opened again from its project file and rebuilt.

mod app;
mod dialogs;
mod jobs;
mod theme;
mod view;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dear_imgui_glow::GlowRenderer;
use dear_imgui_rs::{Context, FontSource, StbTrueTypeFontData};
use dear_imgui_winit::{HiDpiMode, WinitPlatform};
use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::HasWindowHandle;
use reskate_music::assets;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use app::App;

const FONT_SIZE: f32 = 17.0;

type Failure = Box<dyn std::error::Error>;

struct Gui {
    window: Arc<Window>,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    imgui: Context,
    platform: WinitPlatform,
    renderer: GlowRenderer,
    app: App,
    last_input: Instant,
    frames: u32,
    /// RESKATE_SCREENSHOT: save the framebuffer there after a few seconds and quit (a
    /// developer aid for checking the layout without a screen recorder).
    screenshot: Option<PathBuf>,
}

impl Gui {
    fn new(event_loop: &ActiveEventLoop, initial: Vec<PathBuf>) -> Result<Gui, Failure> {
        let attributes = Window::default_attributes()
            .with_title("ReSkate Music Packer")
            .with_inner_size(LogicalSize::new(1100.0, 700.0))
            .with_min_inner_size(LogicalSize::new(900.0, 560.0));
        let (window, config) = glutin_winit::DisplayBuilder::new().with_window_attributes(Some(attributes)).build(
            event_loop,
            ConfigTemplateBuilder::new(),
            |mut configs| configs.next().expect("an OpenGL config"),
        )?;
        let window = Arc::new(window.ok_or("no window")?);
        let handle = window.window_handle()?.as_raw();
        let display = config.display();
        // SAFETY: the handle belongs to the window, which outlives the context and surface.
        let context = unsafe { display.create_context(&config, &ContextAttributesBuilder::new().build(Some(handle)))? };
        let size = window.inner_size();
        let surface_attributes = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            handle,
            NonZeroU32::new(size.width.max(1)).expect("non-zero"),
            NonZeroU32::new(size.height.max(1)).expect("non-zero"),
        );
        // SAFETY: as above.
        let surface = unsafe { display.create_window_surface(&config, &surface_attributes)? };
        let context = context.make_current(&surface)?;
        let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::new(1).expect("non-zero")));

        let mut imgui = Context::create();
        let _ = imgui.set_ini_filename(None::<String>);
        add_fonts(&imgui);
        theme::apply(imgui.style_mut());
        let mut platform = WinitPlatform::new(&mut imgui)?;
        platform.attach_window(Arc::clone(&window), HiDpiMode::Default, &mut imgui)?;
        // SAFETY: the context is current on this thread for the renderer's whole life.
        let gl = unsafe { glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name).cast()) };
        let renderer = GlowRenderer::new(gl, &mut imgui)?;
        let mut app = App::new();
        app.dropped = initial;
        let screenshot = std::env::var_os("RESKATE_SCREENSHOT").map(PathBuf::from);
        Ok(Gui {
            window,
            surface,
            context,
            imgui,
            platform,
            renderer,
            app,
            last_input: Instant::now(),
            frames: 0,
            screenshot,
        })
    }

    /// Draws one frame; true when the program should quit.
    fn frame(&mut self) -> Result<bool, Failure> {
        self.app.update(&mut self.imgui);
        self.platform.prepare_frame(&mut self.imgui, &self.window)?;
        let ui = self.imgui.frame();
        view::draw(&mut self.app, ui);
        self.platform.prepare_render(ui, &self.window)?;
        if let Some(gl) = self.renderer.gl_context() {
            // SAFETY: the context is current.
            unsafe {
                gl.clear_color(0.11, 0.12, 0.14, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        let pending = self.imgui.render(self.renderer.renderer_consumer()?);
        self.renderer.render(pending)?;
        self.frames += 1;
        let quit = self.frames == 150 && self.screenshot.is_some() && !self.app.busy();
        if quit {
            self.save_screenshot()?;
        }
        self.surface.swap_buffers(&self.context)?;
        Ok(quit)
    }

    fn save_screenshot(&self) -> Result<(), Failure> {
        let (Some(path), Some(gl)) = (&self.screenshot, self.renderer.gl_context()) else { return Ok(()) };
        let size = self.window.inner_size();
        let (width, height) = (size.width as usize, size.height as usize);
        let mut pixels = vec![0u8; width * height * 4];
        // SAFETY: the context is current and the buffer holds the whole framebuffer.
        unsafe {
            gl.read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
        }
        let rows: Vec<u8> = pixels.chunks_exact(width * 4).rev().flatten().copied().collect();
        let png = reskate_music::music::artwork::encode_png(width as u32, height as u32, png::ColorType::Rgba, &rows)?;
        std::fs::write(path, png)?;
        Ok(())
    }

    fn shutdown(&mut self) {
        self.app.shutdown();
        let _ = self.renderer.shutdown(&mut self.imgui);
        let _ = self.platform.shutdown(&mut self.imgui);
    }
}

/// Inter, with system fonts merged in for the scripts it lacks (song titles in Japanese, ...).
fn add_fonts(imgui: &Context) {
    let mut sources = Vec::new();
    if let Ok(data) = StbTrueTypeFontData::from_slice(assets::INTER_REGULAR) {
        sources.push(FontSource::stb_truetype_with_size(data, FONT_SIZE));
    }
    for (path, _) in assets::fallback_fonts() {
        if let Ok(data) = StbTrueTypeFontData::from_file(&path) {
            sources.push(FontSource::stb_truetype_with_size(data, FONT_SIZE));
        }
    }
    if sources.is_empty() {
        sources.push(FontSource::default_font());
    }
    imgui.font_atlas().add_font(&sources);
}

#[derive(Default)]
struct Runtime {
    gui: Option<Gui>,
    failure: Option<String>,
    initial: Vec<PathBuf>,
}

impl ApplicationHandler for Runtime {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gui.is_some() {
            return;
        }
        match Gui::new(event_loop, std::mem::take(&mut self.initial)) {
            Ok(gui) => self.gui = Some(gui),
            Err(error) => {
                self.failure = Some(format!("The window could not be created: {error}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gui) = self.gui.as_mut() else { return };
        let _ = gui.platform.handle_window_event(&mut gui.imgui, &gui.window, &event);
        match event {
            WindowEvent::RedrawRequested => {
                match gui.frame() {
                    Ok(false) => {}
                    Ok(true) => event_loop.exit(),
                    Err(error) => {
                        self.failure = Some(format!("Drawing failed: {error}"));
                        event_loop.exit();
                    }
                }
                return;
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    gui.surface.resize(&gui.context, width, height);
                }
            }
            WindowEvent::DroppedFile(path) => gui.app.dropped.push(path),
            _ => {}
        }
        gui.last_input = Instant::now();
        gui.window.request_redraw();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gui) = self.gui.as_ref() else { return };
        // Full rate while something is happening, a slow tick otherwise (status, game watch).
        let active = gui.last_input.elapsed() < Duration::from_secs(1) || gui.app.busy();
        let wait = if active { Duration::from_millis(16) } else { Duration::from_millis(500) };
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
        gui.window.request_redraw();
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gui) = self.gui.as_mut() {
            gui.shutdown();
        }
    }
}

/// Opens the window; `initial` songs or folders are added to the list once the game and ffmpeg
/// are set up.
pub fn run(initial: Vec<PathBuf>) -> ExitCode {
    let result = EventLoop::new().map_err(|error| error.to_string()).and_then(|event_loop| {
        let mut runtime = Runtime { initial, ..Default::default() };
        event_loop.run_app(&mut runtime).map_err(|error| error.to_string())?;
        runtime.failure.map_or(Ok(()), Err)
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            rfd::MessageDialog::new()
                .set_title("ReSkate Music Packer")
                .set_description(&message)
                .set_level(rfd::MessageLevel::Error)
                .show();
            ExitCode::from(1)
        }
    }
}
