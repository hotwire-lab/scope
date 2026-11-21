use hotwire_core::protocol::{FrameDemuxer, FrameRule};

use crate::protocol::codec::{decode_board_capabilities_frame, decode_scope_frame};
use crate::protocol::types::{
    DecodedFrame, MAX_FRAME_PAYLOAD_LEN, TYPE_BOARD_CAPABILITIES, TYPE_SCOPE_FRAME,
};

pub struct ProtocolDemuxer {
    inner: FrameDemuxer,
}

impl Default for ProtocolDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolDemuxer {
    pub fn new() -> Self {
        Self {
            inner: FrameDemuxer::new(vec![
                FrameRule::board_capabilities(),
                FrameRule::new(TYPE_SCOPE_FRAME, 16, MAX_FRAME_PAYLOAD_LEN),
            ]),
        }
    }

    pub fn reset(&mut self) {
        self.inner.reset();
    }

    pub fn feed<F>(&mut self, bytes: &[u8], timestamp_ms: f64, mut on_frame: F)
    where
        F: FnMut(DecodedFrame),
    {
        self.inner.feed(bytes, |raw| match raw.type_id {
            TYPE_BOARD_CAPABILITIES => {
                if let Some((identity, caps)) = decode_board_capabilities_frame(raw.payload) {
                    on_frame(DecodedFrame::Capabilities(identity, caps));
                }
            }
            TYPE_SCOPE_FRAME => {
                if let Some(frame) = decode_scope_frame(raw.payload, timestamp_ms) {
                    on_frame(DecodedFrame::Scope(frame));
                }
            }
            _ => {}
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::codec::{encode_response_frame, encode_scope_frame_payload};
    use crate::protocol::types::SCOPE_FLAG_CALIBRATED_MV;

    #[test]
    fn demuxer_decodes_chunked_frames() {
        let payload = encode_scope_frame_payload(
            50_000,
            0x01,
            3,
            SCOPE_FLAG_CALIBRATED_MV,
            0,
            4,
            2,
            1500,
            3100,
            &[100, 1500, 2900, 1500],
        );
        let wire = encode_response_frame(TYPE_SCOPE_FRAME, &payload);
        let mut demux = ProtocolDemuxer::new();
        let mut frames = Vec::new();
        // Feed noisy prefix + split chunks
        demux.feed(&[0x00, 0xaa, 0x00], 1.0, |f| frames.push(f));
        demux.feed(&wire[..7], 1.0, |f| frames.push(f));
        demux.feed(&wire[7..], 1.0, |f| frames.push(f));
        assert_eq!(frames.len(), 1);
    }
}
