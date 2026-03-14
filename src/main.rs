mod config;
mod font;
mod icons;
mod render;
mod toplevel;

use std::fs;
use std::io::Write as IoWrite;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use calloop::channel::{Channel, Sender};
use calloop::{EventLoop, LoopHandle, LoopSignal};
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;

use crate::config::Config;
use crate::font::FontRenderer;
use crate::icons::IconCache;
use crate::render::{calc_overlay_size, render_overlay};
use crate::toplevel::ToplevelState;

/// Messages sent to the daemon event loop.
#[derive(Debug)]
enum DaemonMsg {
    ShowOverlay,
}

/// Main application state.
pub struct AppState {
    // Wayland state
    pub registry_state: RegistryState,
    pub seat_state: SeatState,
    pub output_state: OutputState,
    pub compositor_state: CompositorState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
    pub pool: SlotPool,
    pub qh: QueueHandle<Self>,

    // Current seat and keyboard
    pub seat: Option<wl_seat::WlSeat>,
    pub keyboard: Option<wl_keyboard::WlKeyboard>,

    // Toplevel tracking
    pub toplevel_state: ToplevelState,

    // Overlay state
    pub layer_surface: Option<LayerSurface>,
    pub overlay_visible: bool,
    pub selected_index: usize,
    pub needs_redraw: bool,
    pub configured: bool,
    pub width: u32,
    pub height: u32,
    pub current_buffer: Option<Buffer>,

    // Track if Alt was seen as pressed when we entered
    pub alt_held: bool,
    // The serial for keyboard enter, used for activation
    pub keyboard_serial: u32,

    // Configuration
    pub config: Config,

    // Icon cache
    pub icon_cache: IconCache,

    // Font renderer
    pub font_renderer: FontRenderer,
}

impl AppState {
    fn show_overlay(&mut self) {
        if self.overlay_visible {
            return;
        }

        let windows = self.toplevel_state.window_list();
        if windows.len() < 2 {
            log::info!("Not enough windows to switch ({})", windows.len());
            return;
        }

        // Start with the second window selected (the one to switch to)
        self.selected_index = 1;

        let (w, h) = calc_overlay_size(windows.len(), &self.config);
        self.width = w;
        self.height = h;

        // Determine which output the focused window is on
        let target_output = self.toplevel_state.focused_output().cloned();

        let qh = &self.qh.clone();
        let surface = self.compositor_state.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("alttab"),
            target_output.as_ref(),
        );

        layer.set_size(w, h);
        layer.set_anchor(Anchor::empty()); // centered
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();

        self.layer_surface = Some(layer);
        self.overlay_visible = true;
        self.configured = false;
        self.alt_held = true; // Assume Alt is held since compositor launched us on Alt+Tab
        self.needs_redraw = true;

        log::info!("Overlay shown with {} windows", windows.len());
    }

    fn close_overlay(&mut self, activate: bool) {
        if !self.overlay_visible {
            return;
        }

        if activate {
            let windows = self.toplevel_state.window_list();
            if let Some((handle, info)) = windows.get(self.selected_index) {
                log::info!("Activating window: {}", info.title);
                if info.is_minimized {
                    handle.unset_minimized();
                }
                if let Some(seat) = &self.seat {
                    handle.activate(seat);
                }
            }
        }

        // Destroy overlay surface
        if let Some(layer) = self.layer_surface.take() {
            drop(layer);
        }
        self.current_buffer = None;
        self.overlay_visible = false;
        self.configured = false;

        // Do NOT exit — daemon continues tracking MRU
        log::info!("Overlay closed (activate={})", activate);
    }

    fn draw(&mut self) {
        if !self.configured || !self.overlay_visible {
            return;
        }

        let layer = match &self.layer_surface {
            Some(l) => l,
            None => return,
        };

        let stride = self.width as i32 * 4;
        let pool_size = (stride * self.height as i32) as usize;

        // Ensure pool is large enough
        if self.pool.len() < pool_size {
            self.pool.resize(pool_size).expect("Failed to resize pool");
        }

        let (buffer, canvas) = self
            .pool
            .create_buffer(
                self.width as i32,
                self.height as i32,
                stride,
                wl_shm::Format::Argb8888,
            )
            .expect("Failed to create buffer");

        // Get window list for rendering
        let windows = self.toplevel_state.window_list();
        let infos: Vec<&toplevel::ToplevelInfo> = windows.iter().map(|(_, info)| *info).collect();

        // Pre-populate the icon cache for all app_ids (triggers loading if not cached)
        let app_ids: Vec<String> = infos.iter().map(|info| info.app_id.clone()).collect();
        for app_id in &app_ids {
            let _ = self.icon_cache.get(app_id);
        }
        // Now build the icon references in a separate pass (cache is fully populated)
        let icons: Vec<Option<&icons::IconData>> = app_ids
            .iter()
            .map(|app_id| self.icon_cache.peek(app_id))
            .collect();

        render_overlay(
            canvas,
            self.width,
            self.height,
            &infos,
            &icons,
            self.selected_index,
            &self.config,
            &mut self.font_renderer,
        );

        // Attach and commit
        layer
            .wl_surface()
            .damage_buffer(0, 0, self.width as i32, self.height as i32);
        buffer
            .attach_to(layer.wl_surface())
            .expect("Failed to attach buffer");
        layer.commit();

        self.current_buffer = Some(buffer);
        self.needs_redraw = false;
    }

    fn cycle_selection(&mut self, forward: bool) {
        let count = self.toplevel_state.window_list().len();
        if count == 0 {
            return;
        }
        if forward {
            self.selected_index = (self.selected_index + 1) % count;
        } else {
            self.selected_index = (self.selected_index + count - 1) % count;
        }
        self.needs_redraw = true;
    }
}

// --- Handler implementations ---

impl CompositorHandler for AppState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if self.needs_redraw {
            self.draw();
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for AppState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for AppState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for AppState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        if self.seat.is_none() {
            self.seat = Some(seat);
        }
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let keyboard = self
                .seat_state
                .get_keyboard(qh, &seat, None)
                .expect("Failed to create keyboard");
            self.keyboard = Some(keyboard);
        }
        if self.seat.is_none() {
            self.seat = Some(seat);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            self.keyboard = None;
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl KeyboardHandler for AppState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        serial: u32,
        _raw: &[u32],
        keysyms: &[Keysym],
    ) {
        self.keyboard_serial = serial;
        // Check if Alt is currently pressed
        self.alt_held = keysyms
            .iter()
            .any(|k| *k == Keysym::Alt_L || *k == Keysym::Alt_R);
        log::debug!("Keyboard enter, alt_held={}", self.alt_held);
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        serial: u32,
        event: KeyEvent,
    ) {
        self.keyboard_serial = serial;
        log::debug!("Key press: {:?}", event.keysym);

        match event.keysym {
            Keysym::Tab | Keysym::Down | Keysym::Right => {
                self.cycle_selection(true);
                if self.needs_redraw {
                    self.draw();
                }
            }
            Keysym::ISO_Left_Tab | Keysym::Up | Keysym::Left => {
                self.cycle_selection(false);
                if self.needs_redraw {
                    self.draw();
                }
            }
            Keysym::Escape => {
                self.close_overlay(false);
            }
            Keysym::Return | Keysym::KP_Enter => {
                self.close_overlay(true);
            }
            _ => {}
        }
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        serial: u32,
        event: KeyEvent,
    ) {
        self.keyboard_serial = serial;
        log::debug!("Key release: {:?}", event.keysym);

        // When Alt is released, confirm the selection
        if event.keysym == Keysym::Alt_L || event.keysym == Keysym::Alt_R {
            self.alt_held = false;
            self.close_overlay(true);
        }
    }

    fn repeat_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        match event.keysym {
            Keysym::Tab | Keysym::Down | Keysym::Right => {
                self.cycle_selection(true);
                if self.needs_redraw {
                    self.draw();
                }
            }
            Keysym::ISO_Left_Tab | Keysym::Up | Keysym::Left => {
                self.cycle_selection(false);
                if self.needs_redraw {
                    self.draw();
                }
            }
            _ => {}
        }
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        serial: u32,
        modifiers: Modifiers,
        _raw_modifiers: RawModifiers,
        _layout: u32,
    ) {
        self.keyboard_serial = serial;
        // Track alt state via modifiers too
        if !modifiers.alt && self.alt_held && self.overlay_visible {
            self.alt_held = false;
            self.close_overlay(true);
        }
        if modifiers.alt {
            self.alt_held = true;
        }
    }
}

impl LayerShellHandler for AppState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        // Compositor closed our layer surface — just clean up, don't exit
        self.overlay_visible = false;
        self.layer_surface = None;
        self.current_buffer = None;
        self.configured = false;
        log::info!("Layer surface closed by compositor");
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        log::debug!("Layer surface configured: {:?}", configure.new_size);

        if configure.new_size.0 > 0 {
            self.width = configure.new_size.0;
        }
        if configure.new_size.1 > 0 {
            self.height = configure.new_size.1;
        }

        self.configured = true;
        self.draw();
    }
}

// --- Delegate macros ---
delegate_compositor!(AppState);
delegate_output!(AppState);
delegate_shm!(AppState);
delegate_seat!(AppState);
delegate_keyboard!(AppState);
delegate_layer!(AppState);
delegate_registry!(AppState);

impl ProvidesRegistryState for AppState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

/// Get the path for the Unix socket used to trigger the overlay.
fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("alttab.sock")
}

/// Send a "show" command to the running daemon via Unix socket.
fn send_show() {
    let path = socket_path();
    match UnixStream::connect(&path) {
        Ok(mut stream) => {
            let _ = stream.write_all(b"show");
            log::info!("Sent show command to daemon");
        }
        Err(e) => {
            eprintln!("Failed to connect to daemon socket at {:?}: {}", path, e);
            eprintln!("Is the alttab daemon running? Start it with: alttab");
            std::process::exit(1);
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 && args[1] == "--show" {
        // Client mode: signal the running daemon to show overlay
        send_show();
        return;
    }

    // Daemon mode
    log::info!("alttab daemon: starting");

    // Load configuration
    let config = config::load_config();

    // Guard against multiple instances
    let sock_path = socket_path();
    if sock_path.exists() {
        // Try connecting — if it works, a daemon is already running
        if UnixStream::connect(&sock_path).is_ok() {
            eprintln!(
                "Another alttab daemon is already running (socket {:?} is live)",
                sock_path
            );
            eprintln!("Kill it first, or use 'alttab --show' to trigger it.");
            std::process::exit(1);
        }
        // Stale socket from a crashed daemon — remove it
        log::info!("Removing stale socket {:?}", sock_path);
        let _ = fs::remove_file(&sock_path);
    }

    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland display");
    let (globals, mut event_queue) =
        registry_queue_init(&conn).expect("Failed to initialize registry");
    let qh = event_queue.handle();

    // Bind core globals
    let compositor_state =
        CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
    let pool = SlotPool::new(1024 * 1024, &shm).expect("Failed to create SHM pool");

    let registry_state = RegistryState::new(&globals);

    // Bind the foreign toplevel manager
    let toplevel_manager: ZwlrForeignToplevelManagerV1 = registry_state
        .bind_one(&qh, 1..=3, ())
        .expect("wlr-foreign-toplevel-management not available");

    let mut app_state = AppState {
        registry_state,
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor_state,
        layer_shell,
        shm,
        pool,
        qh: qh.clone(),
        seat: None,
        keyboard: None,
        toplevel_state: ToplevelState::new(),
        layer_surface: None,
        overlay_visible: false,
        selected_index: 0,
        needs_redraw: false,
        configured: false,
        width: 500,
        height: 200,
        current_buffer: None,
        alt_held: false,
        keyboard_serial: 0,
        config,
        icon_cache: IconCache::new(),
        font_renderer: FontRenderer::new(),
    };

    app_state.toplevel_state.manager = Some(toplevel_manager);

    // Do initial roundtrips to get the toplevel list and seat capabilities
    log::info!("Performing initial roundtrips to collect window list...");
    event_queue
        .roundtrip(&mut app_state)
        .expect("Initial roundtrip failed");
    event_queue
        .roundtrip(&mut app_state)
        .expect("Second roundtrip failed");

    let window_count = app_state.toplevel_state.window_list().len();
    log::info!("Found {} windows at startup", window_count);

    // Sort MRU so the currently-focused window is first
    app_state.toplevel_state.sort_initial_mru();

    // Set up calloop event loop
    let mut event_loop: EventLoop<AppState> =
        EventLoop::try_new().expect("Failed to create calloop event loop");
    let loop_handle: LoopHandle<AppState> = event_loop.handle();
    let loop_signal: LoopSignal = event_loop.get_signal();

    // Insert Wayland event source
    WaylandSource::new(conn, event_queue)
        .insert(loop_handle.clone())
        .expect("Failed to insert Wayland source");

    // Register SIGTERM and SIGINT for graceful shutdown via calloop ping
    let sock_path_for_signal = sock_path.clone();
    let loop_signal_for_handler = loop_signal.clone();
    let (shutdown_ping, shutdown_ping_source) =
        calloop::ping::make_ping().expect("Failed to create shutdown ping");
    loop_handle
        .insert_source(shutdown_ping_source, move |_, _, _state: &mut AppState| {
            log::info!("Shutdown signal received, cleaning up");
            let _ = fs::remove_file(&sock_path_for_signal);
            loop_signal_for_handler.stop();
        })
        .expect("Failed to insert shutdown ping source");

    // Set up signal handlers that trigger the ping
    // Block SIGTERM/SIGINT in the main thread so our dedicated thread catches them
    {
        use nix::sys::signal::{SigSet, Signal};
        let mut mask = SigSet::empty();
        mask.add(Signal::SIGTERM);
        mask.add(Signal::SIGINT);
        mask.thread_block()
            .expect("Failed to block signals in main thread");
    }
    let shutdown_ping_clone = shutdown_ping.clone();
    std::thread::spawn(move || {
        use nix::sys::signal::{SigSet, Signal};
        let mut mask = SigSet::empty();
        mask.add(Signal::SIGTERM);
        mask.add(Signal::SIGINT);
        loop {
            match mask.wait() {
                Ok(sig) => {
                    log::info!("Caught signal {:?}", sig);
                    shutdown_ping_clone.ping();
                    return;
                }
                Err(e) => {
                    log::error!("sigwait failed: {}", e);
                }
            }
        }
    });

    // Set up Unix socket listener for --show trigger
    let (sender, channel): (Sender<DaemonMsg>, Channel<DaemonMsg>) = calloop::channel::channel();

    // Insert channel source into calloop
    loop_handle
        .insert_source(channel, |event, _, state: &mut AppState| {
            if let calloop::channel::Event::Msg(DaemonMsg::ShowOverlay) = event {
                log::info!("Received ShowOverlay message");
                state.show_overlay();
            }
        })
        .expect("Failed to insert channel source");

    // Spawn a thread to accept Unix socket connections and relay "show" messages
    let sender_clone = sender.clone();
    let sock_path_clone = sock_path.clone();
    std::thread::spawn(move || {
        let listener = match std::os::unix::net::UnixListener::bind(&sock_path_clone) {
            Ok(l) => l,
            Err(e) => {
                log::error!("Failed to bind Unix socket at {:?}: {}", sock_path_clone, e);
                return;
            }
        };
        log::info!("Listening on {:?}", sock_path_clone);

        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let mut buf = [0u8; 16];
                    let n = std::io::Read::read(&mut stream, &mut buf).unwrap_or(0);
                    let msg = std::str::from_utf8(&buf[..n]).unwrap_or("");
                    if msg.starts_with("show") {
                        if let Err(e) = sender_clone.send(DaemonMsg::ShowOverlay) {
                            log::error!("Failed to send ShowOverlay: {}", e);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Socket accept error: {}", e);
                }
            }
        }
    });

    log::info!("alttab daemon: entering event loop");

    // Main event loop — runs until signal stops it
    event_loop
        .run(None, &mut app_state, |_| {})
        .expect("Event loop failed");

    // Clean up socket on normal exit too
    let _ = fs::remove_file(&sock_path);
    log::info!("alttab daemon: exiting");
}
