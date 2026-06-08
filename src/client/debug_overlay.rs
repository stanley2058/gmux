use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crate::protocol::{FrameDebugTiming, RenderEncoding};

const ENV_VAR: &str = "GMUX_CLIENT_DEBUG_OVERLAY";
const MAX_PENDING_INPUTS: usize = 1024;
const OVERLAY_WIDTH: usize = 52;

#[derive(Clone, Copy)]
pub(super) enum ClientFrameKind {
    Semantic,
    TerminalAnsi { seq: u64 },
}

#[derive(Clone, Copy)]
pub(super) struct ClientFrameMetrics {
    pub(super) kind: ClientFrameKind,
    pub(super) frame_bytes: usize,
    pub(super) encoded_bytes: usize,
    pub(super) full: bool,
    pub(super) encode_duration: Option<Duration>,
    pub(super) write_duration: Duration,
    pub(super) server_timing: Option<FrameDebugTiming>,
    pub(super) now: Instant,
}

#[derive(Clone, Copy)]
struct WindowReport {
    fps: f64,
    input_hz: f64,
    avg_input_latency: Option<Duration>,
    max_input_latency: Option<Duration>,
    avg_frame_interval: Option<Duration>,
    max_frame_interval: Option<Duration>,
}

impl Default for WindowReport {
    fn default() -> Self {
        Self {
            fps: 0.0,
            input_hz: 0.0,
            avg_input_latency: None,
            max_input_latency: None,
            avg_frame_interval: None,
            max_frame_interval: None,
        }
    }
}

pub(super) struct ClientDebugOverlay {
    enabled: bool,
    encoding: RenderEncoding,
    started: Instant,
    window_started: Instant,
    window_frame_count: u64,
    window_input_count: u64,
    window_latency_count: u64,
    window_latency_total: Duration,
    window_latency_max: Option<Duration>,
    window_interval_count: u64,
    window_interval_total: Duration,
    window_interval_max: Option<Duration>,
    report: WindowReport,
    total_frames: u64,
    total_inputs: u64,
    pending_inputs: VecDeque<Instant>,
    last_input_latency: Option<Duration>,
    last_input_write_duration: Option<Duration>,
    last_input_bytes: usize,
    last_frame_at: Option<Instant>,
    last_frame_kind: Option<ClientFrameKind>,
    last_frame_bytes: usize,
    last_encoded_bytes: usize,
    last_frame_full: bool,
    last_encode_duration: Option<Duration>,
    last_write_duration: Option<Duration>,
    last_server_timing: Option<FrameDebugTiming>,
    last_event_queue_len: usize,
}

impl ClientDebugOverlay {
    pub(super) fn from_env(encoding: RenderEncoding) -> Self {
        Self::new(env_enabled(), encoding, Instant::now())
    }

    fn new(enabled: bool, encoding: RenderEncoding, now: Instant) -> Self {
        Self {
            enabled,
            encoding,
            started: now,
            window_started: now,
            window_frame_count: 0,
            window_input_count: 0,
            window_latency_count: 0,
            window_latency_total: Duration::ZERO,
            window_latency_max: None,
            window_interval_count: 0,
            window_interval_total: Duration::ZERO,
            window_interval_max: None,
            report: WindowReport::default(),
            total_frames: 0,
            total_inputs: 0,
            pending_inputs: VecDeque::new(),
            last_input_latency: None,
            last_input_write_duration: None,
            last_input_bytes: 0,
            last_frame_at: None,
            last_frame_kind: None,
            last_frame_bytes: 0,
            last_encoded_bytes: 0,
            last_frame_full: false,
            last_encode_duration: None,
            last_write_duration: None,
            last_server_timing: None,
            last_event_queue_len: 0,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn record_event_queue_len(&mut self, len: usize) {
        if self.enabled {
            self.last_event_queue_len = len;
        }
    }

    pub(super) fn record_input(
        &mut self,
        received_at: Instant,
        bytes: usize,
        write_duration: Duration,
    ) {
        if !self.enabled {
            return;
        }

        self.total_inputs += 1;
        self.window_input_count += 1;
        self.last_input_bytes = bytes;
        self.last_input_write_duration = Some(write_duration);
        if self.pending_inputs.len() == MAX_PENDING_INPUTS {
            self.pending_inputs.pop_front();
        }
        self.pending_inputs.push_back(received_at);
        self.roll_window_if_due(Instant::now());
    }

    pub(super) fn record_frame(&mut self, metrics: ClientFrameMetrics) {
        if !self.enabled {
            return;
        }

        self.total_frames += 1;
        self.window_frame_count += 1;

        if let Some(last_frame_at) = self.last_frame_at {
            let interval = metrics.now.saturating_duration_since(last_frame_at);
            self.window_interval_count += 1;
            self.window_interval_total += interval;
            self.window_interval_max = Some(
                self.window_interval_max
                    .map_or(interval, |current| current.max(interval)),
            );
        }

        while let Some(input_at) = self.pending_inputs.pop_front() {
            let latency = metrics.now.saturating_duration_since(input_at);
            self.last_input_latency = Some(latency);
            self.window_latency_count += 1;
            self.window_latency_total += latency;
            self.window_latency_max = Some(
                self.window_latency_max
                    .map_or(latency, |current| current.max(latency)),
            );
        }

        self.last_frame_at = Some(metrics.now);
        self.last_frame_kind = Some(metrics.kind);
        self.last_frame_bytes = metrics.frame_bytes;
        self.last_encoded_bytes = metrics.encoded_bytes;
        self.last_frame_full = metrics.full;
        self.last_encode_duration = metrics.encode_duration;
        self.last_write_duration = Some(metrics.write_duration);
        self.last_server_timing = metrics.server_timing;
        self.roll_window_if_due(metrics.now);
    }

    pub(super) fn write(&self, writer: &mut impl io::Write) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let lines = self.lines(Instant::now());
        writer.write_all(b"\x1b7")?;
        for (index, line) in lines.iter().enumerate() {
            write!(
                writer,
                "\x1b[{};1H\x1b[48;5;236m\x1b[38;5;231m {:<width$} \x1b[0m",
                index + 1,
                truncate_for_overlay(line),
                width = OVERLAY_WIDTH
            )?;
        }
        writer.write_all(b"\x1b8")
    }

    fn roll_window_if_due(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.window_started);
        if elapsed < Duration::from_secs(1) {
            return;
        }

        let elapsed_secs = elapsed.as_secs_f64();
        self.report = WindowReport {
            fps: self.window_frame_count as f64 / elapsed_secs,
            input_hz: self.window_input_count as f64 / elapsed_secs,
            avg_input_latency: average_duration(
                self.window_latency_total,
                self.window_latency_count,
            ),
            max_input_latency: self.window_latency_max,
            avg_frame_interval: average_duration(
                self.window_interval_total,
                self.window_interval_count,
            ),
            max_frame_interval: self.window_interval_max,
        };

        self.window_started = now;
        self.window_frame_count = 0;
        self.window_input_count = 0;
        self.window_latency_count = 0;
        self.window_latency_total = Duration::ZERO;
        self.window_latency_max = None;
        self.window_interval_count = 0;
        self.window_interval_total = Duration::ZERO;
        self.window_interval_max = None;
    }

    fn lines(&self, now: Instant) -> Vec<String> {
        let age = self
            .last_frame_at
            .map(|last_frame_at| now.saturating_duration_since(last_frame_at));
        vec![
            format!(
                "gmux client fps {:>5.1} input {:>5.1}/s q {}",
                self.report.fps, self.report.input_hz, self.last_event_queue_len
            ),
            format!(
                "input->frame last {} avg {} max {} pending {}",
                fmt_duration(self.last_input_latency),
                fmt_duration(self.report.avg_input_latency),
                fmt_duration(self.report.max_input_latency),
                self.pending_inputs.len()
            ),
            format!(
                "frame dt avg {} max {} age {} total {}",
                fmt_duration(self.report.avg_frame_interval),
                fmt_duration(self.report.max_frame_interval),
                fmt_duration(age),
                self.total_frames
            ),
            format!(
                "write frame {} input {} encode {}",
                fmt_duration(self.last_write_duration),
                fmt_duration(self.last_input_write_duration),
                fmt_duration(self.last_encode_duration)
            ),
            format!(
                "server q {} handle->frame {} dirty->frame {}",
                fmt_server_us(
                    self.last_server_timing
                        .map(|timing| timing.server_input_queue_us)
                ),
                fmt_server_us(
                    self.last_server_timing
                        .map(|timing| timing.server_input_to_frame_us)
                ),
                fmt_server_us(
                    self.last_server_timing
                        .and_then(|timing| timing.server_pty_dirty_to_frame_us)
                )
            ),
            format!(
                "srv render {} build {} gfx {} prep {}",
                fmt_server_us(
                    self.last_server_timing
                        .and_then(|timing| timing.server_render_us)
                ),
                fmt_server_us(
                    self.last_server_timing
                        .and_then(|timing| timing.server_frame_build_us)
                ),
                fmt_server_us(
                    self.last_server_timing
                        .and_then(|timing| timing.server_graphics_us)
                ),
                fmt_server_us(
                    self.last_server_timing
                        .and_then(|timing| timing.server_prepare_us)
                ),
            ),
            format!(
                "srv targets {} role {} mirror-pending {}",
                self.last_server_timing
                    .map(|timing| timing.server_target_count)
                    .unwrap_or(0),
                server_role_label(self.last_server_timing),
                yes_no(
                    self.last_server_timing
                        .map(|timing| timing.server_pending_mirror)
                        .unwrap_or(false)
                )
            ),
            format!(
                "{} frame {}B encoded {}B full {} in {}B",
                self.frame_kind_label(),
                self.last_frame_bytes,
                self.last_encoded_bytes,
                yes_no(self.last_frame_full),
                self.last_input_bytes
            ),
            format!(
                "encoding {:?} uptime {} inputs {}",
                self.encoding,
                fmt_duration(Some(now.saturating_duration_since(self.started))),
                self.total_inputs
            ),
        ]
    }

    fn frame_kind_label(&self) -> String {
        match self.last_frame_kind {
            Some(ClientFrameKind::Semantic) => "semantic".to_string(),
            Some(ClientFrameKind::TerminalAnsi { seq }) => format!("terminal seq {seq}"),
            None => "frame none".to_string(),
        }
    }
}

fn server_role_label(timing: Option<FrameDebugTiming>) -> &'static str {
    match timing {
        Some(timing) if timing.server_active_only => "active",
        Some(timing) if timing.server_mirror_flush => "mirror",
        Some(_) => "normal",
        None => "--",
    }
}

fn fmt_server_us(us: Option<u64>) -> String {
    fmt_duration(us.map(Duration::from_micros))
}

pub(super) fn env_enabled() -> bool {
    std::env::var(ENV_VAR)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn average_duration(total: Duration, count: u64) -> Option<Duration> {
    if count == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(total.as_secs_f64() / count as f64))
}

fn fmt_duration(duration: Option<Duration>) -> String {
    let Some(duration) = duration else {
        return "--".to_string();
    };

    if duration >= Duration::from_secs(1) {
        format!("{:.2}s", duration.as_secs_f64())
    } else if duration >= Duration::from_millis(1) {
        format!("{:.1}ms", duration.as_secs_f64() * 1_000.0)
    } else {
        format!("{}us", duration.as_micros())
    }
}

fn truncate_for_overlay(line: &str) -> String {
    line.chars().take(OVERLAY_WIDTH).collect()
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_input_to_next_frame_latency() {
        let start = Instant::now();
        let mut overlay = ClientDebugOverlay::new(true, RenderEncoding::TerminalAnsi, start);

        overlay.record_input(
            start + Duration::from_millis(5),
            1,
            Duration::from_millis(1),
        );
        overlay.record_frame(ClientFrameMetrics {
            kind: ClientFrameKind::TerminalAnsi { seq: 1 },
            frame_bytes: 20,
            encoded_bytes: 20,
            full: false,
            encode_duration: None,
            write_duration: Duration::from_millis(2),
            server_timing: None,
            now: start + Duration::from_millis(17),
        });

        assert_eq!(overlay.pending_inputs.len(), 0);
        assert_eq!(overlay.last_input_latency, Some(Duration::from_millis(12)));
    }

    #[test]
    fn rolls_recent_fps_window() {
        let start = Instant::now();
        let mut overlay = ClientDebugOverlay::new(true, RenderEncoding::TerminalAnsi, start);

        overlay.record_frame(ClientFrameMetrics {
            kind: ClientFrameKind::TerminalAnsi { seq: 1 },
            frame_bytes: 20,
            encoded_bytes: 20,
            full: true,
            encode_duration: None,
            write_duration: Duration::from_millis(1),
            server_timing: None,
            now: start + Duration::from_millis(100),
        });
        overlay.record_frame(ClientFrameMetrics {
            kind: ClientFrameKind::TerminalAnsi { seq: 2 },
            frame_bytes: 10,
            encoded_bytes: 10,
            full: false,
            encode_duration: None,
            write_duration: Duration::from_millis(1),
            server_timing: None,
            now: start + Duration::from_millis(1100),
        });

        assert!(overlay.report.fps > 1.7 && overlay.report.fps < 1.9);
        assert_eq!(
            overlay.report.avg_frame_interval,
            Some(Duration::from_millis(1000))
        );
    }

    #[test]
    fn writes_overlay_with_saved_cursor() {
        let start = Instant::now();
        let overlay = ClientDebugOverlay::new(true, RenderEncoding::TerminalAnsi, start);
        let mut output = Vec::new();

        overlay.write(&mut output).unwrap();

        assert!(output.starts_with(b"\x1b7\x1b[1;1H"));
        assert!(output.ends_with(b"\x1b8"));
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("gmux client fps"));
    }
}
