//! Headless server mode — runs the gmux event loop without a real terminal.
//!
//! The server:
//! - Does not enter raw mode or read stdin
//! - Creates and listens on both `gmux.sock` (existing JSON API) and
//!   `gmux-client.sock` (new binary protocol)
//! - Initializes AppState and all PTYs from session restore or fresh state
//! - Runs the main event loop (drain events, drain API requests, scheduled tasks)
//! - Renders to a virtual ratatui Buffer in memory
//! - Accepts client connections on the client socket
//! - Streams frames to connected clients after each render
//! - Routes client input events through the existing input pipeline
//! - Continues running after client disconnect
//! - Handles stale socket cleanup, explicit server stop, minimum terminal size,
//!   and pane spawn failure during restore

use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use base64::Engine;
use bytes::Bytes;

use crate::api;
use crate::app;
use crate::config;
use crate::events::AppEvent;
use crate::ipc::{remove_socket_file_if_owned, socket_file_identity, SocketFileIdentity};
use crate::protocol::{
    self, AttachScrollDirection, AttachScrollSource, FrameData, FrameDebugTiming, ServerMessage,
    MAX_FRAME_SIZE, MAX_GRAPHICS_FRAME_SIZE,
};
use crate::server::client_accept::{
    accept_pending_client_connections, reject_pending_client_connections,
};
use crate::server::client_transport::ServerEvent;
use crate::server::clients::{
    events_include_interaction, latest_app_client, render_targets, terminal_attach_client_ids,
    ClientConnection, ClientConnectionMode,
};
use crate::server::keybindings::{app_keybindings, apply_keybindings};
use crate::server::notifications::{should_forward_toast_to_clients, toast_notify_kind};
use crate::server::render_actor::{ClientRenderDebugContext, ClientRenderPublish};
use crate::server::render_snapshot::{AppFrameSnapshot, ServerRenderDebug};
use crate::server::socket_paths::{
    client_socket_path, prepare_socket_path, restrict_socket_permissions,
};
use crate::server::terminal_attach::paste_payload_for_runtime;

#[cfg(test)]
use crate::protocol::RenderEncoding;
#[cfg(test)]
use crate::server::client_transport::{ClientWriter, LatestRenderReceiver, LatestRenderSender};
#[cfg(test)]
use std::fs;

// ---------------------------------------------------------------------------
// Loop event enum for the headless server event loop
// ---------------------------------------------------------------------------

/// Events that the headless server event loop can process.
enum LoopEvent {
    Timer,
    Internal(AppEvent),
    Api(api::ApiRequestMessage),
    ServerEvent(ServerEvent),
    RenderRequested,
}

fn rect_fits_frame(rect: Rect, frame: &FrameData) -> bool {
    rect.x.saturating_add(rect.width) <= frame.width
        && rect.y.saturating_add(rect.height) <= frame.height
}

fn apply_terminal_dirty_patch(
    frame: &mut FrameData,
    area: Rect,
    patch: crate::pane::TerminalDirtyPatch,
) -> bool {
    if !rect_fits_frame(area, frame) {
        return false;
    }
    let width = usize::from(frame.width);
    for (local_y, row_cells) in patch.rows {
        if local_y >= area.height || row_cells.len() != usize::from(area.width) {
            return false;
        }
        let frame_y = area.y + local_y;
        let start = usize::from(frame_y) * width + usize::from(area.x);
        let end = start + usize::from(area.width);
        if end > frame.cells.len() {
            return false;
        }
        frame.cells[start..end].clone_from_slice(&row_cells);
    }
    true
}

fn dirty_patch_intersects_hyperlinks(
    frame: &FrameData,
    area: Rect,
    patch: &crate::pane::TerminalDirtyPatch,
) -> bool {
    if frame.hyperlinks.is_empty() || !rect_fits_frame(area, frame) {
        return false;
    }
    let width = usize::from(frame.width);
    for (local_y, _) in &patch.rows {
        if *local_y >= area.height {
            return true;
        }
        let frame_y = area.y + *local_y;
        let start = usize::from(frame_y) * width + usize::from(area.x);
        let end = start + usize::from(area.width);
        if end > frame.cells.len() {
            return true;
        }
        if frame.cells[start..end]
            .iter()
            .any(|cell| cell.hyperlink.is_some())
        {
            return true;
        }
    }
    false
}

fn debug_duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy, Default)]
struct ServerFrameDebugContext {
    render: ServerRenderDebug,
    target_count: u16,
}

impl ServerFrameDebugContext {
    fn apply_to(self, timing: &mut FrameDebugTiming) {
        timing.server_render_us = self.render.render_us;
        timing.server_frame_build_us = self.render.frame_build_us;
        timing.server_target_count = self.target_count;
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default shared runtime size (columns, rows) when no clients are attached.
const MIN_COLS: u16 = 80;
const MIN_ROWS: u16 = 24;

/// Timeout for in-flight API requests during shutdown.
#[allow(dead_code)]
const SHUTDOWN_API_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the idle headless loop wakes to poll the std UnixListener for new
/// client connections.
///
/// The listener is non-blocking and not integrated into `tokio::select!`, so
/// a low-frequency wake is required to notice new thin-client attaches while
/// otherwise idle. Keep this much slower than the old resize-poll cadence to
/// avoid reintroducing the idle CPU spin.
const CLIENT_ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const FOCUSED_INPUT_LATENCY_WINDOW: Duration = Duration::from_millis(50);
const LATENCY_BACKGROUND_RENDER_INTERVAL: Duration = Duration::from_millis(100);
const LATENCY_OPPORTUNISTIC_PATCH_BUDGET: Duration = Duration::from_millis(2);
/// Keep continuous input bursts from monopolizing the loop and starving render.
const PRIORITY_INPUT_DRAIN_LIMIT: usize = 64;

// ---------------------------------------------------------------------------
// Headless server
// ---------------------------------------------------------------------------

/// The headless server — runs the gmux event loop without a real terminal.
pub struct HeadlessServer {
    app: app::App,
    api_tx: Option<api::ApiRequestSender>,
    api_server: Option<api::ServerHandle>,
    client_listener: UnixListener,
    client_socket_path: PathBuf,
    client_socket_identity: SocketFileIdentity,
    clients: HashMap<u64, ClientConnection>,
    next_client_id: u64,
    /// The client currently driving the shared pane runtime size, theme, and input keybindings.
    foreground_client_id: Option<u64>,
    /// Server-owned keybindings, restored when foreground clients use server mode.
    server_keybindings: crate::config::LiveKeybindConfig,
    /// Full server config warning shown to clients that use server keybindings.
    server_config_diagnostic: Option<String>,
    /// Server config warning with keybinding diagnostics removed for local-keybinding clients.
    server_config_diagnostic_without_keybindings: Option<String>,
    /// Writable direct attach owner per terminal id string.
    terminal_attach_owners: HashMap<String, u64>,
    /// Monotonic activity counter used to pick the most recently active client.
    next_activity_stamp: u64,
    /// Shared pane runtime size derived from the foreground client,
    /// or MIN_COLS × MIN_ROWS when no clients are connected.
    effective_size: (u16, u16),
    /// App client that should receive the next frame before mirrors.
    latency_critical_client_id: Option<u64>,
    /// Monotonic generation for active-sized app render snapshots.
    next_app_snapshot_generation: u64,
    /// Last active-sized app frame. Retained PTY updates patch this canonical
    /// geometry; actor-local fitted mirror baselines must not drive server decisions.
    last_app_frame: Option<Arc<AppFrameSnapshot>>,
    /// Flag set when shutdown is initiated.
    shutting_down: bool,
    /// Flag set while exporting live PTYs to a replacement server.
    handoff_in_progress: bool,
    /// Imported panes get one app-safe resize nudge after the first client attaches.
    pending_handoff_repaint_nudge: bool,
    /// Flag set by Ctrl+C or `server stop` signal.
    should_quit: Arc<AtomicBool>,
    /// Channel for receiving server events from client connection threads.
    server_event_rx: mpsc::Receiver<ServerEvent>,
    /// Sender for server events (cloned for each client thread).
    server_event_tx: mpsc::Sender<ServerEvent>,
    /// High-priority input events from client reader threads.
    server_input_rx: mpsc::UnboundedReceiver<ServerEvent>,
    /// Sender for high-priority input events.
    server_input_tx: mpsc::UnboundedSender<ServerEvent>,
    /// Pane whose recent input owns the short focused-render latency window.
    focused_input_pane_id: Option<crate::layout::PaneId>,
    /// Deadline after which background panes return to normal render cadence.
    focused_input_deadline: Option<Instant>,
    /// Next capped background catch-up while the focused pane is still active.
    latency_background_render_at: Option<Instant>,
    /// True when background pane updates were skipped during focused rendering.
    latency_background_dirty: bool,
}

fn apply_terminal_attach_scroll(
    runtime: &crate::terminal::TerminalRuntime,
    source: AttachScrollSource,
    direction: AttachScrollDirection,
    lines: u16,
    column: Option<u16>,
    row: Option<u16>,
    modifiers: u8,
) -> Result<(), String> {
    let wheel_kind = match direction {
        AttachScrollDirection::Up => MouseEventKind::ScrollUp,
        AttachScrollDirection::Down => MouseEventKind::ScrollDown,
    };
    if let AttachScrollSource::PageKey { input } = source {
        let host_scroll = runtime.input_state().is_some_and(|input_state| {
            !input_state.alternate_screen && !input_state.mouse_reporting_enabled()
        });
        if host_scroll {
            match direction {
                AttachScrollDirection::Up => runtime.scroll_up(lines.max(1) as usize),
                AttachScrollDirection::Down => runtime.scroll_down(lines.max(1) as usize),
            }
            return Ok(());
        }
        return apply_terminal_attach_input(runtime, input);
    }

    match runtime.wheel_routing() {
        Some(crate::pane::WheelRouting::MouseReport) => {
            runtime.scroll_reset();
            let column = column.unwrap_or(0);
            let row = row.unwrap_or(0);
            let Some(bytes) = runtime.encode_mouse_wheel(
                wheel_kind,
                column,
                row,
                KeyModifiers::from_bits_truncate(modifiers),
            ) else {
                return Err(format!(
                    "failed to encode terminal attach mouse wheel event: {wheel_kind:?}"
                ));
            };
            runtime
                .try_send_bytes(Bytes::from(bytes))
                .map_err(|err| format!("terminal attach mouse wheel input failed: {err}"))?;
        }
        Some(crate::pane::WheelRouting::AlternateScroll) => {
            runtime.scroll_reset();
            let Some(bytes) = runtime.encode_alternate_scroll(wheel_kind) else {
                return Ok(());
            };
            runtime
                .try_send_bytes(Bytes::from(bytes))
                .map_err(|err| format!("terminal attach alternate scroll input failed: {err}"))?;
        }
        Some(crate::pane::WheelRouting::HostScroll) | None => match direction {
            AttachScrollDirection::Up => runtime.scroll_up(lines.max(1) as usize),
            AttachScrollDirection::Down => runtime.scroll_down(lines.max(1) as usize),
        },
    }
    Ok(())
}

fn apply_terminal_attach_input(
    runtime: &crate::terminal::TerminalRuntime,
    data: Vec<u8>,
) -> Result<(), String> {
    runtime.scroll_reset();
    runtime
        .try_send_bytes(Bytes::from(data))
        .map_err(|err| format!("terminal attach input failed: {err}"))
}

impl HeadlessServer {
    /// Creates and starts the headless server.
    ///
    /// This:
    /// 1. Prepares the client socket path (cleans up stale sockets)
    /// 2. Binds the client socket listener
    /// 3. Returns the server ready to run
    pub fn new(
        app: app::App,
        config_diagnostics: &[String],
        api_tx: Option<api::ApiRequestSender>,
        api_server: Option<api::ServerHandle>,
    ) -> io::Result<Self> {
        let client_path = client_socket_path();
        prepare_socket_path(&client_path)?;

        let listener = UnixListener::bind(&client_path)?;
        restrict_socket_permissions(&client_path)?;
        let client_socket_identity = socket_file_identity(&client_path)?;
        info!(path = %client_path.display(), "client protocol socket listening");

        // Set non-blocking on the listener so we can poll it from the event loop.
        listener.set_nonblocking(true)?;

        let should_quit = Arc::new(AtomicBool::new(false));

        // Channel for server events from client threads.
        let (server_event_tx, server_event_rx) = mpsc::channel(64);
        let (server_input_tx, server_input_rx) = mpsc::unbounded_channel();
        let server_keybindings = app_keybindings(&app);
        let (server_config_diagnostic, server_config_diagnostic_without_keybindings) =
            server_config_diagnostic_summaries(config_diagnostics);

        Ok(Self {
            app,
            api_tx,
            api_server,
            client_listener: listener,
            client_socket_path: client_path,
            client_socket_identity,
            clients: HashMap::new(),
            next_client_id: 1,
            foreground_client_id: None,
            server_keybindings,
            server_config_diagnostic,
            server_config_diagnostic_without_keybindings,
            terminal_attach_owners: HashMap::new(),
            next_activity_stamp: 1,
            effective_size: (MIN_COLS, MIN_ROWS),
            latency_critical_client_id: None,
            next_app_snapshot_generation: 1,
            last_app_frame: None,
            shutting_down: false,
            handoff_in_progress: false,
            pending_handoff_repaint_nudge: false,
            should_quit,
            server_event_rx,
            server_event_tx,
            server_input_rx,
            server_input_tx,
            focused_input_pane_id: None,
            focused_input_deadline: None,
            latency_background_render_at: None,
            latency_background_dirty: false,
        })
    }

    /// Runs the headless server event loop until shutdown.
    ///
    /// This is the server's main loop — analogous to `App::run()` but without
    /// a real terminal. It:
    /// - Drains internal events (pane death, state changes)
    /// - Drains API requests (from the JSON socket)
    /// - Accepts new client connections
    /// - Reads client messages and routes input
    /// - Handles scheduled tasks (resize poll, animation, session save, etc.)
    /// - Renders virtually and streams frames to clients
    pub async fn run(&mut self) -> io::Result<()> {
        crate::logging::startup("server");

        // Register SIGINT handler for graceful shutdown.
        let should_quit = self.should_quit.clone();
        let quit_notify = self.server_event_tx.clone();
        ctrlc_handler(should_quit, quit_notify);

        // No input_rx needed — server doesn't read stdin.
        // We use None for input_rx so the event loop doesn't try to read from stdin.
        self.app.input_rx = None;

        let mut needs_render = true;
        let mut needs_full_render = true;

        loop {
            crate::render_prof::event("loop.tick");
            crate::render_prof::flush_if_due();

            // If shutdown has been initiated, complete it and exit.
            if self.shutting_down {
                self.complete_shutdown()?;
                break;
            }

            // Check if we should start shutting down.
            if self.app.state.should_quit || self.should_quit.load(Ordering::Acquire) {
                self.initiate_shutdown();
                continue;
            }

            // 1. Check render_dirty flag from PTY reader tasks.
            if self.app.render_dirty.load(Ordering::Acquire) {
                needs_render = true;
                self.record_debug_pty_dirty_for_pending_inputs(Instant::now());
                crate::render_prof::event("render.request.pty_dirty");
            }

            // 2. Drain client input before slower internal/API/normal server work.
            if self.drain_priority_input_events() {
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.priority_input");
            }

            // 3. Drain a bounded internal-event batch. API handlers perform an
            // exhaustive forwarding-aware drain before reading pane/runtime state.
            if self.drain_internal_events_with_forwarding() {
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.internal_events");
            }

            // 4. Drain API requests.
            if self.drain_api_requests_with_shutdown_check() {
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.api_requests");
            }

            self.app.sync_focus_events();
            self.app.sync_session_save_schedule();

            // 5. Accept new client connections.
            self.accept_client_connections()?;

            // 6. Drain server events from client threads.
            if self.drain_server_events() {
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.server_events");
            }

            // 7. Handle scheduled tasks.
            let now = Instant::now();
            if self.handle_scheduled_tasks_headless(now, needs_render) {
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.scheduled_tasks");
            }
            if self.handle_focused_latency_timers(now) {
                needs_render = true;
                crate::render_prof::event("render.request.focused_latency_timer");
            }

            // Handle deferred requests.
            if self.app.state.request_complete_onboarding {
                self.app.state.request_complete_onboarding = false;
                self.app.open_settings_from_onboarding();
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.deferred_onboarding");
            }

            if self.app.state.request_new_tab {
                self.app.state.request_new_tab = false;
                self.app.create_tab();
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.deferred_new_tab");
            }

            if self.app.state.request_reload_config {
                self.app.state.request_reload_config = false;
                self.reload_server_config(true);
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.config_reload");
            }

            if self.app.state.request_start_self_update {
                self.app.state.request_start_self_update = false;
                self.app.start_self_update();
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.self_update");
            }

            if latest_app_client(&self.clients).is_some() && self.app.ensure_default_session() {
                needs_render = true;
                needs_full_render = true;
                crate::render_prof::event("full_render_cause.default_session");
            }

            self.drain_client_config_reload_request();
            self.stream_host_mouse_capture_mode();

            self.app.sync_headless_animation_timer(now);

            // 8. Render virtually and stream frames.
            let input_bypass = self.app.input_render_bypass_pending
                && self.app.render_dirty.load(Ordering::Acquire);
            if needs_render && (self.app.can_render_now(now) || input_bypass) {
                crate::render_prof::event("render.attempt");
                let pty_dirty = self.app.render_dirty.swap(false, Ordering::AcqRel);
                if pty_dirty {
                    self.app.clear_input_render_bypass_after_pty_dirty();
                }
                if pty_dirty {
                    crate::render_prof::event("render.attempt.pty_dirty");
                }
                if needs_full_render {
                    crate::render_prof::event("retained_gate.needs_full_render");
                } else if !pty_dirty {
                    crate::render_prof::event("retained_gate.not_pty_dirty");
                }
                let rendered_focused_latency = pty_dirty
                    && !needs_full_render
                    && self.focused_latency_active(now)
                    && self.render_focused_latency_update_and_stream(now);
                let rendered_retained = !rendered_focused_latency
                    && pty_dirty
                    && !needs_full_render
                    && self.render_retained_pty_update_and_stream();
                if !rendered_retained {
                    if rendered_focused_latency {
                        crate::render_prof::event("focused_latency.rendered");
                    } else {
                        crate::render_prof::event("full_render.invoke");
                        self.render_and_stream();
                    }
                }
                if !rendered_focused_latency {
                    self.clear_latency_background_dirty();
                }
                self.app.last_render_at = Some(now);
                needs_render = false;
                needs_full_render = false;
                continue;
            }

            // 9. Wait for next event.
            let next_deadline = self
                .app
                .next_headless_loop_deadline(now, needs_render)
                .into_iter()
                .chain(self.next_focused_latency_deadline(now))
                .min()
                .map(|deadline| deadline.min(now + CLIENT_ACCEPT_POLL_INTERVAL))
                .or(Some(now + CLIENT_ACCEPT_POLL_INTERVAL));
            let event = {
                tokio::select! {
                    biased;
                    maybe_input_ev = self.server_input_rx.recv() => match maybe_input_ev {
                        Some(ev) => LoopEvent::ServerEvent(ev),
                        None => LoopEvent::Timer,
                    },
                    maybe_api = self.app.api_rx.recv() => match maybe_api {
                        Some(msg) => LoopEvent::Api(msg),
                        None => LoopEvent::Timer,
                    },
                    maybe_ev = self.app.event_rx.recv() => match maybe_ev {
                        Some(ev) => LoopEvent::Internal(ev),
                        None => LoopEvent::Timer,
                    },
                    maybe_server_ev = self.server_event_rx.recv() => match maybe_server_ev {
                        Some(ev) => LoopEvent::ServerEvent(ev),
                        None => LoopEvent::Timer,
                    },
                    _ = sleep_until_or_pending(next_deadline) => LoopEvent::Timer,
                    _ = self.app.render_notify.notified() => LoopEvent::RenderRequested,
                }
            };

            match event {
                LoopEvent::Timer => {}
                LoopEvent::Internal(ev) => {
                    if self.handle_internal_event_with_forwarding(ev) {
                        needs_render = true;
                        needs_full_render = true;
                    }
                }
                LoopEvent::Api(msg) => {
                    if self.handle_api_request_with_shutdown_check(msg) {
                        needs_render = true;
                        needs_full_render = true;
                    }
                }
                LoopEvent::ServerEvent(ev) => {
                    if self.handle_server_event(ev) {
                        needs_render = true;
                        needs_full_render = true;
                    }
                }
                LoopEvent::RenderRequested => {
                    if self.app.render_dirty.load(Ordering::Acquire) {
                        needs_render = true;
                    }
                }
            }
        }

        // Save session on exit.
        if !self.app.no_session {
            self.app.save_session_now();
        }

        info!("headless server exiting");
        Ok(())
    }

    fn allocate_activity_stamp(&mut self) -> u64 {
        let stamp = self.next_activity_stamp;
        self.next_activity_stamp = self.next_activity_stamp.saturating_add(1);
        stamp
    }

    fn focused_latency_active(&self, now: Instant) -> bool {
        self.focused_input_pane_id.is_some()
            && self
                .focused_input_deadline
                .is_some_and(|deadline| now < deadline)
    }

    fn next_focused_latency_deadline(&self, now: Instant) -> Option<Instant> {
        [
            self.focused_input_deadline,
            self.latency_background_render_at
                .filter(|_| self.latency_background_dirty),
        ]
        .into_iter()
        .flatten()
        .filter(|deadline| *deadline > now)
        .min()
    }

    fn handle_focused_latency_timers(&mut self, now: Instant) -> bool {
        if self
            .focused_input_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.focused_input_deadline = None;
            self.focused_input_pane_id = None;
            self.latency_background_render_at = None;
            if self.latency_background_dirty {
                self.latency_background_dirty = false;
                return true;
            }
            return false;
        }

        if self.latency_background_dirty
            && self
                .latency_background_render_at
                .is_some_and(|deadline| now >= deadline)
        {
            self.latency_background_render_at = None;
            return true;
        }

        false
    }

    fn arm_focused_input_latency(&mut self, now: Instant) {
        let Some(focused_pane_id) = self.app.state.session().and_then(|ws| ws.focused_pane_id())
        else {
            return;
        };
        self.focused_input_pane_id = Some(focused_pane_id);
        self.focused_input_deadline = Some(now + FOCUSED_INPUT_LATENCY_WINDOW);
    }

    fn note_latency_background_skipped(&mut self, now: Instant) {
        self.latency_background_dirty = true;
        if self.focused_latency_active(now) && self.latency_background_render_at.is_none() {
            self.latency_background_render_at = Some(now + LATENCY_BACKGROUND_RENDER_INTERVAL);
        }
    }

    fn clear_latency_background_dirty(&mut self) {
        self.latency_background_dirty = false;
        self.latency_background_render_at = None;
    }

    fn latency_background_render_due(&self, now: Instant) -> bool {
        self.latency_background_dirty
            && self
                .latency_background_render_at
                .is_some_and(|deadline| now >= deadline)
    }

    fn resize_shared_runtime_to_effective_size(&mut self) {
        self.resize_shared_runtime_to_effective_size_inner();
    }

    fn resize_shared_runtime_to_effective_size_before_input(&mut self) {
        self.resize_shared_runtime_to_effective_size_inner();
    }

    fn resize_shared_runtime_to_effective_size_inner(&mut self) {
        if self.foreground_client_id.is_none() {
            return;
        }
        self.compute_foreground_view_geometry();

        // Shared runtime size changes affect pane wrapping and foreground-driven
        // rendering semantics. Force one fresh frame to every remaining client
        // even if the next rendered buffer compares equal to its cached frame.
        for client in self.clients.values_mut() {
            client.request_full_redraw();
        }
    }

    fn compute_foreground_view_geometry(&mut self) {
        let Some(client_id) = self.foreground_client_id else {
            return;
        };
        let Some(client) = self.clients.get(&client_id) else {
            return;
        };
        let (cols, rows) = self.effective_size;
        let area = Rect::new(0, 0, cols, rows);
        if self.app.state.kitty_graphics_enabled && client.cell_size.is_known() {
            crate::ui::compute_view_with_cell_size(
                &mut self.app.state,
                &self.app.terminal_runtimes,
                area,
                client.cell_size,
            );
        } else {
            crate::ui::compute_view_with_runtime_registry(
                &mut self.app.state,
                &self.app.terminal_runtimes,
                area,
            );
        }
    }

    fn compute_foreground_pane_geometry(&mut self) {
        let Some(client_id) = self.foreground_client_id else {
            return;
        };
        let Some(client) = self.clients.get(&client_id) else {
            return;
        };
        let (cols, rows) = self.effective_size;
        let area = Rect::new(0, 0, cols, rows);
        let cell_size = if self.app.state.kitty_graphics_enabled && client.cell_size.is_known() {
            client.cell_size
        } else {
            crate::kitty_graphics::HostCellSize::default()
        };
        crate::ui::compute_pane_geometry_with_runtime_registry(
            &mut self.app.state,
            &self.app.terminal_runtimes,
            area,
            cell_size,
        );
    }

    fn sync_foreground_client_state(&mut self) {
        let Some(client_id) = self.foreground_client_id else {
            self.effective_size = (MIN_COLS, MIN_ROWS);
            self.app.state.outer_terminal_focus = None;
            let server_keybindings = self.server_keybindings.clone();
            apply_keybindings(&mut self.app, &server_keybindings);
            self.sync_visible_server_config_diagnostic(false);
            return;
        };
        let Some(client) = self.clients.get(&client_id) else {
            self.foreground_client_id = None;
            self.effective_size = (MIN_COLS, MIN_ROWS);
            self.app.state.outer_terminal_focus = None;
            let server_keybindings = self.server_keybindings.clone();
            apply_keybindings(&mut self.app, &server_keybindings);
            self.sync_visible_server_config_diagnostic(false);
            return;
        };

        let terminal_size = client.terminal_size;
        let outer_terminal_focus = client.outer_terminal_focus;
        let host_terminal_theme = client.host_terminal_theme;
        let uses_local_keybindings = client.keybindings.is_some();
        let keybindings = client
            .keybindings
            .as_deref()
            .unwrap_or(&self.server_keybindings)
            .clone();

        self.effective_size = terminal_size;
        self.app.state.outer_terminal_focus = outer_terminal_focus;
        apply_keybindings(&mut self.app, &keybindings);
        self.sync_visible_server_config_diagnostic(uses_local_keybindings);
        if outer_terminal_focus == Some(true) {
            self.app.state.mark_active_tab_seen();
        }
        if !host_terminal_theme.is_empty() {
            self.app.set_host_terminal_theme(host_terminal_theme);
        }
    }

    #[cfg(unix)]
    fn perform_live_handoff(
        &mut self,
        params: crate::api::schema::ServerLiveHandoffParams,
    ) -> io::Result<()> {
        info!("starting live handoff");
        let import_exe = params.import_exe.as_deref().map(std::path::PathBuf::from);
        let socket_path = crate::server::handoff::handoff_socket_path();
        let token = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let listener = match crate::server::handoff::bind_listener(&socket_path) {
            Ok(listener) => listener,
            Err(err) => {
                self.handoff_in_progress = false;
                return Err(err);
            }
        };

        let mut pane_by_terminal = HashMap::new();
        for entry in self.app.state.session_tab_entries() {
            for (pane_id, pane) in &entry.tab.panes {
                pane_by_terminal.insert(pane.attached_terminal_id.clone(), pane_id.raw());
            }
        }
        if pane_by_terminal.len() > crate::server::handoff::MAX_FDS_PER_HANDOFF {
            let _ = std::fs::remove_file(&socket_path);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "live handoff supports at most {} panes in one update; close panes or restart gmux normally",
                    crate::server::handoff::MAX_FDS_PER_HANDOFF
                ),
            ));
        }

        self.handoff_in_progress = true;
        self.disconnect_all_clients_for_handoff();
        let _ = reject_pending_client_connections(&self.client_listener);

        let mut paused_terminal_ids = Vec::new();
        for terminal_id in pane_by_terminal.keys() {
            if let Some(runtime) = self.app.terminal_runtimes.get(terminal_id) {
                if let Err(err) = runtime.pause_handoff_reader(Duration::from_secs(2)) {
                    self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
                    return Err(err);
                }
                paused_terminal_ids.push(terminal_id.clone());
            }
        }

        self.app.state.collapse_to_single_session();
        let snapshot = crate::persist::capture(
            self.app.state.session(),
            &self.app.state.terminals,
            &self.app.terminal_runtimes,
            self.app.state.restore_processes,
        );

        let mut handoff_entries = Vec::new();
        for (terminal_id, runtime) in self.app.terminal_runtimes.iter() {
            let Some(pane_id) = pane_by_terminal.get(terminal_id).copied() else {
                continue;
            };
            let mut handoff_runtime = runtime.handoff_runtime_state(pane_id);
            handoff_runtime.initial_history_ansi = runtime.handoff_history_ansi();
            handoff_entries.push((terminal_id.clone(), handoff_runtime));
        }

        let panes = handoff_entries
            .iter()
            .map(|(_, runtime)| runtime.clone())
            .collect();
        let manifest = crate::server::handoff::manifest_for(
            snapshot,
            panes,
            params.expected_protocol,
            params.expected_version,
        );
        let mut import_child = match crate::server::handoff::spawn_handoff_import(
            import_exe.as_deref(),
            &socket_path,
            &token,
        ) {
            Ok(child) => child,
            Err(err) => {
                self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
                return Err(err);
            }
        };
        let child_pid = import_child.id();
        info!(pid = child_pid, socket = %socket_path.display(), "spawned handoff import server");

        let mut fds = Vec::new();
        let duplicate_result = (|| {
            for (terminal_id, _) in &handoff_entries {
                let Some(runtime) = self.app.terminal_runtimes.get(terminal_id) else {
                    continue;
                };
                fds.push(runtime.duplicate_handoff_fd()?);
            }
            Ok::<(), io::Error>(())
        })();
        if let Err(err) = duplicate_result {
            for fd in fds {
                let _ = unsafe { libc::close(fd) };
            }
            crate::server::handoff::cleanup_failed_import_child(&mut import_child);
            self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
            return Err(err);
        }

        let mut stream = match crate::server::handoff::accept_and_validate_on(
            listener,
            &socket_path,
            &token,
            &manifest,
        ) {
            Ok(stream) => stream,
            Err(err) => {
                for fd in fds {
                    let _ = unsafe { libc::close(fd) };
                }
                crate::server::handoff::cleanup_failed_import_child(&mut import_child);
                self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
                return Err(err);
            }
        };

        let send_result = crate::server::handoff::send_fds_and_wait_restored(&mut stream, &fds);
        for fd in fds {
            let _ = unsafe { libc::close(fd) };
        }
        if let Err(err) = send_result {
            crate::server::handoff::cleanup_failed_import_child(&mut import_child);
            self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
            return Err(err);
        }

        if let Some(api_server) = &self.api_server {
            let _ = api_server.remove_socket_file_if_owned();
        } else {
            let _ = std::fs::remove_file(crate::api::socket_path());
        }
        let _ = remove_socket_file_if_owned(&self.client_socket_path, self.client_socket_identity);
        if let Err(err) = crate::server::handoff::wait_ready(&mut stream) {
            crate::server::handoff::cleanup_failed_import_child(&mut import_child);
            match self.wait_then_restore_public_sockets_after_failed_handoff() {
                Ok(()) => {
                    self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
                }
                Err(restore_err) => {
                    self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
                    return Err(io::Error::other(format!(
                        "handoff replacement server did not become ready: {err}; old server could not restore public sockets: {restore_err}"
                    )));
                }
            }
            return Err(io::Error::other(format!(
                "handoff replacement server did not become ready: {err}"
            )));
        }
        if let Err(err) = crate::server::handoff::report_committed(&mut stream) {
            crate::server::handoff::cleanup_failed_import_child(&mut import_child);
            match self.wait_then_restore_public_sockets_after_failed_handoff() {
                Ok(()) => {
                    self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
                }
                Err(restore_err) => {
                    self.rollback_handoff_before_commit(&socket_path, &paused_terminal_ids);
                    return Err(io::Error::other(format!(
                        "handoff replacement server was ready, but commit failed: {err}; old server could not restore public sockets: {restore_err}"
                    )));
                }
            }
            return Err(err);
        }

        for (terminal_id, runtime) in self.app.terminal_runtimes.drain_for_handoff() {
            if !pane_by_terminal.contains_key(&terminal_id) {
                continue;
            }
            debug!(terminal = %terminal_id, "preserving pane runtime for handoff");
            runtime.preserve_for_handoff();
        }
        crate::server::handoff::wait_owned_ack(&mut stream);

        self.shutting_down = true;
        self.app.state.should_quit = true;
        self.app.no_session = true;
        info!("live handoff completed; old server exiting");
        Ok(())
    }

    #[cfg(not(unix))]
    fn perform_live_handoff(
        &mut self,
        _params: crate::api::schema::ServerLiveHandoffParams,
    ) -> io::Result<()> {
        Err(io::Error::other("live handoff is only supported on Unix"))
    }

    fn sync_visible_server_config_diagnostic(&mut self, uses_local_keybindings: bool) {
        let visible = if uses_local_keybindings {
            &self.server_config_diagnostic_without_keybindings
        } else {
            &self.server_config_diagnostic
        };
        if self.app.state.config_diagnostic == self.server_config_diagnostic
            || self.app.state.config_diagnostic == self.server_config_diagnostic_without_keybindings
        {
            self.app.state.config_diagnostic = visible.clone();
        }
    }

    #[cfg(unix)]
    fn restore_public_sockets_after_failed_handoff(&mut self) -> io::Result<()> {
        let api_tx = self
            .api_tx
            .clone()
            .ok_or_else(|| io::Error::other("cannot restore api socket without api sender"))?;
        let api_server = api::start_server(api_tx, self.app.event_hub.clone())?;

        let client_path = client_socket_path();
        prepare_socket_path(&client_path)?;
        let listener = UnixListener::bind(&client_path)?;
        restrict_socket_permissions(&client_path)?;
        let client_socket_identity = socket_file_identity(&client_path)?;
        listener.set_nonblocking(true)?;

        self.api_server = Some(api_server);
        self.client_listener = listener;
        self.client_socket_path = client_path;
        self.client_socket_identity = client_socket_identity;
        Ok(())
    }

    #[cfg(unix)]
    fn wait_then_restore_public_sockets_after_failed_handoff(&mut self) -> io::Result<()> {
        let timeout = crate::server::handoff::COMMIT_TIMEOUT + Duration::from_secs(2);
        wait_for_old_public_sockets_to_close(timeout)?;
        self.restore_public_sockets_after_failed_handoff()
    }

    #[cfg(unix)]
    fn rollback_handoff_before_commit(
        &mut self,
        socket_path: &Path,
        paused_terminal_ids: &[crate::terminal::TerminalId],
    ) {
        for terminal_id in paused_terminal_ids {
            if let Some(runtime) = self.app.terminal_runtimes.get(terminal_id) {
                runtime.set_handoff_reader_paused(false);
            }
        }
        self.handoff_in_progress = false;
        let _ = std::fs::remove_file(socket_path);
    }

    #[cfg(unix)]
    fn nudge_handoff_panes_on_first_client_attach(&mut self) {
        if !self.pending_handoff_repaint_nudge {
            return;
        }
        self.pending_handoff_repaint_nudge = false;
        self.app
            .terminal_runtimes
            .nudge_child_redraw_after_handoff();
    }

    #[cfg(not(unix))]
    fn nudge_handoff_panes_on_first_client_attach(&mut self) {}

    fn reload_server_config(&mut self, notify_success: bool) -> crate::config::ConfigReloadReport {
        let server_keybindings = self.server_keybindings.clone();
        apply_keybindings(&mut self.app, &server_keybindings);
        let report = self.app.apply_config_from_disk(notify_success);
        self.app.take_config_reloaded_from_disk();
        self.server_keybindings = app_keybindings(&self.app);
        let (server_config_diagnostic, server_config_diagnostic_without_keybindings) =
            server_config_diagnostic_summaries(&report.diagnostics);
        self.server_config_diagnostic = server_config_diagnostic;
        self.server_config_diagnostic_without_keybindings =
            server_config_diagnostic_without_keybindings;
        self.sync_foreground_client_state();
        report
    }

    fn promote_client_to_foreground(&mut self, client_id: u64) -> bool {
        let stamp = self.allocate_activity_stamp();
        let Some(client) = self.clients.get_mut(&client_id) else {
            return false;
        };
        client.last_activity = stamp;

        let changed = self.foreground_client_id != Some(client_id);
        self.foreground_client_id = Some(client_id);
        self.sync_foreground_client_state();
        changed
    }

    fn promote_latest_remaining_client(&mut self) -> bool {
        let next_foreground = latest_app_client(&self.clients);
        let changed = next_foreground != self.foreground_client_id;
        self.foreground_client_id = next_foreground;
        self.sync_foreground_client_state();
        changed
    }

    #[cfg(test)]
    fn app_client_count(&self) -> usize {
        self.clients
            .values()
            .filter(|client| client.is_full_app_client() && client.writer.is_some())
            .count()
    }

    #[cfg(test)]
    fn has_app_client(&self) -> bool {
        self.app_client_count() > 0
    }

    fn remove_client(&mut self, client_id: u64) -> bool {
        let was_foreground = self.foreground_client_id == Some(client_id);
        self.send_client_graphics_cleanup(client_id);
        let removed = self.clients.remove(&client_id);
        if let Some(removed) = removed {
            crate::server::clipboard_image::remove_files(removed.staged_clipboard_files);
            if let ClientConnectionMode::TerminalAttach { terminal_id } = removed.mode {
                self.terminal_attach_owners.remove(&terminal_id);
                if let Some(terminal_id) = self.terminal_id_by_string(&terminal_id) {
                    self.app
                        .state
                        .direct_attach_resize_locks
                        .remove(&terminal_id);
                }
            }
        }
        if was_foreground {
            self.promote_latest_remaining_client()
        } else {
            false
        }
    }

    fn client_removal_needs_shared_resize(&self, client_id: u64) -> bool {
        if self.foreground_client_id == Some(client_id) {
            return true;
        }
        matches!(
            self.clients.get(&client_id).map(|client| &client.mode),
            Some(ClientConnectionMode::TerminalAttach { .. })
        ) && self.foreground_client_id.is_some()
    }

    fn remove_client_and_resize_if_needed(&mut self, client_id: u64) {
        let needs_shared_resize = self.client_removal_needs_shared_resize(client_id);
        let foreground_changed = self.remove_client(client_id);
        if needs_shared_resize || foreground_changed {
            self.resize_shared_runtime_to_effective_size();
        }
    }

    fn send_client_graphics_cleanup(&mut self, client_id: u64) {
        let (writer, bytes) = match self.clients.get_mut(&client_id) {
            Some(client) => {
                let bytes = client.graphics_cache.clear_bytes();
                (client.writer.as_ref().cloned(), bytes)
            }
            None => return,
        };
        if bytes.is_empty() {
            return;
        }
        let Some(writer) = writer else {
            return;
        };
        let Ok(serialized) = Self::frame_server_message(&ServerMessage::Graphics { bytes }) else {
            return;
        };
        let _ = writer.control.send(serialized);
    }

    fn send_all_clients_graphics_cleanup(&mut self) {
        let client_ids = self.clients.keys().copied().collect::<Vec<_>>();
        for client_id in client_ids {
            self.send_client_graphics_cleanup(client_id);
        }
    }

    fn update_client_host_theme_from_events(
        &mut self,
        client_id: u64,
        events: &[crate::raw_input::RawInputEvent],
    ) -> bool {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return false;
        };

        if !client.update_host_theme_from_events(events) {
            return false;
        }

        if self.foreground_client_id == Some(client_id) {
            let changed = self.app.set_host_terminal_theme(client.host_terminal_theme);
            if changed {
                self.resize_shared_runtime_to_effective_size_before_input();
            }
            changed
        } else {
            false
        }
    }

    fn update_client_outer_focus_from_events(
        &mut self,
        client_id: u64,
        events: &[crate::raw_input::RawInputEvent],
    ) {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return;
        };
        let Some(next_focus) = client.update_outer_focus_from_events(events) else {
            return;
        };
        if self.foreground_client_id == Some(client_id) {
            self.app.state.outer_terminal_focus = Some(next_focus);
        }
    }

    /// Accepts pending client connections from the non-blocking listener.
    fn accept_client_connections(&mut self) -> io::Result<()> {
        if self.handoff_in_progress {
            return reject_pending_client_connections(&self.client_listener);
        }
        accept_pending_client_connections(
            &self.client_listener,
            &mut self.next_client_id,
            &self.should_quit,
            &self.server_event_tx,
            &self.server_input_tx,
        )
    }

    /// Drains high-priority client input before normal server work.
    fn drain_priority_input_events(&mut self) -> bool {
        let mut changed = false;
        let mut pending = None;
        let mut drained = 0usize;
        loop {
            let Some(ev) = pending.take().or_else(|| {
                if drained >= PRIORITY_INPUT_DRAIN_LIMIT {
                    return None;
                }
                let ev = self.server_input_rx.try_recv().ok()?;
                drained = drained.saturating_add(1);
                Some(ev)
            }) else {
                break;
            };
            let ev = self.coalesce_priority_input_event(ev, &mut pending, &mut drained);
            changed |= self.handle_server_event(ev);
        }
        changed
    }

    fn coalesce_priority_input_event(
        &mut self,
        event: ServerEvent,
        pending: &mut Option<ServerEvent>,
        drained: &mut usize,
    ) -> ServerEvent {
        match event {
            ServerEvent::ClientInput {
                client_id,
                mut data,
                received_at,
            } => {
                while *drained < PRIORITY_INPUT_DRAIN_LIMIT {
                    let Ok(next) = self.server_input_rx.try_recv() else {
                        break;
                    };
                    *drained = (*drained).saturating_add(1);
                    match next {
                        ServerEvent::ClientInput {
                            client_id: next_client_id,
                            data: next_data,
                            ..
                        } if next_client_id == client_id => data.extend(next_data),
                        other => {
                            *pending = Some(other);
                            break;
                        }
                    }
                }
                ServerEvent::ClientInput {
                    client_id,
                    data,
                    received_at,
                }
            }
            ServerEvent::ClientAttachScroll {
                client_id,
                source: AttachScrollSource::Wheel,
                direction,
                lines,
                column,
                row,
                modifiers,
            } if self.terminal_attach_scroll_uses_host_scroll(client_id) => {
                let mut total_lines = lines.max(1);
                while *drained < PRIORITY_INPUT_DRAIN_LIMIT {
                    let Ok(next) = self.server_input_rx.try_recv() else {
                        break;
                    };
                    *drained = (*drained).saturating_add(1);
                    match next {
                        ServerEvent::ClientAttachScroll {
                            client_id: next_client_id,
                            source: AttachScrollSource::Wheel,
                            direction: next_direction,
                            lines: next_lines,
                            ..
                        } if next_client_id == client_id && next_direction == direction => {
                            total_lines = total_lines.saturating_add(next_lines.max(1));
                        }
                        other => {
                            *pending = Some(other);
                            break;
                        }
                    }
                }
                ServerEvent::ClientAttachScroll {
                    client_id,
                    source: AttachScrollSource::Wheel,
                    direction,
                    lines: total_lines,
                    column,
                    row,
                    modifiers,
                }
            }
            other => other,
        }
    }

    fn terminal_attach_scroll_uses_host_scroll(&self, client_id: u64) -> bool {
        let Some(ClientConnection {
            mode: ClientConnectionMode::TerminalAttach { terminal_id },
            ..
        }) = self.clients.get(&client_id)
        else {
            return false;
        };
        let Some(runtime) = self.runtime_for_terminal_id_string(terminal_id) else {
            return false;
        };
        matches!(
            runtime.wheel_routing(),
            Some(crate::pane::WheelRouting::HostScroll) | None
        )
    }

    /// Drains server events from the dedicated channel.
    ///
    /// Returns true if any input was processed (requiring a re-render).
    fn drain_server_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(ev) = self.server_event_rx.try_recv() {
            changed |= self.handle_server_event(ev);
        }
        changed
    }

    fn terminal_id_by_string(&self, terminal_id: &str) -> Option<crate::terminal::TerminalId> {
        self.app
            .state
            .terminals
            .keys()
            .find(|id| id.to_string() == terminal_id)
            .cloned()
    }

    fn runtime_for_terminal_id_string(
        &self,
        terminal_id: &str,
    ) -> Option<&crate::terminal::TerminalRuntime> {
        let terminal_id = self.terminal_id_by_string(terminal_id)?;
        self.app.terminal_runtimes.get(&terminal_id)
    }

    fn write_client_clipboard_image(
        &mut self,
        client_id: u64,
        extension: &str,
        data: &[u8],
    ) -> std::io::Result<String> {
        let staged = crate::server::clipboard_image::stage(client_id, extension, data)?;
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.staged_clipboard_files.push(staged.path);
        }
        info!(client_id, bytes = data.len(), path = %staged.paste_text, "staged client clipboard image");
        Ok(staged.paste_text)
    }

    fn paste_client_clipboard_image_path(&mut self, client_id: u64, path: String) -> bool {
        if let Some(ClientConnection {
            mode: ClientConnectionMode::TerminalAttach { terminal_id },
            ..
        }) = self.clients.get(&client_id)
        {
            if let Some(runtime) = self.runtime_for_terminal_id_string(terminal_id) {
                let payload = paste_payload_for_runtime(runtime, &path);
                if let Err(err) = runtime.try_send_bytes(Bytes::from(payload)) {
                    warn!(client_id, terminal_id = %terminal_id, err = %err, "terminal attach clipboard image paste failed");
                } else {
                    self.app.arm_input_render_bypass();
                }
            }
            return true;
        }

        let foreground_changed = self.promote_client_to_foreground(client_id);
        if foreground_changed {
            self.resize_shared_runtime_to_effective_size_before_input();
        }
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.request_semantic_redraw_after_input();
        }
        self.app.route_client_events(
            vec![crate::raw_input::RawInputEvent::Paste(path)],
            self.foreground_client_id == Some(client_id),
        );
        true
    }

    fn handle_terminal_attach_scroll(
        &mut self,
        client_id: u64,
        source: AttachScrollSource,
        direction: AttachScrollDirection,
        lines: u16,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    ) -> bool {
        let Some(ClientConnection {
            mode: ClientConnectionMode::TerminalAttach { terminal_id },
            ..
        }) = self.clients.get(&client_id)
        else {
            return false;
        };
        let Some(runtime) = self.runtime_for_terminal_id_string(terminal_id) else {
            return false;
        };

        match apply_terminal_attach_scroll(
            runtime, source, direction, lines, column, row, modifiers,
        ) {
            Ok(()) => self.app.arm_input_render_bypass(),
            Err(err) => {
                warn!(client_id, terminal_id = %terminal_id, err = %err, "terminal attach scroll failed");
            }
        }
        true
    }

    /// Handles a single internal event with forwarding logic for clipboard
    /// and toast notifications to connected clients.
    ///
    /// ALL internal events MUST be routed through this method to ensure
    /// clipboard/notify forwarding is never bypassed. Do not call
    /// `self.app.handle_internal_event()` directly for any internal event
    /// in the headless server — use this method instead.
    ///
    /// Returns true if the event changed visual state (requiring a re-render).
    fn handle_internal_event_with_forwarding(&mut self, ev: AppEvent) -> bool {
        match ev {
            AppEvent::ClipboardWrite { content } => {
                // Clipboard writes are client-local side effects. Forward them only to
                // the foreground client instead of broadcasting to every attached client.
                let data = base64::engine::general_purpose::STANDARD.encode(content.as_slice());
                if self.send_to_foreground_client(ServerMessage::Clipboard { data }) {
                    self.app.show_clipboard_feedback();
                }
                true
            }
            AppEvent::PaneDied { pane_id } => {
                let pane_id_val = pane_id;
                let terminal_id = self.app.state.session_tab_entries().find_map(|entry| {
                    entry
                        .tab
                        .panes
                        .get(&pane_id)
                        .map(|pane| pane.attached_terminal_id.to_string())
                });

                self.app
                    .handle_internal_event(AppEvent::PaneDied { pane_id });

                if self.app.find_pane(pane_id_val).is_none() {
                    if let Some(terminal_id) = terminal_id {
                        self.shutdown_terminal_attach_clients(
                            &terminal_id,
                            format!("terminal {terminal_id} exited"),
                        );
                    }
                }

                true
            }
            AppEvent::UpdateInstallFinished(crate::update::UpdateInstallResult::Success(
                success,
            )) => {
                let params = crate::api::schema::ServerLiveHandoffParams {
                    import_exe: Some(success.binary_path.display().to_string()),
                    expected_protocol: None,
                    expected_version: Some(success.version.clone()),
                };
                if let Err(err) = self.perform_live_handoff(params) {
                    self.app
                        .handle_internal_event(AppEvent::UpdateInstallFinished(
                            crate::update::UpdateInstallResult::Failed {
                                message: format!("updated binary but relaunch failed: {err}"),
                            },
                        ));
                }
                true
            }
            AppEvent::UpdateCheckFinished(_) | AppEvent::UpdateInstallFinished(_) => {
                self.app.handle_internal_event(ev);
                true
            }
        }
    }

    /// Drains internal events, forwarding clipboard and toast
    /// notifications to connected clients instead of processing them locally.
    ///
    /// In the monolithic mode:
    /// - `ClipboardWrite` events are written to stdout via `write_osc52_bytes`.
    /// - Toast notifications are set on AppState and rendered into the frame.
    ///
    /// In the headless server, there is no stdout terminal,
    /// so we:
    /// - Forward `ClipboardWrite` as `ServerMessage::Clipboard` to the
    ///   foreground client only.
    /// - Detect when a toast is set on AppState and forward as
    ///   `ServerMessage::Notify` to the foreground client for terminal/system delivery.
    fn drain_internal_events_with_forwarding(&mut self) -> bool {
        self.drain_internal_events_with_forwarding_up_to(crate::app::APP_EVENT_DRAIN_LIMIT)
            .1
    }

    fn drain_all_internal_events_with_forwarding(&mut self) -> bool {
        let mut changed = false;
        loop {
            let (had_event, batch_changed) =
                self.drain_internal_events_with_forwarding_up_to(crate::app::APP_EVENT_DRAIN_LIMIT);
            changed |= batch_changed;
            if !had_event {
                break;
            }
        }
        changed
    }

    fn drain_internal_events_with_forwarding_up_to(&mut self, limit: usize) -> (bool, bool) {
        let mut had_event = false;
        let mut changed = false;
        for _ in 0..limit {
            let Ok(ev) = self.app.event_rx.try_recv() else {
                break;
            };
            had_event = true;
            changed |= self.handle_internal_event_with_forwarding(ev);
        }
        (had_event, changed)
    }

    fn drain_client_config_reload_request(&mut self) {
        if !self.app.state.request_client_config_reload {
            return;
        }
        self.app.state.request_client_config_reload = false;
        self.send_to_all_clients(ServerMessage::ReloadClientConfig);
    }

    /// Encodes a server message into a length-prefixed frame.
    fn frame_server_message(msg: &ServerMessage) -> Result<Vec<u8>, protocol::FramingError> {
        Self::frame_server_message_with_max(msg, MAX_FRAME_SIZE)
    }

    /// Encodes a server message using an explicit payload cap.
    fn frame_server_message_with_max(
        msg: &ServerMessage,
        max_frame_size: usize,
    ) -> Result<Vec<u8>, protocol::FramingError> {
        let mut framed = Vec::new();
        protocol::write_message(&mut framed, msg)?;
        let payload_len = framed.len().saturating_sub(4);
        if payload_len > max_frame_size {
            return Err(protocol::FramingError::Oversized {
                claimed: payload_len,
                max: max_frame_size,
            });
        }
        Ok(framed)
    }

    /// Sends a message to all connected clients.
    /// Broken connections are tracked and cleaned up.
    fn send_to_all_clients(&mut self, msg: ServerMessage) {
        let serialized = match Self::frame_server_message(&msg) {
            Ok(framed) => framed,
            Err(err) => {
                warn!(err = %err, "failed to serialize message for clients");
                return;
            }
        };

        let mut broken_clients: Vec<u64> = Vec::new();
        for (&client_id, client) in &mut self.clients {
            if let Some(writer) = &client.writer {
                if writer.control.send(serialized.clone()).is_err() {
                    debug!(client_id, "client writer channel closed during broadcast");
                    broken_clients.push(client_id);
                }
            }
        }

        // Remove broken clients.
        for client_id in broken_clients {
            self.remove_client_and_resize_if_needed(client_id);
        }
    }

    /// Sends a client-local side effect to the foreground client only.
    fn send_to_foreground_client(&mut self, msg: ServerMessage) -> bool {
        let Some(client_id) = self.foreground_client_id else {
            return false;
        };
        self.send_to_client(client_id, msg)
    }

    /// Sends a message to a specific client. Returns false if the client
    /// was not found or the send failed (client removed).
    fn send_to_client(&mut self, client_id: u64, msg: ServerMessage) -> bool {
        let serialized = match Self::frame_server_message(&msg) {
            Ok(framed) => framed,
            Err(err) => {
                warn!(client_id, err = %err, "failed to serialize message for client");
                return false;
            }
        };

        if let Some(client) = self.clients.get(&client_id) {
            if let Some(writer) = &client.writer {
                if writer.control.send(serialized).is_err() {
                    debug!(
                        client_id,
                        "client writer channel closed during targeted send"
                    );
                    self.remove_client_and_resize_if_needed(client_id);
                    return false;
                }
            }
            true
        } else {
            false
        }
    }

    fn shutdown_terminal_attach_clients(&mut self, terminal_id: &str, reason: String) {
        let client_ids = terminal_attach_client_ids(&self.clients, terminal_id);

        for client_id in client_ids {
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some(reason.clone()),
                },
            );
            self.remove_client_and_resize_if_needed(client_id);
        }
    }

    fn disconnect_all_clients_for_handoff(&mut self) {
        let client_ids = self.clients.keys().copied().collect::<Vec<_>>();
        for client_id in client_ids {
            self.send_client_graphics_cleanup(client_id);
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some(
                        "live update in progress; reconnect after handoff completes".to_owned(),
                    ),
                },
            );
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.writer = None;
            }
            let _ = self.remove_client(client_id);
        }
        self.foreground_client_id = None;
        self.sync_foreground_client_state();
        self.resize_shared_runtime_to_effective_size();
    }

    fn attach_terminal_client(
        &mut self,
        client_id: u64,
        terminal_id: String,
        takeover: bool,
    ) -> bool {
        let Some(real_terminal_id) = self.terminal_id_by_string(&terminal_id) else {
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some(format!(
                        "terminal attach failed: terminal {terminal_id} not found"
                    )),
                },
            );
            self.remove_client_and_resize_if_needed(client_id);
            return false;
        };

        if let Some(existing_owner) = self.terminal_attach_owners.get(&terminal_id).copied() {
            if existing_owner != client_id && !takeover {
                self.send_to_client(
                    client_id,
                    ServerMessage::ServerShutdown {
                        reason: Some(format!(
                            "terminal attach failed: terminal {terminal_id} already has an attached client; retry with --takeover"
                        )),
                    },
                );
                self.remove_client_and_resize_if_needed(client_id);
                return false;
            }
            if existing_owner != client_id {
                self.send_to_client(
                    existing_owner,
                    ServerMessage::ServerShutdown {
                        reason: Some("terminal attach taken over".to_owned()),
                    },
                );
                self.remove_client_and_resize_if_needed(existing_owner);
            }
        }

        let stamp = self.allocate_activity_stamp();
        let Some(client) = self.clients.get_mut(&client_id) else {
            return false;
        };
        let (cols, rows) = client.terminal_size;
        let cell_size = client.cell_size;
        client.mode = ClientConnectionMode::TerminalAttach {
            terminal_id: terminal_id.clone(),
        };
        client.pending_terminal_attach = false;
        if let Some(render_actor) = &mut client.render_actor {
            render_actor.reset_baseline();
        }
        client.last_activity = stamp;
        let was_foreground = self.foreground_client_id == Some(client_id);
        if was_foreground {
            self.promote_latest_remaining_client();
        }

        info!(client_id, cols, rows, terminal_id = %terminal_id, "terminal attach client connected");
        self.terminal_attach_owners
            .insert(terminal_id.clone(), client_id);
        self.app
            .state
            .direct_attach_resize_locks
            .insert(real_terminal_id.clone());
        if let Some(runtime) = self.app.terminal_runtimes.get(&real_terminal_id) {
            runtime.resize(rows, cols, cell_size.width_px, cell_size.height_px);
        }
        true
    }

    /// Handles a server event. Returns true if the event requires a re-render.
    fn handle_server_event(&mut self, ev: ServerEvent) -> bool {
        if self.handoff_in_progress && Self::ignore_client_event_during_handoff(&ev) {
            return false;
        }

        match ev {
            ServerEvent::ClientConnected {
                client_id,
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                keybindings,
                writer,
                render_encoding,
                direct_attach_requested,
            } => {
                if self.handoff_in_progress {
                    if let Ok(message) =
                        Self::frame_server_message(&ServerMessage::ServerShutdown {
                            reason: Some(
                                "live update in progress; reconnect after handoff completes"
                                    .to_owned(),
                            ),
                        })
                    {
                        let _ = writer.control.send(message);
                    }
                    return false;
                }
                info!(
                    client_id,
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                    ?render_encoding,
                    "client connected"
                );
                let last_activity = self.allocate_activity_stamp();
                let connection = ClientConnection::new_with_mode(
                    ClientConnectionMode::App,
                    keybindings,
                    (cols, rows),
                    crate::kitty_graphics::HostCellSize {
                        width_px: cell_width_px,
                        height_px: cell_height_px,
                    },
                    crate::terminal_theme::TerminalTheme::default(),
                    None,
                    last_activity,
                    render_encoding,
                    direct_attach_requested,
                    Some(writer),
                );
                self.clients.insert(client_id, connection);
                let became_foreground =
                    !direct_attach_requested && self.foreground_client_id.is_none();
                if became_foreground {
                    self.foreground_client_id = Some(client_id);
                }
                if became_foreground {
                    self.sync_foreground_client_state();
                    self.resize_shared_runtime_to_effective_size();
                }
                self.nudge_handoff_panes_on_first_client_attach();
                true
            }
            ServerEvent::ClientAttachTerminal {
                client_id,
                terminal_id,
                takeover,
            } => self.attach_terminal_client(client_id, terminal_id, takeover),
            ServerEvent::ClientAttachScroll {
                client_id,
                source,
                direction,
                lines,
                column,
                row,
                modifiers,
            } => self.handle_terminal_attach_scroll(
                client_id, source, direction, lines, column, row, modifiers,
            ),
            ServerEvent::ClientInput {
                client_id,
                data,
                received_at,
            } => {
                if self.handoff_in_progress {
                    debug!(
                        client_id,
                        len = data.len(),
                        "ignored client input during handoff"
                    );
                    return false;
                }
                debug!(client_id, len = data.len(), "client input received");
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.record_debug_input_received(received_at, Instant::now());
                }
                if let Some(ClientConnection {
                    mode: ClientConnectionMode::TerminalAttach { terminal_id },
                    ..
                }) = self.clients.get(&client_id)
                {
                    if let Some(runtime) = self.runtime_for_terminal_id_string(terminal_id) {
                        if let Err(err) = apply_terminal_attach_input(runtime, data) {
                            warn!(client_id, terminal_id = %terminal_id, err = %err);
                        } else {
                            self.app.arm_input_render_bypass();
                        }
                    }
                    return false;
                }
                let events = if let Some(client) = self.clients.get_mut(&client_id) {
                    let mut events = client.raw_input.push(&data);
                    // The thin client only forwards a bare ESC after its local input timeout.
                    if data.as_slice() == b"\x1b" {
                        events.extend(client.raw_input.flush_timeout());
                    }
                    events
                } else {
                    Vec::new()
                };
                let host_surface_redraw = crate::raw_input::events_require_host_surface_redraw(
                    &events,
                    self.app.state.redraw_on_focus_gained,
                );
                if host_surface_redraw {
                    if let Some(client) = self.clients.get_mut(&client_id) {
                        client.request_full_redraw();
                    }
                }
                self.update_client_outer_focus_from_events(client_id, &events);
                let interaction = events_include_interaction(&events);
                let foreground_changed = if interaction {
                    self.promote_client_to_foreground(client_id)
                } else {
                    false
                };
                if interaction
                    && self
                        .clients
                        .get(&client_id)
                        .is_some_and(|client| client.is_full_app_client())
                {
                    self.latency_critical_client_id = Some(client_id);
                }
                if foreground_changed {
                    self.resize_shared_runtime_to_effective_size_before_input();
                }
                let theme_changed = self.update_client_host_theme_from_events(client_id, &events);
                let route_result = self
                    .app
                    .route_client_events(events, self.foreground_client_id == Some(client_id));
                if route_result.forwarded_to_pty
                    && self
                        .clients
                        .get(&client_id)
                        .is_some_and(|client| client.is_full_app_client())
                {
                    self.arm_focused_input_latency(Instant::now());
                }
                if self.app.take_config_reloaded_from_disk() {
                    self.reload_server_config(false);
                } else {
                    self.sync_foreground_client_state();
                }

                // Check if the detach keybind was triggered during input processing.
                if self.app.state.detach_requested {
                    self.app.state.detach_requested = false;
                    info!(client_id, "client detach requested via keybind");

                    // Clear client-local host graphics before sending ServerShutdown
                    // so the outer terminal does not retain stale images.
                    self.send_client_graphics_cleanup(client_id);

                    // Send a ServerShutdown with "detached" reason to this client
                    // so it exits cleanly (not with a connection-lost error).
                    // The client will close its connection after receiving this,
                    // which triggers a ClientDisconnected event that removes it.
                    self.send_to_client(
                        client_id,
                        ServerMessage::ServerShutdown {
                            reason: Some("detached".to_owned()),
                        },
                    );

                    // Don't remove the client here — let the client disconnect
                    // naturally after receiving the ServerShutdown. The client's
                    // read loop will see EOF and the server will get a
                    // ClientDisconnected event which handles cleanup.
                    //
                    // However, we do need to stop sending frames to this client
                    // since it's detaching. Drop the writer channel so no more
                    // frames are queued for this client.
                    if let Some(client) = self.clients.get_mut(&client_id) {
                        client.writer = None;
                    }

                    // No re-render needed for remaining clients.
                    false
                } else {
                    host_surface_redraw
                        || foreground_changed
                        || theme_changed
                        || route_result.visual_change
                }
            }
            ServerEvent::ClientClipboardImage {
                client_id,
                extension,
                data,
            } => {
                debug!(
                    client_id,
                    len = data.len(),
                    extension = %extension,
                    "client clipboard image received"
                );
                match self.write_client_clipboard_image(client_id, &extension, &data) {
                    Ok(path) => self.paste_client_clipboard_image_path(client_id, path),
                    Err(err) => {
                        warn!(client_id, err = %err, "failed to stage client clipboard image");
                        true
                    }
                }
            }
            ServerEvent::ClientResize {
                client_id,
                cols,
                rows,
                cell_width_px,
                cell_height_px,
            } => {
                info!(
                    client_id,
                    cols, rows, cell_width_px, cell_height_px, "client resize"
                );
                let direct_terminal_id = if let Some(ClientConnection {
                    mode: ClientConnectionMode::TerminalAttach { terminal_id },
                    terminal_size,
                    cell_size,
                    render_actor,
                    ..
                }) = self.clients.get_mut(&client_id)
                {
                    *terminal_size = (cols, rows);
                    *cell_size = crate::kitty_graphics::HostCellSize {
                        width_px: cell_width_px,
                        height_px: cell_height_px,
                    };
                    if let Some(render_actor) = render_actor {
                        render_actor.reset_baseline();
                    }
                    Some(terminal_id.clone())
                } else {
                    None
                };
                if let Some(terminal_id) = direct_terminal_id {
                    if let Some(runtime) = self.runtime_for_terminal_id_string(&terminal_id) {
                        runtime.resize(rows, cols, cell_width_px, cell_height_px);
                    }
                    return true;
                }
                let should_resize_shared = self.foreground_client_id == Some(client_id)
                    || self.foreground_client_id.is_none();
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.terminal_size = (cols, rows);
                    client.cell_size = crate::kitty_graphics::HostCellSize {
                        width_px: cell_width_px,
                        height_px: cell_height_px,
                    };
                    client.request_full_redraw();
                } else {
                    return false;
                }
                if should_resize_shared {
                    self.promote_client_to_foreground(client_id);
                    self.resize_shared_runtime_to_effective_size();
                }
                should_resize_shared
            }
            ServerEvent::ClientDetach { client_id } => {
                info!(client_id, "client detached");
                self.remove_client_and_resize_if_needed(client_id);
                true
            }
            ServerEvent::ClientDisconnected { client_id } => {
                info!(client_id, "client disconnected");
                self.remove_client_and_resize_if_needed(client_id);
                true
            }
            ServerEvent::QuitSignal => {
                // The quit check at the top of the loop handles this.
                // No render needed — the next iteration will initiate shutdown.
                false
            }
        }
    }

    fn ignore_client_event_during_handoff(ev: &ServerEvent) -> bool {
        !matches!(
            ev,
            ServerEvent::ClientConnected { .. }
                | ServerEvent::ClientDisconnected { .. }
                | ServerEvent::QuitSignal
        )
    }

    fn record_debug_pty_dirty_for_pending_inputs(&mut self, dirty_at: Instant) {
        for client in self.clients.values_mut() {
            client.record_debug_pty_dirty(dirty_at);
        }
    }

    /// Drains API requests with shutdown awareness.
    ///
    /// During shutdown, remaining requests get a `server_unavailable` error.
    fn drain_api_requests_with_shutdown_check(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.app.api_rx.try_recv() {
            changed |= self.handle_api_request_with_shutdown_check(msg);
        }
        changed
    }

    /// Handles a single API request with shutdown awareness.
    ///
    /// Also forwards any toast notifications that result from the API request
    /// to connected clients.
    fn handle_api_request_with_shutdown_check(&mut self, msg: api::ApiRequestMessage) -> bool {
        if self.shutting_down {
            // During shutdown, respond with server_unavailable.
            let response = serde_json::to_string(&api::schema::ErrorResponse {
                id: msg.request.id,
                error: api::schema::ErrorBody {
                    code: "server_unavailable".into(),
                    message: "server is shutting down".into(),
                },
            })
            .unwrap_or_else(|_| {
                r#"{"id":"","error":{"code":"server_unavailable","message":"server is shutting down"}}"#
                    .to_string()
            });
            let _ = msg.respond_to.send(response);
            return false;
        }

        if let api::schema::Method::ServerLiveHandoff(params) = &msg.request.method {
            let response = match self.perform_live_handoff(params.clone()) {
                Ok(()) => serde_json::to_string(&api::schema::SuccessResponse {
                    id: msg.request.id,
                    result: api::schema::ResponseResult::Ok {},
                }),
                Err(err) => serde_json::to_string(&api::schema::ErrorResponse {
                    id: msg.request.id,
                    error: api::schema::ErrorBody {
                        code: "handoff_failed".into(),
                        message: err.to_string(),
                    },
                }),
            }
            .unwrap_or_else(|_| "{}".to_string());
            let _ = msg.respond_to.send(response);
            return true;
        }

        let mut changed = api::request_changes_ui(&msg.request);
        let skip_default_session = matches!(
            &msg.request.method,
            api::schema::Method::ServerStop(_) | api::schema::Method::ServerLiveHandoff(_)
        );
        changed |= self.drain_all_internal_events_with_forwarding();

        let toast_before = self.app.state.toast.clone();

        self.sync_foreground_client_state();
        let response = if matches!(
            &msg.request.method,
            api::schema::Method::ServerReloadConfig(_)
        ) {
            let report = self.reload_server_config(true);
            serde_json::to_string(&api::schema::SuccessResponse {
                id: msg.request.id.clone(),
                result: api::schema::ResponseResult::ConfigReload {
                    status: report.status,
                    diagnostics: report.diagnostics,
                },
            })
            .unwrap_or_else(|err| {
                serde_json::to_string(&api::schema::ErrorResponse {
                    id: String::new(),
                    error: api::schema::ErrorBody {
                        code: "serialization_error".into(),
                        message: err.to_string(),
                    },
                })
                .unwrap_or_else(|_| "{}".to_string())
            })
        } else {
            self.app
                .handle_api_request_after_internal_events_drained(msg.request)
        };
        let _ = msg.respond_to.send(response);

        // Forward new toast state only when a client-local delivery mode is selected.
        // Gmux delivery renders the toast in-frame and must not ask clients to
        // show a terminal or system notification.
        let toast_after = self.app.state.toast.clone();
        if should_forward_toast_to_clients(self.app.state.toast_config.delivery)
            && toast_after.is_some()
            && toast_after != toast_before
        {
            if let Some(toast) = &toast_after {
                let msg_text = format!("{}: {}", toast.title, toast.context);
                debug!(msg = %msg_text, "forwarding toast notification from API request");
                self.send_to_foreground_client(ServerMessage::Notify {
                    kind: toast_notify_kind(self.app.state.toast_config.delivery)
                        .expect("toast forwarding requires a client notification kind"),
                    message: msg_text,
                });
            }
        }

        if !skip_default_session && latest_app_client(&self.clients).is_some() {
            changed |= self.app.ensure_default_session();
        }

        changed
    }

    fn stream_host_mouse_capture_mode(&mut self) {
        let enabled = self
            .app
            .state
            .should_capture_host_mouse_from(&self.app.terminal_runtimes);
        let serialized = match Self::frame_server_message(&ServerMessage::MouseCapture { enabled })
        {
            Ok(framed) => framed,
            Err(err) => {
                warn!(err = %err, "failed to serialize mouse capture mode for clients");
                return;
            }
        };

        let mut broken_clients: Vec<u64> = Vec::new();
        for (&client_id, client) in &mut self.clients {
            if !client.is_full_app_client() {
                continue;
            }
            if client.host_mouse_capture_active == Some(enabled) {
                continue;
            }
            let Some(writer) = &client.writer else {
                continue;
            };
            if writer.control.send(serialized.clone()).is_err() {
                debug!(
                    client_id,
                    "client writer channel closed during mouse capture update"
                );
                broken_clients.push(client_id);
                continue;
            }
            client.host_mouse_capture_active = Some(enabled);
        }

        for client_id in broken_clients {
            self.remove_client_and_resize_if_needed(client_id);
        }
    }

    /// Renders the current state to client-sized virtual buffers and streams
    /// frames to all connected clients.
    fn render_focused_latency_update_and_stream(&mut self, now: Instant) -> bool {
        crate::render_prof::event("focused_latency.attempt");
        let focused_started = crate::render_prof::timer();
        macro_rules! focused_fallback {
            ($reason:literal) => {{
                crate::render_prof::event(concat!("focused_latency_fallback.", $reason));
                crate::render_prof::duration_since("focused_latency.total", focused_started);
                return false;
            }};
        }
        macro_rules! focused_success {
            ($reason:literal) => {{
                crate::render_prof::event("focused_latency.success");
                crate::render_prof::event(concat!("focused_latency_success.", $reason));
                crate::render_prof::duration_since("focused_latency.total", focused_started);
                return true;
            }};
        }

        if !self.retained_pty_update_allowed_by_app_state() {
            focused_fallback!("unsafe_app_state");
        }
        self.compute_foreground_pane_geometry();

        let render_targets = render_targets(&self.clients, self.foreground_client_id);
        let app_target_count = render_targets
            .iter()
            .filter(|(_, _, _, _, mode)| matches!(mode, ClientConnectionMode::App))
            .count();
        if app_target_count == 0 {
            focused_fallback!("no_app_target");
        }

        let Some(canonical_snapshot) = self.last_app_frame.clone() else {
            focused_fallback!("no_last_frame");
        };
        if canonical_snapshot.active_size != self.effective_size {
            focused_fallback!("frame_size_mismatch");
        }
        if self.foreground_client_id != Some(canonical_snapshot.active_client_id) {
            focused_fallback!("active_client_mismatch");
        }

        let active_client_id = canonical_snapshot.active_client_id;
        let Some(active_client) = self.clients.get(&active_client_id) else {
            focused_fallback!("client_missing");
        };
        let active_cell_size = active_client.cell_size;
        if self.app.state.kitty_graphics_enabled && !active_client.graphics_cache.is_empty() {
            focused_fallback!("graphics_cache_active");
        }
        if active_client.graphics_surface_reset_pending {
            focused_fallback!("graphics_surface_reset");
        }
        if self.app.state.kitty_graphics_enabled
            && active_cell_size.is_known()
            && crate::kitty_graphics::has_visible_pane_graphics(
                &self.app.state,
                &self.app.terminal_runtimes,
                active_cell_size,
            )
        {
            focused_fallback!("visible_kitty_graphics");
        }

        let Some(ws_idx) = self.app.state.session_index() else {
            focused_fallback!("no_session");
        };
        let focused_pane_id = self
            .focused_input_pane_id
            .or_else(|| self.app.state.session().and_then(|ws| ws.focused_pane_id()));
        let Some(focused_pane_id) = focused_pane_id else {
            focused_fallback!("no_focused_pane");
        };
        let pane_infos = self.app.state.view.pane_infos.clone();
        let Some(focused_info) = pane_infos
            .iter()
            .find(|info| info.id == focused_pane_id)
            .cloned()
        else {
            focused_fallback!("focused_pane_not_visible");
        };

        let canonical_frame = canonical_snapshot.frame.as_ref();
        if !rect_fits_frame(focused_info.inner_rect, canonical_frame) {
            focused_fallback!("focused_rect_outside_frame");
        }

        let Some(focused_runtime) = self.app.state.runtime_for_pane_in_session_at(
            &self.app.terminal_runtimes,
            ws_idx,
            focused_pane_id,
        ) else {
            focused_fallback!("missing_focused_runtime");
        };
        let focused_patch = match focused_runtime.collect_dirty_patch(
            focused_info.inner_rect.width,
            focused_info.inner_rect.height,
        ) {
            crate::pane::TerminalDirtyPatchOutcome::Clean => None,
            crate::pane::TerminalDirtyPatchOutcome::Fallback => {
                focused_fallback!("focused_dirty_patch_fallback");
            }
            crate::pane::TerminalDirtyPatchOutcome::Patch(patch) => Some(patch),
        };

        let next_cursor = crate::server::render_stream::focused_terminal_cursor(
            &self.app.state,
            &self.app.terminal_runtimes,
        );

        let mut frame = canonical_snapshot.frame.as_ref().clone();
        frame.graphics.clear();
        let mut changed = false;
        if let Some(patch) = focused_patch {
            if dirty_patch_intersects_hyperlinks(canonical_frame, focused_info.inner_rect, &patch) {
                focused_fallback!("focused_hyperlink_intersection");
            }
            if !apply_terminal_dirty_patch(&mut frame, focused_info.inner_rect, patch) {
                focused_fallback!("focused_patch_apply_failed");
            }
            changed = true;
        }
        if frame.cursor != next_cursor {
            frame.cursor = next_cursor;
            changed = true;
        }

        if !changed {
            if self.latency_background_render_due(now) {
                self.clear_latency_background_dirty();
                focused_fallback!("background_due");
            }
            self.note_latency_background_skipped(now);
            focused_success!("focused_clean_background_deferred");
        }

        let patch_budget_started = Instant::now();
        let mut background_skipped = false;
        for info in pane_infos.iter().filter(|info| info.id != focused_pane_id) {
            if patch_budget_started.elapsed() >= LATENCY_OPPORTUNISTIC_PATCH_BUDGET {
                background_skipped = true;
                crate::render_prof::event("focused_latency.background_budget_exhausted");
                break;
            }
            if !rect_fits_frame(info.inner_rect, canonical_frame) {
                background_skipped = true;
                continue;
            }
            let Some(runtime) = self.app.state.runtime_for_pane_in_session_at(
                &self.app.terminal_runtimes,
                ws_idx,
                info.id,
            ) else {
                background_skipped = true;
                continue;
            };
            match runtime.try_collect_dirty_patch(info.inner_rect.width, info.inner_rect.height) {
                Some(crate::pane::TerminalDirtyPatchOutcome::Clean) => {
                    crate::render_prof::event("focused_latency.background_clean");
                }
                Some(crate::pane::TerminalDirtyPatchOutcome::Patch(patch)) => {
                    if dirty_patch_intersects_hyperlinks(canonical_frame, info.inner_rect, &patch) {
                        focused_fallback!("background_hyperlink_intersection");
                    }
                    if !apply_terminal_dirty_patch(&mut frame, info.inner_rect, patch) {
                        focused_fallback!("background_patch_apply_failed");
                    }
                    changed = true;
                    crate::render_prof::event("focused_latency.background_patch");
                }
                Some(crate::pane::TerminalDirtyPatchOutcome::Fallback) => {
                    background_skipped = true;
                    crate::render_prof::event("focused_latency.background_fallback_deferred");
                }
                None => {
                    background_skipped = true;
                    crate::render_prof::event("focused_latency.background_lock_busy");
                }
            }
        }

        if background_skipped {
            self.note_latency_background_skipped(now);
        } else {
            self.clear_latency_background_dirty();
        }

        if !changed {
            focused_success!("clean_no_cursor_change");
        }

        let snapshot = self.store_app_frame_snapshot(frame, ServerRenderDebug::default());
        let mut focused_targets = render_targets
            .into_iter()
            .filter(|(_, _, _, _, mode)| matches!(mode, ClientConnectionMode::App))
            .collect::<Vec<_>>();
        if let Some(latency_client_id) = self.latency_critical_client_id {
            focused_targets.sort_by_key(|(client_id, _, _, is_foreground, _)| {
                if *client_id == latency_client_id {
                    0
                } else if *is_foreground {
                    1
                } else {
                    2
                }
            });
        }
        let target_count = focused_targets.len().min(usize::from(u16::MAX)) as u16;
        let mut broken_clients = Vec::new();
        let mut attempted_send = false;
        let mut sent_any = false;
        for (client_id, size, cell_size, is_foreground, _mode) in focused_targets {
            attempted_send = true;
            if self.stream_app_snapshot_to_client(
                client_id,
                snapshot.clone(),
                size,
                cell_size,
                is_foreground,
                ServerFrameDebugContext {
                    target_count,
                    ..Default::default()
                },
                &mut broken_clients,
                "focused_latency_send",
            ) {
                sent_any = true;
            }
        }
        for broken_client in broken_clients {
            self.remove_client_and_resize_if_needed(broken_client);
        }
        if !attempted_send {
            focused_success!("no_target");
        }
        if sent_any {
            self.latency_critical_client_id = None;
            focused_success!("sent");
        }
        focused_fallback!("send_failed");
    }

    fn render_retained_pty_update_and_stream(&mut self) -> bool {
        crate::render_prof::event("retained.attempt");
        let retained_started = crate::render_prof::timer();
        macro_rules! retained_fallback {
            ($reason:literal) => {{
                crate::render_prof::event(concat!("retained_fallback.", $reason));
                crate::render_prof::duration_since("retained.total", retained_started);
                return false;
            }};
        }
        macro_rules! retained_success {
            ($reason:literal) => {{
                crate::render_prof::event("retained.success");
                crate::render_prof::event(concat!("retained_success.", $reason));
                crate::render_prof::duration_since("retained.total", retained_started);
                return true;
            }};
        }

        if !self.retained_pty_update_allowed_by_app_state() {
            retained_fallback!("unsafe_app_state");
        }
        self.compute_foreground_pane_geometry();

        let render_targets = render_targets(&self.clients, self.foreground_client_id);
        if render_targets.is_empty() {
            retained_fallback!("no_target");
        }
        let app_target_count = render_targets
            .iter()
            .filter(|(_, _, _, _, mode)| matches!(mode, ClientConnectionMode::App))
            .count();
        if app_target_count == 0 {
            retained_fallback!("no_app_target");
        }
        let latency_critical_retained_client_id =
            self.latency_critical_client_id.filter(|client_id| {
                app_target_count > 1
                    && self
                        .clients
                        .get(client_id)
                        .is_some_and(|client| client.is_full_app_client())
                    && render_targets
                        .iter()
                        .any(|(target_client_id, _, _, _, mode)| {
                            target_client_id == client_id
                                && matches!(mode, ClientConnectionMode::App)
                        })
            });
        let Some(canonical_snapshot) = self.last_app_frame.clone() else {
            retained_fallback!("no_last_frame");
        };
        if canonical_snapshot.active_size != self.effective_size {
            retained_fallback!("frame_size_mismatch");
        }
        if self.foreground_client_id != Some(canonical_snapshot.active_client_id) {
            retained_fallback!("active_client_mismatch");
        }

        let active_client_id = canonical_snapshot.active_client_id;
        let Some(active_client) = self.clients.get(&active_client_id) else {
            retained_fallback!("client_missing");
        };
        let active_cell_size = active_client.cell_size;
        if self.app.state.kitty_graphics_enabled && !active_client.graphics_cache.is_empty() {
            retained_fallback!("graphics_cache_active");
        }
        if active_client.graphics_surface_reset_pending {
            retained_fallback!("graphics_surface_reset");
        }
        if self.app.state.kitty_graphics_enabled
            && active_cell_size.is_known()
            && crate::kitty_graphics::has_visible_pane_graphics(
                &self.app.state,
                &self.app.terminal_runtimes,
                active_cell_size,
            )
        {
            retained_fallback!("visible_kitty_graphics");
        }

        let Some(ws_idx) = self.app.state.session_index() else {
            retained_fallback!("no_session");
        };
        let pane_infos = self.app.state.view.pane_infos.clone();
        if pane_infos.is_empty() {
            retained_fallback!("no_pane_info");
        }

        let canonical_frame = canonical_snapshot.frame.as_ref();
        for info in &pane_infos {
            if !rect_fits_frame(info.inner_rect, canonical_frame) {
                retained_fallback!("pane_rect_outside_frame");
            }
        }

        let mut patches = Vec::new();
        for info in pane_infos {
            let Some(runtime) = self.app.state.runtime_for_pane_in_session_at(
                &self.app.terminal_runtimes,
                ws_idx,
                info.id,
            ) else {
                retained_fallback!("missing_runtime");
            };
            match runtime.collect_dirty_patch(info.inner_rect.width, info.inner_rect.height) {
                crate::pane::TerminalDirtyPatchOutcome::Clean => {
                    crate::render_prof::event("retained.pane_clean");
                }
                crate::pane::TerminalDirtyPatchOutcome::Fallback => {
                    retained_fallback!("dirty_patch_fallback");
                }
                crate::pane::TerminalDirtyPatchOutcome::Patch(patch) => {
                    crate::render_prof::event("retained.pane_patch");
                    crate::render_prof::counter("retained.patch_rows", patch.rows.len() as u64);
                    if dirty_patch_intersects_hyperlinks(canonical_frame, info.inner_rect, &patch) {
                        retained_fallback!("hyperlink_intersection");
                    }
                    patches.push((info.inner_rect, patch));
                }
            }
        }

        let next_cursor = crate::server::render_stream::focused_terminal_cursor(
            &self.app.state,
            &self.app.terminal_runtimes,
        );

        let mut broken_clients = Vec::new();
        let mut attempted_send = false;
        let mut sent_any = false;
        let mut frame = canonical_snapshot.frame.as_ref().clone();
        frame.graphics.clear();
        for (inner_rect, patch) in &patches {
            if !apply_terminal_dirty_patch(&mut frame, *inner_rect, patch.clone()) {
                retained_fallback!("patch_apply_failed");
            }
        }
        let cursor_changed = frame.cursor != next_cursor;
        frame.cursor = next_cursor;

        if patches.is_empty() && !cursor_changed {
            retained_success!("clean_no_cursor_change");
        }

        let snapshot = self.store_app_frame_snapshot(frame, ServerRenderDebug::default());
        let mut retained_targets = render_targets
            .into_iter()
            .filter(|(_, _, _, _, mode)| matches!(mode, ClientConnectionMode::App))
            .collect::<Vec<_>>();
        if let Some(latency_client_id) = latency_critical_retained_client_id {
            retained_targets.sort_by_key(|(client_id, _, _, is_foreground, _)| {
                if *client_id == latency_client_id {
                    0
                } else if *is_foreground {
                    1
                } else {
                    2
                }
            });
        }
        let target_count = retained_targets.len().min(usize::from(u16::MAX)) as u16;
        for (client_id, size, cell_size, is_foreground, _mode) in retained_targets {
            attempted_send = true;
            let sent = self.send_retained_snapshot_to_client(
                client_id,
                snapshot.clone(),
                size,
                cell_size,
                is_foreground,
                target_count,
                &mut broken_clients,
            );
            if sent {
                sent_any = true;
            }
        }
        for broken_client in broken_clients {
            self.remove_client_and_resize_if_needed(broken_client);
        }
        if !attempted_send {
            retained_success!("clean_no_cursor_change");
        }
        if sent_any {
            self.latency_critical_client_id = None;
            retained_success!("sent");
        }
        retained_fallback!("send_failed");
    }

    fn retained_pty_update_allowed_by_app_state(&self) -> bool {
        self.app.state.mode == app::Mode::Terminal
            && self.app.state.selection.is_none()
            && self.app.state.copy_mode.is_none()
            && self.app.state.context_menu.is_none()
            && self.app.state.toast.is_none()
            && !self.app.full_redraw_pending
    }

    fn send_retained_snapshot_to_client(
        &mut self,
        client_id: u64,
        snapshot: Arc<AppFrameSnapshot>,
        target_size: (u16, u16),
        cell_size: crate::kitty_graphics::HostCellSize,
        is_foreground: bool,
        target_count: u16,
        broken_clients: &mut Vec<u64>,
    ) -> bool {
        self.stream_app_snapshot_to_client(
            client_id,
            snapshot,
            target_size,
            cell_size,
            is_foreground,
            ServerFrameDebugContext {
                target_count,
                ..Default::default()
            },
            broken_clients,
            "retained_send",
        )
    }

    fn handle_client_render_publish(
        &mut self,
        client_id: u64,
        publish: ClientRenderPublish,
        commit_graphics_cache: bool,
        next_graphics_cache: crate::kitty_graphics::HostGraphicsCache,
        event_prefix: &str,
        broken_clients: &mut Vec<u64>,
    ) -> bool {
        match publish {
            ClientRenderPublish::Sent => {
                if commit_graphics_cache {
                    if let Some(client) = self.clients.get_mut(&client_id) {
                        client.graphics_cache = next_graphics_cache;
                        client.graphics_surface_reset_pending = false;
                    }
                }
                match event_prefix {
                    "retained_send" => crate::render_prof::event("retained_send.sent"),
                    _ => crate::render_prof::event("full_render.sent"),
                }
                true
            }
            ClientRenderPublish::SkippedUnchanged => {
                match event_prefix {
                    "retained_send" => crate::render_prof::event("retained_send.skip_identical"),
                    _ => crate::render_prof::event("full_render.skip_identical"),
                }
                true
            }
            ClientRenderPublish::Disconnected => {
                debug!(client_id, "client writer channel closed, marking as broken");
                broken_clients.push(client_id);
                match event_prefix {
                    "retained_send" => {
                        crate::render_prof::event("retained_send_fallback.writer_disconnected")
                    }
                    _ => crate::render_prof::event("full_render.writer_disconnected"),
                }
                false
            }
            ClientRenderPublish::Oversized => {
                match event_prefix {
                    "retained_send" => {
                        crate::render_prof::event("retained_send_fallback.serialize_oversized")
                    }
                    _ => crate::render_prof::event("full_render.serialize_oversized"),
                }
                false
            }
            ClientRenderPublish::SerializeError => {
                broken_clients.push(client_id);
                match event_prefix {
                    "retained_send" => {
                        crate::render_prof::event("retained_send_fallback.serialize_error")
                    }
                    _ => crate::render_prof::event("full_render.serialize_error"),
                }
                false
            }
        }
    }

    fn store_app_frame_snapshot(
        &mut self,
        frame: FrameData,
        debug: ServerRenderDebug,
    ) -> Arc<AppFrameSnapshot> {
        let generation = self.next_app_snapshot_generation;
        self.next_app_snapshot_generation = self.next_app_snapshot_generation.saturating_add(1);
        let snapshot = Arc::new(AppFrameSnapshot::new(
            generation,
            self.foreground_client_id.unwrap_or_default(),
            frame,
            debug,
        ));
        self.last_app_frame = Some(snapshot.clone());
        snapshot
    }

    fn render_shared_app_snapshot(&mut self) -> Arc<AppFrameSnapshot> {
        let (cols, rows) = self.effective_size;
        let area = Rect::new(0, 0, cols, rows);
        let active_client_id = self.foreground_client_id.unwrap_or_default();
        let cell_size = self
            .clients
            .get(&active_client_id)
            .map(|client| client.cell_size)
            .unwrap_or_default();
        let render_cell_size = if self.app.state.kitty_graphics_enabled && cell_size.is_known() {
            cell_size
        } else {
            crate::kitty_graphics::HostCellSize::default()
        };
        let debug_render_started = Instant::now();
        let render_started = crate::render_prof::timer();
        let (buffer, cursor) = crate::server::render_stream::render_virtual_with_runtime_registry(
            &mut self.app.state,
            &self.app.terminal_runtimes,
            area,
            true,
            render_cell_size,
        );
        let render_duration = debug_render_started.elapsed();
        crate::render_prof::duration_since("full_render.render_virtual", render_started);
        let debug_frame_build_started = Instant::now();
        let hyperlinks_started = crate::render_prof::timer();
        let hyperlinks = crate::server::render_stream::visible_hyperlinks(
            &self.app.state,
            &self.app.terminal_runtimes,
        );
        crate::render_prof::duration_since("full_render.visible_hyperlinks", hyperlinks_started);
        let decorations_started = crate::render_prof::timer();
        let decorations = crate::server::render_stream::visible_decorations(
            &self.app.state,
            &self.app.terminal_runtimes,
        );
        crate::render_prof::duration_since("full_render.visible_decorations", decorations_started);
        let frame_started = crate::render_prof::timer();
        let frame = FrameData::from_ratatui_buffer_with_hyperlinks_and_decorations(
            &buffer,
            cursor,
            &hyperlinks,
            &decorations,
        );
        crate::render_prof::duration_since("full_render.frame_build", frame_started);
        let frame_build_duration = debug_frame_build_started.elapsed();
        self.store_app_frame_snapshot(
            frame,
            ServerRenderDebug {
                render_us: Some(debug_duration_us(render_duration)),
                frame_build_us: Some(debug_duration_us(frame_build_duration)),
            },
        )
    }

    fn stream_app_snapshot_to_client(
        &mut self,
        client_id: u64,
        snapshot: Arc<AppFrameSnapshot>,
        target_size: (u16, u16),
        cell_size: crate::kitty_graphics::HostCellSize,
        allow_graphics: bool,
        debug_context: ServerFrameDebugContext,
        broken_clients: &mut Vec<u64>,
        event_prefix: &str,
    ) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            match event_prefix {
                "retained_send" => {
                    crate::render_prof::event("retained_send_fallback.client_missing")
                }
                _ => crate::render_prof::event("full_render.client_missing"),
            }
            return false;
        };
        let mut next_graphics_cache = client.graphics_cache.clone();
        let graphics_surface_reset_pending = client.graphics_surface_reset_pending;

        let mut graphics_us = None;
        let mut direct_frame = None;
        if allow_graphics && self.app.state.kitty_graphics_enabled && cell_size.is_known() {
            let mut frame = snapshot.frame.as_ref().clone();
            if graphics_surface_reset_pending {
                frame.graphics = next_graphics_cache.clear_bytes();
            }
            let debug_graphics_started = Instant::now();
            let graphics_started = crate::render_prof::timer();
            frame
                .graphics
                .extend(crate::kitty_graphics::encode_local_pane_graphics(
                    &self.app.state,
                    &self.app.terminal_runtimes,
                    cell_size,
                    &mut next_graphics_cache,
                ));
            graphics_us = Some(debug_duration_us(debug_graphics_started.elapsed()));
            crate::render_prof::duration_since("full_render.graphics_encode", graphics_started);
            direct_frame = Some(frame);
        } else if graphics_surface_reset_pending || !next_graphics_cache.is_empty() {
            let mut frame = snapshot.frame.as_ref().clone();
            frame.graphics = next_graphics_cache.clear_bytes();
            direct_frame = Some(frame);
        }

        let mut commit_graphics_cache = direct_frame.is_some();
        if let Some(frame) = direct_frame.as_mut() {
            if frame.graphics.len() > MAX_GRAPHICS_FRAME_SIZE {
                warn!(
                    client_id,
                    graphics_bytes = frame.graphics.len(),
                    max = MAX_GRAPHICS_FRAME_SIZE,
                    "dropping oversized graphics payload for client frame"
                );
                frame.graphics.clear();
                commit_graphics_cache = false;
            }
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            match event_prefix {
                "retained_send" => {
                    crate::render_prof::event("retained_send_fallback.client_missing")
                }
                _ => crate::render_prof::event("full_render.client_missing"),
            }
            return false;
        };
        let mut debug_timing = client.take_frame_debug_timing(Instant::now());
        if let Some(timing) = &mut debug_timing {
            debug_context.apply_to(timing);
        }
        let Some(render_actor) = client.render_actor.as_mut() else {
            match event_prefix {
                "retained_send" => {
                    crate::render_prof::event("retained_send_fallback.writer_missing")
                }
                _ => crate::render_prof::event("full_render.writer_missing"),
            }
            return false;
        };
        let publish = if let Some(frame) = direct_frame {
            render_actor.publish_frame(
                client_id,
                frame,
                target_size,
                debug_timing,
                ClientRenderDebugContext {
                    graphics_us,
                    prepare_us: None,
                },
            )
        } else {
            render_actor.publish_snapshot(
                client_id,
                snapshot,
                target_size,
                debug_timing,
                ClientRenderDebugContext {
                    graphics_us,
                    prepare_us: None,
                },
            )
        };
        self.handle_client_render_publish(
            client_id,
            publish,
            commit_graphics_cache,
            next_graphics_cache,
            event_prefix,
            broken_clients,
        )
    }

    fn stream_frame_to_client(
        &mut self,
        client_id: u64,
        mut frame: FrameData,
        is_app_client: bool,
        cell_size: crate::kitty_graphics::HostCellSize,
        allow_graphics: bool,
        debug_context: ServerFrameDebugContext,
        broken_clients: &mut Vec<u64>,
    ) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let mut next_graphics_cache = client.graphics_cache.clone();
        let graphics_surface_reset_pending = client.graphics_surface_reset_pending;

        let mut graphics_us = None;
        if is_app_client
            && allow_graphics
            && self.app.state.kitty_graphics_enabled
            && cell_size.is_known()
        {
            if graphics_surface_reset_pending {
                frame.graphics = next_graphics_cache.clear_bytes();
            }
            let debug_graphics_started = Instant::now();
            let graphics_started = crate::render_prof::timer();
            frame
                .graphics
                .extend(crate::kitty_graphics::encode_local_pane_graphics(
                    &self.app.state,
                    &self.app.terminal_runtimes,
                    cell_size,
                    &mut next_graphics_cache,
                ));
            graphics_us = Some(debug_duration_us(debug_graphics_started.elapsed()));
            crate::render_prof::duration_since("full_render.graphics_encode", graphics_started);
        } else {
            frame.graphics = next_graphics_cache.clear_bytes();
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            return false;
        };
        let mut commit_graphics_cache = true;
        if frame.graphics.len() > MAX_GRAPHICS_FRAME_SIZE {
            warn!(
                client_id,
                graphics_bytes = frame.graphics.len(),
                max = MAX_GRAPHICS_FRAME_SIZE,
                "dropping oversized graphics payload for client frame"
            );
            frame.graphics.clear();
            commit_graphics_cache = false;
        }

        let mut debug_timing = client.take_frame_debug_timing(Instant::now());
        if let Some(timing) = &mut debug_timing {
            debug_context.apply_to(timing);
        }
        let Some(render_actor) = client.render_actor.as_mut() else {
            crate::render_prof::event("full_render.writer_missing");
            return false;
        };
        match render_actor.publish_frame(
            client_id,
            frame,
            client.terminal_size,
            debug_timing,
            ClientRenderDebugContext {
                graphics_us,
                prepare_us: None,
            },
        ) {
            ClientRenderPublish::SkippedUnchanged => {
                crate::render_prof::event("full_render.skip_identical");
                true
            }
            ClientRenderPublish::Oversized => {
                crate::render_prof::event("full_render.serialize_oversized");
                false
            }
            ClientRenderPublish::SerializeError => {
                broken_clients.push(client_id);
                crate::render_prof::event("full_render.serialize_error");
                false
            }
            ClientRenderPublish::Sent => {
                if commit_graphics_cache {
                    client.graphics_cache = next_graphics_cache;
                    client.graphics_surface_reset_pending = false;
                }
                crate::render_prof::event("full_render.sent");
                true
            }
            ClientRenderPublish::Disconnected => {
                debug!(client_id, "client writer channel closed, marking as broken");
                broken_clients.push(client_id);
                crate::render_prof::event("full_render.writer_disconnected");
                false
            }
        }
    }

    fn render_and_stream(&mut self) {
        let full_started = crate::render_prof::timer();
        let _ = self.latency_critical_client_id.take();
        let render_targets = render_targets(&self.clients, self.foreground_client_id);
        let target_count = render_targets.len().min(usize::from(u16::MAX)) as u16;

        if render_targets.is_empty() {
            let (cols, rows) = self.effective_size;
            let area = Rect::new(0, 0, cols, rows);
            let resize_panes = self.app.state.view.pane_infos.is_empty();
            let render_started = crate::render_prof::timer();
            let _ = crate::server::render_stream::render_virtual_with_runtime_registry(
                &mut self.app.state,
                &self.app.terminal_runtimes,
                area,
                resize_panes,
                crate::kitty_graphics::HostCellSize::default(),
            );
            crate::render_prof::duration_since("full_render.render_virtual", render_started);
            self.app.full_redraw_pending = false;
            crate::render_prof::duration_since("full_render.total", full_started);
            debug!(
                cols,
                rows, resize_panes, "rendered virtual frame with no attached clients"
            );
            return;
        }

        let has_app_targets = render_targets
            .iter()
            .any(|(_, _, _, _, mode)| matches!(mode, ClientConnectionMode::App));
        let shared_app_render = if has_app_targets {
            Some(self.render_shared_app_snapshot())
        } else {
            None
        };

        let mut broken_clients: Vec<u64> = Vec::new();
        for (client_id, (cols, rows), cell_size, _is_foreground, mode) in render_targets {
            let mut debug_context = ServerFrameDebugContext {
                target_count,
                ..Default::default()
            };
            let frame = match mode {
                ClientConnectionMode::App => {
                    let snapshot = match shared_app_render.as_ref() {
                        Some(snapshot) => snapshot,
                        None => continue,
                    };
                    debug_assert_eq!(
                        snapshot.active_size,
                        (snapshot.frame.width, snapshot.frame.height)
                    );
                    debug_assert_eq!(
                        Some(snapshot.active_client_id),
                        self.foreground_client_id.or(Some(0))
                    );
                    let _snapshot_age = snapshot.created_at.elapsed();
                    crate::render_prof::counter("app_snapshot.generation", snapshot.generation);
                    debug_context.render = snapshot.debug;
                    self.stream_app_snapshot_to_client(
                        client_id,
                        snapshot.clone(),
                        (cols, rows),
                        cell_size,
                        Some(client_id) == self.foreground_client_id,
                        debug_context,
                        &mut broken_clients,
                        "full_render",
                    );
                    continue;
                }
                ClientConnectionMode::TerminalAttach { terminal_id } => {
                    let Some(runtime) = self.runtime_for_terminal_id_string(&terminal_id) else {
                        self.send_to_client(
                            client_id,
                            ServerMessage::ServerShutdown {
                                reason: Some(format!(
                                    "terminal attach ended: terminal {terminal_id} not found"
                                )),
                            },
                        );
                        broken_clients.push(client_id);
                        continue;
                    };
                    let area = Rect::new(0, 0, cols, rows);
                    let debug_render_started = Instant::now();
                    let render_started = crate::render_prof::timer();
                    let (buffer, cursor) =
                        crate::server::render_stream::render_terminal_virtual(runtime, area);
                    let render_duration = debug_render_started.elapsed();
                    crate::render_prof::duration_since(
                        "full_render.render_terminal_virtual",
                        render_started,
                    );
                    let debug_frame_build_started = Instant::now();
                    let hyperlinks_started = crate::render_prof::timer();
                    let hyperlinks = runtime.visible_hyperlinks(area);
                    crate::render_prof::duration_since(
                        "full_render.visible_hyperlinks",
                        hyperlinks_started,
                    );
                    let decorations_started = crate::render_prof::timer();
                    let decorations = runtime.visible_decorations(area);
                    crate::render_prof::duration_since(
                        "full_render.visible_decorations",
                        decorations_started,
                    );
                    let frame_started = crate::render_prof::timer();
                    let frame = FrameData::from_ratatui_buffer_with_hyperlinks_and_decorations(
                        &buffer,
                        cursor,
                        &hyperlinks,
                        &decorations,
                    );
                    crate::render_prof::duration_since("full_render.frame_build", frame_started);
                    debug_context.render = ServerRenderDebug {
                        render_us: Some(debug_duration_us(render_duration)),
                        frame_build_us: Some(debug_duration_us(
                            debug_frame_build_started.elapsed(),
                        )),
                    };
                    frame
                }
            };
            self.stream_frame_to_client(
                client_id,
                frame,
                false,
                cell_size,
                false,
                debug_context,
                &mut broken_clients,
            );
        }

        if !broken_clients.is_empty() {
            for client_id in broken_clients {
                self.remove_client_and_resize_if_needed(client_id);
            }
        }
        self.compute_foreground_view_geometry();

        let (cols, rows) = self.effective_size;
        self.app.full_redraw_pending = false;
        crate::render_prof::duration_since("full_render.total", full_started);
        debug!(cols, rows, foreground_client_id = ?self.foreground_client_id, "rendered virtual frame(s)");
    }

    /// Handle scheduled tasks for the headless server.
    ///
    /// Similar to `App::handle_scheduled_tasks` but without resize polling
    /// (the server doesn't have a terminal to resize).
    fn handle_scheduled_tasks_headless(&mut self, now: Instant, _geometry_dirty: bool) -> bool {
        let mut changed = false;

        self.app.sync_headless_animation_timer(now);

        // No resize polling needed — server has no terminal.
        // Client resize messages drive size changes instead.

        if self
            .app
            .config_diagnostic_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.config_diagnostic_deadline = None;
            self.app.state.config_diagnostic = None;
            changed = true;
        }

        if self
            .app
            .toast_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.toast_deadline = None;
            self.app.state.toast = None;
            changed = true;
        }

        if self
            .app
            .copy_feedback_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.copy_feedback_deadline = None;
            self.app.state.copy_feedback = None;
            changed = true;
        }

        if self
            .app
            .next_animation_tick
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.state.spinner_tick = self
                .app
                .state
                .spinner_tick
                .wrapping_add(app::HEADLESS_ANIMATION_TICK_STEP);
            self.app.next_animation_tick = Some(now + app::HEADLESS_ANIMATION_INTERVAL);
            changed = true;
        }

        if self
            .app
            .selection_autoscroll_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.tick_selection_autoscroll(now);
            changed = true;
        }

        changed |= self.app.clear_due_selection_highlight(now);

        if self
            .app
            .session_save_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.save_session_now();
        }

        self.app.sync_headless_animation_timer(now);
        changed
    }

    /// Initiates graceful shutdown.
    fn initiate_shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        info!("server shutdown initiated");
        self.shutting_down = true;

        // Clear client-local host graphics, then send ServerShutdown to all connected clients.
        self.send_all_clients_graphics_cleanup();
        let shutdown_msg = ServerMessage::ServerShutdown {
            reason: Some("server is shutting down".to_owned()),
        };
        self.send_to_all_clients(shutdown_msg);

        // Give client writer threads a moment to flush the shutdown message.
        // A short sleep ensures the message is written to the socket before
        // we close the connections.
        std::thread::sleep(Duration::from_millis(50));

        // Signal the main loop to exit.
        self.should_quit.store(true, Ordering::Release);
        self.app.state.should_quit = true;
    }

    /// Completes the shutdown sequence: send ServerShutdown to clients,
    /// close client connections, remove socket files, and clean up.
    fn complete_shutdown(&mut self) -> io::Result<()> {
        info!("completing server shutdown");

        // Send ServerShutdown to all remaining clients.
        if !self.clients.is_empty() {
            self.send_all_clients_graphics_cleanup();
            let shutdown_msg = ServerMessage::ServerShutdown {
                reason: Some("server is shutting down".to_owned()),
            };
            self.send_to_all_clients(shutdown_msg);

            // Give writer threads a moment to flush before closing.
            std::thread::sleep(Duration::from_millis(50));
        }

        // Drain remaining API requests with server_unavailable.
        self.drain_api_requests_with_shutdown_check();

        // Close all client connections.
        let staged_files = self
            .clients
            .drain()
            .flat_map(|(_, client)| client.staged_clipboard_files)
            .collect::<Vec<_>>();
        crate::server::clipboard_image::remove_files(staged_files);

        // Remove socket files.
        self.cleanup_sockets()?;

        Ok(())
    }

    /// Removes socket files created by the server.
    fn cleanup_sockets(&self) -> io::Result<()> {
        if let Err(err) =
            remove_socket_file_if_owned(&self.client_socket_path, self.client_socket_identity)
        {
            if err.kind() != io::ErrorKind::NotFound {
                warn!(
                    path = %self.client_socket_path.display(),
                    err = %err,
                    "failed to remove client socket on shutdown"
                );
            }
        }
        Ok(())
    }
}

impl Drop for HeadlessServer {
    fn drop(&mut self) {
        let staged_files = self
            .clients
            .drain()
            .flat_map(|(_, client)| client.staged_clipboard_files)
            .collect::<Vec<_>>();
        crate::server::clipboard_image::remove_files(staged_files);
        let _ = self.cleanup_sockets();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Installs a Ctrl+C handler that sets the should_quit flag and wakes up
/// the event loop by sending a QuitSignal on the server event channel.
fn ctrlc_handler(should_quit: Arc<AtomicBool>, server_event_tx: mpsc::Sender<ServerEvent>) {
    let _ = ctrlc::set_handler(move || {
        should_quit.store(true, Ordering::Release);
        // Wake up the event loop so the quit flag is checked promptly.
        let _ = server_event_tx.try_send(ServerEvent::QuitSignal);
    });
}

/// Sleep until a deadline, or return pending if none.
async fn sleep_until_or_pending(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
        None => std::future::pending().await,
    }
}

fn server_config_diagnostic_summaries(diagnostics: &[String]) -> (Option<String>, Option<String>) {
    let without_keybindings = diagnostics
        .iter()
        .filter(|diagnostic| !is_keybinding_config_diagnostic(diagnostic))
        .cloned()
        .collect::<Vec<_>>();
    (
        config::config_diagnostic_summary(diagnostics),
        config::config_diagnostic_summary(&without_keybindings),
    )
}

fn is_keybinding_config_diagnostic(diagnostic: &str) -> bool {
    diagnostic.contains("keybinding") || diagnostic.contains("keys.")
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the headless server. This is the entry point called from main.rs.
pub fn run_server() -> io::Result<()> {
    init_logging();
    crate::platform::raise_server_nofile_limit();

    let args: Vec<String> = std::env::args().collect();
    if args.get(2).map(String::as_str) == Some("--handoff-import") {
        let socket_path = args
            .get(3)
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing handoff socket"))?;
        let token = args
            .get(4)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing handoff token"))?;
        return run_handoff_import_server(&socket_path, token);
    }

    let loaded_config = config::Config::load();
    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_hub = api::EventHub::default();

    // Start the JSON API socket server.
    let _api_server = match api::start_server(api_tx.clone(), event_hub.clone()) {
        Ok(server) => server,
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            eprintln!("error: gmux server is already running");
            eprintln!("api socket: {}", api::socket_path().display());
            std::process::exit(1);
        }
        Err(err) => return Err(err),
    };

    let no_session = false; // Server always does session persistence.

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io::Error::other)?;

    let result = rt.block_on(async {
        // Create the App (with AppState, event channels, etc.).
        let mut app = app::App::new(
            &loaded_config.config,
            no_session,
            config::config_diagnostic_summary(&loaded_config.diagnostics),
            api_rx,
            event_hub,
        );

        // The server runs headless — disable local terminal notification side effects.
        // Terminal notifications are forwarded to connected clients as
        // ServerMessage::Notify instead of emitted by the server process.
        app.local_terminal_notifications = false;
        app.start_update_check();

        // Create the headless server.
        let mut server = match HeadlessServer::new(
            app,
            &loaded_config.diagnostics,
            Some(api_tx.clone()),
            Some(_api_server),
        ) {
            Ok(server) => server,
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
                eprintln!("error: gmux server is already running");
                eprintln!("client socket: {}", client_socket_path().display());
                std::process::exit(1);
            }
            Err(err) => return Err(err),
        };

        info!(
            api_socket = %api::socket_path().display(),
            client_socket = %client_socket_path().display(),
            "gmux server started"
        );
        print_ready_message(&api::socket_path(), &client_socket_path());

        server.run().await
    });

    rt.shutdown_timeout(Duration::from_millis(100));
    crate::logging::shutdown("server");
    result
}

#[cfg(unix)]
fn run_handoff_import_server(socket_path: &Path, token: &str) -> io::Result<()> {
    let loaded_config = config::Config::load();
    let mut received = crate::server::handoff::receive(socket_path, token)?;
    crate::server::handoff::log_import_result(received.manifest.panes.len());

    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_hub = api::EventHub::default();

    let mut imports = HashMap::new();
    for (pane, fd) in received.manifest.panes.into_iter().zip(received.fds) {
        let pane_id = pane.pane_id;
        imports.insert(
            pane_id,
            crate::handoff_runtime::ImportedHandoffRuntime {
                master_fd: fd,
                state: pane,
            },
        );
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io::Error::other)?;

    let result = rt.block_on(async {
        let mut app = app::App::new_from_handoff(
            &loaded_config.config,
            config::config_diagnostic_summary(&loaded_config.diagnostics),
            api_rx,
            event_hub.clone(),
            &received.manifest.snapshot,
            &mut imports,
        )?;
        app.local_terminal_notifications = false;
        crate::server::handoff::report_restored(&mut received.stream)?;
        if std::env::var("GMUX_TEST_HANDOFF_IMPORT_FAIL").as_deref() == Ok("after_restored") {
            return Err(io::Error::other(
                "test handoff import failure after restored",
            ));
        }
        wait_for_old_public_sockets_to_close(Duration::from_secs(5))?;

        let api_server = api::start_server(api_tx.clone(), event_hub.clone())?;
        let mut server = HeadlessServer::new(
            app,
            &loaded_config.diagnostics,
            Some(api_tx.clone()),
            Some(api_server),
        )?;
        crate::server::handoff::report_ready(&mut received.stream)?;
        crate::server::handoff::wait_committed(&mut received.stream)?;
        server.app.assume_handoff_ownership();
        server.app.unpause_handoff_readers();
        server.pending_handoff_repaint_nudge = true;
        if let Err(err) = crate::server::handoff::report_owned(&mut received.stream) {
            warn!(err = %err, "failed to report handoff ownership; continuing as owner");
        }
        info!("handoff import server started");
        print_ready_message(&api::socket_path(), &client_socket_path());
        server.run().await
    });

    rt.shutdown_timeout(Duration::from_millis(100));
    crate::logging::shutdown("server");
    result
}

#[cfg(unix)]
fn wait_for_old_public_sockets_to_close(timeout: Duration) -> io::Result<()> {
    use std::os::unix::net::UnixStream;

    let deadline = Instant::now() + timeout;
    let api_socket = api::socket_path();
    let client_socket = client_socket_path();
    while Instant::now() < deadline {
        let api_open = api_socket.exists() && UnixStream::connect(&api_socket).is_ok();
        let client_open = client_socket.exists() && UnixStream::connect(&client_socket).is_ok();
        if !api_open && !client_open {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "old server sockets did not close before handoff import bind",
    ))
}

#[cfg(not(unix))]
fn run_handoff_import_server(_socket_path: &Path, _token: &str) -> io::Result<()> {
    Err(io::Error::other("live handoff is only supported on Unix"))
}

fn print_ready_message(api_socket: &Path, client_socket: &Path) {
    eprintln!("gmux server running; you can use any gmux CLI command in another terminal.");
    eprintln!("api socket: {}", api_socket.display());
    eprintln!("client socket: {}", client_socket.display());
    eprintln!(
        "logs: {}",
        crate::session::data_dir().join("gmux-server.log").display()
    );
    eprintln!("did you mean to open the Gmux TUI? run `gmux`; you do not need `gmux server`.");
}

/// Initialize logging for the server process.
fn init_logging() {
    crate::logging::init_file_logging("gmux-server.log");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::AppState;
    use crate::protocol::CursorState;
    use std::sync::atomic::AtomicU64;

    static TEST_SERVER_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_headless_server() -> HeadlessServer {
        let config = crate::config::Config::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(&config, true, None, api_rx, api::EventHub::default());
        app.local_terminal_notifications = false;

        let dir = std::path::PathBuf::from("/tmp").join(format!(
            "hh-{}-{}-{}",
            std::process::id(),
            TEST_SERVER_COUNTER.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&dir);
        let socket_path = dir.join("client.sock");
        let _ = fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).expect("bind test listener");
        let client_socket_identity =
            socket_file_identity(&socket_path).expect("test listener socket identity");
        listener
            .set_nonblocking(true)
            .expect("set listener nonblocking");
        let (server_event_tx, server_event_rx) = mpsc::channel(64);
        let (server_input_tx, server_input_rx) = mpsc::unbounded_channel();
        let server_keybindings = app_keybindings(&app);

        HeadlessServer {
            app,
            api_tx: None,
            api_server: None,
            client_listener: listener,
            client_socket_path: socket_path,
            client_socket_identity,
            clients: HashMap::new(),
            next_client_id: 1,
            foreground_client_id: None,
            server_keybindings,
            server_config_diagnostic: None,
            server_config_diagnostic_without_keybindings: None,
            terminal_attach_owners: HashMap::new(),
            next_activity_stamp: 1,
            effective_size: (MIN_COLS, MIN_ROWS),
            latency_critical_client_id: None,
            next_app_snapshot_generation: 1,
            last_app_frame: None,
            shutting_down: false,
            handoff_in_progress: false,
            pending_handoff_repaint_nudge: false,
            should_quit: Arc::new(AtomicBool::new(false)),
            server_event_rx,
            server_event_tx,
            server_input_rx,
            server_input_tx,
            focused_input_pane_id: None,
            focused_input_deadline: None,
            latency_background_render_at: None,
            latency_background_dirty: false,
        }
    }

    fn read_server_message(bytes: Vec<u8>) -> ServerMessage {
        let mut cursor = std::io::Cursor::new(bytes);
        protocol::read_message(&mut cursor, MAX_FRAME_SIZE).expect("decode server message")
    }

    fn read_server_frame(bytes: Vec<u8>) -> FrameData {
        match read_server_message(bytes) {
            ServerMessage::Frame(frame) => frame,
            other => panic!("expected frame, got {other:?}"),
        }
    }

    fn recv_server_frame_within(
        rx: &LatestRenderReceiver,
        timeout: Duration,
        label: &str,
    ) -> FrameData {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            assert!(now < deadline, "timed out waiting for {label}");
            let remaining = deadline.saturating_duration_since(now);
            match read_server_message(rx.recv_timeout(remaining).expect(label)) {
                ServerMessage::Frame(frame) => return frame,
                ServerMessage::Terminal(_) => panic!("expected frame for {label}, got terminal"),
                _ => continue,
            }
        }
    }

    fn test_client_input(client_id: u64, data: impl Into<Vec<u8>>) -> ServerEvent {
        ServerEvent::ClientInput {
            client_id,
            data: data.into(),
            received_at: Instant::now(),
        }
    }

    fn test_attach_wheel_scroll(
        client_id: u64,
        direction: AttachScrollDirection,
        lines: u16,
    ) -> ServerEvent {
        ServerEvent::ClientAttachScroll {
            client_id,
            source: AttachScrollSource::Wheel,
            direction,
            lines,
            column: Some(0),
            row: Some(0),
            modifiers: 0,
        }
    }

    fn frame_text(frame: &FrameData) -> String {
        frame
            .cells
            .chunks(usize::from(frame.width))
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn read_server_shutdown_reason(bytes: Vec<u8>) -> Option<String> {
        match read_server_message(bytes) {
            ServerMessage::ServerShutdown { reason } => reason,
            other => panic!("expected shutdown, got {other:?}"),
        }
    }

    #[test]
    fn headless_api_request_drains_all_pending_internal_events_before_reading_state() {
        let mut server = test_headless_server();
        for i in 0..=crate::app::APP_EVENT_DRAIN_LIMIT {
            server
                .app
                .event_tx
                .try_send(AppEvent::ClipboardWrite {
                    content: vec![i as u8],
                })
                .unwrap();
        }

        let (respond_to, response_rx) = std::sync::mpsc::channel();
        assert!(
            server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
                request: api::schema::Request {
                    id: "headless_stop_after_events".into(),
                    method: api::schema::Method::ServerStop(api::schema::EmptyParams::default()),
                },
                respond_to,
            })
        );
        let response = response_rx
            .recv_timeout(Duration::from_millis(100))
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert!(server.app.event_rx.try_recv().is_err());
    }

    fn test_client_writer() -> (
        ClientWriter,
        std::sync::mpsc::Receiver<Vec<u8>>,
        LatestRenderReceiver,
    ) {
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let (render_tx, render_rx) = LatestRenderSender::channel();
        (
            ClientWriter {
                control: control_tx,
                render: render_tx,
            },
            control_rx,
            render_rx,
        )
    }

    fn retained_test_server(
        initial_screen: &[u8],
    ) -> (HeadlessServer, LatestRenderReceiver, crate::layout::PaneId) {
        retained_test_server_with_size(initial_screen, (80, 24))
    }

    fn retained_test_server_with_size(
        initial_screen: &[u8],
        size: (u16, u16),
    ) -> (HeadlessServer, LatestRenderReceiver, crate::layout::PaneId) {
        let mut server = test_headless_server();
        let mut workspace = crate::workspace::Workspace::test_new("test");
        let pane_id = workspace.focused_pane_id().expect("focused pane");
        workspace.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(
                size.0,
                size.1,
                initial_screen,
            ),
        );
        server.app.state.sessions = vec![workspace];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        let (client_tx, _client_control_rx, client_rx) = test_client_writer();
        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);
        server.clients.get_mut(&1).unwrap().terminal_size = size;
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();

        (server, client_rx, pane_id)
    }

    fn interaction_test_server_with_size(
        initial_screen: &[u8],
        size: (u16, u16),
    ) -> (
        HeadlessServer,
        LatestRenderReceiver,
        crate::layout::PaneId,
        mpsc::Receiver<Bytes>,
    ) {
        let mut server = test_headless_server();
        let mut workspace = crate::workspace::Workspace::test_new("test");
        let pane_id = workspace.focused_pane_id().expect("focused pane");
        let (runtime, pane_input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                size.0,
                size.1,
                1_000_000,
                initial_screen,
                1024,
            );
        workspace.insert_test_runtime(pane_id, runtime);
        server.app.state.sessions = vec![workspace];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        let (client_tx, _client_control_rx, client_rx) = test_client_writer();
        server.clients.insert(
            1,
            ClientConnection::new(
                size,
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);
        server.clients.get_mut(&1).unwrap().terminal_size = size;
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();

        (server, client_rx, pane_id, pane_input_rx)
    }

    fn bench_env_u16(name: &str, default: u16) -> u16 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(default)
    }

    fn bench_env_usize(name: &str, default: usize) -> usize {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(default)
    }

    fn bench_sorted(values: &[u64]) -> Vec<u64> {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        sorted
    }

    fn bench_average(values: &[u64]) -> u64 {
        if values.is_empty() {
            return 0;
        }
        values.iter().sum::<u64>() / values.len() as u64
    }

    fn bench_percentile(sorted_values: &[u64], percentile: usize) -> u64 {
        if sorted_values.is_empty() {
            return 0;
        }
        let index = ((sorted_values.len() - 1) * percentile) / 100;
        sorted_values[index]
    }

    fn print_latency_bench_stats(label: &str, values: &[u64]) {
        let sorted = bench_sorted(values);
        println!(
            "{label}: avg={}us p50={}us p95={}us max={}us",
            bench_average(values),
            bench_percentile(&sorted, 50),
            bench_percentile(&sorted, 95),
            sorted.last().copied().unwrap_or_default()
        );
    }

    #[tokio::test]
    #[ignore = "interaction benchmark; run with `just bench-input-latency`"]
    async fn held_key_interaction_latency_benchmark() {
        let cols = bench_env_u16("GMUX_INTERACTION_BENCH_COLS", 160);
        let rows = bench_env_u16("GMUX_INTERACTION_BENCH_ROWS", 48);
        let frames = bench_env_usize("GMUX_INTERACTION_BENCH_FRAMES", 240);
        let initial_screen = b"gmux interaction latency benchmark\r\n$ ";
        let (mut server, client_rx, pane_id, mut pane_input_rx) =
            interaction_test_server_with_size(initial_screen, (cols, rows));

        server.render_and_stream();
        let _ = recv_server_frame_within(
            &client_rx,
            Duration::from_secs(1),
            "initial interaction benchmark frame",
        );
        server
            .clients
            .get(&1)
            .unwrap()
            .render_actor
            .as_ref()
            .unwrap()
            .wait_for_last_frame(Duration::from_secs(1))
            .expect("initial interaction benchmark baseline");

        let mut total_loop_us = Vec::with_capacity(frames);
        let mut input_queue_us = Vec::with_capacity(frames);
        let mut input_to_frame_us = Vec::with_capacity(frames);
        let mut dirty_to_frame_us = Vec::with_capacity(frames);
        let mut render_us = Vec::with_capacity(frames);
        let mut frame_build_us = Vec::with_capacity(frames);
        let mut prepare_us = Vec::with_capacity(frames);
        let mut checksum = 0usize;

        for _ in 0..frames {
            let loop_started = Instant::now();
            let _ = server.handle_server_event(test_client_input(1, b"a"));
            let echoed = pane_input_rx
                .try_recv()
                .expect("pane should receive held-key input");
            server
                .app
                .state
                .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
                .expect("runtime")
                .test_process_pty_bytes(&echoed);
            server.record_debug_pty_dirty_for_pending_inputs(Instant::now());
            server.render_and_stream();
            let frame = recv_server_frame_within(
                &client_rx,
                Duration::from_secs(1),
                "held-key interaction benchmark frame",
            );
            let timing = frame
                .debug_timing
                .expect("interaction benchmark frame should include debug timing");
            total_loop_us.push(debug_duration_us(loop_started.elapsed()));
            input_queue_us.push(timing.server_input_queue_us);
            input_to_frame_us.push(timing.server_input_to_frame_us);
            if let Some(value) = timing.server_pty_dirty_to_frame_us {
                dirty_to_frame_us.push(value);
            }
            if let Some(value) = timing.server_render_us {
                render_us.push(value);
            }
            if let Some(value) = timing.server_frame_build_us {
                frame_build_us.push(value);
            }
            if let Some(value) = timing.server_prepare_us {
                prepare_us.push(value);
            }
            checksum ^= frame.cells.len();
        }
        std::hint::black_box(checksum);

        println!(
            "held_key_interaction_latency: frames={} cols={} rows={} input=a",
            frames, cols, rows
        );
        print_latency_bench_stats("total_loop", &total_loop_us);
        print_latency_bench_stats("server_input_queue", &input_queue_us);
        print_latency_bench_stats("server_input_to_frame", &input_to_frame_us);
        print_latency_bench_stats("server_pty_dirty_to_frame", &dirty_to_frame_us);
        print_latency_bench_stats("server_render", &render_us);
        print_latency_bench_stats("server_frame_build", &frame_build_us);
        print_latency_bench_stats("server_prepare", &prepare_us);
    }

    fn assert_frame_data_eq(actual: &FrameData, expected: &FrameData) {
        assert_eq!(
            (actual.width, actual.height),
            (expected.width, expected.height)
        );
        assert_eq!(actual.cursor, expected.cursor, "cursor mismatch");
        assert_eq!(actual.hyperlinks, expected.hyperlinks, "hyperlink mismatch");
        assert_eq!(actual.graphics, expected.graphics, "graphics mismatch");
        assert_eq!(
            actual.cells.len(),
            expected.cells.len(),
            "cell length mismatch"
        );
        for (idx, (actual_cell, expected_cell)) in
            actual.cells.iter().zip(expected.cells.iter()).enumerate()
        {
            assert_eq!(
                actual_cell,
                expected_cell,
                "cell mismatch at index {idx} (x={}, y={})",
                idx % usize::from(actual.width),
                idx / usize::from(actual.width),
            );
        }
    }

    #[test]
    fn focused_latency_timer_requests_catch_up_after_quiet_window() {
        let mut server = test_headless_server();
        let workspace = crate::workspace::Workspace::test_new("test");
        let pane_id = workspace.focused_pane_id().expect("focused pane");
        server.app.state.sessions = vec![workspace];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        let now = Instant::now();
        server.focused_input_pane_id = Some(pane_id);
        server.focused_input_deadline = Some(now + FOCUSED_INPUT_LATENCY_WINDOW);
        server.note_latency_background_skipped(now);

        assert!(!server.handle_focused_latency_timers(now + Duration::from_millis(49)));
        assert!(server.latency_background_dirty);

        assert!(server.handle_focused_latency_timers(now + Duration::from_millis(50)));
        assert_eq!(server.focused_input_pane_id, None);
        assert_eq!(server.focused_input_deadline, None);
        assert_eq!(server.latency_background_render_at, None);
        assert!(!server.latency_background_dirty);
    }

    #[test]
    fn focused_latency_timer_caps_background_catch_up() {
        let mut server = test_headless_server();
        let workspace = crate::workspace::Workspace::test_new("test");
        let pane_id = workspace.focused_pane_id().expect("focused pane");
        server.app.state.sessions = vec![workspace];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        let now = Instant::now();
        server.focused_input_pane_id = Some(pane_id);
        server.focused_input_deadline = Some(now + Duration::from_secs(1));
        server.note_latency_background_skipped(now);

        assert!(!server.handle_focused_latency_timers(
            now + LATENCY_BACKGROUND_RENDER_INTERVAL - Duration::from_millis(1)
        ));
        assert!(server.latency_background_dirty);

        assert!(server.handle_focused_latency_timers(now + LATENCY_BACKGROUND_RENDER_INTERVAL));
        assert!(server.latency_background_dirty);
        assert_eq!(server.latency_background_render_at, None);
    }

    #[test]
    fn foreground_client_applies_client_keybindings() {
        let mut server = test_headless_server();
        let local_config: crate::config::Config = toml::from_str(
            r#"
[keys]
prefix = "ctrl+a"
new_tab = "prefix+t"
"#,
        )
        .unwrap();
        let local_keybindings = local_config.live_keybinds().unwrap();
        let (writer_a, _control_a, _render_a) = test_client_writer();
        let (writer_b, _control_b, _render_b) = test_client_writer();

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 1,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: Some(Box::new(local_keybindings)),
            direct_attach_requested: false,
            writer: writer_a,
        }));
        assert_eq!(
            server.app.state.prefix_code,
            crossterm::event::KeyCode::Char('a')
        );
        assert!(server
            .app
            .state
            .keybinds
            .new_tab
            .bindings
            .iter()
            .any(|binding| binding.label == "prefix+t"));

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 2,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: None,
            direct_attach_requested: false,
            writer: writer_b,
        }));
        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(
            server.app.state.prefix_code,
            crossterm::event::KeyCode::Char('a')
        );

        assert!(server.promote_client_to_foreground(2));
        assert_eq!(
            server.app.state.prefix_code,
            crossterm::event::KeyCode::Char('b')
        );
        assert!(server
            .app
            .state
            .keybinds
            .new_tab
            .bindings
            .iter()
            .any(|binding| binding.label == "prefix+c"));
    }

    #[test]
    fn local_keybinding_client_hides_server_keybinding_warnings() {
        let mut server = test_headless_server();
        let diagnostics = vec![
            "unsafe direct keybinding: keys.close_pane = \"x\" would intercept typing".to_owned(),
            "theme warning".to_owned(),
        ];
        let (full, without_keybindings) = server_config_diagnostic_summaries(&diagnostics);
        server.server_config_diagnostic = full.clone();
        server.server_config_diagnostic_without_keybindings = without_keybindings.clone();
        server.app.state.config_diagnostic = full;
        let local_keybindings = crate::config::Config::default().live_keybinds().unwrap();
        let (writer_a, _control_a, _render_a) = test_client_writer();
        let (writer_b, _control_b, _render_b) = test_client_writer();

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 1,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: Some(Box::new(local_keybindings)),
            direct_attach_requested: false,
            writer: writer_a,
        }));
        assert_eq!(server.app.state.config_diagnostic, without_keybindings);

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 2,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: None,
            direct_attach_requested: false,
            writer: writer_b,
        }));
        assert_eq!(server.app.state.config_diagnostic, without_keybindings);
        assert!(server.promote_client_to_foreground(2));
        assert_eq!(
            server.app.state.config_diagnostic,
            server.server_config_diagnostic
        );
    }

    #[test]
    fn local_keybinding_client_keeps_local_keybindings_after_settings_save() {
        let path = std::env::temp_dir().join(format!(
            "gmux-headless-settings-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, "onboarding = false\n").unwrap();
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut server = test_headless_server();
        let local_config: crate::config::Config = toml::from_str(
            r#"
[keys]
prefix = "ctrl+a"
new_tab = "prefix+n"
next_tab = ""
"#,
        )
        .unwrap();
        let local_keybindings = local_config.live_keybinds().unwrap();
        let (writer, _control, _render) = test_client_writer();
        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 1,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: Some(Box::new(local_keybindings)),
            direct_attach_requested: false,
            writer,
        }));
        server.app.state.mode = crate::app::Mode::Settings;
        server.app.state.settings.page = crate::app::state::SettingsPage::ToastDelivery;
        server.app.state.settings.list.selected = 1;

        assert!(server.handle_server_event(test_client_input(1, b"\r".to_vec())));

        assert_eq!(
            server.app.state.prefix_code,
            crossterm::event::KeyCode::Char('a')
        );
        assert!(server
            .app
            .state
            .keybinds
            .new_tab
            .bindings
            .iter()
            .any(|binding| binding.label == "prefix+n"));
        assert!(server.app.state.toast.is_none());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("delivery = \"gmux\""));

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_server_keybindings_do_not_cache_local_keybindings_after_settings_save() {
        let path = std::env::temp_dir().join(format!(
            "gmux-headless-invalid-settings-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(
            &path,
            "onboarding = false\n[keys]\nnew_tab = \"x\"\n[ui.toast]\ndelivery = \"off\"\n",
        )
        .unwrap();
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut server = test_headless_server();
        let previous_server_config: crate::config::Config =
            toml::from_str("[keys]\nprefix = \"ctrl+c\"\nnew_tab = \"prefix+m\"\n").unwrap();
        server.server_keybindings = previous_server_config.live_keybinds().unwrap();
        let local_config: crate::config::Config = toml::from_str(
            r#"
[keys]
prefix = "ctrl+a"
new_tab = "prefix+n"
next_tab = ""
"#,
        )
        .unwrap();
        let (writer_a, _control_a, _render_a) = test_client_writer();
        let (writer_b, _control_b, _render_b) = test_client_writer();

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 1,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: Some(Box::new(local_config.live_keybinds().unwrap())),
            direct_attach_requested: false,
            writer: writer_a,
        }));
        server.app.state.mode = crate::app::Mode::Settings;
        server.app.state.settings.page = crate::app::state::SettingsPage::ToastDelivery;
        server.app.state.settings.list.selected = 1;

        assert!(server.handle_server_event(test_client_input(1, b"\r".to_vec())));

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 2,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: None,
            direct_attach_requested: false,
            writer: writer_b,
        }));
        assert_eq!(
            server.app.state.prefix_code,
            crossterm::event::KeyCode::Char('a')
        );

        assert!(server.promote_client_to_foreground(2));
        assert_eq!(
            server.app.state.prefix_code,
            crossterm::event::KeyCode::Char('c')
        );
        assert!(server
            .app
            .state
            .keybinds
            .new_tab
            .bindings
            .iter()
            .any(|binding| binding.label == "prefix+m"));

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn terminal_attach_rejects_missing_terminal_and_removes_client() {
        let mut server = test_headless_server();
        let (writer, control_rx, _render_rx) = test_client_writer();

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 7,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::TerminalAnsi,
            keybindings: None,
            direct_attach_requested: true,
            writer,
        }));
        assert!(server.clients.contains_key(&7));

        assert!(
            !server.handle_server_event(ServerEvent::ClientAttachTerminal {
                client_id: 7,
                terminal_id: "term_missing".to_owned(),
                takeover: false,
            })
        );
        assert!(!server.clients.contains_key(&7));
        let reason = read_server_shutdown_reason(control_rx.recv().expect("shutdown message"));
        assert_eq!(
            reason,
            Some("terminal attach failed: terminal term_missing not found".to_owned())
        );
    }

    #[test]
    fn pending_terminal_attach_client_does_not_affect_headless_deadline() {
        let mut server = test_headless_server();
        server
            .app
            .state
            .sessions
            .push(crate::workspace::Workspace::test_new("test"));
        let (writer, _control_rx, _render_rx) = test_client_writer();

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 7,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::TerminalAnsi,
            keybindings: None,
            direct_attach_requested: true,
            writer,
        }));

        assert!(!server.has_app_client());
        assert_eq!(
            server
                .app
                .next_headless_loop_deadline(Instant::now(), false),
            None
        );
    }

    #[test]
    fn writerless_app_client_does_not_affect_headless_deadline() {
        let mut server = test_headless_server();
        server
            .app
            .state
            .sessions
            .push(crate::workspace::Workspace::test_new("test"));
        let (writer, _control_rx, _render_rx) = test_client_writer();

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 7,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: None,
            direct_attach_requested: false,
            writer,
        }));
        assert!(server.has_app_client());

        server.clients.get_mut(&7).expect("client").writer = None;

        assert!(!server.has_app_client());
        assert_eq!(
            server
                .app
                .next_headless_loop_deadline(Instant::now(), false),
            None
        );
    }

    #[test]
    fn terminal_attach_client_exits_when_attached_pane_dies() {
        let mut server = test_headless_server();
        let workspace = crate::workspace::Workspace::test_new("attached");
        let pane_id = workspace.tabs[0].root_pane;
        server.app.state.sessions = vec![workspace];
        server.app.state.ensure_test_terminals();
        let terminal_id = server.app.state.sessions[0]
            .pane_state(pane_id)
            .expect("pane")
            .attached_terminal_id
            .to_string();
        let (writer, control_rx, _render_rx) = test_client_writer();

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 7,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::TerminalAnsi,
            keybindings: None,
            direct_attach_requested: true,
            writer,
        }));
        assert!(
            server.handle_server_event(ServerEvent::ClientAttachTerminal {
                client_id: 7,
                terminal_id: terminal_id.clone(),
                takeover: false,
            })
        );
        assert_eq!(server.terminal_attach_owners.get(&terminal_id), Some(&7));

        assert!(server.handle_internal_event_with_forwarding(AppEvent::PaneDied { pane_id }));

        assert!(!server.clients.contains_key(&7));
        assert!(!server.terminal_attach_owners.contains_key(&terminal_id));
        let reason = read_server_shutdown_reason(control_rx.recv().expect("shutdown message"));
        assert_eq!(reason, Some(format!("terminal {terminal_id} exited")));
    }

    #[test]
    fn terminal_attach_scroll_moves_attached_runtime_viewport() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut bytes = Vec::new();
        for line in 0..80 {
            bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
        }
        let runtime =
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 4096, &bytes);

        apply_terminal_attach_scroll(
            &runtime,
            AttachScrollSource::Wheel,
            AttachScrollDirection::Up,
            3,
            None,
            None,
            0,
        )
        .expect("scroll up");
        let metrics = runtime.scroll_metrics().expect("scroll metrics");
        assert_eq!(metrics.offset_from_bottom, 3);

        apply_terminal_attach_scroll(
            &runtime,
            AttachScrollSource::Wheel,
            AttachScrollDirection::Down,
            2,
            None,
            None,
            0,
        )
        .expect("scroll down");
        let metrics = runtime.scroll_metrics().expect("scroll metrics");
        assert_eq!(metrics.offset_from_bottom, 1);
        drop(runtime);
        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn priority_drain_coalesces_terminal_attach_host_wheel_scroll() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut server = test_headless_server();
        let terminal_id = crate::terminal::TerminalId::alloc();
        let terminal_id_string = terminal_id.to_string();
        let mut bytes = Vec::new();
        for line in 0..80 {
            bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
        }
        let runtime =
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 4096, &bytes);
        server.app.state.terminals.insert(
            terminal_id.clone(),
            crate::terminal::TerminalState::new(terminal_id.clone(), "/tmp".into()),
        );
        server
            .app
            .terminal_runtimes
            .insert(terminal_id.clone(), runtime);
        server.clients.insert(
            7,
            ClientConnection::new_with_mode(
                ClientConnectionMode::TerminalAttach {
                    terminal_id: terminal_id_string,
                },
                None,
                (20, 5),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                false,
                None,
            ),
        );

        server
            .server_input_tx
            .send(test_attach_wheel_scroll(7, AttachScrollDirection::Up, 3))
            .expect("queue first scroll");
        server
            .server_input_tx
            .send(test_attach_wheel_scroll(7, AttachScrollDirection::Up, 4))
            .expect("queue second scroll");
        server
            .server_input_tx
            .send(test_attach_wheel_scroll(7, AttachScrollDirection::Up, 5))
            .expect("queue third scroll");

        assert!(server.drain_priority_input_events());
        let runtime = server
            .app
            .terminal_runtimes
            .get(&terminal_id)
            .expect("runtime after drain");
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            12
        );
        assert!(server.app.input_render_bypass_pending);

        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn priority_drain_limits_terminal_attach_host_wheel_scroll_burst() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut server = test_headless_server();
        let terminal_id = crate::terminal::TerminalId::alloc();
        let terminal_id_string = terminal_id.to_string();
        let mut bytes = Vec::new();
        for line in 0..200 {
            bytes.extend_from_slice(format!("line {line:03}\r\n").as_bytes());
        }
        let runtime =
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 16 * 1024, &bytes);
        server.app.state.terminals.insert(
            terminal_id.clone(),
            crate::terminal::TerminalState::new(terminal_id.clone(), "/tmp".into()),
        );
        server
            .app
            .terminal_runtimes
            .insert(terminal_id.clone(), runtime);
        server.clients.insert(
            7,
            ClientConnection::new_with_mode(
                ClientConnectionMode::TerminalAttach {
                    terminal_id: terminal_id_string,
                },
                None,
                (20, 5),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                false,
                None,
            ),
        );

        for _ in 0..(PRIORITY_INPUT_DRAIN_LIMIT + 10) {
            server
                .server_input_tx
                .send(test_attach_wheel_scroll(7, AttachScrollDirection::Up, 1))
                .expect("queue scroll");
        }

        assert!(server.drain_priority_input_events());
        let runtime = server
            .app
            .terminal_runtimes
            .get(&terminal_id)
            .expect("runtime after drain");
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            PRIORITY_INPUT_DRAIN_LIMIT
        );
        assert!(server.server_input_rx.try_recv().is_ok());

        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn priority_drain_preserves_terminal_attach_mouse_report_wheel_events() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut server = test_headless_server();
        let terminal_id = crate::terminal::TerminalId::alloc();
        let terminal_id_string = terminal_id.to_string();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                20,
                5,
                0,
                b"\x1b[?1000h\x1b[?1006h",
                4,
            );
        server.app.state.terminals.insert(
            terminal_id.clone(),
            crate::terminal::TerminalState::new(terminal_id.clone(), "/tmp".into()),
        );
        server.app.terminal_runtimes.insert(terminal_id, runtime);
        server.clients.insert(
            7,
            ClientConnection::new_with_mode(
                ClientConnectionMode::TerminalAttach {
                    terminal_id: terminal_id_string,
                },
                None,
                (20, 5),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                false,
                None,
            ),
        );

        server
            .server_input_tx
            .send(test_attach_wheel_scroll(7, AttachScrollDirection::Up, 3))
            .expect("queue first scroll");
        server
            .server_input_tx
            .send(test_attach_wheel_scroll(7, AttachScrollDirection::Up, 3))
            .expect("queue second scroll");

        assert!(server.drain_priority_input_events());
        assert_eq!(
            input_rx.try_recv().expect("first wheel report"),
            Bytes::from_static(b"\x1b[<64;1;1M")
        );
        assert_eq!(
            input_rx.try_recv().expect("second wheel report"),
            Bytes::from_static(b"\x1b[<64;1;1M")
        );
        assert!(input_rx.try_recv().is_err());

        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn terminal_attach_input_resets_scrolled_viewport() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut bytes = Vec::new();
        for line in 0..80 {
            bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
        }
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                20, 5, 4096, &bytes, 4,
            );

        runtime.scroll_up(4);
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            4
        );

        apply_terminal_attach_input(&runtime, b"x".to_vec()).expect("attach input");
        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            0
        );
        assert_eq!(
            input_rx.try_recv().expect("forwarded input"),
            Bytes::from("x")
        );

        drop(runtime);
        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn drain_server_events_drains_terminal_attach_input_without_waiting_for_render() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut server = test_headless_server();
        let terminal_id = crate::terminal::TerminalId::alloc();
        let terminal_id_string = terminal_id.to_string();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 2);

        server.app.state.terminals.insert(
            terminal_id.clone(),
            crate::terminal::TerminalState::new(terminal_id.clone(), "/tmp".into()),
        );
        server.app.terminal_runtimes.insert(terminal_id, runtime);
        server.clients.insert(
            1,
            ClientConnection::new_with_mode(
                ClientConnectionMode::TerminalAttach {
                    terminal_id: terminal_id_string,
                },
                None,
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::TerminalAnsi,
                false,
                None,
            ),
        );

        server
            .server_event_tx
            .try_send(test_client_input(1, b"a".to_vec()))
            .unwrap();
        server
            .server_event_tx
            .try_send(test_client_input(1, b"b".to_vec()))
            .unwrap();

        assert!(!server.drain_server_events());
        assert!(server.app.input_render_bypass_pending);
        assert_eq!(input_rx.try_recv().unwrap(), Bytes::from_static(b"a"));
        assert_eq!(input_rx.try_recv().unwrap(), Bytes::from_static(b"b"));
        assert!(input_rx.try_recv().is_err());
        assert!(server.server_event_rx.try_recv().is_err());
        drop(server);
        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn terminal_attach_page_key_host_scrolls_plain_terminal() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut bytes = Vec::new();
        for line in 0..80 {
            bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
        }
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                20, 5, 4096, &bytes, 4,
            );

        apply_terminal_attach_scroll(
            &runtime,
            AttachScrollSource::PageKey {
                input: b"\x1b[5~".to_vec(),
            },
            AttachScrollDirection::Up,
            4,
            None,
            None,
            0,
        )
        .expect("page key scroll");

        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            4
        );
        assert!(input_rx.try_recv().is_err());
        drop(runtime);
        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn terminal_attach_page_key_forwards_when_mouse_reporting() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut bytes = b"\x1b[?1000h".to_vec();
        for line in 0..80 {
            bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
        }
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                20, 5, 4096, &bytes, 4,
            );
        runtime.scroll_up(3);

        apply_terminal_attach_scroll(
            &runtime,
            AttachScrollSource::PageKey {
                input: b"\x1b[5~".to_vec(),
            },
            AttachScrollDirection::Up,
            4,
            None,
            None,
            0,
        )
        .expect("page key forward");

        assert_eq!(
            runtime
                .scroll_metrics()
                .expect("scroll metrics")
                .offset_from_bottom,
            0
        );
        assert_eq!(
            input_rx.try_recv().expect("forwarded page key"),
            Bytes::from_static(b"\x1b[5~")
        );
        drop(runtime);
        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn virtual_render_produces_nonempty_buffer() {
        let mut state = AppState::test_new();
        let area = Rect::new(0, 0, 80, 24);
        let (buffer, _cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);
        assert_eq!(buffer.area.width, 80);
        assert_eq!(buffer.area.height, 24);
    }

    #[test]
    fn virtual_render_without_frame_cursor_keeps_cursor_hidden() {
        let mut state = AppState::test_new();
        let area = Rect::new(0, 0, 80, 24);
        let (_buffer, cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);

        assert_eq!(cursor, None);
    }

    #[tokio::test]
    async fn virtual_render_preserves_explicit_frame_cursor_position() {
        let mut state = AppState::test_new();
        let mut ws = crate::workspace::Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        ws.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left"),
        );

        state.sessions = vec![ws];
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = crate::app::Mode::Terminal;

        let area = Rect::new(0, 0, 80, 24);
        let (_buffer, cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);
        let pane = state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == pane_id)
            .expect("focused pane info");

        assert_eq!(
            cursor,
            Some(CursorState {
                x: pane.inner_rect.x + 4,
                y: pane.inner_rect.y,
                visible: true,
                shape: cursor.as_ref().map(|c| c.shape).unwrap_or(0),
            })
        );
    }

    #[tokio::test]
    async fn virtual_render_preserves_hidden_focused_pane_cursor_position() {
        let mut state = AppState::test_new();
        let mut ws = crate::workspace::Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        ws.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left\x1b[?25l"),
        );

        state.sessions = vec![ws];
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = crate::app::Mode::Terminal;

        let area = Rect::new(0, 0, 80, 24);
        let (_buffer, cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);
        let pane = state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == pane_id)
            .expect("focused pane info");

        assert_eq!(
            cursor,
            Some(CursorState {
                x: pane.inner_rect.x + 4,
                y: pane.inner_rect.y,
                visible: false,
                shape: cursor.as_ref().map(|c| c.shape).unwrap_or(0),
            })
        );
    }

    #[tokio::test]
    async fn virtual_render_exposes_hidden_pane_cursor_when_reveal_hidden_for_cjk_ime() {
        let mut state = AppState::test_new();
        state.reveal_hidden_cursor_for_cjk_ime = true;
        let mut ws = crate::workspace::Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        ws.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left\x1b[?25l"),
        );

        state.sessions = vec![ws];
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = crate::app::Mode::Terminal;

        let area = Rect::new(0, 0, 80, 24);
        let (_buffer, cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);
        let pane = state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == pane_id)
            .expect("focused pane info");

        assert_eq!(
            cursor,
            Some(CursorState {
                x: pane.inner_rect.x + 4,
                y: pane.inner_rect.y,
                visible: true,
                shape: state.cjk_ime_cursor_shape,
            })
        );
    }

    #[tokio::test]
    async fn virtual_render_keeps_cursor_hidden_when_scrolled_back_even_with_reveal_hidden_for_cjk_ime(
    ) {
        let mut state = AppState::test_new();
        state.reveal_hidden_cursor_for_cjk_ime = true;
        let mut ws = crate::workspace::Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let mut bytes = Vec::new();
        for line in 0..80 {
            bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
        }
        let runtime =
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 4096, &bytes);
        ws.insert_test_runtime(pane_id, runtime);

        state.sessions = vec![ws];
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = crate::app::Mode::Terminal;

        let area = Rect::new(0, 0, 80, 24);
        let _ = crate::server::render_stream::render_virtual(&mut state, area, true);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let runtime = state
            .runtime_for_pane(&terminal_runtimes, pane_id)
            .expect("pane runtime after initial render");
        runtime.scroll_up(6);
        assert!(crate::ui::pane_is_scrolled_back(runtime));

        let (_buffer, cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);

        assert!(
            cursor.as_ref().is_none_or(|cursor| !cursor.visible),
            "scrolled-back focused pane should keep the cursor hidden even when reveal_hidden_cursor_for_cjk_ime is true; got {cursor:?}",
        );
    }

    #[tokio::test]
    async fn virtual_render_fallback_cursor_when_viewport_none_and_reveal_hidden_for_cjk_ime() {
        let mut state = AppState::test_new();
        state.reveal_hidden_cursor_for_cjk_ime = true;
        let mut ws = crate::workspace::Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        // Feed only ?25l with no prior cursor movement — exercises the fallback
        // path for TUIs whose viewport has no cursor position.
        ws.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"\x1b[?25l"),
        );

        state.sessions = vec![ws];
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = crate::app::Mode::Terminal;

        let area = Rect::new(0, 0, 80, 24);
        let (_buffer, cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);
        let pane = state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == pane_id)
            .expect("focused pane info");

        assert_eq!(
            cursor,
            Some(CursorState {
                x: pane.inner_rect.x,
                y: pane.inner_rect.y,
                visible: true,
                shape: state.cjk_ime_cursor_shape,
            }),
            "fallback should anchor at pane top-left with the configured shape",
        );
    }

    #[tokio::test]
    async fn virtual_render_omits_focused_pane_cursor_while_mobile_switcher_open() {
        let mut state = AppState::test_new();
        let mut ws = crate::workspace::Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        ws.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left"),
        );

        state.sessions = vec![ws];
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = crate::app::Mode::Navigate;

        let area = Rect::new(0, 0, 44, 24);
        let (_buffer, cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);

        assert_eq!(cursor, None);
    }

    #[tokio::test]
    async fn virtual_render_hides_focused_pane_cursor_while_scrolled_back() {
        let mut state = AppState::test_new();
        let mut ws = crate::workspace::Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let mut bytes = Vec::new();
        for line in 0..80 {
            bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
        }
        let runtime =
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 4096, &bytes);
        ws.insert_test_runtime(pane_id, runtime);

        state.sessions = vec![ws];
        state.active_session = Some(0);
        state.selected_session = 0;
        state.mode = crate::app::Mode::Terminal;

        let area = Rect::new(0, 0, 80, 24);
        let _ = crate::server::render_stream::render_virtual(&mut state, area, true);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let runtime = state
            .runtime_for_pane(&terminal_runtimes, pane_id)
            .expect("pane runtime after initial render");
        runtime.scroll_up(6);
        assert!(crate::ui::pane_is_scrolled_back(runtime));

        let (_buffer, cursor) =
            crate::server::render_stream::render_virtual(&mut state, area, true);

        assert!(
            cursor.as_ref().is_none_or(|cursor| !cursor.visible),
            "cursor: {cursor:?}"
        );
    }

    #[test]
    fn latest_active_client_drives_shared_size_theme_and_fallback() {
        let mut server = test_headless_server();

        server.clients.insert(
            1,
            ClientConnection::new(
                (160, 45),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme {
                    foreground: Some(crate::terminal_theme::RgbColor {
                        r: 0xaa,
                        g: 0xbb,
                        b: 0xcc,
                    }),
                    background: Some(crate::terminal_theme::RgbColor {
                        r: 0x11,
                        g: 0x22,
                        b: 0x33,
                    }),
                },
                None,
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme {
                    foreground: Some(crate::terminal_theme::RgbColor {
                        r: 0x10,
                        g: 0x20,
                        b: 0x30,
                    }),
                    background: Some(crate::terminal_theme::RgbColor {
                        r: 0xdd,
                        g: 0xee,
                        b: 0xff,
                    }),
                },
                None,
                2,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );

        assert!(server.promote_client_to_foreground(1));
        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.effective_size, (160, 45));
        assert_eq!(
            server.app.state.host_terminal_theme,
            server.clients[&1].host_terminal_theme
        );

        assert!(server.promote_client_to_foreground(2));
        assert_eq!(server.foreground_client_id, Some(2));
        assert_eq!(server.effective_size, (80, 24));
        assert_eq!(
            server.app.state.host_terminal_theme,
            server.clients[&2].host_terminal_theme
        );

        assert!(server.remove_client(2));
        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.effective_size, (160, 45));
        assert_eq!(
            server.app.state.host_terminal_theme,
            server.clients[&1].host_terminal_theme
        );
    }

    #[test]
    fn focus_lost_updates_client_without_promoting_foreground() {
        let mut server = test_headless_server();

        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                Some(true),
                2,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(2);
        server.sync_foreground_client_state();

        let changed = server.handle_server_event(test_client_input(1, b"\x1b[O".to_vec()));

        assert!(!changed);
        assert_eq!(server.foreground_client_id, Some(2));
        assert_eq!(server.clients[&1].outer_terminal_focus, Some(false));
        assert_eq!(server.app.state.outer_terminal_focus, Some(true));
    }

    #[test]
    fn focus_gained_promotes_client_to_foreground() {
        let mut server = test_headless_server();

        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                Some(true),
                2,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(2);
        server.sync_foreground_client_state();

        let changed = server.handle_server_event(test_client_input(1, b"\x1b[I".to_vec()));

        assert!(changed);
        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.clients[&1].outer_terminal_focus, Some(true));
        assert_eq!(server.app.state.outer_terminal_focus, Some(true));
    }

    #[test]
    fn foreground_client_focus_event_updates_app_focus_state() {
        let mut server = test_headless_server();

        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                Some(true),
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();

        let changed = server.handle_server_event(test_client_input(1, b"\x1b[O".to_vec()));

        assert!(!changed);
        assert_eq!(server.clients[&1].outer_terminal_focus, Some(false));
        assert_eq!(server.app.state.outer_terminal_focus, Some(false));
    }

    #[test]
    fn background_client_resize_does_not_promote_foreground() {
        let mut server = test_headless_server();
        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();

        assert!(!server.handle_server_event(ServerEvent::ClientResize {
            client_id: 2,
            cols: 100,
            rows: 30,
            cell_width_px: 9,
            cell_height_px: 18,
        }));

        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.effective_size, (120, 40));
        assert_eq!(server.clients[&2].terminal_size, (100, 30));
        assert_eq!(server.clients[&2].cell_size.width_px, 9);
        assert_eq!(server.clients[&2].cell_size.height_px, 18);
    }

    #[test]
    fn background_client_attach_does_not_change_effective_size() {
        let mut server = test_headless_server();
        let (active_writer, _active_control, _active_render) = test_client_writer();
        let (mirror_writer, _mirror_control, _mirror_render) = test_client_writer();

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 1,
            cols: 120,
            rows: 40,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: None,
            direct_attach_requested: false,
            writer: active_writer,
        }));
        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.effective_size, (120, 40));

        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 2,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            render_encoding: RenderEncoding::SemanticFrame,
            keybindings: None,
            direct_attach_requested: false,
            writer: mirror_writer,
        }));

        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.effective_size, (120, 40));
        assert_eq!(server.clients[&2].terminal_size, (80, 24));
        assert_eq!(server.clients[&2].cell_size.width_px, 9);
        assert_eq!(server.clients[&2].cell_size.height_px, 18);
    }

    #[test]
    fn background_client_cell_size_only_resize_does_not_request_global_render() {
        let mut server = test_headless_server();
        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();

        assert!(!server.handle_server_event(ServerEvent::ClientResize {
            client_id: 2,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
        }));

        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.effective_size, (120, 40));
        assert_eq!(server.clients[&2].terminal_size, (80, 24));
        assert_eq!(server.clients[&2].cell_size.width_px, 9);
        assert_eq!(server.clients[&2].cell_size.height_px, 18);
    }

    #[test]
    fn foreground_client_resize_updates_shared_size() {
        let mut server = test_headless_server();
        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();

        assert!(server.handle_server_event(ServerEvent::ClientResize {
            client_id: 1,
            cols: 100,
            rows: 30,
            cell_width_px: 9,
            cell_height_px: 18,
        }));

        assert_eq!(server.foreground_client_id, Some(1));
        assert_eq!(server.effective_size, (100, 30));
    }

    #[test]
    fn app_client_lone_escape_closes_navigate_mode() {
        let mut server = test_headless_server();
        server.app.state.sessions = vec![crate::workspace::Workspace::test_new("test")];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Navigate;
        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                Some(true),
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();

        assert!(server.handle_server_event(test_client_input(1, b"\x1b".to_vec())));

        assert_eq!(server.app.state.mode, crate::app::Mode::Terminal);
    }

    #[tokio::test]
    async fn split_default_background_response_updates_theme_without_forwarding_tail() {
        let mut server = test_headless_server();
        let mut workspace = crate::workspace::Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 1);
        workspace.tabs[0].runtimes.insert(focused, runtime);
        server.app.state.sessions = vec![workspace];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;
        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                Some(true),
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();

        let _ = server.handle_server_event(test_client_input(1, b"\x1b]".to_vec()));
        assert!(rx.try_recv().is_err());

        assert!(server.handle_server_event(test_client_input(1, b"11;#123456\x07".to_vec())));

        assert!(rx.try_recv().is_err());
        assert_eq!(
            server.clients[&1].host_terminal_theme.background,
            Some(crate::terminal_theme::RgbColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            })
        );
        assert_eq!(
            server.app.state.host_terminal_theme.background,
            Some(crate::terminal_theme::RgbColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            })
        );
    }

    #[test]
    fn render_and_stream_uses_each_client_terminal_size() {
        let mut server = test_headless_server();
        server.app.state.sessions = vec![crate::workspace::Workspace::test_new("test")];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        let (desktop_tx, _desktop_control_rx, desktop_rx) = test_client_writer();
        let (phone_tx, _phone_control_rx, phone_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(desktop_tx),
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(phone_tx),
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();

        server.render_and_stream();

        let desktop_frame = read_server_frame(desktop_rx.recv().expect("desktop frame"));
        let phone_frame = read_server_frame(phone_rx.recv().expect("phone frame"));

        assert_eq!((desktop_frame.width, desktop_frame.height), (120, 40));
        assert_eq!((phone_frame.width, phone_frame.height), (80, 24));
    }

    #[test]
    fn render_and_stream_does_not_expand_beyond_shared_app_frame() {
        let mut server = test_headless_server();
        server.app.state.sessions = vec![crate::workspace::Workspace::test_new("test")];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        let (foreground_tx, _foreground_control_rx, foreground_rx) = test_client_writer();
        let (background_tx, _background_control_rx, background_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(foreground_tx),
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(background_tx),
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();

        server.render_and_stream();

        let foreground_frame = read_server_frame(foreground_rx.recv().expect("foreground frame"));
        let background_frame = read_server_frame(background_rx.recv().expect("background frame"));

        assert_eq!((foreground_frame.width, foreground_frame.height), (80, 24));
        assert_eq!((background_frame.width, background_frame.height), (120, 40));
    }

    #[test]
    fn latency_critical_render_publishes_active_then_mirrors() {
        let mut server = test_headless_server();
        server.app.state.sessions = vec![crate::workspace::Workspace::test_new("test")];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        let (foreground_tx, _foreground_control_rx, foreground_rx) = test_client_writer();
        let (background_tx, _background_control_rx, background_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(foreground_tx),
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(background_tx),
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();
        server.latency_critical_client_id = Some(1);

        server.render_and_stream();

        let foreground_frame = read_server_frame(
            foreground_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("foreground frame"),
        );
        assert_eq!((foreground_frame.width, foreground_frame.height), (80, 24));
        let background_frame = read_server_frame(
            background_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("background mirror frame"),
        );
        assert_eq!((background_frame.width, background_frame.height), (80, 24));
    }

    #[test]
    fn full_background_mirror_slot_keeps_latest_frame_without_drain_event() {
        let mut server = test_headless_server();
        server.app.state.sessions = vec![crate::workspace::Workspace::test_new("test")];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        let (foreground_tx, _foreground_control_rx, foreground_rx) = test_client_writer();
        let (background_tx, _background_control_rx, background_rx) = test_client_writer();
        let queued = HeadlessServer::frame_server_message(&ServerMessage::ReloadClientConfig)
            .expect("serialize dummy message");
        background_tx
            .render
            .send(queued)
            .expect("pre-fill background render queue");

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(foreground_tx),
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(background_tx),
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();
        server.latency_critical_client_id = Some(1);

        server.render_and_stream();
        let foreground_frame = read_server_frame(
            foreground_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("foreground frame"),
        );
        assert_eq!((foreground_frame.width, foreground_frame.height), (80, 24));

        let background_frame = recv_server_frame_within(
            &background_rx,
            Duration::from_millis(100),
            "latest background frame",
        );
        assert_eq!((background_frame.width, background_frame.height), (80, 24));
    }

    #[test]
    fn render_targets_prioritize_foreground_client() {
        let mut clients = HashMap::new();
        let (background_writer, _background_control_rx, _background_render_rx) =
            test_client_writer();
        let (foreground_writer, _foreground_control_rx, _foreground_render_rx) =
            test_client_writer();
        clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(background_writer),
            ),
        );
        clients.insert(
            2,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(foreground_writer),
            ),
        );

        let targets = render_targets(&clients, Some(2));

        assert_eq!(targets.first().map(|target| target.0), Some(2));
    }

    #[test]
    fn render_and_stream_restores_foreground_view_geometry() {
        let mut server = test_headless_server();
        server.app.state.sessions = vec![crate::workspace::Workspace::test_new("test")];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        let (foreground_tx, _foreground_control_rx, _foreground_rx) = test_client_writer();
        let (background_tx, _background_control_rx, _background_rx) = test_client_writer();
        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(foreground_tx),
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(background_tx),
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();
        let foreground_terminal_area = server.app.state.view.terminal_area;

        server.render_and_stream();

        assert_eq!(
            server.app.state.view.terminal_area,
            foreground_terminal_area
        );
    }

    #[tokio::test]
    async fn resize_shared_runtime_resizes_background_tabs() {
        let mut server = test_headless_server();
        let mut workspace = crate::workspace::Workspace::test_new("test");
        let background_tab = workspace.test_add_tab(Some("background"));
        let active_pane = workspace.tabs[0].root_pane;
        let background_pane = workspace.tabs[background_tab].root_pane;
        workspace.tabs[0].runtimes.insert(
            active_pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
        );
        workspace.tabs[background_tab].runtimes.insert(
            background_pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
        );
        server.app.state.sessions = vec![workspace];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();

        let terminal_area = server.app.state.view.terminal_area;
        let expected = (terminal_area.height, terminal_area.width);
        assert_eq!(
            server
                .app
                .state
                .runtime_for_pane(&server.app.terminal_runtimes, active_pane)
                .unwrap()
                .current_size(),
            expected
        );
        assert_eq!(
            server
                .app
                .state
                .runtime_for_pane(&server.app.terminal_runtimes, background_pane)
                .unwrap()
                .current_size(),
            expected
        );
    }

    #[test]
    fn terminal_attach_disconnect_restores_app_pane_size() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let _runtime_guard = rt.enter();
        let mut server = test_headless_server();
        let workspace = crate::workspace::Workspace::test_new("test");
        let pane_id = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane_id).expect("terminal id").clone();
        let terminal_id_string = terminal_id.to_string();
        server.app.state.sessions = vec![workspace];
        server.app.state.ensure_test_terminals();
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;
        server.app.terminal_runtimes.insert(
            terminal_id.clone(),
            crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
        );
        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                None,
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();
        let expected_app_size = server
            .app
            .terminal_runtimes
            .get(&terminal_id)
            .expect("runtime")
            .current_size();
        assert_ne!(expected_app_size, (24, 80));

        let (writer, _control_rx, _render_rx) = test_client_writer();
        assert!(server.handle_server_event(ServerEvent::ClientConnected {
            client_id: 2,
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
            render_encoding: RenderEncoding::TerminalAnsi,
            keybindings: None,
            direct_attach_requested: true,
            writer,
        }));
        assert!(
            server.handle_server_event(ServerEvent::ClientAttachTerminal {
                client_id: 2,
                terminal_id: terminal_id_string.clone(),
                takeover: false,
            })
        );
        assert_eq!(server.foreground_client_id, Some(1));
        assert!(server
            .app
            .state
            .direct_attach_resize_locks
            .contains(&terminal_id));
        assert_eq!(
            server
                .app
                .terminal_runtimes
                .get(&terminal_id)
                .expect("runtime")
                .current_size(),
            (24, 80)
        );

        assert!(server.handle_server_event(ServerEvent::ClientDisconnected { client_id: 2 }));

        assert!(!server
            .app
            .state
            .direct_attach_resize_locks
            .contains(&terminal_id));
        assert_eq!(
            server
                .app
                .terminal_runtimes
                .get(&terminal_id)
                .expect("runtime")
                .current_size(),
            expected_app_size
        );
        drop(server);
        drop(_runtime_guard);
        rt.shutdown_timeout(Duration::from_millis(100));
    }

    #[test]
    fn render_and_stream_sends_terminal_frame_for_terminal_ansi_client() {
        let mut server = test_headless_server();
        let (client_tx, _client_control_rx, client_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::TerminalAnsi,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);

        server.render_and_stream();

        match read_server_message(
            client_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("terminal frame"),
        ) {
            ServerMessage::Terminal(frame) => {
                assert_eq!(frame.seq, 1);
                assert_eq!((frame.width, frame.height), (80, 24));
                assert!(frame.full);
                assert!(!frame.bytes.is_empty());
            }
            other => panic!("expected terminal frame, got {other:?}"),
        }
        assert_eq!(
            server
                .clients
                .get(&1)
                .unwrap()
                .render_actor
                .as_ref()
                .unwrap()
                .wait_for_terminal_seq(Duration::from_millis(100)),
            Some(1)
        );
    }

    #[test]
    fn terminal_ansi_input_does_not_reset_blit_baseline() {
        let mut server = test_headless_server();
        let (client_tx, _client_control_rx, client_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::TerminalAnsi,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);

        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial terminal frame");
        assert_eq!(
            server
                .clients
                .get(&1)
                .unwrap()
                .render_actor
                .as_ref()
                .unwrap()
                .terminal_seq()
                .unwrap(),
            1
        );

        assert!(!server.handle_server_event(test_client_input(1, Vec::new())));
        server.render_and_stream();

        assert_eq!(
            server
                .clients
                .get(&1)
                .unwrap()
                .render_actor
                .as_ref()
                .unwrap()
                .terminal_seq()
                .unwrap(),
            1
        );
        assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[tokio::test]
    async fn semantic_terminal_input_waits_for_pty_dirty_frame() {
        let mut server = test_headless_server();
        let (client_tx, _client_control_rx, client_rx) = test_client_writer();
        let mut workspace = crate::workspace::Workspace::test_new("test");
        let pane_id = workspace.focused_pane_id().expect("focused pane");
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 1);
        workspace.tabs[0].runtimes.insert(pane_id, runtime);
        server.app.state.sessions = vec![workspace];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;
        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);

        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial semantic frame");

        assert!(!server.handle_server_event(test_client_input(1, b"j".to_vec())));
        assert_eq!(input_rx.try_recv().unwrap(), Bytes::from_static(b"j"));
        assert!(server.app.input_render_bypass_pending);

        server.render_and_stream();
        assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[tokio::test]
    async fn semantic_mouse_report_waits_for_pty_dirty_frame() {
        let mut server = test_headless_server();
        let (client_tx, _client_control_rx, client_rx) = test_client_writer();
        let mut workspace = crate::workspace::Workspace::test_new("test");
        let pane_id = workspace.focused_pane_id().expect("focused pane");
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80,
                24,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                1,
            );
        workspace.tabs[0].runtimes.insert(pane_id, runtime);
        server.app.state.sessions = vec![workspace];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;
        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);

        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial semantic frame");
        let pane = server
            .app
            .state
            .view
            .pane_infos
            .first()
            .expect("pane geometry");
        let mouse_column = pane.inner_rect.x + 1;
        let mouse_row = pane.inner_rect.y + 1;
        let mouse_input = format!("\x1b[<0;{};{}M", mouse_column + 1, mouse_row + 1);

        assert!(!server.handle_server_event(test_client_input(1, mouse_input.into_bytes())));
        assert!(!input_rx.try_recv().unwrap().is_empty());
        assert!(server.app.input_render_bypass_pending);

        server.render_and_stream();
        assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    fn outer_focus_gained_forces_terminal_ansi_full_redraw() {
        let mut server = test_headless_server();
        let (client_tx, _client_control_rx, client_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::TerminalAnsi,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);

        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial terminal frame");

        assert!(server.handle_server_event(test_client_input(1, b"\x1b[I".to_vec())));
        server.render_and_stream();

        match read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()) {
            ServerMessage::Terminal(frame) => {
                assert_eq!(frame.seq, 2);
                assert!(frame.full);
            }
            other => panic!("expected terminal frame, got {other:?}"),
        }
    }

    #[test]
    fn outer_focus_gained_does_not_force_terminal_ansi_full_redraw_when_disabled() {
        let mut server = test_headless_server();
        server.app.state.redraw_on_focus_gained = false;
        let (client_tx, _client_control_rx, client_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::TerminalAnsi,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);

        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial terminal frame");

        server.handle_server_event(test_client_input(1, b"\x1b[I".to_vec()));
        server.render_and_stream();

        assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
        assert_eq!(server.clients[&1].outer_terminal_focus, Some(true));
        assert_eq!(server.app.state.outer_terminal_focus, Some(true));
        assert_eq!(
            server
                .clients
                .get(&1)
                .unwrap()
                .render_actor
                .as_ref()
                .unwrap()
                .terminal_seq()
                .unwrap(),
            1
        );
    }

    #[test]
    fn full_render_latest_slot_advances_terminal_ansi_baseline() {
        let mut server = test_headless_server();
        let (client_tx, _client_control_rx, client_rx) = test_client_writer();
        let queued = HeadlessServer::frame_server_message(&ServerMessage::ReloadClientConfig)
            .expect("serialize dummy message");
        client_tx
            .render
            .send(queued)
            .expect("pre-fill render queue");

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::TerminalAnsi,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);

        server.render_and_stream();

        let mut terminal_seq = None;
        for _ in 0..2 {
            if let ServerMessage::Terminal(frame) =
                read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap())
            {
                terminal_seq = Some(frame.seq);
                break;
            }
        }
        assert_eq!(terminal_seq, Some(1));
        assert_eq!(
            server
                .clients
                .get(&1)
                .unwrap()
                .render_actor
                .as_ref()
                .unwrap()
                .terminal_seq()
                .unwrap(),
            1
        );
    }

    #[test]
    fn render_and_stream_skips_identical_frame_sends() {
        let mut server = test_headless_server();
        server.app.state.sessions = vec![crate::workspace::Workspace::test_new("test")];
        server.app.state.active_session = Some(0);
        server.app.state.selected_session = 0;
        server.app.state.mode = crate::app::Mode::Terminal;

        let (client_tx, _client_control_rx, client_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(client_tx),
            ),
        );
        server.foreground_client_id = Some(1);
        server.sync_foreground_client_state();
        server.resize_shared_runtime_to_effective_size();

        server.render_and_stream();
        let first = client_rx.recv_timeout(Duration::from_millis(100));
        assert!(first.is_ok(), "expected first frame to be sent");
        assert!(server
            .clients
            .get(&1)
            .and_then(|client| client.render_actor.as_ref())
            .and_then(|actor| actor.wait_for_last_frame(Duration::from_millis(100)))
            .is_some());

        server.render_and_stream();
        assert!(
            client_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "identical frame should not be sent twice"
        );
    }

    #[tokio::test]
    async fn retained_pty_update_streams_dirty_row_from_last_frame() {
        let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
        server.render_and_stream();
        let first = read_server_frame(
            client_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("initial frame"),
        );
        assert!(first.cells.iter().any(|cell| cell.symbol == "a"));

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");

        assert!(server.render_retained_pty_update_and_stream());
        let patched = read_server_frame(
            client_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("retained frame"),
        );
        assert!(patched.cells.iter().any(|cell| cell.symbol == "Z"));
        assert_eq!((patched.width, patched.height), (80, 24));
    }

    #[tokio::test]
    async fn retained_pty_update_streams_to_multiple_app_clients() {
        let (mut server, first_rx, pane_id) = retained_test_server(b"aaaa");
        let (second_tx, _second_control_rx, second_rx) = test_client_writer();
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(second_tx),
            ),
        );

        server.render_and_stream();
        let _ = first_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial first frame");
        let _ = second_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial second frame");
        server
            .clients
            .get(&1)
            .unwrap()
            .render_actor
            .as_ref()
            .unwrap()
            .wait_for_last_frame(Duration::from_millis(100))
            .expect("first baseline");
        server
            .clients
            .get(&2)
            .unwrap()
            .render_actor
            .as_ref()
            .unwrap()
            .wait_for_last_frame(Duration::from_millis(100))
            .expect("second baseline");

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");

        assert!(server.render_retained_pty_update_and_stream());
        let first_patched = read_server_frame(
            first_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("retained first frame"),
        );
        let second_patched = read_server_frame(
            second_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("retained second frame"),
        );

        assert!(first_patched.cells.iter().any(|cell| cell.symbol == "Z"));
        assert!(second_patched.cells.iter().any(|cell| cell.symbol == "Z"));
    }

    #[tokio::test]
    async fn retained_pty_update_survives_clipped_mirror_client() {
        let (mut server, active_rx, pane_id) = retained_test_server_with_size(b"aaaa", (120, 40));
        let (mirror_tx, _mirror_control_rx, mirror_rx) = test_client_writer();
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(mirror_tx),
            ),
        );

        server.render_and_stream();
        let active_initial = read_server_frame(
            active_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("initial active frame"),
        );
        let mirror_initial = read_server_frame(
            mirror_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("initial mirror frame"),
        );
        assert_eq!((active_initial.width, active_initial.height), (120, 40));
        assert_eq!((mirror_initial.width, mirror_initial.height), (80, 24));

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");

        assert!(
            server.render_retained_pty_update_and_stream(),
            "clipped mirror baseline must not force retained fallback"
        );
        let active_patched = read_server_frame(
            active_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("active retained frame"),
        );
        let mirror_patched = read_server_frame(
            mirror_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("mirror retained frame"),
        );

        assert_eq!((active_patched.width, active_patched.height), (120, 40));
        assert_eq!((mirror_patched.width, mirror_patched.height), (80, 24));
        assert!(active_patched.cells.iter().any(|cell| cell.symbol == "Z"));
        assert!(mirror_patched.cells.iter().any(|cell| cell.symbol == "Z"));
    }

    #[tokio::test]
    async fn retained_pty_update_sends_active_first_for_latency_critical_client() {
        let (mut server, first_rx, pane_id) = retained_test_server(b"aaaa");
        let (second_tx, _second_control_rx, second_rx) = test_client_writer();
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(second_tx),
            ),
        );

        server.render_and_stream();
        let _ = first_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial first frame");
        let _ = second_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial second frame");
        server
            .clients
            .get(&1)
            .unwrap()
            .render_actor
            .as_ref()
            .unwrap()
            .wait_for_last_frame(Duration::from_millis(100))
            .expect("first baseline");
        server
            .clients
            .get(&2)
            .unwrap()
            .render_actor
            .as_ref()
            .unwrap()
            .wait_for_last_frame(Duration::from_millis(100))
            .expect("second baseline");

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");
        server.latency_critical_client_id = Some(1);

        assert!(server.render_retained_pty_update_and_stream());
        let first_frame = read_server_frame(
            first_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("active retained frame"),
        );
        assert!(first_frame.cells.iter().any(|cell| cell.symbol == "Z"));
        let second_frame = read_server_frame(
            second_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("mirror retained frame"),
        );
        assert!(second_frame.cells.iter().any(|cell| cell.symbol == "Z"));
        assert!(server.latency_critical_client_id.is_none());
    }

    #[tokio::test]
    async fn retained_pty_update_declines_while_toast_is_visible() {
        let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
        server.app.state.toast = Some(crate::app::state::ToastNotification {
            kind: crate::app::state::ToastKind::NeedsAttention,
            title: "pi needs attention".to_owned(),
            context: "background · 2".to_owned(),
            target: None,
        });
        server.render_and_stream();
        let initial = read_server_frame(
            client_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("initial frame"),
        );
        assert!(
            frame_text(&initial).contains("pi needs attention"),
            "expected initial full frame to include toast text"
        );

        let toast_row = server.app.state.view.toast_hit_area.y;
        let inner_rect = server.app.state.view.pane_infos[0].inner_rect;
        let pane_row = toast_row
            .checked_sub(inner_rect.y)
            .expect("toast should overlap the pane")
            + 1;
        assert!(pane_row <= inner_rect.height);
        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(format!("\x1b[{pane_row};1Hzzzz").as_bytes());

        assert!(!server.render_retained_pty_update_and_stream());
        assert!(
            client_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "retained path should not stream a frame that can overwrite toast cells"
        );
    }

    #[tokio::test]
    async fn retained_pty_update_matches_full_render_frame() {
        let initial = b"\x1b[6 qleft \xe4\xb8\xad";
        let update = b"\r\x1b[44mZ\x1b[0m";
        let (mut retained_server, retained_rx, retained_pane_id) = retained_test_server(initial);
        let (mut full_server, full_rx, full_pane_id) = retained_test_server(initial);

        retained_server.render_and_stream();
        let _ = retained_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial retained baseline");
        full_server.render_and_stream();
        let _ = full_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial full baseline");

        retained_server
            .app
            .state
            .runtime_for_pane_in_session_at(
                &retained_server.app.terminal_runtimes,
                0,
                retained_pane_id,
            )
            .expect("retained runtime")
            .test_process_pty_bytes(update);
        full_server
            .app
            .state
            .runtime_for_pane_in_session_at(&full_server.app.terminal_runtimes, 0, full_pane_id)
            .expect("full runtime")
            .test_process_pty_bytes(update);

        assert!(retained_server.render_retained_pty_update_and_stream());
        full_server.render_and_stream();

        let retained_frame = read_server_frame(
            retained_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("retained frame"),
        );
        let full_frame = read_server_frame(
            full_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("full frame"),
        );
        assert_frame_data_eq(&retained_frame, &full_frame);
    }

    #[tokio::test]
    async fn retained_pty_update_streams_cursor_only_change() {
        let initial = b"abcd";
        let update = b"\x1b[D";
        let (mut retained_server, retained_rx, retained_pane_id) = retained_test_server(initial);
        let (mut full_server, full_rx, full_pane_id) = retained_test_server(initial);

        retained_server.render_and_stream();
        let _ = retained_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial retained baseline");
        full_server.render_and_stream();
        let _ = full_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial full baseline");

        retained_server
            .app
            .state
            .runtime_for_pane_in_session_at(
                &retained_server.app.terminal_runtimes,
                0,
                retained_pane_id,
            )
            .expect("retained runtime")
            .test_process_pty_bytes(update);
        full_server
            .app
            .state
            .runtime_for_pane_in_session_at(&full_server.app.terminal_runtimes, 0, full_pane_id)
            .expect("full runtime")
            .test_process_pty_bytes(update);

        assert!(retained_server.render_retained_pty_update_and_stream());
        full_server.render_and_stream();

        let retained_frame = read_server_frame(
            retained_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("retained cursor frame"),
        );
        let full_frame = read_server_frame(
            full_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("full cursor frame"),
        );
        assert_frame_data_eq(&retained_frame, &full_frame);
    }

    #[tokio::test]
    async fn retained_pty_update_declines_unsafe_mode_without_consuming_dirty_rows() {
        let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial frame");
        server
            .clients
            .get(&1)
            .unwrap()
            .render_actor
            .as_ref()
            .unwrap()
            .wait_for_last_frame(Duration::from_millis(100))
            .expect("initial baseline");

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");

        server.app.state.mode = crate::app::Mode::Navigate;
        assert!(!server.render_retained_pty_update_and_stream());
        assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());

        server.app.state.mode = crate::app::Mode::Terminal;
        assert!(server.render_retained_pty_update_and_stream());
        let patched = read_server_frame(
            client_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("retained frame after safe mode"),
        );
        assert!(patched.cells.iter().any(|cell| cell.symbol == "Z"));
    }

    #[tokio::test]
    async fn headless_full_render_clears_full_redraw_pending_for_future_retained_updates() {
        let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
        server.app.full_redraw_pending = true;

        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("full redraw frame");
        assert!(!server.app.full_redraw_pending);
        server
            .clients
            .get(&1)
            .unwrap()
            .render_actor
            .as_ref()
            .unwrap()
            .wait_for_last_frame(Duration::from_millis(100))
            .expect("full redraw baseline");

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");

        assert!(server.render_retained_pty_update_and_stream());
    }

    #[tokio::test]
    async fn retained_pty_update_declines_when_patch_would_stale_hyperlinks() {
        let (mut server, client_rx, pane_id) = retained_test_server(b"link");
        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial frame");
        let inner_rect = server.app.state.view.pane_infos[0].inner_rect;
        let client = server.clients.get_mut(&1).unwrap();
        let mut frame = client
            .render_actor
            .as_ref()
            .and_then(|render_actor| render_actor.wait_for_last_frame(Duration::from_millis(100)))
            .unwrap();
        frame.hyperlinks = vec!["https://example.com".to_owned()];
        let hyperlink_idx =
            usize::from(inner_rect.y) * usize::from(frame.width) + usize::from(inner_rect.x);
        frame.cells[hyperlink_idx].hyperlink = Some(0);
        server.store_app_frame_snapshot(frame, ServerRenderDebug::default());

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rplain");

        assert!(!server.render_retained_pty_update_and_stream());
        assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());

        server.render_and_stream();
        let full = read_server_frame(
            client_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("full frame after hyperlink overwrite"),
        );
        assert!(
            full.cells.iter().all(|cell| cell.hyperlink.is_none()),
            "full render should clear overwritten hyperlink cells"
        );
    }

    #[tokio::test]
    async fn retained_pty_update_allows_kitty_enabled_empty_graphics_cache() {
        let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
        server.app.state.kitty_graphics_enabled = true;
        server.clients.get_mut(&1).unwrap().cell_size = crate::kitty_graphics::HostCellSize {
            width_px: 10,
            height_px: 20,
        };

        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial frame");

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");

        assert!(server.render_retained_pty_update_and_stream());
        let retained = read_server_frame(
            client_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("retained frame with kitty enabled"),
        );
        assert!(retained.cells.iter().any(|cell| cell.symbol == "Z"));
    }

    #[tokio::test]
    async fn retained_pty_update_declines_when_graphics_cache_has_content() {
        let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
        server.app.state.kitty_graphics_enabled = true;
        let client = server.clients.get_mut(&1).unwrap();
        client.cell_size = crate::kitty_graphics::HostCellSize {
            width_px: 10,
            height_px: 20,
        };

        server.render_and_stream();
        let _ = client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial frame");
        server
            .clients
            .get_mut(&1)
            .unwrap()
            .graphics_cache
            .test_mark_non_empty();

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");

        assert!(!server.render_retained_pty_update_and_stream());
        assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[tokio::test]
    async fn full_redraw_pending_clears_after_latest_slot_publish() {
        let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
        let queued = HeadlessServer::frame_server_message(&ServerMessage::ReloadClientConfig)
            .expect("serialize dummy message");
        server
            .clients
            .get(&1)
            .unwrap()
            .writer
            .as_ref()
            .unwrap()
            .render
            .send(queued)
            .expect("pre-fill render queue");
        server.app.full_redraw_pending = true;

        server.render_and_stream();

        assert!(!server.app.full_redraw_pending);
        let _ =
            recv_server_frame_within(&client_rx, Duration::from_millis(100), "full redraw frame");

        let runtime = server
            .app
            .state
            .runtime_for_pane_in_session_at(&server.app.terminal_runtimes, 0, pane_id)
            .expect("runtime");
        runtime.test_process_pty_bytes(b"\rZ");

        assert!(server.render_retained_pty_update_and_stream());
        assert!(matches!(
            read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()),
            ServerMessage::Frame(_)
        ));
    }

    #[test]
    fn client_config_reload_request_refreshes_attached_clients() {
        let mut server = test_headless_server();
        let (client_tx, client_control_rx, _client_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(client_tx),
            ),
        );
        server.app.state.request_client_config_reload = true;

        server.drain_client_config_reload_request();

        match read_server_message(
            client_control_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("client config reload message"),
        ) {
            ServerMessage::ReloadClientConfig => {}
            other => panic!("expected ReloadClientConfig, got {other:?}"),
        }
        assert!(!server.app.state.request_client_config_reload);
    }

    #[test]
    fn clipboard_write_targets_foreground_client_only() {
        let mut server = test_headless_server();
        let (background_tx, background_control_rx, _background_rx) = test_client_writer();
        let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(background_tx),
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(foreground_tx),
            ),
        );
        server.foreground_client_id = Some(2);
        server.sync_foreground_client_state();

        let changed = server.handle_internal_event_with_forwarding(AppEvent::ClipboardWrite {
            content: b"test".to_vec(),
        });

        assert!(changed);
        assert_eq!(
            server
                .app
                .state
                .copy_feedback
                .as_ref()
                .map(|feedback| feedback.message.as_str()),
            Some("copied to clipboard")
        );
        match read_server_message(
            foreground_control_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("foreground clipboard message"),
        ) {
            ServerMessage::Clipboard { data } => assert_eq!(data, "dGVzdA=="),
            other => panic!("expected clipboard message, got {other:?}"),
        }
        assert!(
            background_control_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "background client should not receive clipboard writes"
        );
    }

    #[test]
    fn clipboard_write_without_foreground_client_does_not_show_feedback() {
        let mut server = test_headless_server();
        server.foreground_client_id = None;

        let changed = server.handle_internal_event_with_forwarding(AppEvent::ClipboardWrite {
            content: b"test".to_vec(),
        });

        assert!(changed);
        assert!(
            server.app.state.copy_feedback.is_none(),
            "clipboard feedback should only show when a foreground client can receive the write"
        );
    }

    #[test]
    fn clipboard_write_failed_foreground_send_does_not_show_feedback() {
        let mut server = test_headless_server();
        let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();
        drop(foreground_control_rx);

        server.clients.insert(
            1,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(foreground_tx),
            ),
        );
        server.foreground_client_id = Some(1);

        let changed = server.handle_internal_event_with_forwarding(AppEvent::ClipboardWrite {
            content: b"test".to_vec(),
        });

        assert!(changed);
        assert!(
            server.app.state.copy_feedback.is_none(),
            "clipboard feedback should only show after the foreground client receives the write"
        );
        assert!(
            !server.clients.contains_key(&1),
            "failed targeted send should remove the broken foreground client"
        );
    }

    #[test]
    fn client_local_notifications_target_foreground_client_only() {
        let mut server = test_headless_server();
        let (background_tx, background_control_rx, _background_rx) = test_client_writer();
        let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();

        server.clients.insert(
            1,
            ClientConnection::new(
                (120, 40),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                1,
                RenderEncoding::SemanticFrame,
                Some(background_tx),
            ),
        );
        server.clients.insert(
            2,
            ClientConnection::new(
                (80, 24),
                crate::kitty_graphics::HostCellSize::default(),
                crate::terminal_theme::TerminalTheme::default(),
                None,
                2,
                RenderEncoding::SemanticFrame,
                Some(foreground_tx),
            ),
        );
        server.foreground_client_id = Some(2);
        server.sync_foreground_client_state();

        assert!(server.send_to_foreground_client(ServerMessage::Notify {
            kind: protocol::NotifyKind::Toast,
            message: "pi finished: workspace 1".to_string(),
        }));

        match read_server_message(
            foreground_control_rx
                .recv_timeout(Duration::from_millis(100))
                .expect("foreground toast message"),
        ) {
            ServerMessage::Notify { kind, message } => {
                assert_eq!(kind, protocol::NotifyKind::Toast);
                assert_eq!(message, "pi finished: workspace 1");
            }
            other => panic!("expected toast notify, got {other:?}"),
        }
        assert!(
            background_control_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "background client should not receive client-local notifications"
        );
    }

    /// Verify that no direct calls to `self.app.handle_internal_event`
    /// exist outside of `handle_internal_event_with_forwarding` in this
    /// module. This ensures the forwarding bypass cannot be reintroduced.
    ///
    /// The search pattern looks for `handle_internal_event` calls that
    /// are NOT inside the `handle_internal_event_with_forwarding` method.
    #[test]
    fn no_handle_internal_event_bypass_in_module() {
        let source = include_str!("headless.rs");

        // Find all lines containing handle_internal_event
        let mut bypass_lines: Vec<String> = Vec::new();
        let mut inside_forwarding_method = false;
        let mut forwarding_method_brace_depth = 0u32;

        for (i, line) in source.lines().enumerate() {
            let line_num = i + 1;

            // Track when we're inside handle_internal_event_with_forwarding
            if line.contains("fn handle_internal_event_with_forwarding") {
                inside_forwarding_method = true;
                forwarding_method_brace_depth = 0;
            }

            if inside_forwarding_method {
                // Count braces to track when we exit the method
                for ch in line.chars() {
                    match ch {
                        '{' => forwarding_method_brace_depth += 1,
                        '}' => {
                            forwarding_method_brace_depth =
                                forwarding_method_brace_depth.saturating_sub(1);
                            if forwarding_method_brace_depth == 0 {
                                inside_forwarding_method = false;
                            }
                        }
                        _ => {}
                    }
                }
            } else if line.contains("self.app.handle_internal_event(")
                && !line.trim().starts_with("///")
                && !line.contains("contains(")
            {
                // Direct call to handle_internal_event outside the forwarding method
                bypass_lines.push(format!("line {}: {}", line_num, line.trim()));
            }
        }

        assert!(
            bypass_lines.is_empty(),
            "Found direct calls to self.app.handle_internal_event outside \
             handle_internal_event_with_forwarding (bypass risk):\n  {}",
            bypass_lines.join("\n  ")
        );
    }
}
