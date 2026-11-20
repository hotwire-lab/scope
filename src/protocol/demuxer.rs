use crate::protocol::codec::{
    compute_xor_checksum, decode_board_capabilities_frame, decode_scope_frame,
};
use crate::protocol::types::{
    DecodedFrame, MAX_FRAME_PAYLOAD_LEN, PROTOCOL_SYNC_0, PROTOCOL_SYNC_1,
    TYPE_BOARD_CAPABILITIES, TYPE_SCOPE_FRAME,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    Sync0,
    Sync1,
    Type,
    LenLo,
    LenHi,
    Payload,
    Checksum,
}

pub struct ProtocolDemuxer {
    state: ParserState,
    frame_type: u8,
    payload_len: usize,
    payload: Vec<u8>,
}

impl Default for ProtocolDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolDemuxer {
    pub fn new() -> Self {
        Self {
            state: ParserState::Sync0,
            frame_type: 0,
            payload_len: 0,
            payload: Vec::with_capacity(4096),
        }
    }

    pub fn reset(&mut self) {
        self.state = ParserState::Sync0;
        self.frame_type = 0;
        self.payload_len = 0;
        self.payload.clear();
    }

    pub fn feed<F>(&mut self, bytes: &[u8], timestamp_ms: f64, mut on_frame: F)
    where
        F: FnMut(DecodedFrame),
    {
        for &b in bytes {
            match self.state {
                ParserState::Sync0 => {
                    if b == PROTOCOL_SYNC_0 {
                        self.state = ParserState::Sync1;
                    }
                }
                ParserState::Sync1 => {
                    if b == PROTOCOL_SYNC_1 {
                        self.state = ParserState::Type;
                    } else if b == PROTOCOL_SYNC_0 {
                        self.state = ParserState::Sync1;
                    } else {
                        self.state = ParserState::Sync0;
                    }
                }
                ParserState::Type => {
                    if b == TYPE_BOARD_CAPABILITIES || b == TYPE_SCOPE_FRAME {
                        self.frame_type = b;
                        self.state = ParserState::LenLo;
                    } else {
                        self.state = ParserState::Sync0;
                    }
                }
                ParserState::LenLo => {
                    self.payload_len = b as usize;
                    self.state = ParserState::LenHi;
                }
                ParserState::LenHi => {
                    self.payload_len |= (b as usize) << 8;
                    if self.payload_len > MAX_FRAME_PAYLOAD_LEN {
                        self.reset();
                    } else if self.payload_len == 0 {
                        self.payload.clear();
                        self.state = ParserState::Checksum;
                    } else {
                        self.payload.clear();
                        self.payload.reserve(self.payload_len);
                        self.state = ParserState::Payload;
                    }
                }
                ParserState::Payload => {
                    self.payload.push(b);
                    if self.payload.len() >= self.payload_len {
                        self.state = ParserState::Checksum;
                    }
                }
                ParserState::Checksum => {
                    let header = [
                        PROTOCOL_SYNC_0,
                        PROTOCOL_SYNC_1,
                        self.frame_type,
                        (self.payload_len & 0xff) as u8,
                        ((self.payload_len >> 8) & 0xff) as u8,
                    ];
                    let expected = compute_xor_checksum(&header) ^ compute_xor_checksum(&self.payload);
                    if b == expected {
                        match self.frame_type {
                            TYPE_BOARD_CAPABILITIES => {
                                if let Some(caps) = decode_board_capabilities_frame(&self.payload) {
                                    on_frame(DecodedFrame::Capabilities(caps));
                                }
                            }
                            TYPE_SCOPE_FRAME => {
                                if let Some(frame) = decode_scope_frame(&self.payload, timestamp_ms) {
                                    on_frame(DecodedFrame::Scope(frame));
                                }
                            }
                            _ => {}
                        }
                    }
                    self.reset();
                }
            }
        }
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
