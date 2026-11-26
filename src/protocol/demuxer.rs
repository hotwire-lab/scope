use crate::protocol::codec::decode_scope_frame;
use crate::protocol::types::ScopeFrame;

/// Decodes a `TYPE_SCOPE_FRAME` payload with the current UI timestamp.
///
/// Capabilities frames are handled by `hotwire-core`'s shared session.
pub fn decode_scope_payload(payload: &[u8], timestamp_ms: f64) -> Option<ScopeFrame> {
    decode_scope_frame(payload, timestamp_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::codec::{encode_response_frame, encode_scope_frame_payload};
    use crate::protocol::types::{SCOPE_FLAG_CALIBRATED_MV, TYPE_SCOPE_FRAME};

    #[test]
    fn decodes_scope_frame_payload() {
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
        let frame = decode_scope_payload(&payload, 1.0).expect("scope frame");
        assert_eq!(frame.sample_rate_hz, 50_000);
        assert_eq!(frame.channel_mask, 0x01);
        assert_eq!(frame.samples_per_ch, 4);
        assert_eq!(frame.channels_mv[0].len(), 4);
        let wire = encode_response_frame(TYPE_SCOPE_FRAME, &payload);
        assert_eq!(wire[2], TYPE_SCOPE_FRAME);
    }
}
