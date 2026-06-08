use std::time::Instant;

use tracing::warn;

use crate::protocol::{
    self, FrameData, FrameDebugTiming, RenderEncoding, ServerMessage, MAX_FRAME_SIZE,
    MAX_GRAPHICS_FRAME_SIZE,
};
use crate::server::client_transport::{LatestRenderDisconnected, LatestRenderSender};
use crate::server::render_stream::ClientRenderState;

pub(crate) struct ClientRenderActor {
    render_state: ClientRenderState,
    writer: LatestRenderSender,
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
        Self {
            render_state: ClientRenderState::new(render_encoding),
            writer,
        }
    }

    pub(crate) fn reset_baseline(&mut self) {
        self.render_state.reset_baseline();
    }

    pub(crate) fn reset_semantic_input_baseline(&mut self) {
        self.render_state.reset_semantic_input_baseline();
    }

    pub(crate) fn last_frame(&self) -> Option<&FrameData> {
        self.render_state.last_frame()
    }

    pub(crate) fn is_semantic(&self) -> bool {
        self.render_state.is_semantic()
    }

    pub(crate) fn is_semantic_frame_current(&self, frame: &FrameData) -> bool {
        self.render_state.semantic_frame_is_current(frame)
    }

    pub(crate) fn commit_semantic_frame(&mut self, frame: FrameData) -> bool {
        self.render_state.commit_semantic_frame(frame)
    }

    pub(crate) fn publish_frame(
        &mut self,
        client_id: u64,
        mut frame: FrameData,
        debug_timing: Option<FrameDebugTiming>,
        debug_context: ClientRenderDebugContext,
    ) -> ClientRenderPublish {
        frame.debug_timing = debug_timing;
        let prepare_started = Instant::now();
        let Some(mut prepared) = self.render_state.prepare_frame(&frame) else {
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
            Err(protocol::FramingError::Oversized { claimed, max })
                if !frame.graphics.is_empty() =>
            {
                warn!(
                    client_id,
                    claimed, max, "dropping graphics from oversized frame for client"
                );
                let mut text_only_frame = frame.clone();
                text_only_frame.graphics.clear();
                let Some(text_only_prepared) = self.render_state.prepare_frame(&text_only_frame)
                else {
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

        match self.writer.send(serialized) {
            Ok(()) => {
                let mut frame_to_commit = frame;
                frame_to_commit.debug_timing = None;
                self.render_state
                    .commit_sent_frame(frame_to_commit, prepared);
                ClientRenderPublish::Sent
            }
            Err(LatestRenderDisconnected) => ClientRenderPublish::Disconnected,
        }
    }

    #[cfg(test)]
    pub(crate) fn terminal_seq(&self) -> Option<u64> {
        self.render_state.terminal_seq()
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
    use crate::server::client_transport::LatestRenderSender;

    fn frame(symbol: &str) -> FrameData {
        FrameData {
            cells: vec![CellData {
                symbol: symbol.to_owned(),
                fg: 0,
                bg: 0,
                modifier: 0,
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
            actor.publish_frame(1, frame("a"), None, ClientRenderDebugContext::default()),
            ClientRenderPublish::Sent
        );
        assert!(matches!(
            read_server_message(rx.recv().expect("semantic frame")),
            ServerMessage::Frame(_)
        ));
        assert_eq!(
            actor.publish_frame(1, frame("a"), None, ClientRenderDebugContext::default()),
            ClientRenderPublish::SkippedUnchanged
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn terminal_actor_commits_sequence_after_publish() {
        let (tx, rx) = LatestRenderSender::channel();
        let mut actor = ClientRenderActor::new(RenderEncoding::TerminalAnsi, tx);

        assert_eq!(
            actor.publish_frame(1, frame("a"), None, ClientRenderDebugContext::default()),
            ClientRenderPublish::Sent
        );

        match read_server_message(rx.recv().expect("terminal frame")) {
            ServerMessage::Terminal(frame) => assert_eq!(frame.seq, 1),
            other => panic!("expected terminal frame, got {other:?}"),
        }
        assert_eq!(actor.terminal_seq(), Some(1));
    }
}
