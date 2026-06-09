//! Application orchestration.
//!
//! - `state.rs` — AppState, Mode, and pure data structs
//! - `actions.rs` — state mutations (testable without PTYs/async)
//! - `input.rs` — key/mouse → action translation

pub(crate) mod actions;
mod api;
mod api_helpers;
mod config_io;
mod creation;
mod ids;
mod input;
mod runtime;
mod session;
pub(crate) mod settings_catalog;
pub mod state;
mod theme_sync;

use std::collections::{HashMap, HashSet};
use std::future::pending;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(8);
pub(crate) const ANIMATION_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const HEADLESS_ANIMATION_INTERVAL: Duration = Duration::from_millis(128);
pub(crate) const HEADLESS_ANIMATION_TICK_STEP: u32 = 8;
pub(crate) const SELECTION_AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(30);
const RESIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SESSION_SAVE_DEBOUNCE: Duration = Duration::from_secs(5);
const SIDEBAR_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);
const PANE_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);
const PANE_COPY_HIGHLIGHT_DURATION: Duration = Duration::from_millis(500);
const COPY_FEEDBACK_DURATION: Duration = Duration::from_secs(2);

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute, terminal,
};
use ratatui::layout::Rect;
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, Notify};
use tracing::info;

use crate::config::Config;
use crate::events::AppEvent;

pub use state::{AppState, Mode, ToastKind, ViewState};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClientInputRouteResult {
    pub(crate) visual_change: bool,
    pub(crate) forwarded_to_pty: bool,
}

/// Full application: AppState + runtime concerns (event channels, async I/O).
#[derive(Debug, Clone)]
pub(crate) struct OverlayPaneState {
    ws_idx: usize,
    tab_idx: usize,
    previous_focus: crate::layout::PaneId,
    previous_zoomed: bool,
    temp_files: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneClickState {
    pane_id: crate::layout::PaneId,
    viewport_row: u16,
    col: u16,
    at: Instant,
}

impl PaneClickState {
    fn is_double_click_for(self, next: Self) -> bool {
        self.pane_id == next.pane_id
            && next.at.duration_since(self.at) <= PANE_DOUBLE_CLICK_WINDOW
            && self.viewport_row.abs_diff(next.viewport_row) <= 1
            && self.col.abs_diff(next.col) <= 1
    }
}

pub struct App {
    pub state: AppState,
    pub(crate) terminal_runtimes: crate::terminal::TerminalRuntimeRegistry,
    pub event_tx: mpsc::Sender<AppEvent>,
    pub(crate) event_rx: mpsc::Receiver<AppEvent>,
    pub(crate) api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
    pub(crate) event_hub: crate::api::EventHub,
    pub(crate) last_focus: Option<(usize, crate::layout::PaneId)>,
    pub(crate) no_session: bool,
    pub(crate) input_rx: Option<mpsc::Receiver<crate::raw_input::RawInputEvent>>,
    pub(crate) last_terminal_size: Option<(u16, u16)>,
    pub(crate) config_diagnostic_deadline: Option<Instant>,
    pub(crate) toast_deadline: Option<Instant>,
    pub(crate) copy_feedback_deadline: Option<Instant>,
    pub(crate) last_sidebar_divider_click: Option<Instant>,
    pub(crate) last_pane_click: Option<PaneClickState>,
    pub(crate) next_resize_poll: Instant,
    pub(crate) next_animation_tick: Option<Instant>,
    pub(crate) selection_autoscroll_deadline: Option<Instant>,
    pub(crate) selection_highlight_clear_deadline: Option<Instant>,
    pub(crate) session_save_deadline: Option<Instant>,
    pub(crate) persist_pane_history: bool,
    pub(crate) last_render_at: Option<Instant>,
    pub(crate) suppressed_repeat_keys:
        HashSet<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)>,
    pub render_notify: Arc<Notify>,
    pub render_dirty: Arc<AtomicBool>,
    pub(crate) full_redraw_pending: bool,
    pub(crate) input_render_bypass_pending: bool,
    pub(crate) overlay_panes: HashMap<crate::layout::PaneId, OverlayPaneState>,
    pub(crate) local_terminal_notifications: bool,
    pub(crate) config_reloaded_from_disk: bool,
    prefix_input_source: Box<dyn crate::platform::PrefixInputSource>,
}

pub(crate) const APP_EVENT_CHANNEL_CAPACITY: usize = 256;
pub(crate) const APP_EVENT_DRAIN_LIMIT: usize = 64;

pub(crate) enum LoopEvent {
    Timer,
    Internal(AppEvent),
    Api(crate::api::ApiRequestMessage),
    RawInput(crate::raw_input::RawInputEvent),
    InputClosed,
    RenderRequested,
}

struct SyncOutputGuard;

impl SyncOutputGuard {
    fn begin() -> io::Result<Self> {
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\x1b[?2026h")?;
        stdout.flush()?;
        Ok(Self)
    }
}

impl Drop for SyncOutputGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(b"\x1b[?2026l");
        let _ = stdout.flush();
    }
}

async fn recv_raw_input_or_pending(
    input_rx: Option<&mut mpsc::Receiver<crate::raw_input::RawInputEvent>>,
) -> Option<crate::raw_input::RawInputEvent> {
    match input_rx {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}

async fn sleep_until_or_pending(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
        None => pending().await,
    }
}

fn repeat_key_identity(
    key: &crate::input::TerminalKey,
) -> (crossterm::event::KeyCode, crossterm::event::KeyModifiers) {
    (key.code, key.modifiers)
}

fn pane_panel_scope_from_config(
    scope: crate::config::PanePanelScopeConfig,
) -> state::PanePanelScope {
    match scope {
        crate::config::PanePanelScopeConfig::Current => state::PanePanelScope::Current,
        crate::config::PanePanelScopeConfig::All => state::PanePanelScope::All,
    }
}

/// Resolve the palette from config: base theme + optional custom overrides.
fn resolve_palette(config: &crate::config::Config) -> state::Palette {
    resolve_palette_with_legacy_accent(config, true)
}

fn resolve_palette_with_legacy_accent(
    config: &crate::config::Config,
    use_legacy_ui_accent: bool,
) -> state::Palette {
    // Start with the named theme (default: catppuccin)
    let base_name = config.theme.name.as_deref().unwrap_or("catppuccin");
    let mut palette = state::Palette::from_name(base_name).unwrap_or_else(|| {
        tracing::warn!(
            theme = base_name,
            "unknown theme, falling back to catppuccin"
        );
        state::Palette::catppuccin()
    });

    // Apply custom overrides if present
    if let Some(custom) = &config.theme.custom {
        palette = palette.with_overrides(custom);
    }

    // Legacy: if ui.accent is set and no theme.custom.accent, use it for compat
    if use_legacy_ui_accent
        && config.ui.accent != "cyan"
        && config
            .theme
            .custom
            .as_ref()
            .and_then(|c| c.accent.as_ref())
            .is_none()
    {
        palette.accent = crate::config::parse_color(&config.ui.accent);
    }

    palette
}

impl App {
    pub fn new(
        config: &Config,
        no_session: bool,
        config_diagnostic: Option<String>,
        api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
        event_hub: crate::api::EventHub,
    ) -> Self {
        let (prefix_code, prefix_mods) = config.prefix_key();
        crate::kitty_graphics::set_enabled(config.experimental.kitty_graphics);
        let (event_tx, event_rx) = mpsc::channel::<AppEvent>(APP_EVENT_CHANNEL_CAPACITY);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(AtomicBool::new(false));

        // Try to restore previous session
        let mut restored_terminals = std::collections::HashMap::new();
        let mut restored_terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let restored_session = if no_session {
            None
        } else if let Some(snap) = crate::persist::load() {
            let history = config
                .experimental
                .pane_history
                .then(crate::persist::load_history)
                .flatten();
            let (restored_session, terminals, terminal_runtimes) = crate::persist::restore(
                &snap,
                history.as_ref(),
                24,
                80,
                config.advanced.scrollback_limit_bytes,
                &config.terminal.default_shell,
                config.terminal.shell_mode,
                &config.terminal.term,
                event_tx.clone(),
                render_notify.clone(),
                render_dirty.clone(),
            );
            restored_terminals = terminals;
            restored_terminal_runtimes = terminal_runtimes.into();
            if let Some(restored_session) = restored_session {
                let restored_tabs = restored_session.tabs.len();
                crate::logging::session_restored(restored_tabs, "ok");
                Some(restored_session)
            } else {
                crate::logging::session_restored(0, "empty");
                None
            }
        } else {
            None
        };

        let pane_panel_scope = pane_panel_scope_from_config(config.ui.pane_panel_scope);

        // Validate sidebar bounds before they reach any `u16::clamp(min, max)`
        // call: `clamp` panics when `min > max`. On bad config, fall back to
        // the built-in defaults rather than crashing on the first render.
        let (sidebar_min_width, sidebar_max_width) = crate::config::validated_sidebar_bounds(
            config.ui.sidebar_min_width,
            config.ui.sidebar_max_width,
        )
        .unwrap_or_else(|| {
            tracing::warn!(
                min = config.ui.sidebar_min_width,
                max = config.ui.sidebar_max_width,
                "ui.sidebar_min_width is greater than sidebar_max_width; falling back to default bounds (18, 36)"
            );
            (18, 36)
        });

        info!(
            pane_scrollback_limit_bytes = config.advanced.scrollback_limit_bytes,
            "using pane scrollback configuration"
        );

        let mode = if config.should_show_onboarding() {
            state::Mode::Onboarding
        } else if restored_session.is_some() {
            state::Mode::Terminal
        } else {
            state::Mode::Navigate
        };

        let mut state = AppState {
            terminals: std::collections::HashMap::new(),
            direct_attach_resize_locks: std::collections::HashSet::new(),
            pane_id_aliases: std::collections::HashMap::new(),
            sessions: Vec::new(),
            active_session: None,
            previous_pane_focus: None,
            selected_session: 0,
            mode,
            should_quit: false,
            detach_exits: no_session,
            detach_requested: false,
            request_new_tab: false,
            request_reload_config: false,
            request_client_config_reload: false,
            request_clipboard_write: None,
            creating_new_tab: false,
            requested_new_tab_name: None,
            rename_pane_target: None,
            request_complete_onboarding: false,
            name_input: String::new(),
            name_input_replace_on_type: false,
            keybind_help: state::KeybindHelpState { scroll: 0 },
            navigator: state::NavigatorState::default(),
            copy_mode: None,
            pane_panel_scroll: 0,
            tab_scroll: 0,
            tab_scroll_follow_active: true,
            mobile_switcher_scroll: 0,
            view: state::ViewState {
                layout: state::ViewLayout::Desktop,
                sidebar_rect: Rect::default(),
                tab_bar_rect: Rect::default(),
                tab_hit_areas: Vec::new(),
                tab_scroll_left_hit_area: Rect::default(),
                tab_scroll_right_hit_area: Rect::default(),
                new_tab_hit_area: Rect::default(),
                terminal_area: Rect::default(),
                mobile_header_rect: Rect::default(),
                mobile_menu_hit_area: Rect::default(),
                toast_hit_area: Rect::default(),
                pane_infos: Vec::new(),
                split_borders: Vec::new(),
            },
            drag: None,
            tab_press: None,
            selection: None,
            selection_autoscroll: None,
            context_menu: None,
            config_diagnostic,
            toast: None,
            copy_feedback: None,
            outer_terminal_focus: None,
            prefix_code,
            prefix_mods,
            default_sidebar_width: config.ui.sidebar_width,
            sidebar_width: config.ui.sidebar_width,
            sidebar_min_width,
            sidebar_max_width,
            mobile_width_threshold: config.ui.mobile_width_threshold,
            sidebar_width_source: state::SidebarWidthSource::ConfigDefault,
            sidebar_width_auto: false,
            sidebar_collapsed: false,
            pane_panel_scope,
            mouse_capture: config.ui.mouse_capture,
            right_click_passthrough_modifiers: config.ui.right_click_passthrough_modifiers(),
            right_click_passthrough: None,
            redraw_on_focus_gained: config.ui.redraw_on_focus_gained,
            mouse_scroll_lines: config.ui.mouse_scroll_lines(),
            confirm_close: config.ui.confirm_close,
            prompt_new_tab_name: config.ui.prompt_new_tab_name,
            show_onboarding_on_next_launch: config.should_show_onboarding(),
            allow_nested_gmux: config.experimental.allow_nested,
            pane_history_persistence: config.experimental.pane_history,
            reveal_hidden_cursor_for_cjk_ime: config.experimental.reveal_hidden_cursor_for_cjk_ime,
            cjk_ime_cursor_shape: config.experimental.cjk_ime_cursor_shape.to_decscusr(),
            switch_ascii_input_source_in_prefix: config
                .experimental
                .switch_ascii_input_source_in_prefix,
            kitty_graphics_enabled: config.experimental.kitty_graphics,
            pane_term: config.terminal.term.clone(),
            default_shell: config.terminal.default_shell.clone(),
            shell_mode: config.terminal.shell_mode,
            new_terminal_cwd: config.terminal.new_cwd.clone(),
            pane_scrollback_limit_bytes: config.advanced.scrollback_limit_bytes,
            accent: crate::config::parse_color(&config.ui.accent),
            toast_config: config.ui.toast.clone(),
            remote_manage_ssh_config: config.remote.manage_ssh_config,
            keybinds: config.keybinds(),
            spinner_tick: 0,
            palette: resolve_palette(config),
            theme_name: config
                .theme
                .name
                .clone()
                .unwrap_or_else(|| "catppuccin".to_string()),
            settings: state::SettingsState {
                page: state::SettingsPage::Main,
                list: state::SelectionListState::new(0),
                edit: None,
                original_palette: None,
                original_theme: None,
            },
            global_menu: state::MenuListState::new(0),
            host_terminal_theme: crate::terminal_theme::TerminalTheme::default(),
            session_dirty: false,
            terminal_runtime_shutdowns: Vec::new(),
        };

        state.terminals = restored_terminals;
        if let Some(restored_session) = restored_session {
            state.set_session(restored_session);
        }
        state.collapse_to_single_session();

        let last_focus = state.session_index().and_then(|idx| {
            state
                .session()
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });

        Self {
            config_diagnostic_deadline: None,
            toast_deadline: None,
            copy_feedback_deadline: None,
            state,
            terminal_runtimes: restored_terminal_runtimes,
            event_tx,
            event_rx,
            last_sidebar_divider_click: None,
            last_pane_click: None,
            next_resize_poll: Instant::now() + RESIZE_POLL_INTERVAL,
            next_animation_tick: None,
            session_save_deadline: None,
            selection_autoscroll_deadline: None,
            selection_highlight_clear_deadline: None,
            persist_pane_history: config.experimental.pane_history,
            last_render_at: None,
            suppressed_repeat_keys: HashSet::new(),
            api_rx,
            event_hub,
            last_focus,
            no_session,
            input_rx: None,
            last_terminal_size: terminal::size().ok(),
            render_notify,
            render_dirty,
            full_redraw_pending: false,
            input_render_bypass_pending: false,
            overlay_panes: HashMap::new(),
            local_terminal_notifications: true,
            config_reloaded_from_disk: false,
            prefix_input_source: Box::new(crate::platform::RealPrefixInputSource::default()),
        }
    }

    #[cfg(unix)]
    pub fn new_from_handoff(
        config: &Config,
        config_diagnostic: Option<String>,
        api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
        event_hub: crate::api::EventHub,
        snapshot: &crate::persist::SessionSnapshot,
        imports: &mut std::collections::HashMap<
            u32,
            crate::handoff_runtime::ImportedHandoffRuntime,
        >,
    ) -> io::Result<Self> {
        let mut app = Self::new(config, true, config_diagnostic, api_rx, event_hub);
        let (session, terminals, runtimes) = crate::persist::restore_handoff(
            snapshot,
            config.advanced.scrollback_limit_bytes,
            &config.terminal.default_shell,
            config.terminal.shell_mode,
            &config.terminal.term,
            imports,
            app.event_tx.clone(),
            app.render_notify.clone(),
            app.render_dirty.clone(),
        )?;
        let pane_id_aliases = crate::persist::handoff_pane_aliases(snapshot, session.as_ref());

        app.no_session = false;
        app.state.detach_exits = false;
        app.state.pane_id_aliases = pane_id_aliases;
        app.state.clear_session();
        if let Some(session) = session {
            app.state.set_session(session);
        }
        app.state.terminals = terminals;
        app.terminal_runtimes = runtimes.into();
        app.state.collapse_to_single_session();
        app.state.mode = app.state.terminal_or_navigate_mode();
        app.last_focus = app.state.session_index().and_then(|idx| {
            app.state
                .session()
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
        Ok(app)
    }

    #[cfg(unix)]
    pub fn unpause_handoff_readers(&self) {
        self.terminal_runtimes.set_handoff_readers_paused(false);
    }

    #[cfg(unix)]
    pub fn assume_handoff_ownership(&mut self) {
        self.terminal_runtimes.assume_handoff_ownership();
    }

    fn request_full_redraw(&mut self) {
        self.full_redraw_pending = true;
    }

    pub(crate) fn sync_prefix_input_source(&mut self, previous_mode: Mode) {
        match (
            previous_mode == Mode::Prefix,
            self.state.mode == Mode::Prefix,
        ) {
            (false, true) if self.state.switch_ascii_input_source_in_prefix => {
                self.prefix_input_source.switch_to_ascii();
            }
            (true, false) => self.prefix_input_source.restore(),
            _ => {}
        }
    }

    pub(crate) fn handle_internal_event_with_prefix_sync(
        &mut self,
        event: crate::events::AppEvent,
    ) {
        let previous_mode = self.state.mode;
        self.handle_internal_event(event);
        self.sync_prefix_input_source(previous_mode);
    }

    #[cfg(test)]
    pub(crate) fn set_prefix_input_source(
        &mut self,
        source: Box<dyn crate::platform::PrefixInputSource>,
    ) {
        self.prefix_input_source = source;
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        if self.input_rx.is_none() {
            self.input_rx = Some(crate::raw_input::spawn_input_reader());
        }
        self.query_host_terminal_theme();

        let mut needs_render = true;
        let mut host_mouse_capture_active = self.state.mouse_capture;

        while !self.state.should_quit {
            if self.render_dirty.load(Ordering::Acquire) {
                needs_render = true;
            }

            // Drain a bounded internal-event batch for responsiveness. API handlers
            // perform an exhaustive drain before reading pane/runtime state.
            if self.drain_internal_events() {
                needs_render = true;
            }
            if self.drain_api_requests() {
                needs_render = true;
            }

            self.sync_focus_events();
            self.sync_session_save_schedule();

            let now = Instant::now();
            if self.handle_scheduled_tasks(now, needs_render) {
                needs_render = true;
            }

            if self.state.request_complete_onboarding {
                self.state.request_complete_onboarding = false;
                self.open_settings_from_onboarding();
                needs_render = true;
            }

            if self.state.request_new_tab {
                self.state.request_new_tab = false;
                self.create_tab();
                needs_render = true;
            }

            if self.state.request_reload_config {
                self.state.request_reload_config = false;
                self.reload_config();
                needs_render = true;
            }

            if self.ensure_default_session() {
                needs_render = true;
            }

            let now = Instant::now();
            self.sync_animation_timer(now);
            self.sync_host_mouse_capture(&mut host_mouse_capture_active)?;

            let input_bypass =
                self.input_render_bypass_pending && self.render_dirty.load(Ordering::Acquire);
            if needs_render && (self.can_render_now(now) || input_bypass) {
                let pty_dirty = self.render_dirty.swap(false, Ordering::AcqRel);
                if pty_dirty {
                    self.clear_input_render_bypass_after_pty_dirty();
                }
                let _sync_output = SyncOutputGuard::begin()?;
                let kitty_graphics_enabled = self.state.kitty_graphics_enabled;
                if self.full_redraw_pending {
                    if kitty_graphics_enabled {
                        crate::kitty_graphics::clear_all_host_graphics()?;
                    }
                    terminal.clear()?;
                    self.full_redraw_pending = false;
                }
                let mut cell_size = crate::kitty_graphics::HostCellSize::default();
                terminal.draw(|frame| {
                    let area = frame.area();
                    if kitty_graphics_enabled {
                        cell_size = crate::kitty_graphics::HostCellSize::from_terminal(area);
                        crate::ui::compute_view_with_cell_size(
                            &mut self.state,
                            &self.terminal_runtimes,
                            area,
                            cell_size,
                        );
                    } else {
                        crate::ui::compute_view_with_runtime_registry(
                            &mut self.state,
                            &self.terminal_runtimes,
                            area,
                        );
                    }
                    crate::ui::render_with_runtime_registry(
                        &self.state,
                        &self.terminal_runtimes,
                        frame,
                    );
                })?;
                if kitty_graphics_enabled {
                    crate::kitty_graphics::paint_local_pane_graphics(
                        &self.state,
                        &self.terminal_runtimes,
                        cell_size,
                    )?;
                }
                self.last_render_at = Some(now);
                needs_render = false;
                continue;
            }

            let next_deadline = self.next_loop_deadline(now, needs_render);
            let event = {
                let input_rx = self.input_rx.as_mut();
                tokio::select! {
                    maybe_api = self.api_rx.recv() => match maybe_api {
                        Some(msg) => LoopEvent::Api(msg),
                        None => LoopEvent::Timer,
                    },
                    maybe_ev = self.event_rx.recv() => match maybe_ev {
                        Some(ev) => LoopEvent::Internal(ev),
                        None => LoopEvent::Timer,
                    },
                    maybe_input = recv_raw_input_or_pending(input_rx) => match maybe_input {
                        Some(input) => LoopEvent::RawInput(input),
                        None => LoopEvent::InputClosed,
                    },
                    _ = sleep_until_or_pending(next_deadline) => LoopEvent::Timer,
                    _ = self.render_notify.notified() => LoopEvent::RenderRequested,
                }
            };

            match event {
                LoopEvent::Timer => {}
                LoopEvent::Internal(ev) => {
                    self.handle_internal_event_with_prefix_sync(ev);
                    needs_render = true;
                }
                LoopEvent::Api(msg) => {
                    if self.handle_api_request_message(msg) {
                        needs_render = true;
                    }
                }
                LoopEvent::RawInput(input) => {
                    if self.handle_raw_input_batch(input).await {
                        needs_render = true;
                    }
                }
                LoopEvent::InputClosed => {
                    self.input_rx = None;
                }
                LoopEvent::RenderRequested => {
                    if self.render_dirty.load(Ordering::Acquire) {
                        needs_render = true;
                    }
                }
            }
        }

        // Save session on exit (skip in --no-session mode)
        if !self.no_session {
            self.save_session_now();
        }

        Ok(())
    }

    fn sync_host_mouse_capture(&self, active: &mut bool) -> io::Result<()> {
        let desired = self
            .state
            .should_capture_host_mouse_from(&self.terminal_runtimes);
        if desired == *active {
            return Ok(());
        }
        if desired {
            execute!(io::stdout(), EnableMouseCapture)?;
        } else {
            execute!(io::stdout(), DisableMouseCapture)?;
        }
        *active = desired;
        Ok(())
    }

    pub(crate) fn ensure_default_session(&mut self) -> bool {
        if self.state.has_session() || self.state.mode == Mode::Onboarding {
            return false;
        }

        let previous_mode = self.state.mode;
        let preserve_mode = matches!(previous_mode, Mode::Settings);
        let cwd = self.resolve_new_terminal_cwd(None);

        match self.create_session_with_options(cwd, true) {
            Ok(_) => {
                if preserve_mode {
                    self.state.mode = previous_mode;
                }
                true
            }
            Err(err) => {
                tracing::error!(err = %err, "failed to create default session");
                self.state.mode = Mode::Navigate;
                false
            }
        }
    }

    pub(crate) fn open_settings_from_onboarding(&mut self) {
        self.mark_onboarding_complete();
        self.state.mode = state::Mode::Terminal;
    }

    pub(crate) fn reload_config(&mut self) -> crate::config::ConfigReloadReport {
        self.apply_config_from_disk(true)
    }

    pub(crate) fn take_config_reloaded_from_disk(&mut self) -> bool {
        let reloaded = self.config_reloaded_from_disk;
        self.config_reloaded_from_disk = false;
        reloaded
    }

    pub(crate) fn apply_config_from_disk(
        &mut self,
        notify_success: bool,
    ) -> crate::config::ConfigReloadReport {
        self.config_reloaded_from_disk = true;
        let previous_toast = self.state.toast.clone();
        let report = match crate::config::load_live_config() {
            Ok(loaded) => self.apply_live_config(
                &loaded.config,
                &loaded.diagnostics,
                &loaded.invalid_sections,
                notify_success,
            ),
            Err(diagnostics) => {
                self.state.toast = None;
                self.state.config_diagnostic =
                    crate::config::config_diagnostic_summary(&diagnostics);
                self.config_diagnostic_deadline = None;
                crate::config::ConfigReloadReport {
                    status: crate::config::ConfigReloadStatus::Failed,
                    diagnostics,
                }
            }
        };
        self.sync_toast_deadline(previous_toast);
        report
    }

    fn apply_live_config(
        &mut self,
        config: &crate::config::Config,
        load_diagnostics: &[String],
        invalid_sections: &[String],
        notify_success: bool,
    ) -> crate::config::ConfigReloadReport {
        let mut diagnostics = load_diagnostics.to_vec();
        let invalid_section =
            |section: &str| invalid_sections.iter().any(|invalid| invalid == section);

        self.state.show_onboarding_on_next_launch = config.should_show_onboarding();

        if !invalid_section("keys") {
            match config.live_keybinds() {
                Ok(live) => {
                    self.state.prefix_code = live.prefix.0;
                    self.state.prefix_mods = live.prefix.1;
                    self.state.keybinds = live.keybinds;
                }
                Err(keybind_diagnostics) => {
                    diagnostics.extend(
                        keybind_diagnostics
                            .into_iter()
                            .map(|diagnostic| format!("{diagnostic}; kept current keybinds")),
                    );
                }
            }
        }

        if !invalid_section("ui") {
            // Validate sidebar bounds before they reach any `u16::clamp` call.
            // On `min > max`, treat the entire `[ui]` section as invalid: keep
            // the previous settings and skip the section so the re-clamp below
            // — and every subsequent render/drag — can never panic.
            if crate::config::validated_sidebar_bounds(
                config.ui.sidebar_min_width,
                config.ui.sidebar_max_width,
            )
            .is_none()
            {
                diagnostics.push(format!(
                    "ui.sidebar_min_width ({}) is greater than sidebar_max_width ({}); keeping previous [ui] settings",
                    config.ui.sidebar_min_width, config.ui.sidebar_max_width,
                ));
            } else {
                self.state.default_sidebar_width = config.ui.sidebar_width;
                if self.state.sidebar_width_source == state::SidebarWidthSource::ConfigDefault {
                    self.state.sidebar_width = config.ui.sidebar_width;
                }
                self.state.sidebar_min_width = config.ui.sidebar_min_width;
                self.state.sidebar_max_width = config.ui.sidebar_max_width;
                self.state.mobile_width_threshold = config.ui.mobile_width_threshold;
                // Re-clamp the live width to the new bounds. No source guard — bounds
                // always apply, including to manually adjusted widths.
                self.state.sidebar_width = self
                    .state
                    .sidebar_width
                    .clamp(self.state.sidebar_min_width, self.state.sidebar_max_width);
                self.state.mouse_capture = config.ui.mouse_capture;
                if self.state.redraw_on_focus_gained != config.ui.redraw_on_focus_gained {
                    self.state.request_client_config_reload = true;
                }
                self.state.redraw_on_focus_gained = config.ui.redraw_on_focus_gained;
                self.state.mouse_scroll_lines = config.ui.mouse_scroll_lines();
                self.state.right_click_passthrough_modifiers =
                    config.ui.right_click_passthrough_modifiers();
                self.state.confirm_close = config.ui.confirm_close;
                self.state.prompt_new_tab_name = config.ui.prompt_new_tab_name;
                self.state.pane_panel_scope =
                    pane_panel_scope_from_config(config.ui.pane_panel_scope);
                self.state.pane_panel_scroll = 0;
                self.state.accent = crate::config::parse_color(&config.ui.accent);
                self.state.toast_config = config.ui.toast.clone();
            }
        }

        if !invalid_section("experimental") {
            let was_kitty_graphics_enabled = self.state.kitty_graphics_enabled;
            self.state.allow_nested_gmux = config.experimental.allow_nested;
            self.state.kitty_graphics_enabled = config.experimental.kitty_graphics;
            crate::kitty_graphics::set_enabled(config.experimental.kitty_graphics);
            if was_kitty_graphics_enabled && !config.experimental.kitty_graphics {
                let _ = crate::kitty_graphics::clear_all_host_graphics();
            }
            self.state.reveal_hidden_cursor_for_cjk_ime =
                config.experimental.reveal_hidden_cursor_for_cjk_ime;
            self.state.cjk_ime_cursor_shape =
                config.experimental.cjk_ime_cursor_shape.to_decscusr();
            self.state.switch_ascii_input_source_in_prefix =
                config.experimental.switch_ascii_input_source_in_prefix;
            self.persist_pane_history = config.experimental.pane_history;
            self.state.pane_history_persistence = config.experimental.pane_history;
            if !self.persist_pane_history {
                crate::persist::clear_history();
            }
        }

        if !invalid_section("advanced") {
            self.state.pane_scrollback_limit_bytes = config.advanced.scrollback_limit_bytes;
        }

        if !invalid_section("terminal") {
            self.state.pane_term = config.terminal.term.clone();
            self.state.default_shell = config.terminal.default_shell.clone();
            self.state.shell_mode = config.terminal.shell_mode;
            self.state.new_terminal_cwd = config.terminal.new_cwd.clone();
        }

        if !invalid_section("remote") {
            self.state.remote_manage_ssh_config = config.remote.manage_ssh_config;
        }

        if !invalid_section("theme") {
            self.state.palette = resolve_palette_with_legacy_accent(config, !invalid_section("ui"));
            self.state.theme_name = config
                .theme
                .name
                .clone()
                .unwrap_or_else(|| "catppuccin".to_string());
        }

        let status = if diagnostics.is_empty() {
            crate::config::ConfigReloadStatus::Applied
        } else {
            crate::config::ConfigReloadStatus::Partial
        };

        if diagnostics.is_empty() {
            self.state.config_diagnostic = None;
            self.config_diagnostic_deadline = None;
            if notify_success {
                self.state.toast = Some(crate::app::state::ToastNotification {
                    kind: crate::app::state::ToastKind::Finished,
                    title: "reloaded config".to_string(),
                    context: "using config.toml".to_string(),
                    target: None,
                });
            }
        } else {
            self.state.config_diagnostic = crate::config::config_diagnostic_summary(&diagnostics);
            self.config_diagnostic_deadline = None;
            if notify_success {
                self.state.toast = Some(crate::app::state::ToastNotification {
                    kind: crate::app::state::ToastKind::Finished,
                    title: "reloaded config".to_string(),
                    context: "with warnings".to_string(),
                    target: None,
                });
            }
        }

        crate::config::ConfigReloadReport {
            status,
            diagnostics,
        }
    }
}

// ---------------------------------------------------------------------------
// Input routing for headless server mode
// ---------------------------------------------------------------------------

impl App {
    /// Routes raw input bytes from a client through the existing input pipeline.
    ///
    /// The input bytes are parsed into `RawInputEvent`s and then processed.
    /// In terminal mode, keys are routed through the same semantic
    /// key-handling path as monolithic gmux so they are re-encoded for the
    /// focused pane's negotiated keyboard protocol instead of passing host
    /// terminal escape sequences through unchanged.
    #[cfg(test)]
    pub(crate) fn route_client_input(&mut self, data: Vec<u8>) {
        let events = crate::raw_input::parse_raw_input_bytes_sync(&data);
        self.route_client_events(events, true);
    }

    pub(crate) fn route_client_events(
        &mut self,
        events: Vec<crate::raw_input::RawInputEvent>,
        apply_host_terminal_theme: bool,
    ) -> ClientInputRouteResult {
        let mut result = ClientInputRouteResult::default();
        for event in events {
            let previous_mode = self.state.mode;
            match event {
                crate::raw_input::RawInputEvent::Key(key) => {
                    let key_id = repeat_key_identity(&key);
                    match key.kind {
                        crossterm::event::KeyEventKind::Press => {
                            if self.state.mode == Mode::Terminal {
                                self.suppressed_repeat_keys.remove(&key_id);
                                match self.handle_terminal_key_headless(key) {
                                    input::TerminalInputDispatch::Forwarded => {
                                        result.forwarded_to_pty = true;
                                    }
                                    input::TerminalInputDispatch::HandledByApp => {
                                        result.visual_change = true;
                                    }
                                    input::TerminalInputDispatch::Ignored => {}
                                }
                            } else {
                                self.suppressed_repeat_keys.insert(key_id);
                                self.handle_non_terminal_key(key);
                                result.visual_change = true;
                            }
                        }
                        crossterm::event::KeyEventKind::Repeat => {
                            if self.state.mode == Mode::Terminal
                                && !self.suppressed_repeat_keys.contains(&key_id)
                            {
                                match self.handle_terminal_key_headless(key) {
                                    input::TerminalInputDispatch::Forwarded => {
                                        result.forwarded_to_pty = true;
                                    }
                                    input::TerminalInputDispatch::HandledByApp => {
                                        result.visual_change = true;
                                    }
                                    input::TerminalInputDispatch::Ignored => {}
                                }
                            }
                            // Repeats in non-terminal modes are ignored
                            // (same as monolithic behavior).
                        }
                        crossterm::event::KeyEventKind::Release => {
                            self.suppressed_repeat_keys.remove(&key_id);
                        }
                    }
                }
                crate::raw_input::RawInputEvent::Mouse(mouse) => {
                    let forwarded_to_pty = if self.state.mouse_capture {
                        let forwarded_to_pty = self.mouse_event_would_forward_to_pty(mouse);
                        self.handle_mouse_event_headless(mouse);
                        forwarded_to_pty
                    } else {
                        self.state
                            .handle_pane_mouse_only(&self.terminal_runtimes, mouse)
                    };
                    if forwarded_to_pty {
                        self.arm_input_render_bypass();
                        result.forwarded_to_pty = true;
                    } else {
                        result.visual_change = true;
                    }
                }
                crate::raw_input::RawInputEvent::Paste(text) => {
                    if self.state.mode == Mode::Terminal {
                        if let Some(runtime) = self
                            .state
                            .focused_runtime_in_session(&self.terminal_runtimes)
                        {
                            let sent = runtime.try_send_bytes(bytes::Bytes::from(
                                if runtime
                                    .input_state()
                                    .map(|s| s.bracketed_paste)
                                    .unwrap_or(false)
                                {
                                    format!("\x1b[200~{text}\x1b[201~")
                                } else {
                                    text
                                },
                            ));
                            if sent.is_ok() {
                                self.arm_input_render_bypass();
                                result.forwarded_to_pty = true;
                            }
                        }
                    }
                }
                crate::raw_input::RawInputEvent::OuterFocusGained
                | crate::raw_input::RawInputEvent::OuterFocusLost => {}
                crate::raw_input::RawInputEvent::HostDefaultColor { kind, color } => {
                    if apply_host_terminal_theme {
                        result.visual_change |= self.update_host_terminal_theme(kind, color);
                    }
                }
                crate::raw_input::RawInputEvent::Unsupported => {}
            }
            self.sync_prefix_input_source(previous_mode);
        }
        result
    }

    /// Handles a key event in non-terminal mode for the headless server.
    ///
    /// Uses the standalone handler functions that work on `&mut AppState`
    /// since the server doesn't have the async context of the monolithic App.
    fn handle_non_terminal_key(&mut self, key: crate::input::TerminalKey) {
        let key_event = key.as_key_event();
        match self.state.mode {
            Mode::Prefix => {
                self.handle_prefix_key(key);
            }
            Mode::Navigate => {
                self.handle_navigate_key(key);
            }
            Mode::Copy => {
                self.handle_copy_mode_key(key);
            }
            Mode::RenameTab | Mode::RenamePane => {
                input::handle_rename_key(&mut self.state, key_event);
            }
            Mode::Resize => {
                input::handle_resize_key(&mut self.state, key);
            }
            Mode::ConfirmClose => {
                input::handle_confirm_close_key(&mut self.state, key_event);
            }
            Mode::ContextMenu => {
                input::handle_context_menu_key(
                    &mut self.state,
                    &mut self.terminal_runtimes,
                    key_event,
                );
            }
            Mode::KeybindHelp => {
                input::handle_keybind_help_key(&mut self.state, key_event);
            }
            Mode::GlobalMenu => {
                input::handle_global_menu_key(&mut self.state, key_event);
            }
            Mode::Onboarding => {
                self.handle_onboarding_key(key_event);
            }
            Mode::Settings => {
                self.handle_settings_key(key_event);
            }
            Mode::Navigator => {
                input::handle_navigator_key(&mut self.state, &self.terminal_runtimes, key_event);
            }
            Mode::Terminal => {
                // Should not be called in terminal mode.
            }
        }
    }

    /// Handles a mouse event for the headless server.
    ///
    /// Delegates to the same mouse handling logic used in the monolithic
    /// mode (hit-testing against the rendered UI), which works because
    /// the server's AppState maintains view geometry from virtual rendering.
    fn handle_mouse_event_headless(&mut self, mouse: crossterm::event::MouseEvent) {
        self.handle_mouse(mouse);
    }

    fn mouse_event_would_forward_to_pty(&self, mouse: crossterm::event::MouseEvent) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};

        if self.state.mode != Mode::Terminal {
            return false;
        }

        if self.state.selection.is_some()
            && matches!(
                mouse.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            )
        {
            return false;
        }

        let Some(info) = self.state.view.pane_infos.iter().find(|pane| {
            mouse.column >= pane.inner_rect.x
                && mouse.column < pane.inner_rect.x + pane.inner_rect.width
                && mouse.row >= pane.inner_rect.y
                && mouse.row < pane.inner_rect.y + pane.inner_rect.height
        }) else {
            return false;
        };
        let Some(ws_idx) = self.state.session_index() else {
            return false;
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_session_at(&self.terminal_runtimes, ws_idx, info.id)
        else {
            return false;
        };
        let column = mouse.column.saturating_sub(info.inner_rect.x);
        let row = mouse.row.saturating_sub(info.inner_rect.y);

        match mouse.kind {
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => match runtime.wheel_routing() {
                Some(crate::pane::WheelRouting::MouseReport) => runtime
                    .encode_mouse_wheel(mouse.kind, column, row, mouse.modifiers)
                    .is_some(),
                Some(crate::pane::WheelRouting::AlternateScroll) => {
                    runtime.encode_alternate_scroll(mouse.kind).is_some()
                }
                Some(crate::pane::WheelRouting::HostScroll) | None => false,
            },
            MouseEventKind::Down(_) | MouseEventKind::Up(_) | MouseEventKind::Drag(_) => runtime
                .encode_mouse_button(mouse.kind, column, row, mouse.modifiers)
                .is_some(),
            MouseEventKind::Moved => runtime
                .encode_mouse_motion(mouse.kind, column, row, mouse.modifiers)
                .is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::terminal::TerminalRuntime;
    use crate::workspace::Workspace;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Mutex;

    fn raw_key(
        code: KeyCode,
        modifiers: KeyModifiers,
        kind: KeyEventKind,
    ) -> crate::raw_input::RawInputEvent {
        crate::raw_input::RawInputEvent::Key(
            crate::input::TerminalKey::new(code, modifiers).with_kind(kind),
        )
    }

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    #[derive(Clone, Default)]
    struct FakePrefixInputSource {
        switch_calls: Rc<Cell<usize>>,
        restore_calls: Rc<Cell<usize>>,
        switched: Rc<Cell<bool>>,
        will_switch: bool,
    }

    impl FakePrefixInputSource {
        fn switching() -> Self {
            Self {
                will_switch: true,
                ..Self::default()
            }
        }

        fn no_op() -> Self {
            Self {
                will_switch: false,
                ..Self::default()
            }
        }
    }

    impl crate::platform::PrefixInputSource for FakePrefixInputSource {
        fn switch_to_ascii(&mut self) {
            self.switch_calls.set(self.switch_calls.get() + 1);
            if self.will_switch {
                self.switched.set(true);
            }
        }

        fn restore(&mut self) {
            if self.switched.replace(false) {
                self.restore_calls.set(self.restore_calls.get() + 1);
            }
        }
    }

    #[test]
    fn sync_prefix_input_source_switches_then_restores_when_enabled() {
        let mut app = test_app();
        app.state.switch_ascii_input_source_in_prefix = true;
        let fake = FakePrefixInputSource::switching();
        let switch_calls = fake.switch_calls.clone();
        let restore_calls = fake.restore_calls.clone();
        app.set_prefix_input_source(Box::new(fake));

        // Terminal -> Prefix should switch to ASCII.
        app.state.mode = Mode::Prefix;
        app.sync_prefix_input_source(Mode::Terminal);
        assert_eq!(switch_calls.get(), 1);
        assert_eq!(restore_calls.get(), 0);

        // Prefix -> Terminal should restore the saved source.
        app.state.mode = Mode::Terminal;
        app.sync_prefix_input_source(Mode::Prefix);
        assert_eq!(switch_calls.get(), 1);
        assert_eq!(restore_calls.get(), 1);
    }

    #[test]
    fn sync_prefix_input_source_is_noop_when_flag_disabled() {
        let mut app = test_app();
        app.state.switch_ascii_input_source_in_prefix = false;
        let fake = FakePrefixInputSource::switching();
        let switch_calls = fake.switch_calls.clone();
        let restore_calls = fake.restore_calls.clone();
        app.set_prefix_input_source(Box::new(fake));

        app.state.mode = Mode::Prefix;
        app.sync_prefix_input_source(Mode::Terminal);
        app.state.mode = Mode::Terminal;
        app.sync_prefix_input_source(Mode::Prefix);

        assert_eq!(switch_calls.get(), 0);
        assert_eq!(restore_calls.get(), 0);
    }

    #[test]
    fn sync_prefix_input_source_restore_is_safe_when_switch_was_noop() {
        // Simulates the already-ASCII / failed-switch case: switch reports no
        // change, and the later restore on leave must stay harmless.
        let mut app = test_app();
        app.state.switch_ascii_input_source_in_prefix = true;
        let fake = FakePrefixInputSource::no_op();
        let switch_calls = fake.switch_calls.clone();
        let restore_calls = fake.restore_calls.clone();
        app.set_prefix_input_source(Box::new(fake));

        app.state.mode = Mode::Prefix;
        app.sync_prefix_input_source(Mode::Terminal);
        app.state.mode = Mode::Terminal;
        app.sync_prefix_input_source(Mode::Prefix);

        assert_eq!(switch_calls.get(), 1);
        assert_eq!(restore_calls.get(), 0);
    }

    #[test]
    fn create_session_with_existing_sessions_collapses_instead_of_appending() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.state.active_session = None;
        app.state.selected_session = 1;

        let idx = app
            .create_session_with_options(std::path::PathBuf::from("/tmp"), true)
            .expect("existing session should be reused");

        assert_eq!(idx, 0);
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].tabs.len(), 2);
    }

    #[tokio::test]
    async fn raw_input_dispatch_restores_input_source_when_leaving_prefix() {
        // Leaving prefix mode happens inside the raw-input dispatch, not in
        // `handle_key` itself — the sync must sit at the dispatch layer so any
        // event that exits prefix (here Esc) still restores the host source.
        let mut app = test_app();
        app.state.switch_ascii_input_source_in_prefix = true;
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;
        let fake = FakePrefixInputSource::switching();
        let switch_calls = fake.switch_calls.clone();
        let restore_calls = fake.restore_calls.clone();
        app.set_prefix_input_source(Box::new(fake));

        // ctrl+b (the default prefix key) enters prefix mode → switch edge.
        app.handle_raw_input_event(raw_key(
            KeyCode::Char('b'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        ))
        .await;
        assert_eq!(app.state.mode, Mode::Prefix);
        assert_eq!(switch_calls.get(), 1);
        assert_eq!(restore_calls.get(), 0);

        // Esc leaves prefix mode → restore edge, even though the exit is decided
        // below `handle_key`.
        app.handle_raw_input_event(raw_key(
            KeyCode::Esc,
            KeyModifiers::empty(),
            KeyEventKind::Press,
        ))
        .await;
        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(restore_calls.get(), 1);
    }

    fn config_env_lock() -> &'static Mutex<()> {
        crate::config::test_config_env_lock()
    }

    fn temp_config_path(name: &str) -> std::path::PathBuf {
        let unique = format!(
            "gmux-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique).join("config.toml")
    }

    #[test]
    fn clipboard_write_event_shows_feedback_toast() {
        let mut app = test_app();

        app.handle_internal_event(AppEvent::ClipboardWrite {
            content: b"copied".to_vec(),
        });

        assert!(app.state.toast.is_none());
        let feedback = app.state.copy_feedback.as_ref().expect("copy feedback");
        assert_eq!(feedback.message, "copied to clipboard");
        assert!(app.copy_feedback_deadline.is_some());
    }

    #[test]
    fn clipboard_feedback_does_not_replace_notification_toast() {
        let mut app = test_app();
        app.state.toast = Some(crate::app::state::ToastNotification {
            kind: crate::app::state::ToastKind::NeedsAttention,
            title: "pi needs attention".to_string(),
            context: "background · 2".to_string(),
            target: None,
        });
        let original_toast = app.state.toast.clone();

        app.handle_internal_event(AppEvent::ClipboardWrite {
            content: b"copied".to_vec(),
        });

        assert_eq!(app.state.toast, original_toast);
        assert_eq!(
            app.state
                .copy_feedback
                .as_ref()
                .map(|feedback| feedback.message.as_str()),
            Some("copied to clipboard")
        );
    }

    #[test]
    fn startup_uses_configured_pane_panel_scope() {
        let mut config = Config::default();
        config.ui.pane_panel_scope = crate::config::PanePanelScopeConfig::Current;
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

        let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

        assert_eq!(app.state.pane_panel_scope, state::PanePanelScope::Current);
    }

    #[test]
    fn startup_uses_redraw_on_focus_gained_config() {
        let mut config = Config::default();
        config.ui.redraw_on_focus_gained = false;
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

        let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

        assert!(!app.state.redraw_on_focus_gained);
    }

    #[test]
    fn reload_config_updates_live_state() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-success");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[terminal]\nterm = \"xterm-256color\"\ndefault_shell = \"nu\"\nshell_mode = \"non_login\"\nnew_cwd = \"home\"\n[keys]\nnew_tab = \"prefix+m\"\nprefix = \"ctrl+a\"\n[ui]\npane_panel_scope = \"current\"\nredraw_on_focus_gained = false\nright_click_passthrough_modifier = \"ctrl\"\n[ui.toast]\ndelivery = \"gmux\"\n[experimental]\nswitch_ascii_input_source_in_prefix = true\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.prefix_code, KeyCode::Char('a'));
        assert_eq!(app.state.prefix_mods, KeyModifiers::CONTROL);
        assert!(app
            .state
            .keybinds
            .new_tab
            .matches_prefix(&KeyEvent::new(KeyCode::Char('m'), KeyModifiers::empty())));
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Gmux
        );
        assert_eq!(app.state.pane_panel_scope, state::PanePanelScope::Current);
        assert!(!app.state.redraw_on_focus_gained);
        assert_eq!(
            app.state.right_click_passthrough_modifiers,
            Some(KeyModifiers::CONTROL)
        );
        assert!(app.state.request_client_config_reload);
        assert_eq!(app.state.pane_term, "xterm-256color");
        assert_eq!(app.state.default_shell, "nu");
        assert_eq!(
            app.state.shell_mode,
            crate::config::ShellModeConfig::NonLogin
        );
        assert_eq!(
            app.state.new_terminal_cwd,
            crate::config::NewTerminalCwdConfig::Home
        );
        assert!(app.state.switch_ascii_input_source_in_prefix);
        assert!(app.state.config_diagnostic.is_none());
        let toast = app.state.toast.as_ref().unwrap();
        assert_eq!(toast.kind, crate::app::state::ToastKind::Finished);
        assert_eq!(toast.title, "reloaded config");
        assert_eq!(toast.context, "using config.toml");

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_updates_sidebar_width_only_when_config_owned() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-sidebar-width");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        assert_eq!(
            app.state.sidebar_width_source,
            state::SidebarWidthSource::ConfigDefault
        );

        std::fs::write(&path, "[ui]\nsidebar_width = 34\n").unwrap();
        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.default_sidebar_width, 34);
        assert_eq!(app.state.sidebar_width, 34);

        app.state.sidebar_width = 31;
        app.state.sidebar_width_source = state::SidebarWidthSource::Manual;
        std::fs::write(&path, "[ui]\nsidebar_width = 35\n").unwrap();
        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.default_sidebar_width, 35);
        assert_eq!(app.state.sidebar_width, 31);

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_updates_sidebar_bounds_and_reclamps() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-sidebar-bounds");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        // Default bounds.
        assert_eq!(app.state.sidebar_min_width, 18);
        assert_eq!(app.state.sidebar_max_width, 36);
        assert_eq!(
            app.state.mobile_width_threshold,
            crate::config::DEFAULT_MOBILE_WIDTH_THRESHOLD
        );

        // Manually set a width and flip the source so the existing
        // sidebar_width-only-when-config-owned guard does NOT update it.
        app.state.sidebar_width = 30;
        app.state.sidebar_width_source = state::SidebarWidthSource::Manual;

        // Tightening max below the current width must re-clamp the live width
        // even when source is Manual — bounds always apply.
        std::fs::write(&path, "[ui]\nsidebar_max_width = 24\n").unwrap();
        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.sidebar_max_width, 24);
        assert_eq!(
            app.state.sidebar_width, 24,
            "manual width must re-clamp to new max"
        );

        // Loosening max leaves the live width alone (it's already within bounds).
        app.state.sidebar_width = 24;
        std::fs::write(&path, "[ui]\nsidebar_max_width = 60\n").unwrap();
        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.sidebar_max_width, 60);
        assert_eq!(app.state.sidebar_width, 24);

        // Raising min above the current width re-clamps upward.
        std::fs::write(&path, "[ui]\nsidebar_min_width = 30\n").unwrap();
        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.sidebar_min_width, 30);
        assert_eq!(
            app.state.sidebar_width, 30,
            "manual width must re-clamp up to new min"
        );

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_updates_mobile_width_threshold() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-mobile-width-threshold");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        assert_eq!(
            app.state.mobile_width_threshold,
            crate::config::DEFAULT_MOBILE_WIDTH_THRESHOLD
        );

        std::fs::write(&path, "[ui]\nmobile_width_threshold = 96\n").unwrap();
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.mobile_width_threshold, 96);

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn app_new_falls_back_to_default_bounds_on_inverted_config() {
        let mut config = Config::default();
        config.ui.sidebar_min_width = 50;
        config.ui.sidebar_max_width = 30;

        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

        assert_eq!(
            app.state.sidebar_min_width, 18,
            "App::new must fall back to default min when bounds are inverted"
        );
        assert_eq!(
            app.state.sidebar_max_width, 36,
            "App::new must fall back to default max when bounds are inverted"
        );
    }

    #[test]
    fn reload_config_invalid_sidebar_bounds_keeps_previous_ui_and_returns_partial() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-invalid-sidebar-bounds");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let original_min = app.state.sidebar_min_width;
        let original_max = app.state.sidebar_max_width;
        let original_mouse_capture = app.state.mouse_capture;
        // Pair the bad bounds with another `[ui]` field change to confirm the
        // entire section is treated as invalid (not just the bounds).
        let target_mouse_capture = !original_mouse_capture;
        std::fs::write(
            &path,
            format!(
                "[ui]\nsidebar_min_width = 50\nsidebar_max_width = 30\nmouse_capture = {}\n",
                target_mouse_capture
            ),
        )
        .unwrap();

        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert_eq!(app.state.sidebar_min_width, original_min);
        assert_eq!(app.state.sidebar_max_width, original_max);
        assert_eq!(
            app.state.mouse_capture, original_mouse_capture,
            "[ui] is treated as invalid on bad bounds; mouse_capture must not apply"
        );
        assert!(app
            .state
            .config_diagnostic
            .as_deref()
            .is_some_and(|message| {
                message.contains("sidebar_min_width")
                    && message.contains("sidebar_max_width")
                    && message.contains("greater")
            }));

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_keeps_current_keybinds_on_invalid_binding_but_applies_other_sections() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-invalid-keybind");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[keys]\nnew_tab = \"wat\"\n[ui.toast]\ndelivery = \"terminal\"\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let original_prefix = (app.state.prefix_code, app.state.prefix_mods);
        let original_keybinds = app.state.keybinds.new_tab.clone();
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert_eq!(
            (app.state.prefix_code, app.state.prefix_mods),
            original_prefix
        );
        assert_eq!(app.state.keybinds.new_tab, original_keybinds);
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Terminal
        );
        assert!(app
            .state
            .config_diagnostic
            .as_deref()
            .is_some_and(|message| {
                message.contains("keys.new_tab") && message.contains("kept current keybinds")
            }));

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_preserves_invalid_ui_section_but_applies_valid_keys() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-invalid-ui-section");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[keys]\nnew_tab = \"prefix+m\"\n[ui.toast]\ndelivery = \"desktop\"\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        app.state.toast_config.delivery = crate::config::ToastDelivery::Gmux;
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert!(app
            .state
            .keybinds
            .new_tab
            .matches_prefix(&KeyEvent::new(KeyCode::Char('m'), KeyModifiers::empty())));
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Gmux
        );
        assert!(app
            .state
            .config_diagnostic
            .as_deref()
            .is_some_and(|message| message.contains("invalid ui config")));

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_preserves_invalid_terminal_section_but_applies_valid_ui() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-invalid-terminal-section");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[terminal]\ndefault_shell = \"nu\"\nshell_mode = \"sideways\"\nnew_cwd = \"home\"\n[ui.toast]\ndelivery = \"terminal\"\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let original_default_shell = app.state.default_shell.clone();
        let original_shell_mode = app.state.shell_mode;
        let original_new_cwd = app.state.new_terminal_cwd.clone();
        let original_pane_term = app.state.pane_term.clone();
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert_eq!(app.state.default_shell, original_default_shell);
        assert_eq!(app.state.shell_mode, original_shell_mode);
        assert_eq!(app.state.new_terminal_cwd, original_new_cwd);
        assert_eq!(app.state.pane_term, original_pane_term);
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Terminal
        );
        assert!(app
            .state
            .config_diagnostic
            .as_deref()
            .is_some_and(|message| message.contains("invalid terminal config")));

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn settings_save_toast_delivery_persists_then_applies_live_config() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("settings-save-toast-delivery");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "onboarding = false\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Off
        );

        app.save_toast_delivery(crate::config::ToastDelivery::Terminal);

        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Terminal
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("delivery = \"terminal\""));
        assert!(app.state.config_diagnostic.is_none());

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_pane_panel_scope_persists_then_applies_live_config() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("save-pane-panel-scope");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "onboarding = false\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        assert_eq!(app.state.pane_panel_scope, state::PanePanelScope::All);

        app.save_pane_panel_scope(state::PanePanelScope::Current);

        assert_eq!(app.state.pane_panel_scope, state::PanePanelScope::Current);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("pane_panel_scope = \"current\""));
        assert!(app.state.config_diagnostic.is_none());

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn settings_save_pane_history_persists_then_applies_live_config() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("settings-save-pane-history");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "onboarding = false\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        assert!(!app.persist_pane_history);
        assert!(!app.state.pane_history_persistence);

        app.save_pane_history_persistence(true);

        assert!(app.persist_pane_history);
        assert!(app.state.pane_history_persistence);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[experimental]"));
        assert!(content.contains("pane_history = true"));
        assert!(app.state.config_diagnostic.is_none());

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn settings_save_scalar_values_persist_then_apply_live_config() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("settings-save-scalar-values");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "onboarding = false\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();

        app.save_section_value("default shell", "terminal", "default_shell", "\"nu\"");
        app.save_section_value("sidebar width", "ui", "sidebar_width", "30");
        app.save_section_bool(
            "remote ssh config setting",
            "remote",
            "manage_ssh_config",
            false,
        );
        app.save_top_level_bool("onboarding setting", "onboarding", true);

        assert_eq!(app.state.default_shell, "nu");
        assert_eq!(app.state.default_sidebar_width, 30);
        assert!(!app.state.remote_manage_ssh_config);
        assert!(app.state.show_onboarding_on_next_launch);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[terminal]"));
        assert!(content.contains("default_shell = \"nu\""));
        assert!(content.contains("[ui]"));
        assert!(content.contains("sidebar_width = 30"));
        assert!(content.contains("[remote]"));
        assert!(content.contains("manage_ssh_config = false"));
        assert!(content.contains("onboarding = true"));
        assert!(app.state.config_diagnostic.is_none());

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_keeps_current_state_on_invalid_toml() {
        let _guard = config_env_lock().lock().unwrap();
        let path = temp_config_path("reload-config-invalid-toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[keys\nnew_tab = \"g\"\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let original_prefix = (app.state.prefix_code, app.state.prefix_mods);
        let original_keybinds = app.state.keybinds.new_tab.clone();
        let original_toast_delivery = app.state.toast_config.delivery;
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Failed);
        assert_eq!(
            (app.state.prefix_code, app.state.prefix_mods),
            original_prefix
        );
        assert_eq!(app.state.keybinds.new_tab, original_keybinds);
        assert_eq!(app.state.toast_config.delivery, original_toast_delivery);
        assert!(app
            .state
            .config_diagnostic
            .as_deref()
            .is_some_and(|message| {
                message.contains("config parse error") && message.contains("keeping current config")
            }));
        assert!(app.state.toast.is_none());

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn raw_input_waits_when_reader_is_gone() {
        let result =
            tokio::time::timeout(Duration::from_millis(20), recv_raw_input_or_pending(None)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn terminal_mode_handles_repeat_key_events() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        let handled = app
            .handle_raw_input_event(raw_key(
                KeyCode::Backspace,
                KeyModifiers::empty(),
                KeyEventKind::Repeat,
            ))
            .await;

        assert!(handled);
    }

    #[tokio::test]
    async fn outer_focus_gained_marks_visible_panes_seen() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        let split_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        let background_tab = workspace.test_add_tab(Some("background"));
        let background_pane = workspace.tabs[background_tab].root_pane;

        app.state.sessions = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.sessions[0].tabs[0]
            .panes
            .get_mut(&root_pane)
            .unwrap()
            .seen = false;
        app.state.sessions[0].tabs[0]
            .panes
            .get_mut(&split_pane)
            .unwrap()
            .seen = false;
        app.state.sessions[0].tabs[background_tab]
            .panes
            .get_mut(&background_pane)
            .unwrap()
            .seen = false;

        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;
        app.state.outer_terminal_focus = Some(false);

        let handled = app
            .handle_raw_input_event(crate::raw_input::RawInputEvent::OuterFocusGained)
            .await;

        assert!(handled);
        assert_eq!(app.state.outer_terminal_focus, Some(true));
        assert!(app.state.sessions[0].tabs[0].panes[&root_pane].seen);
        assert!(app.state.sessions[0].tabs[0].panes[&split_pane].seen);
        assert!(!app.state.sessions[0].tabs[background_tab].panes[&background_pane].seen);
    }

    #[tokio::test]
    async fn outer_focus_gained_does_not_require_full_redraw_when_disabled() {
        let mut app = test_app();
        app.state.redraw_on_focus_gained = false;

        let handled = app
            .handle_raw_input_event(crate::raw_input::RawInputEvent::OuterFocusGained)
            .await;

        assert!(handled);
        assert_eq!(app.state.outer_terminal_focus, Some(true));
        assert!(!app.full_redraw_pending);
    }

    #[tokio::test]
    async fn repeat_key_events_are_ignored_outside_terminal_mode() {
        let mut app = test_app();
        app.state.mode = Mode::KeybindHelp;

        let handled = app
            .handle_raw_input_event(raw_key(
                KeyCode::Enter,
                KeyModifiers::empty(),
                KeyEventKind::Repeat,
            ))
            .await;

        assert!(!handled);
        assert_eq!(app.state.mode, Mode::KeybindHelp);
    }

    #[tokio::test]
    async fn modal_press_does_not_leak_repeat_into_terminal_mode() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::KeybindHelp;

        let press_handled = app
            .handle_raw_input_event(raw_key(
                KeyCode::Enter,
                KeyModifiers::empty(),
                KeyEventKind::Press,
            ))
            .await;
        let repeat_handled = app
            .handle_raw_input_event(raw_key(
                KeyCode::Enter,
                KeyModifiers::empty(),
                KeyEventKind::Repeat,
            ))
            .await;
        let release_handled = app
            .handle_raw_input_event(raw_key(
                KeyCode::Enter,
                KeyModifiers::empty(),
                KeyEventKind::Release,
            ))
            .await;
        let next_press_handled = app
            .handle_raw_input_event(raw_key(
                KeyCode::Enter,
                KeyModifiers::empty(),
                KeyEventKind::Press,
            ))
            .await;

        assert!(press_handled);
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(!repeat_handled);
        assert!(!release_handled);
        assert!(next_press_handled);
    }

    #[test]
    fn read_only_api_requests_do_not_force_rerender() {
        let read_only = crate::api::schema::Request {
            id: "req_1".into(),
            method: crate::api::schema::Method::TabList(
                crate::api::schema::TabListParams::default(),
            ),
        };
        let mutating = crate::api::schema::Request {
            id: "req_2".into(),
            method: crate::api::schema::Method::TabFocus(crate::api::schema::TabTarget {
                tab_id: "t_1".into(),
            }),
        };
        let pane_rename = crate::api::schema::Request {
            id: "req_3".into(),
            method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
                pane_id: "p_1".into(),
                label: Some("logs".into()),
            }),
        };
        assert!(!crate::api::request_changes_ui(&read_only));
        assert!(crate::api::request_changes_ui(&mutating));
        assert!(crate::api::request_changes_ui(&pane_rename));
    }

    #[test]
    fn tab_create_response_includes_root_pane() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("api-tab-root-pane");
        workspace.test_add_tab(None);
        app.state.sessions = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let crate::api::schema::ResponseResult::TabCreated { tab, root_pane } =
            app.tab_created_result(0, 1).unwrap()
        else {
            panic!("expected tab_created response");
        };

        assert_eq!(root_pane.tab_id, tab.tab_id);
        assert_eq!(tab.pane_count, 1);
    }

    #[test]
    fn tab_focus_request_collapses_legacy_workspace_target() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_tab_id = app.public_tab_id(1, 0).unwrap();
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_tab_focus_legacy".into(),
            method: crate::api::schema::Method::TabFocus(crate::api::schema::TabTarget {
                tab_id: target_tab_id.clone(),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "tab_info");
        assert_eq!(response["result"]["tab"]["tab_id"], target_tab_id);
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 1);
    }

    #[test]
    fn tab_get_request_collapses_legacy_workspace_target() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_tab_id = app.public_tab_id(1, 0).unwrap();
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_tab_get_legacy".into(),
            method: crate::api::schema::Method::TabGet(crate::api::schema::TabTarget {
                tab_id: target_tab_id.clone(),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "tab_info");
        assert_eq!(response["result"]["tab"]["tab_id"], target_tab_id);
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 0);
    }

    #[test]
    fn tab_list_request_collapses_legacy_workspaces() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(1);
        app.state.selected_session = 1;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_tab_list_legacy".into(),
            method: crate::api::schema::Method::TabList(
                crate::api::schema::TabListParams::default(),
            ),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "tab_list");
        assert_eq!(response["result"]["tabs"].as_array().unwrap().len(), 2);
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 1);
    }

    #[test]
    fn tab_close_request_closes_legacy_workspace_tab_after_collapse() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_tab_id = app.public_tab_id(1, 0).unwrap();
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_tab_close_legacy".into(),
            method: crate::api::schema::Method::TabClose(crate::api::schema::TabTarget {
                tab_id: target_tab_id,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.sessions[0].tabs.len(), 1);
        assert_eq!(app.state.sessions[0].display_name(), "one");
    }

    #[test]
    fn tab_rename_request_collapses_legacy_workspace_target() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_tab_id = app.public_tab_id(1, 0).unwrap();
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_tab_rename_legacy".into(),
            method: crate::api::schema::Method::TabRename(crate::api::schema::TabRenameParams {
                tab_id: target_tab_id.clone(),
                label: "worker".into(),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "tab_info");
        assert_eq!(response["result"]["tab"]["tab_id"], target_tab_id);
        assert_eq!(response["result"]["tab"]["label"], "worker");
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 0);
        assert_eq!(app.state.sessions[0].tabs[1].display_name(), "worker");
    }

    #[test]
    fn new_terminal_cwd_follow_uses_source_cwd() {
        let cwd = creation::resolve_new_terminal_cwd(
            &crate::config::NewTerminalCwdConfig::Follow,
            Some(std::path::PathBuf::from("/tmp/gmux-source")),
        );

        assert_eq!(cwd, std::path::PathBuf::from("/tmp/gmux-source"));
    }

    #[test]
    fn new_terminal_cwd_follow_without_source_uses_current_dir() {
        let current = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        let cwd =
            creation::resolve_new_terminal_cwd(&crate::config::NewTerminalCwdConfig::Follow, None);

        assert_eq!(cwd, current);
    }

    #[test]
    fn new_terminal_cwd_path_uses_configured_path() {
        let cwd = creation::resolve_new_terminal_cwd(
            &crate::config::NewTerminalCwdConfig::Path("/tmp/gmux-fixed".into()),
            Some(std::path::PathBuf::from("/tmp/gmux-source")),
        );

        assert_eq!(cwd, std::path::PathBuf::from("/tmp/gmux-fixed"));
    }

    #[test]
    fn server_stop_request_sets_should_quit_flag() {
        let mut app = test_app();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_server_stop".into(),
            method: crate::api::schema::Method::ServerStop(
                crate::api::schema::EmptyParams::default(),
            ),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert!(app.state.should_quit);
    }

    #[test]
    fn pane_rename_request_sets_and_clears_manual_label() {
        let mut app = test_app();
        let workspace = Workspace::test_new("api-pane-rename");
        let pane = workspace.tabs[0].root_pane;
        app.state.sessions = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let pane_id = app.pane_info(0, pane).unwrap().pane_id;
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_rename".into(),
            method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
                pane_id: pane_id.clone(),
                label: Some("reviewer".into()),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(response["result"]["pane"]["label"], "reviewer");
        let terminal_id = app.state.sessions[0]
            .pane_state(pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        assert_eq!(
            app.state
                .terminals
                .get(&terminal_id)
                .unwrap()
                .manual_label
                .as_deref(),
            Some("reviewer")
        );

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_rename_clear".into(),
            method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
                pane_id,
                label: None,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert!(response["result"]["pane"].get("label").is_none());
        assert!(app
            .state
            .terminals
            .get(&terminal_id)
            .unwrap()
            .manual_label
            .is_none());
    }

    #[test]
    fn pane_rename_request_collapses_legacy_workspace_target() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        let target_pane = second.tabs[0].root_pane;
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_pane_id = app.pane_info(1, target_pane).unwrap().pane_id;
        let target_tab_id = app.public_tab_id(1, 0).unwrap();
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_rename_legacy".into(),
            method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
                pane_id: target_pane_id.clone(),
                label: Some("reviewer".into()),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(response["result"]["pane"]["pane_id"], target_pane_id);
        assert_eq!(response["result"]["pane"]["tab_id"], target_tab_id);
        assert_eq!(response["result"]["pane"]["label"], "reviewer");
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 0);
    }

    #[test]
    fn pane_get_request_collapses_legacy_workspace_target() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        let target_pane = second.tabs[0].root_pane;
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_pane_id = app.pane_info(1, target_pane).unwrap().pane_id;
        let target_tab_id = app.public_tab_id(1, 0).unwrap();
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_get_legacy".into(),
            method: crate::api::schema::Method::PaneGet(crate::api::schema::PaneTarget {
                pane_id: target_pane_id.clone(),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(response["result"]["pane"]["pane_id"], target_pane_id);
        assert_eq!(response["result"]["pane"]["tab_id"], target_tab_id);
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 0);
    }

    #[test]
    fn pane_list_request_collapses_legacy_workspaces() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(1);
        app.state.selected_session = 1;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_list_legacy".into(),
            method: crate::api::schema::Method::PaneList(
                crate::api::schema::PaneListParams::default(),
            ),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_list");
        assert_eq!(response["result"]["panes"].as_array().unwrap().len(), 2);
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 1);
    }

    #[tokio::test]
    async fn pane_send_text_request_collapses_legacy_workspace_target() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let mut second = Workspace::test_new("two");
        let target_pane = second.tabs[0].root_pane;
        let (runtime, mut rx) = TerminalRuntime::test_with_channel(80, 24);
        second.tabs[0].runtimes.insert(target_pane, runtime);
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_pane_id = app.pane_info(1, target_pane).unwrap().pane_id;
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_send_text_legacy".into(),
            method: crate::api::schema::Method::PaneSendText(
                crate::api::schema::PaneSendTextParams {
                    pane_id: target_pane_id,
                    text: "echo via collapsed session".into(),
                },
            ),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert_eq!(
            rx.recv().await.unwrap(),
            bytes::Bytes::from("echo via collapsed session")
        );
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 0);
    }

    #[tokio::test]
    async fn pane_split_request_targets_pane_in_background_tab() {
        let _guard = config_env_lock().lock().unwrap();
        let original_shell = std::env::var_os("SHELL");
        std::env::set_var("SHELL", "/usr/bin/true");

        let mut app = test_app();
        let mut workspace = Workspace::test_new("api-pane-split-background-tab");
        let active_pane = workspace.tabs[0].root_pane;
        let background_tab = workspace.test_add_tab(Some("worker"));
        let target_pane = workspace.tabs[background_tab].root_pane;
        workspace.switch_tab(background_tab);
        let background_previous_focus =
            workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.switch_tab(0);
        app.state.sessions = vec![workspace];
        app.state.ensure_test_terminals();
        let split_cwd = std::env::temp_dir();
        let target_terminal_id = app.state.sessions[0]
            .pane_state(target_pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&target_terminal_id)
            .unwrap()
            .cwd = split_cwd.clone();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state
            .focus_pane_in_session_at(0, background_previous_focus);
        app.state.focus_pane_in_session_at(0, active_pane);

        let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;
        let target_tab_id = app.public_tab_id(0, background_tab).unwrap();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_split_background_tab".into(),
            method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
                target_pane_id,
                direction: crate::api::schema::SplitDirection::Right,
                cwd: None,
                focus: false,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(response["result"]["pane"]["tab_id"], target_tab_id);
        let response_cwd =
            std::path::PathBuf::from(response["result"]["pane"]["cwd"].as_str().unwrap());
        assert_eq!(
            std::fs::canonicalize(&response_cwd).unwrap_or(response_cwd),
            std::fs::canonicalize(&split_cwd).unwrap_or(split_cwd)
        );
        assert_eq!(response["result"]["pane"]["focused"], false);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.sessions[0].active_tab, 0);
        assert_eq!(app.state.sessions[0].tabs[0].layout.focused(), active_pane);
        assert_eq!(app.state.sessions[0].tabs[0].layout.pane_count(), 1);
        assert_eq!(
            app.state.sessions[0].tabs[background_tab].layout.focused(),
            background_previous_focus
        );
        assert_eq!(
            app.state.sessions[0].tabs[background_tab]
                .layout
                .pane_count(),
            3
        );
        app.state.last_pane();
        assert_eq!(app.state.sessions[0].active_tab, background_tab);
        assert_eq!(
            app.state.sessions[0].tabs[background_tab].layout.focused(),
            background_previous_focus
        );

        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
        match original_shell {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
    }

    #[tokio::test]
    async fn pane_split_request_collapses_legacy_workspace_target() {
        let _guard = config_env_lock().lock().unwrap();
        let original_shell = std::env::var_os("SHELL");
        std::env::set_var("SHELL", "/usr/bin/true");

        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        let target_pane = second.tabs[0].root_pane;
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_pane_id = app.pane_info(1, target_pane).unwrap().pane_id;
        let target_tab_id = app.public_tab_id(1, 0).unwrap();
        let split_cwd = std::env::temp_dir();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_split_legacy".into(),
            method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
                target_pane_id,
                direction: crate::api::schema::SplitDirection::Right,
                cwd: Some(split_cwd.display().to_string()),
                focus: false,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(response["result"]["pane"]["tab_id"], target_tab_id);
        assert_eq!(response["result"]["pane"]["focused"], false);
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.selected_session, 0);
        assert_eq!(app.state.sessions[0].active_tab, 0);
        assert_eq!(app.state.sessions[0].tabs.len(), 2);
        assert_eq!(app.state.sessions[0].tabs[1].layout.pane_count(), 2);

        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
        match original_shell {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
    }

    #[tokio::test]
    async fn pane_split_request_focuses_new_pane_when_requested() {
        let _guard = config_env_lock().lock().unwrap();
        let original_shell = std::env::var_os("SHELL");
        std::env::set_var("SHELL", "/usr/bin/true");

        let mut app = test_app();
        let mut workspace = Workspace::test_new("api-pane-split-focus-background-tab");
        let background_tab = workspace.test_add_tab(Some("worker"));
        workspace.switch_tab(0);
        app.state.sessions = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_pane = app.state.sessions[0].tabs[background_tab].root_pane;
        let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;
        let target_tab_id = app.public_tab_id(0, background_tab).unwrap();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_split_focus_background_tab".into(),
            method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
                target_pane_id,
                direction: crate::api::schema::SplitDirection::Right,
                cwd: None,
                focus: true,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(response["result"]["pane"]["tab_id"], target_tab_id);
        assert_eq!(response["result"]["pane"]["focused"], true);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.sessions[0].active_tab, background_tab);

        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
        match original_shell {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
    }

    #[test]
    fn pane_close_request_closes_only_the_target_tab_when_other_tabs_exist() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("api-pane-close");
        let second_tab = workspace.test_add_tab(Some("logs"));
        workspace.switch_tab(second_tab);
        app.state.sessions = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_pane = app.state.sessions[0].tabs[second_tab].root_pane;
        let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_close".into(),
            method: crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
                pane_id: target_pane_id,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.sessions[0].tabs.len(), 1);
        assert_eq!(app.state.sessions[0].display_name(), "api-pane-close");
    }

    #[test]
    fn pane_close_request_closes_workspace_when_it_removes_the_last_pane() {
        let mut app = test_app();
        let workspace = Workspace::test_new("api-pane-close-last");
        app.state.sessions = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_pane = app.state.sessions[0].tabs[0].root_pane;
        let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_close_last".into(),
            method: crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
                pane_id: target_pane_id,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert!(app.state.sessions.is_empty());
    }

    #[test]
    fn pane_close_request_closes_legacy_workspace_tab_after_collapse() {
        let mut app = test_app();
        let first = Workspace::test_new("one");
        let second = Workspace::test_new("two");
        app.state.sessions = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        let target_pane = app.state.sessions[1].tabs[0].root_pane;
        let target_pane_id = app.pane_info(1, target_pane).unwrap().pane_id;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_close_legacy".into(),
            method: crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
                pane_id: target_pane_id,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.sessions[0].tabs.len(), 1);
        assert_eq!(app.state.sessions[0].display_name(), "one");
    }

    #[test]
    fn session_dirty_flag_schedules_debounced_save() {
        let mut app = test_app();
        app.no_session = false;
        app.state.session_dirty = true;

        app.sync_session_save_schedule();

        assert!(!app.state.session_dirty);
        assert!(app.session_save_deadline.is_some());
    }

    #[test]
    fn next_loop_deadline_includes_session_save_deadline() {
        let mut app = test_app();
        let now = Instant::now();
        app.session_save_deadline = Some(now + Duration::from_secs(2));
        app.next_resize_poll = now + Duration::from_secs(5);

        assert_eq!(
            app.next_loop_deadline(now, false),
            app.session_save_deadline
        );
    }

    #[test]
    fn headless_next_loop_deadline_ignores_resize_poll() {
        let mut app = test_app();
        let now = Instant::now();
        app.next_resize_poll = now + Duration::from_millis(100);
        app.session_save_deadline = Some(now + Duration::from_secs(2));

        assert_eq!(
            app.next_headless_loop_deadline(now, false),
            app.session_save_deadline
        );
    }

    #[test]
    fn headless_next_loop_deadline_returns_none_when_resize_poll_is_only_deadline() {
        let mut app = test_app();
        let now = Instant::now();
        app.next_resize_poll = now - Duration::from_millis(1);
        app.config_diagnostic_deadline = None;
        app.toast_deadline = None;
        app.next_animation_tick = None;
        app.session_save_deadline = None;
        app.state.sessions.clear();

        assert_eq!(app.next_headless_loop_deadline(now, false), None);
    }

    #[test]
    fn due_session_save_deadline_is_cleared() {
        let mut app = test_app();
        app.session_save_deadline = Some(Instant::now() - Duration::from_secs(1));

        app.handle_scheduled_tasks(Instant::now(), false);

        assert!(app.session_save_deadline.is_none());
    }

    #[test]
    fn next_loop_deadline_includes_selection_autoscroll_deadline() {
        let mut app = test_app();
        let now = Instant::now();
        app.next_resize_poll = now + Duration::from_millis(300);
        app.selection_autoscroll_deadline = Some(now + Duration::from_millis(5));
        app.next_animation_tick = Some(now + Duration::from_millis(100));
        app.session_save_deadline = Some(now + Duration::from_millis(200));
        assert_eq!(
            app.next_loop_deadline(now, false),
            app.selection_autoscroll_deadline
        );
    }

    #[test]
    fn tick_selection_autoscroll_self_heals_when_state_cleared() {
        let mut app = test_app();
        let now = Instant::now();
        app.state.selection_autoscroll = None;
        app.selection_autoscroll_deadline = Some(now);
        app.tick_selection_autoscroll(now);
        assert!(app.selection_autoscroll_deadline.is_none());
    }

    #[test]
    fn tick_selection_autoscroll_stops_on_rect_change() {
        let mut app = test_app();
        let now = Instant::now();
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        app.state.sessions.push(ws);
        app.state.active_session = Some(0);
        app.state.selection = Some(crate::selection::Selection::anchor(pane_id, 0, 0, None));
        // Set autoscroll with a stale inner_rect that doesn't match pane_infos
        app.state.selection_autoscroll = Some(state::SelectionAutoscroll {
            direction: state::SelectionAutoscrollDirection::Down,
            last_mouse_screen_col: 0,
            last_mouse_screen_row: 999,
            inner_rect: ratatui::layout::Rect::new(0, 0, 1, 1), // wrong rect
        });
        app.selection_autoscroll_deadline = Some(now);
        app.tick_selection_autoscroll(now);
        assert!(app.state.selection_autoscroll.is_none());
        assert!(app.selection_autoscroll_deadline.is_none());
    }

    #[tokio::test]
    async fn full_internal_event_queue_accepts_event_after_backpressure() {
        let mut app = test_app();

        for i in 0..APP_EVENT_CHANNEL_CAPACITY {
            app.event_tx
                .try_send(AppEvent::ClipboardWrite {
                    content: vec![i as u8],
                })
                .unwrap();
        }

        let tx = app.event_tx.clone();
        let send = tx.send(AppEvent::ClipboardWrite {
            content: b"later".to_vec(),
        });
        tokio::pin!(send);

        let blocked =
            tokio::time::timeout(Duration::from_millis(20), async { (&mut send).await }).await;
        assert!(
            blocked.is_err(),
            "sender should wait for queue space instead of failing"
        );

        app.drain_internal_events();

        tokio::time::timeout(Duration::from_millis(50), async { (&mut send).await })
            .await
            .expect("event should enqueue once queue space is available")
            .expect("app event receiver should still be alive");

        let max_drains = (APP_EVENT_CHANNEL_CAPACITY / APP_EVENT_DRAIN_LIMIT) + 2;
        for _ in 0..max_drains {
            if app.event_rx.is_empty() {
                break;
            }
            app.drain_internal_events();
        }

        assert!(app.event_rx.is_empty());
    }

    #[test]
    fn route_client_input_dispatches_navigate_mode_keybinds() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        // Start in navigate mode.
        app.state.mode = Mode::Navigate;

        // Send Ctrl+B then Esc (prefix → leave navigate mode).
        // Ctrl+B is 0x02 in raw terminal input.
        // After entering navigate mode and pressing Esc, we should leave navigate mode.
        let esc_bytes = vec![0x1b]; // Esc
        app.route_client_input(esc_bytes);
        // Esc in navigate mode should leave navigate mode.
        assert_eq!(
            app.state.mode,
            Mode::Terminal,
            "Esc should leave navigate mode and return to Terminal mode"
        );
    }

    #[test]
    fn route_client_input_q_detaches_in_persistence_mode() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.detach_exits = false;

        // Start in navigate mode.
        app.state.mode = Mode::Navigate;
        assert!(!app.state.detach_requested);

        let q_bytes = b"q".to_vec();
        app.route_client_input(q_bytes);

        assert!(
            app.state.detach_requested,
            "q should detach in persistence mode"
        );
        assert_eq!(
            app.state.mode,
            Mode::Terminal,
            "q should leave navigate mode"
        );
    }

    #[test]
    fn route_client_input_prefix_then_d_detaches_in_persistence_mode() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.detach_exits = false;

        // Start in terminal mode (default after session creation).
        app.state.mode = Mode::Terminal;
        assert!(!app.state.detach_requested);

        // Send Ctrl+B (prefix key, raw byte 0x02).
        let prefix_bytes = vec![0x02];
        app.route_client_input(prefix_bytes);

        assert_eq!(
            app.state.mode,
            Mode::Prefix,
            "prefix key should enter prefix mode"
        );
        assert!(
            !app.state.detach_requested,
            "prefix key should not set detach flag"
        );

        let d_bytes = b"d".to_vec();
        app.route_client_input(d_bytes);

        assert!(
            app.state.detach_requested,
            "d should detach in persistence mode"
        );
        assert_eq!(
            app.state.mode,
            Mode::Terminal,
            "d should leave navigate mode"
        );
    }

    #[test]
    fn route_client_input_prefix_tab_dispatches_global_last_pane() {
        let config: Config = toml::from_str(
            r#"
[keys]
last_pane = "prefix+tab"
"#,
        )
        .unwrap();
        let mut app = test_app();
        let mut first = Workspace::test_new("one");
        let first_second_tab = first.test_add_tab(Some("logs"));
        let first_second_root = first.tabs[first_second_tab].root_pane;
        let second = Workspace::test_new("two");
        let second_root = second.tabs[0].root_pane;
        app.state.sessions = vec![first, second];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.keybinds = config.keybinds();
        app.state.mode = Mode::Terminal;
        app.state.sessions[0].switch_tab(first_second_tab);
        app.state.focus_session_tab(1, 0);

        app.route_client_input(vec![0x02, b'\t']);

        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.sessions[0].active_tab, first_second_tab);
        assert_eq!(
            app.state.sessions[0].focused_pane_id(),
            Some(first_second_root)
        );

        app.route_client_input(vec![0x02, b'\t']);

        assert_eq!(app.state.sessions.len(), 1);
        assert_eq!(app.state.active_session, Some(0));
        assert_eq!(app.state.sessions[0].active_tab, 2);
        assert_eq!(app.state.sessions[0].focused_pane_id(), Some(second_root));
    }

    #[tokio::test]
    async fn route_client_input_double_prefix_passes_prefix_through_to_focused_pane() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let (runtime, mut rx) = TerminalRuntime::test_with_channel(80, 24);
        workspace.tabs[0].runtimes.insert(focused, runtime);
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;
        app.state.prefix_code = KeyCode::Char('l');
        app.state.prefix_mods = KeyModifiers::CONTROL;

        app.route_client_input(vec![0x0c]);
        assert_eq!(app.state.mode, Mode::Prefix);

        app.route_client_input(vec![0x0c]);
        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(rx.recv().await.unwrap(), bytes::Bytes::from(vec![0x0c]));
    }

    #[tokio::test]
    async fn route_client_input_reencodes_terminal_keys_for_focused_pane_protocol() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let (runtime, mut rx) = TerminalRuntime::test_with_channel(80, 24);
        workspace.tabs[0].runtimes.insert(focused, runtime);
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        // Ghostty/kitty-style Ctrl-C should be normalized back to the pane's
        // negotiated encoding instead of being forwarded verbatim.
        app.route_client_input(b"\x1b[99;5u".to_vec());

        assert_eq!(rx.recv().await.unwrap(), bytes::Bytes::from(vec![3]));
    }

    #[tokio::test]
    async fn route_client_input_preserves_shift_enter_for_modify_other_keys_pane() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let (runtime, mut rx) =
            TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"\x1b[>4;1m", 4);
        workspace.tabs[0].runtimes.insert(focused, runtime);
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        app.route_client_input(b"\x1b[13;2u".to_vec());

        assert_eq!(
            rx.recv().await.unwrap(),
            bytes::Bytes::from_static(b"\x1b[27;2;13~")
        );
    }

    #[tokio::test]
    async fn route_client_input_splits_multi_event_payloads_before_forwarding() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let (runtime, mut rx) = TerminalRuntime::test_with_channel(80, 24);
        workspace.tabs[0].runtimes.insert(focused, runtime);
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        app.route_client_input(b"ab".to_vec());

        assert_eq!(rx.recv().await.unwrap(), bytes::Bytes::from_static(b"a"));
        assert_eq!(rx.recv().await.unwrap(), bytes::Bytes::from_static(b"b"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn raw_input_batch_drains_queued_terminal_input_without_waiting_for_render() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let (runtime, mut pane_rx) = TerminalRuntime::test_with_channel_capacity(80, 24, 2);
        workspace.tabs[0].runtimes.insert(focused, runtime);
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        let (input_tx, input_rx) = tokio::sync::mpsc::channel(4);
        input_tx
            .send(raw_key(
                KeyCode::Char('b'),
                KeyModifiers::empty(),
                KeyEventKind::Press,
            ))
            .await
            .unwrap();
        app.input_rx = Some(input_rx);

        let changed = app
            .handle_raw_input_batch(raw_key(
                KeyCode::Char('a'),
                KeyModifiers::empty(),
                KeyEventKind::Press,
            ))
            .await;

        assert!(changed);
        assert!(app.input_render_bypass_pending);
        assert_eq!(pane_rx.try_recv().unwrap(), bytes::Bytes::from_static(b"a"));
        assert_eq!(pane_rx.try_recv().unwrap(), bytes::Bytes::from_static(b"b"));
        assert!(pane_rx.try_recv().is_err());
        assert!(app.input_rx.as_mut().unwrap().try_recv().is_err());
    }

    #[tokio::test]
    async fn route_client_input_forwards_multilingual_ime_text_to_focused_pane() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let text = "中日한🙂";
        let (runtime, mut rx) =
            TerminalRuntime::test_with_channel_capacity(80, 24, text.chars().count());
        workspace.tabs[0].runtimes.insert(focused, runtime);
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        app.route_client_input(text.as_bytes().to_vec());

        let mut forwarded = Vec::new();
        for _ in text.chars() {
            let chunk = rx.recv().await.unwrap();
            forwarded.extend_from_slice(&chunk);
        }
        assert_eq!(forwarded, text.as_bytes());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn route_client_input_forwards_long_voice_like_cjk_text_without_truncation() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let text = "你好，今天我们测试一段比较长的语音输入。こんにちは。안녕하세요.🙂".repeat(64);
        let char_count = text.chars().count();
        let (runtime, mut rx) = TerminalRuntime::test_with_channel_capacity(80, 24, char_count);
        workspace.tabs[0].runtimes.insert(focused, runtime);
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        app.route_client_input(text.as_bytes().to_vec());

        let mut forwarded = Vec::new();
        for _ in 0..char_count {
            let chunk = rx.recv().await.unwrap();
            forwarded.extend_from_slice(&chunk);
        }
        assert_eq!(forwarded, text.as_bytes());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn route_client_input_handles_mouse_events() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;

        // Send a mouse scroll-up event via SGR encoding.
        let mouse_bytes = b"\x1b[<64;10;5M".to_vec();
        // This should not panic even though mouse handling is simplified
        // in headless mode.
        app.route_client_input(mouse_bytes);
        // No assertions on specific behavior — just no panic.
    }

    #[test]
    fn route_client_input_advances_onboarding_modal() {
        let mut app = test_app();
        app.state.mode = Mode::Onboarding;

        app.route_client_input(b"\r".to_vec());

        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn route_client_input_closes_keybind_help_modal() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::KeybindHelp;

        app.route_client_input(b"\x1b".to_vec());

        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn route_client_input_closes_settings_modal() {
        let mut app = test_app();
        app.state.sessions = vec![Workspace::test_new("test")];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Settings;
        app.state.settings.original_theme = Some(app.state.theme_name.clone());
        app.state.settings.original_palette = Some(app.state.palette.clone());

        app.route_client_input(b"\x1b".to_vec());

        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn route_client_input_updates_host_terminal_theme_from_osc_response() {
        let mut app = test_app();

        app.route_client_input(b"\x1b]11;#123456\x07".to_vec());

        assert_eq!(
            app.state.host_terminal_theme.background,
            Some(crate::terminal_theme::RgbColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            })
        );
    }

    #[tokio::test]
    async fn route_client_input_does_not_forward_incomplete_osc_introducer_to_pane() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.focused_pane_id().unwrap();
        let (runtime, mut rx) = TerminalRuntime::test_with_channel_capacity(80, 24, 1);
        workspace.tabs[0].runtimes.insert(focused, runtime);
        app.state.sessions = vec![workspace];
        app.state.active_session = Some(0);
        app.state.selected_session = 0;
        app.state.mode = Mode::Terminal;

        app.route_client_input(b"\x1b]".to_vec());

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn parse_raw_input_bytes_with_ranges_tracks_offsets() {
        // Verify that the range-aware parser correctly tracks byte offsets
        // for events within a multi-event input buffer.
        let input = b"\x1b[Aa".to_vec(); // Up arrow + 'a'
        let events = crate::raw_input::parse_raw_input_bytes_with_ranges(&input);

        assert_eq!(events.len(), 2, "should parse Up arrow and 'a'");
        // Up arrow: \x1b[A = 3 bytes starting at offset 0
        assert_eq!(events[0].start, 0);
        assert_eq!(events[0].len, 3);
        // 'a': 1 byte starting at offset 3
        assert_eq!(events[1].start, 3);
        assert_eq!(events[1].len, 1);

        // Verify the raw bytes for each event are correct.
        assert_eq!(
            &input[events[0].start..events[0].start + events[0].len],
            b"\x1b[A"
        );
        assert_eq!(
            &input[events[1].start..events[1].start + events[1].len],
            b"a"
        );
    }
}
