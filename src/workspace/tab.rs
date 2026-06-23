use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ratatui::layout::Direction;
use tokio::sync::{mpsc, Notify};

use crate::events::AppEvent;
use crate::layout::{PaneId, TileLayout};
use crate::pane::PaneState;
use crate::terminal::{TerminalId, TerminalRuntime, TerminalRuntimeRegistry, TerminalState};

pub(crate) type DetachedPane = (PaneId, TerminalId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupDimension {
    Cells(u16),
    Percent(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupPosition {
    Cells(u16),
    Percent(u16),
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupGeometry {
    pub width: PopupDimension,
    pub height: PopupDimension,
    pub x: PopupPosition,
    pub y: PopupPosition,
}

impl Default for PopupGeometry {
    fn default() -> Self {
        Self {
            width: PopupDimension::Percent(80),
            height: PopupDimension::Percent(60),
            x: PopupPosition::Center,
            y: PopupPosition::Center,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupPaneState {
    pub geometry: PopupGeometry,
    pub previous_focus: PaneId,
}

pub struct NewPane {
    pub pane_id: PaneId,
    pub terminal: TerminalState,
    pub runtime: TerminalRuntime,
}

enum SplitCommand<'a> {
    Shell {
        command: &'a str,
        extra_env: &'a [(String, String)],
    },
    Argv {
        argv: &'a [String],
    },
}

pub struct Tab {
    pub custom_name: Option<String>,
    pub number: usize,
    /// Identity source for this tab's pane tree.
    pub root_pane: PaneId,
    pub layout: TileLayout,
    /// Pane viewport state — always present, testable without PTYs.
    pub panes: HashMap<PaneId, PaneState>,
    /// Terminal panes drawn above the tiled layout.
    pub popup_panes: HashMap<PaneId, PopupPaneState>,
    pub focused_popup: Option<PaneId>,
    #[cfg(test)]
    pub runtimes: HashMap<PaneId, TerminalRuntime>,
    pub zoomed: bool,
    pub events: mpsc::Sender<AppEvent>,
    pub(crate) render_notify: Arc<Notify>,
    pub(crate) render_dirty: Arc<AtomicBool>,
}

impl Tab {
    pub fn new(
        number: usize,
        initial_cwd: PathBuf,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        shell_config: crate::pane::PaneShellConfig<'_>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<AtomicBool>,
    ) -> std::io::Result<(Self, TerminalState, TerminalRuntime)> {
        Self::new_with_runtime(
            number,
            initial_cwd,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            shell_config,
            events,
            render_notify,
            render_dirty,
            None,
        )
    }

    pub fn new_shell_command(
        number: usize,
        initial_cwd: PathBuf,
        rows: u16,
        cols: u16,
        command: &str,
        term: &str,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<AtomicBool>,
    ) -> std::io::Result<(Self, TerminalState, TerminalRuntime)> {
        Self::new_with_runtime(
            number,
            initial_cwd,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            crate::pane::PaneShellConfig::new("", crate::config::ShellModeConfig::NonLogin)
                .with_term(term),
            events,
            render_notify,
            render_dirty,
            Some(SplitCommand::Shell {
                command,
                extra_env: &[],
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_runtime(
        number: usize,
        initial_cwd: PathBuf,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        shell_config: crate::pane::PaneShellConfig<'_>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<AtomicBool>,
        command: Option<SplitCommand<'_>>,
    ) -> std::io::Result<(Self, TerminalState, TerminalRuntime)> {
        let (layout, root_id) = TileLayout::new();
        let launch_argv = match &command {
            Some(SplitCommand::Argv { argv }) => Some((*argv).to_vec()),
            Some(SplitCommand::Shell { command, .. }) => {
                Some(vec!["/bin/sh".into(), "-c".into(), (*command).to_string()])
            }
            None => None,
        };
        let runtime = match command {
            Some(SplitCommand::Argv { argv }) => TerminalRuntime::spawn_argv_command(
                root_id,
                rows,
                cols,
                initial_cwd.clone(),
                argv,
                shell_config.pane_term(),
                scrollback_limit_bytes,
                host_terminal_theme,
                events.clone(),
                render_notify.clone(),
                render_dirty.clone(),
            ),
            Some(SplitCommand::Shell { command, extra_env }) => {
                TerminalRuntime::spawn_shell_command(
                    root_id,
                    rows,
                    cols,
                    initial_cwd.clone(),
                    crate::pane::PaneCommandConfig::new(
                        command,
                        extra_env,
                        shell_config.pane_term(),
                    ),
                    scrollback_limit_bytes,
                    host_terminal_theme,
                    events.clone(),
                    render_notify.clone(),
                    render_dirty.clone(),
                )
            }
            None => TerminalRuntime::spawn(
                root_id,
                rows,
                cols,
                initial_cwd.clone(),
                scrollback_limit_bytes,
                host_terminal_theme,
                shell_config,
                events.clone(),
                render_notify.clone(),
                render_dirty.clone(),
            ),
        }?;

        let terminal_id = TerminalId::alloc();
        let terminal = match launch_argv {
            Some(argv) => {
                TerminalState::new(terminal_id.clone(), initial_cwd).with_launch_argv(argv)
            }
            None => TerminalState::new(terminal_id.clone(), initial_cwd),
        };
        let mut panes = HashMap::new();
        panes.insert(root_id, PaneState::new(terminal_id));

        Ok((
            Self {
                custom_name: None,
                number,
                root_pane: root_id,
                layout,
                panes,
                popup_panes: HashMap::new(),
                focused_popup: None,
                #[cfg(test)]
                runtimes: HashMap::new(),
                zoomed: false,
                events,
                render_notify,
                render_dirty,
            },
            terminal,
            runtime,
        ))
    }

    pub fn display_name(&self) -> String {
        self.custom_name
            .clone()
            .unwrap_or_else(|| self.number.to_string())
    }

    pub fn is_auto_named(&self) -> bool {
        self.custom_name.is_none()
    }

    pub fn set_custom_name(&mut self, name: String) {
        self.custom_name = Some(name);
    }

    pub fn split_focused(
        &mut self,
        direction: Direction,
        rows: u16,
        cols: u16,
        cwd: Option<PathBuf>,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        shell_config: crate::pane::PaneShellConfig<'_>,
    ) -> std::io::Result<NewPane> {
        self.split_focused_with_runtime(
            direction,
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            shell_config,
            None,
        )
    }

    pub fn split_focused_command(
        &mut self,
        direction: Direction,
        rows: u16,
        cols: u16,
        cwd: Option<PathBuf>,
        command: &str,
        extra_env: &[(String, String)],
        term: &str,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
    ) -> std::io::Result<NewPane> {
        self.split_focused_with_runtime(
            direction,
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            crate::pane::PaneShellConfig::new("", crate::config::ShellModeConfig::NonLogin)
                .with_term(term),
            Some(SplitCommand::Shell { command, extra_env }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_popup_command(
        &mut self,
        rows: u16,
        cols: u16,
        cwd: Option<PathBuf>,
        command: &str,
        extra_env: &[(String, String)],
        term: &str,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        geometry: PopupGeometry,
        focus: bool,
    ) -> std::io::Result<NewPane> {
        let new_id = PaneId::alloc();
        let previous_focus = self.focused_pane_id();
        let actual_cwd =
            cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        let runtime = TerminalRuntime::spawn_shell_command(
            new_id,
            rows,
            cols,
            actual_cwd.clone(),
            crate::pane::PaneCommandConfig::new(command, extra_env, term),
            scrollback_limit_bytes,
            host_terminal_theme,
            self.events.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        )?;
        let terminal_id = TerminalId::alloc();
        let terminal = TerminalState::new(terminal_id.clone(), actual_cwd).with_launch_argv(vec![
            "/bin/sh".into(),
            "-c".into(),
            command.to_string(),
        ]);
        self.panes.insert(new_id, PaneState::new(terminal_id));
        self.popup_panes.insert(
            new_id,
            PopupPaneState {
                geometry,
                previous_focus,
            },
        );
        if focus {
            self.focused_popup = Some(new_id);
        }
        Ok(NewPane {
            pane_id: new_id,
            terminal,
            runtime,
        })
    }

    pub fn split_focused_argv_command(
        &mut self,
        direction: Direction,
        rows: u16,
        cols: u16,
        cwd: Option<PathBuf>,
        argv: &[String],
        term: &str,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
    ) -> std::io::Result<NewPane> {
        self.split_focused_with_runtime(
            direction,
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            crate::pane::PaneShellConfig::new("", crate::config::ShellModeConfig::NonLogin)
                .with_term(term),
            Some(SplitCommand::Argv { argv }),
        )
    }

    fn split_focused_with_runtime(
        &mut self,
        direction: Direction,
        rows: u16,
        cols: u16,
        cwd: Option<PathBuf>,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        shell_config: crate::pane::PaneShellConfig<'_>,
        command: Option<SplitCommand<'_>>,
    ) -> std::io::Result<NewPane> {
        let previous_focus = self.layout.focused();
        let new_id = self.layout.split_focused(direction);
        let actual_cwd =
            cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        let launch_argv = if let Some(SplitCommand::Argv { argv }) = &command {
            Some((*argv).to_vec())
        } else if let Some(SplitCommand::Shell { command, .. }) = &command {
            Some(vec!["/bin/sh".into(), "-c".into(), (*command).to_string()])
        } else {
            None
        };
        let runtime = match command {
            Some(SplitCommand::Shell { command, extra_env }) => {
                TerminalRuntime::spawn_shell_command(
                    new_id,
                    rows,
                    cols,
                    actual_cwd.clone(),
                    crate::pane::PaneCommandConfig::new(
                        command,
                        extra_env,
                        shell_config.pane_term(),
                    ),
                    scrollback_limit_bytes,
                    host_terminal_theme,
                    self.events.clone(),
                    self.render_notify.clone(),
                    self.render_dirty.clone(),
                )
            }
            Some(SplitCommand::Argv { argv }) => TerminalRuntime::spawn_argv_command(
                new_id,
                rows,
                cols,
                actual_cwd.clone(),
                argv,
                shell_config.pane_term(),
                scrollback_limit_bytes,
                host_terminal_theme,
                self.events.clone(),
                self.render_notify.clone(),
                self.render_dirty.clone(),
            ),
            None => TerminalRuntime::spawn(
                new_id,
                rows,
                cols,
                actual_cwd.clone(),
                scrollback_limit_bytes,
                host_terminal_theme,
                shell_config,
                self.events.clone(),
                self.render_notify.clone(),
                self.render_dirty.clone(),
            ),
        };
        let runtime = match runtime {
            Ok(runtime) => runtime,
            Err(err) => {
                self.layout.close_focused();
                self.layout.focus_pane(previous_focus);
                return Err(err);
            }
        };
        let terminal_id = TerminalId::alloc();
        let terminal = match launch_argv {
            Some(argv) => {
                TerminalState::new(terminal_id.clone(), actual_cwd).with_launch_argv(argv)
            }
            None => TerminalState::new(terminal_id.clone(), actual_cwd),
        };
        self.panes.insert(new_id, PaneState::new(terminal_id));
        self.zoomed = false;
        Ok(NewPane {
            pane_id: new_id,
            terminal,
            runtime,
        })
    }

    pub fn close_focused(&mut self) -> Option<DetachedPane> {
        let pane_id = self.focused_pane_id();
        self.detach_pane(pane_id)
    }

    pub fn swap_focused_pane(&mut self, reverse: bool) -> bool {
        let ids = self.layout.pane_ids();
        if ids.len() <= 1 {
            return false;
        }
        let Some(pos) = ids.iter().position(|id| *id == self.layout.focused()) else {
            return false;
        };
        let target = if reverse {
            ids[(pos + ids.len() - 1) % ids.len()]
        } else {
            ids[(pos + 1) % ids.len()]
        };
        self.zoomed = false;
        self.layout.swap_panes(self.layout.focused(), target)
    }

    pub fn remove_pane(&mut self, pane_id: PaneId) -> Option<DetachedPane> {
        self.detach_pane(pane_id)
    }

    fn detach_pane(&mut self, pane_id: PaneId) -> Option<DetachedPane> {
        if self.popup_panes.contains_key(&pane_id) {
            return self.detach_popup_pane(pane_id);
        }

        if self.layout.pane_count() <= 1 {
            return None;
        }

        let next_root = self.promoted_root_if_needed(pane_id);

        if self.layout.focused() == pane_id {
            self.layout.close_focused();
        } else {
            let prev_focus = self.layout.focused();
            self.layout.focus_pane(pane_id);
            self.layout.close_focused();
            self.layout.focus_pane(prev_focus);
        }

        let pane = self.panes.remove(&pane_id)?;
        let terminal_id = pane.attached_terminal_id;
        self.zoomed = false;
        if let Some(next_root) = next_root {
            self.root_pane = next_root;
        }
        Some((pane_id, terminal_id))
    }

    fn detach_popup_pane(&mut self, pane_id: PaneId) -> Option<DetachedPane> {
        let popup = self.popup_panes.remove(&pane_id)?;
        let pane = self.panes.remove(&pane_id)?;
        if self.focused_popup == Some(pane_id) {
            self.focused_popup = None;
            if self.popup_panes.contains_key(&popup.previous_focus) {
                self.focused_popup = Some(popup.previous_focus);
            } else {
                self.layout.focus_pane(popup.previous_focus);
            }
        }
        Some((pane_id, pane.attached_terminal_id))
    }

    fn promoted_root_if_needed(&self, closing: PaneId) -> Option<PaneId> {
        if self.root_pane != closing {
            return None;
        }
        self.layout.pane_ids().into_iter().find(|id| *id != closing)
    }

    pub fn terminal_id(&self, pane_id: PaneId) -> Option<&TerminalId> {
        self.panes
            .get(&pane_id)
            .map(|pane| &pane.attached_terminal_id)
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = self.layout.pane_ids();
        ids.extend(self.popup_panes.keys().copied());
        ids
    }

    pub fn tiled_pane_ids(&self) -> Vec<PaneId> {
        self.layout.pane_ids()
    }

    pub fn focused_pane_id(&self) -> PaneId {
        self.focused_popup.unwrap_or_else(|| self.layout.focused())
    }

    pub fn focus_pane(&mut self, pane_id: PaneId) -> bool {
        if self.popup_panes.contains_key(&pane_id) {
            self.focused_popup = Some(pane_id);
            return true;
        }
        if self.layout.pane_ids().contains(&pane_id) {
            self.focused_popup = None;
            self.layout.focus_pane(pane_id);
            return true;
        }
        false
    }

    pub fn cwd_for_pane(
        &self,
        pane_id: PaneId,
        terminals: &HashMap<TerminalId, TerminalState>,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) -> Option<PathBuf> {
        let terminal_id = self.terminal_id(pane_id)?;
        terminal_runtimes
            .get(terminal_id)
            .and_then(|rt| rt.cwd())
            .or_else(|| {
                terminals
                    .get(terminal_id)
                    .map(|terminal| terminal.cwd.clone())
            })
    }

    pub fn foreground_cwd_for_pane(
        &self,
        pane_id: PaneId,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) -> Option<PathBuf> {
        let terminal_id = self.terminal_id(pane_id)?;
        terminal_runtimes
            .get(terminal_id)
            .and_then(|rt| rt.foreground_cwd())
    }
}
