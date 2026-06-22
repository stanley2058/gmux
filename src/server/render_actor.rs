use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use tracing::warn;

use crate::protocol::{
    self, FrameData, FrameDebugTiming, RenderEncoding, ServerMessage, MAX_FRAME_SIZE,
    MAX_GRAPHICS_FRAME_SIZE,
};
use crate::server::client_transport::{LatestRenderDisconnected, LatestRenderSender};
use crate::server::render_snapshot::{fit_frame_to_client_size, AppFrameSnapshot};
use crate::server::render_stream::ClientRenderState;

pub(crate) struct ClientRenderActor {
    control_tx: std::sync::mpsc::Sender<ClientRenderControl>,
    frame_tx: LatestClientFrameSender,
    // The server no longer reads actor baselines for render decisions, but tests
    // still inspect them to wait for the worker handoff to commit.
    #[cfg_attr(not(test), allow(dead_code))]
    shared: Arc<Mutex<ClientRenderSharedState>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClientRenderDebugContext {
    pub(crate) graphics_us: Option<u64>,
    pub(crate) prepare_us: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClientRenderPublish {
    Sent,
    SkippedUnchanged,
    Disconnected,
    Oversized,
    SerializeError,
}

impl ClientRenderActor {
    pub(crate) fn new(render_encoding: RenderEncoding, writer: LatestRenderSender) -> Self {
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let (frame_tx, frame_rx) = LatestClientFrameSender::channel();
        let shared = Arc::new(Mutex::new(ClientRenderSharedState {
            is_semantic: matches!(render_encoding, RenderEncoding::SemanticFrame),
            ..ClientRenderSharedState::default()
        }));
        let worker_shared = shared.clone();
        std::thread::spawn(move || {
            client_render_worker_loop(render_encoding, writer, control_rx, frame_rx, worker_shared);
        });
        Self {
            control_tx,
            frame_tx,
            shared,
        }
    }

    pub(crate) fn reset_baseline(&mut self) {
        let _ = self.control_tx.send(ClientRenderControl::ResetBaseline);
    }

    pub(crate) fn reset_semantic_input_baseline(&mut self) {
        let _ = self
            .control_tx
            .send(ClientRenderControl::ResetSemanticInputBaseline);
    }

    #[cfg(test)]
    pub(crate) fn last_frame(&self) -> Option<FrameData> {
        self.shared
            .lock()
            .expect("client render shared state lock poisoned")
            .last_frame
            .clone()
    }

    pub(crate) fn publish_frame(
        &mut self,
        client_id: u64,
        frame: FrameData,
        target_size: (u16, u16),
        debug_timing: Option<FrameDebugTiming>,
        debug_context: ClientRenderDebugContext,
    ) -> ClientRenderPublish {
        match self.frame_tx.send(ClientRenderJob {
            client_id,
            source: ClientRenderSource::Frame(frame),
            target_size,
            debug_timing,
            debug_context,
        }) {
            Ok(()) => ClientRenderPublish::Sent,
            Err(LatestClientFrameDisconnected) => ClientRenderPublish::Disconnected,
        }
    }

    pub(crate) fn publish_snapshot(
        &mut self,
        client_id: u64,
        snapshot: Arc<AppFrameSnapshot>,
        target_size: (u16, u16),
        debug_timing: Option<FrameDebugTiming>,
        debug_context: ClientRenderDebugContext,
    ) -> ClientRenderPublish {
        match self.frame_tx.send(ClientRenderJob {
            client_id,
            source: ClientRenderSource::Snapshot(snapshot),
            target_size,
            debug_timing,
            debug_context,
        }) {
            Ok(()) => ClientRenderPublish::Sent,
            Err(LatestClientFrameDisconnected) => ClientRenderPublish::Disconnected,
        }
    }

    #[cfg(test)]
    pub(crate) fn terminal_seq(&self) -> Option<u64> {
        self.shared
            .lock()
            .expect("client render shared state lock poisoned")
            .terminal_seq
    }

    #[cfg(test)]
    pub(crate) fn wait_for_terminal_seq(&self, timeout: std::time::Duration) -> Option<u64> {
        let started = Instant::now();
        loop {
            if let Some(seq) = self.terminal_seq() {
                return Some(seq);
            }
            if started.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_for_last_frame(&self, timeout: std::time::Duration) -> Option<FrameData> {
        let started = Instant::now();
        loop {
            if let Some(frame) = self.last_frame() {
                return Some(frame);
            }
            if started.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[cfg(test)]
    pub(crate) fn processed_jobs(&self) -> u64 {
        self.shared
            .lock()
            .expect("client render shared state lock poisoned")
            .processed_jobs
    }

    #[cfg(test)]
    pub(crate) fn wait_for_processed_jobs(
        &self,
        min_processed_jobs: u64,
        timeout: std::time::Duration,
    ) -> Option<u64> {
        let started = Instant::now();
        loop {
            let processed_jobs = self.processed_jobs();
            if processed_jobs >= min_processed_jobs {
                return Some(processed_jobs);
            }
            if started.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

#[derive(Debug)]
enum ClientRenderControl {
    ResetBaseline,
    ResetSemanticInputBaseline,
}

#[derive(Debug)]
struct ClientRenderJob {
    client_id: u64,
    source: ClientRenderSource,
    target_size: (u16, u16),
    debug_timing: Option<FrameDebugTiming>,
    debug_context: ClientRenderDebugContext,
}

#[derive(Debug)]
enum ClientRenderSource {
    Frame(FrameData),
    Snapshot(Arc<AppFrameSnapshot>),
}

#[derive(Debug, Default)]
struct ClientRenderSharedState {
    is_semantic: bool,
    last_frame: Option<FrameData>,
    terminal_seq: Option<u64>,
    #[cfg(test)]
    processed_jobs: u64,
}

#[derive(Debug)]
struct LatestClientFrameSender {
    shared: Arc<LatestClientFrameShared>,
}

#[derive(Debug)]
struct LatestClientFrameReceiver {
    shared: Arc<LatestClientFrameShared>,
}

#[derive(Debug)]
struct LatestClientFrameShared {
    state: Mutex<LatestClientFrameState>,
    available: Condvar,
    sender_count: std::sync::atomic::AtomicUsize,
}

#[derive(Debug, Default)]
struct LatestClientFrameState {
    frame: Option<ClientRenderJob>,
}

#[derive(Debug)]
struct LatestClientFrameDisconnected;

impl LatestClientFrameSender {
    fn channel() -> (Self, LatestClientFrameReceiver) {
        let shared = Arc::new(LatestClientFrameShared {
            state: Mutex::new(LatestClientFrameState::default()),
            available: Condvar::new(),
            sender_count: std::sync::atomic::AtomicUsize::new(1),
        });
        (
            Self {
                shared: shared.clone(),
            },
            LatestClientFrameReceiver { shared },
        )
    }

    fn send(&self, frame: ClientRenderJob) -> Result<(), LatestClientFrameDisconnected> {
        if self
            .shared
            .sender_count
            .load(std::sync::atomic::Ordering::Acquire)
            == 0
        {
            return Err(LatestClientFrameDisconnected);
        }
        let mut state = self
            .shared
            .state
            .lock()
            .expect("client render slot lock poisoned");
        state.frame = Some(frame);
        self.shared.available.notify_one();
        Ok(())
    }
}

impl Drop for LatestClientFrameSender {
    fn drop(&mut self) {
        if self
            .shared
            .sender_count
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            self.shared.available.notify_one();
        }
    }
}

impl LatestClientFrameReceiver {
    fn recv(&self) -> Option<ClientRenderJob> {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("client render slot lock poisoned");
        loop {
            if let Some(frame) = state.frame.take() {
                return Some(frame);
            }
            if self
                .shared
                .sender_count
                .load(std::sync::atomic::Ordering::Acquire)
                == 0
            {
                return None;
            }
            state = self
                .shared
                .available
                .wait(state)
                .expect("client render slot condvar poisoned");
        }
    }
}

fn client_render_worker_loop(
    render_encoding: RenderEncoding,
    writer: LatestRenderSender,
    control_rx: std::sync::mpsc::Receiver<ClientRenderControl>,
    frame_rx: LatestClientFrameReceiver,
    shared: Arc<Mutex<ClientRenderSharedState>>,
) {
    let mut render_state = ClientRenderState::new(render_encoding);
    loop {
        drain_render_controls(&mut render_state, &control_rx, &shared);
        let Some(job) = frame_rx.recv() else {
            break;
        };
        drain_render_controls(&mut render_state, &control_rx, &shared);
        match publish_frame_now(
            &mut render_state,
            &writer,
            job.client_id,
            job.source,
            job.target_size,
            job.debug_timing,
            job.debug_context,
        ) {
            ClientRenderPublish::Sent | ClientRenderPublish::SkippedUnchanged => {
                update_shared_render_state_after_job(&render_state, &shared);
            }
            ClientRenderPublish::Disconnected => break,
            ClientRenderPublish::Oversized | ClientRenderPublish::SerializeError => {
                update_shared_render_state_after_job(&render_state, &shared);
            }
        }
    }
}

fn drain_render_controls(
    render_state: &mut ClientRenderState,
    control_rx: &std::sync::mpsc::Receiver<ClientRenderControl>,
    shared: &Arc<Mutex<ClientRenderSharedState>>,
) {
    while let Ok(control) = control_rx.try_recv() {
        match control {
            ClientRenderControl::ResetBaseline => render_state.reset_baseline(),
            ClientRenderControl::ResetSemanticInputBaseline => {
                render_state.reset_semantic_input_baseline();
            }
        }
        update_shared_render_state(render_state, shared);
    }
}

fn publish_frame_now(
    render_state: &mut ClientRenderState,
    writer: &LatestRenderSender,
    client_id: u64,
    source: ClientRenderSource,
    target_size: (u16, u16),
    debug_timing: Option<FrameDebugTiming>,
    debug_context: ClientRenderDebugContext,
) -> ClientRenderPublish {
    let (cols, rows) = target_size;
    let mut frame = match source {
        ClientRenderSource::Frame(frame) => {
            let mut fitted = fit_frame_to_client_size(&frame, cols, rows);
            if !frame.graphics.is_empty() {
                fitted.graphics = frame.graphics;
            }
            fitted
        }
        ClientRenderSource::Snapshot(snapshot) => {
            if snapshot.active_size == target_size {
                snapshot.frame.as_ref().clone()
            } else {
                fit_frame_to_client_size(snapshot.frame.as_ref(), cols, rows)
            }
        }
    };
    frame.debug_timing = debug_timing;
    let prepare_started = Instant::now();
    let Some(mut prepared) = render_state.prepare_frame(&frame) else {
        crate::render_prof::event("client_render.skip_identical");
        return ClientRenderPublish::SkippedUnchanged;
    };
    if let Some(mut timing) = frame.debug_timing {
        timing.server_prepare_us = debug_context
            .prepare_us
            .or_else(|| Some(duration_us(prepare_started.elapsed())));
        timing.server_graphics_us = debug_context.graphics_us;
        prepared.set_debug_timing(Some(timing));
    }

    let max_frame_size = if frame.graphics.is_empty() {
        MAX_FRAME_SIZE
    } else {
        MAX_GRAPHICS_FRAME_SIZE
    };
    let serialized = match frame_server_message_with_max(prepared.message(), max_frame_size) {
        Ok(framed) => framed,
        Err(protocol::FramingError::Oversized { claimed, max }) if !frame.graphics.is_empty() => {
            warn!(
                client_id,
                claimed, max, "dropping graphics from oversized frame for client"
            );
            let mut text_only_frame = frame.clone();
            text_only_frame.graphics.clear();
            let Some(text_only_prepared) = render_state.prepare_frame(&text_only_frame) else {
                crate::render_prof::event("client_render.skip_identical_text_only");
                return ClientRenderPublish::SkippedUnchanged;
            };
            let framed = match frame_server_message(text_only_prepared.message()) {
                Ok(framed) => framed,
                Err(err) => {
                    warn!(client_id, err = %err, "failed to serialize text-only frame for client");
                    return ClientRenderPublish::SerializeError;
                }
            };
            prepared = text_only_prepared;
            frame = text_only_frame;
            framed
        }
        Err(protocol::FramingError::Oversized { claimed, max }) => {
            warn!(
                client_id,
                claimed, max, "skipping oversized frame for client"
            );
            return ClientRenderPublish::Oversized;
        }
        Err(err) => {
            warn!(client_id, err = %err, "failed to serialize frame for client");
            return ClientRenderPublish::SerializeError;
        }
    };

    match writer.send(serialized) {
        Ok(()) => {
            let mut frame_to_commit = frame;
            frame_to_commit.debug_timing = None;
            render_state.commit_sent_frame(frame_to_commit, prepared);
            ClientRenderPublish::Sent
        }
        Err(LatestRenderDisconnected) => ClientRenderPublish::Disconnected,
    }
}

fn update_shared_render_state(
    render_state: &ClientRenderState,
    shared: &Arc<Mutex<ClientRenderSharedState>>,
) {
    let mut shared = shared
        .lock()
        .expect("client render shared state lock poisoned");
    shared.is_semantic = render_state.is_semantic();
    shared.last_frame = render_state.last_frame().cloned();
    shared.terminal_seq = render_state.terminal_seq();
}

fn update_shared_render_state_after_job(
    render_state: &ClientRenderState,
    shared: &Arc<Mutex<ClientRenderSharedState>>,
) {
    let mut shared = shared
        .lock()
        .expect("client render shared state lock poisoned");
    shared.is_semantic = render_state.is_semantic();
    shared.last_frame = render_state.last_frame().cloned();
    shared.terminal_seq = render_state.terminal_seq();
    #[cfg(test)]
    {
        shared.processed_jobs = shared.processed_jobs.saturating_add(1);
    }
}

pub(crate) fn frame_server_message(msg: &ServerMessage) -> Result<Vec<u8>, protocol::FramingError> {
    frame_server_message_with_max(msg, MAX_FRAME_SIZE)
}

pub(crate) fn frame_server_message_with_max(
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

fn duration_us(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CellData, FrameData, ServerMessage};
    use crate::server::client_transport::{LatestRenderRecvError, LatestRenderSender};
    use std::time::Duration;

    fn frame(symbol: &str) -> FrameData {
        FrameData {
            cells: vec![CellData {
                symbol: symbol.to_owned(),
                fg: 0,
                bg: 0,
                modifier: 0,
                underline_color: 0,
                underline_style: crate::protocol::UNDERLINE_NONE,
                overline: false,
                skip: false,
                hyperlink: None,
            }],
            width: 1,
            height: 1,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
            debug_timing: None,
        }
    }

    fn read_server_message(bytes: Vec<u8>) -> ServerMessage {
        let mut cursor = std::io::Cursor::new(bytes);
        protocol::read_message(&mut cursor, MAX_GRAPHICS_FRAME_SIZE).expect("decode server message")
    }

    #[test]
    fn semantic_actor_skips_unchanged_frames() {
        let (tx, rx) = LatestRenderSender::channel();
        let mut actor = ClientRenderActor::new(RenderEncoding::SemanticFrame, tx);

        assert_eq!(
            actor.publish_frame(
                1,
                frame("a"),
                (1, 1),
                None,
                ClientRenderDebugContext::default()
            ),
            ClientRenderPublish::Sent
        );
        assert!(matches!(
            read_server_message(rx.recv().expect("semantic frame")),
            ServerMessage::Frame(_)
        ));
        assert_eq!(
            actor.publish_frame(
                1,
                frame("a"),
                (1, 1),
                None,
                ClientRenderDebugContext::default()
            ),
            ClientRenderPublish::Sent
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(50)),
            Err(LatestRenderRecvError::Timeout)
        );
    }

    #[test]
    fn terminal_actor_commits_sequence_after_publish() {
        let (tx, rx) = LatestRenderSender::channel();
        let mut actor = ClientRenderActor::new(RenderEncoding::TerminalAnsi, tx);

        assert_eq!(
            actor.publish_frame(
                1,
                frame("a"),
                (1, 1),
                None,
                ClientRenderDebugContext::default()
            ),
            ClientRenderPublish::Sent
        );

        match read_server_message(rx.recv().expect("terminal frame")) {
            ServerMessage::Terminal(frame) => assert_eq!(frame.seq, 1),
            other => panic!("expected terminal frame, got {other:?}"),
        }
        actor
            .wait_for_processed_jobs(1, Duration::from_millis(100))
            .expect("terminal frame commit");
        assert_eq!(actor.terminal_seq(), Some(1));
    }

    #[test]
    fn actor_fits_app_snapshot_to_target_size() {
        let (tx, rx) = LatestRenderSender::channel();
        let mut actor = ClientRenderActor::new(RenderEncoding::SemanticFrame, tx);
        let frame = FrameData {
            cells: vec![
                CellData {
                    symbol: "a".to_owned(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    underline_color: 0,
                    underline_style: crate::protocol::UNDERLINE_NONE,
                    overline: false,
                    skip: false,
                    hyperlink: None,
                },
                CellData {
                    symbol: "b".to_owned(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    underline_color: 0,
                    underline_style: crate::protocol::UNDERLINE_NONE,
                    overline: false,
                    skip: false,
                    hyperlink: None,
                },
                CellData {
                    symbol: "c".to_owned(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    underline_color: 0,
                    underline_style: crate::protocol::UNDERLINE_NONE,
                    overline: false,
                    skip: false,
                    hyperlink: None,
                },
                CellData {
                    symbol: "d".to_owned(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    underline_color: 0,
                    underline_style: crate::protocol::UNDERLINE_NONE,
                    overline: false,
                    skip: false,
                    hyperlink: None,
                },
            ],
            width: 2,
            height: 2,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
            debug_timing: None,
        };
        let snapshot = Arc::new(AppFrameSnapshot::new(
            1,
            1,
            frame,
            crate::server::render_snapshot::ServerRenderDebug::default(),
        ));

        assert_eq!(
            actor.publish_snapshot(
                1,
                snapshot,
                (1, 1),
                None,
                ClientRenderDebugContext::default()
            ),
            ClientRenderPublish::Sent
        );

        match read_server_message(rx.recv().expect("clipped frame")) {
            ServerMessage::Frame(frame) => {
                assert_eq!((frame.width, frame.height), (1, 1));
                assert_eq!(frame.cells[0].symbol, "a");
            }
            other => panic!("expected semantic frame, got {other:?}"),
        }
    }
}
