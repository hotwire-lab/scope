use serde::Deserialize;

use crate::protocol::types::*;

pub fn compute_xor_checksum(bytes: &[u8]) -> u8 {
    let mut xor = 0u8;
    for &b in bytes {
        xor ^= b;
    }
    xor
}

pub fn build_command_frame(cmd_id: u8, p1: u8, p2: u8, p3: u8, p4: u8) -> [u8; 8] {
    let mut frame = [
        PROTOCOL_SYNC_0,
        PROTOCOL_SYNC_1,
        cmd_id,
        p1,
        p2,
        p3,
        p4,
        0,
    ];
    frame[7] = compute_xor_checksum(&frame[..7]);
    frame
}

#[allow(dead_code)]
pub fn build_ping_status_cmd() -> [u8; 8] {
    build_command_frame(CMD_PING_STATUS, 0, 0, 0, 0)
}

pub fn build_get_caps_cmd() -> [u8; 8] {
    build_command_frame(CMD_GET_CAPS, 0, 0, 0, 0)
}

pub fn build_bootloader_cmd() -> [u8; 8] {
    build_command_frame(CMD_BOOTLOADER, 0xb0, 0x07, 0x10, 0xad)
}

pub fn build_scope_cfg_cmd(
    rate_code: u8,
    channel_mask: u8,
    atten_code: u8,
    samples_code: u8,
) -> [u8; 8] {
    let mask = if channel_mask & 0x0f == 0 {
        0x01
    } else {
        channel_mask & 0x0f
    };
    build_command_frame(
        CMD_SCOPE_CFG,
        rate_code.min(6),
        mask,
        atten_code.min(3),
        samples_code.min(3),
    )
}

pub fn build_scope_trig_cmd(
    trig_mode: u8,
    edge: u8,
    trig_channel: u8,
    level_mv: u16,
    pre_trigger_pct: u8,
) -> [u8; 8] {
    let p1 = (trig_mode & 0x03) | ((edge & 0x01) << 4) | ((trig_channel & 0x03) << 6);
    let level = level_mv.min(3300);
    build_command_frame(
        CMD_SCOPE_TRIG,
        p1,
        (level & 0xff) as u8,
        ((level >> 8) & 0xff) as u8,
        pre_trigger_pct.min(75),
    )
}

pub fn build_scope_run_cmd(
    run_mode: u8,
    target_fps: u8,
    test_gen_code: u8,
    enable_cali: bool,
) -> [u8; 8] {
    let fps = target_fps.clamp(1, 30);
    let flags = if enable_cali { 0x01 } else { 0x00 };
    build_command_frame(
        CMD_SCOPE_RUN,
        run_mode.min(2),
        fps,
        test_gen_code.min(4),
        flags,
    )
}

pub fn encode_response_frame(type_id: u8, payload: &[u8]) -> Vec<u8> {
    let len = payload.len();
    let mut frame = Vec::with_capacity(6 + len);
    frame.push(PROTOCOL_SYNC_0);
    frame.push(PROTOCOL_SYNC_1);
    frame.push(type_id);
    frame.push((len & 0xff) as u8);
    frame.push(((len >> 8) & 0xff) as u8);
    frame.extend_from_slice(payload);
    let chk = compute_xor_checksum(&frame);
    frame.push(chk);
    frame
}

pub fn resolve_rate_preset(rate_code: u8) -> ScopeRatePreset {
    SCOPE_RATE_PRESETS
        .iter()
        .copied()
        .find(|p| p.code == rate_code)
        .unwrap_or(SCOPE_RATE_PRESETS[3])
}

pub fn resolve_atten_preset(atten_code: u8) -> ScopeAttenPreset {
    SCOPE_ATTEN_PRESETS
        .iter()
        .copied()
        .find(|p| p.code == atten_code)
        .unwrap_or(SCOPE_ATTEN_PRESETS[3])
}

pub fn resolve_depth_preset(depth_code: u8) -> ScopeDepthPreset {
    SCOPE_DEPTH_PRESETS
        .iter()
        .copied()
        .find(|p| p.code == depth_code)
        .unwrap_or(SCOPE_DEPTH_PRESETS[1])
}

#[derive(Deserialize)]
struct RawCapsJson {
    board: Option<String>,
    firmware: Option<String>,
    scope: Option<bool>,
    max_channels: Option<u8>,
    max_rate_hz: Option<u32>,
    adc_bits: Option<u8>,
}

pub fn decode_board_capabilities_frame(payload: &[u8]) -> Option<BoardCapabilitiesFrame> {
    let text = std::str::from_utf8(payload).ok()?;
    let parsed: RawCapsJson = serde_json::from_str(text).ok()?;
    Some(BoardCapabilitiesFrame {
        board: parsed.board.unwrap_or_else(|| "XIAO-ESP32-S3".to_string()),
        firmware: parsed.firmware.unwrap_or_else(|| "unknown".to_string()),
        scope_capable: parsed.scope.unwrap_or(true),
        max_channels: parsed.max_channels.unwrap_or(4),
        max_rate_hz: parsed.max_rate_hz.unwrap_or(83_333),
        adc_bits: parsed.adc_bits.unwrap_or(12),
    })
}

pub fn decode_scope_frame(payload: &[u8], timestamp_ms: f64) -> Option<ScopeFrame> {
    if payload.len() < 16 {
        return None;
    }

    let sample_rate_hz = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let channel_mask = payload[4] & 0x0f;
    let atten_code = payload[5] & 0x03;
    let status_flags = payload[6];
    let trig_channel = payload[7] & 0x03;
    let samples_per_ch = u16::from_le_bytes([payload[8], payload[9]]) as usize;
    let trigger_index = u16::from_le_bytes([payload[10], payload[11]]) as usize;
    let trigger_level_mv = u16::from_le_bytes([payload[12], payload[13]]);
    let vref_mv = u16::from_le_bytes([payload[14], payload[15]]).max(500);

    let active_channels = channel_mask.count_ones() as usize;
    if active_channels == 0 || samples_per_ch == 0 {
        return None;
    }

    let expected_bytes = 16 + active_channels * samples_per_ch * 2;
    if payload.len() < expected_bytes {
        return None;
    }

    let calibrated_mv = (status_flags & SCOPE_FLAG_CALIBRATED_MV) != 0;
    let scale = if calibrated_mv {
        1.0f32
    } else {
        (vref_mv as f32) / 4095.0f32
    };

    let mut channels_mv: [Vec<f32>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut offset = 16usize;

    for (ch, slot) in channels_mv.iter_mut().enumerate() {
        if (channel_mask & (1 << ch)) != 0 {
            let mut vec = Vec::with_capacity(samples_per_ch);
            for _ in 0..samples_per_ch {
                let raw = u16::from_le_bytes([payload[offset], payload[offset + 1]]);
                offset += 2;
                vec.push((raw as f32) * scale);
            }
            *slot = vec;
        }
    }

    Some(ScopeFrame {
        sample_rate_hz: sample_rate_hz.max(100),
        channel_mask,
        atten_code,
        status_flags,
        triggered: (status_flags & SCOPE_FLAG_TRIGGERED) != 0,
        overclocked: (status_flags & SCOPE_FLAG_OVERCLOCKED) != 0,
        calibrated_mv,
        adc_saturated: (status_flags & SCOPE_FLAG_ADC_SATURATED) != 0,
        trig_channel,
        samples_per_ch,
        trigger_index: trigger_index.min(samples_per_ch.saturating_sub(1)),
        trigger_level_mv,
        vref_mv,
        channels_mv,
        timestamp_ms,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn encode_scope_frame_payload(
    sample_rate_hz: u32,
    channel_mask: u8,
    atten_code: u8,
    status_flags: u8,
    trig_channel: u8,
    samples_per_ch: u16,
    trigger_index: u16,
    trigger_level_mv: u16,
    vref_mv: u16,
    planar_samples_u16: &[u16],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + planar_samples_u16.len() * 2);
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.push(channel_mask & 0x0f);
    out.push(atten_code & 0x03);
    out.push(status_flags);
    out.push(trig_channel & 0x03);
    out.extend_from_slice(&samples_per_ch.to_le_bytes());
    out.extend_from_slice(&trigger_index.to_le_bytes());
    out.extend_from_slice(&trigger_level_mv.to_le_bytes());
    out.extend_from_slice(&vref_mv.to_le_bytes());
    for &s in planar_samples_u16 {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_checksum_roundtrip() {
        let cmd = build_scope_cfg_cmd(3, 0x03, 3, 1);
        assert_eq!(cmd[0], 0xaa);
        assert_eq!(cmd[1], 0x55);
        assert_eq!(cmd[2], CMD_SCOPE_CFG);
        assert_eq!(cmd[7], compute_xor_checksum(&cmd[..7]));
    }

    #[test]
    fn scope_frame_roundtrip() {
        let samples = vec![100u16, 1500, 3000, 500, 1200, 2400];
        let payload = encode_scope_frame_payload(
            83_333,
            0x03,
            3,
            SCOPE_FLAG_TRIGGERED | SCOPE_FLAG_CALIBRATED_MV,
            0,
            3,
            1,
            1650,
            3100,
            &samples,
        );
        let decoded = decode_scope_frame(&payload, 10.0).expect("valid frame");
        assert_eq!(decoded.sample_rate_hz, 83_333);
        assert_eq!(decoded.samples_per_ch, 3);
        assert!(decoded.triggered);
        assert!(decoded.calibrated_mv);
        assert_eq!(decoded.channels_mv[0], vec![100.0, 1500.0, 3000.0]);
        assert_eq!(decoded.channels_mv[1], vec![500.0, 1200.0, 2400.0]);
        assert!(decoded.channels_mv[2].is_empty());
    }
}
