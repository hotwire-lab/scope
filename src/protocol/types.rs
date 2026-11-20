pub const PROTOCOL_SYNC_0: u8 = 0xaa;
pub const PROTOCOL_SYNC_1: u8 = 0x55;

pub const CMD_PING_STATUS: u8 = 0x01;
pub const CMD_GET_CAPS: u8 = 0x02;
pub const CMD_SCOPE_CFG: u8 = 0x40;
pub const CMD_SCOPE_TRIG: u8 = 0x41;
pub const CMD_SCOPE_RUN: u8 = 0x42;
pub const CMD_BOOTLOADER: u8 = 0xf0;

pub const TYPE_BOARD_CAPABILITIES: u8 = 0x83;
pub const TYPE_SCOPE_FRAME: u8 = 0x86;

/// Header (16 bytes) + up to 4 channels * 4096 samples * 2 bytes = 32784 bytes.
pub const MAX_FRAME_PAYLOAD_LEN: usize = 33024;

pub const SCOPE_RUN_STOP: u8 = 0;
pub const SCOPE_RUN_CONTINUOUS: u8 = 1;
pub const SCOPE_RUN_SINGLE: u8 = 2;

pub const SCOPE_TRIG_AUTO: u8 = 0;
#[allow(dead_code)]
pub const SCOPE_TRIG_NORMAL: u8 = 1;
#[allow(dead_code)]
pub const SCOPE_TRIG_SINGLE: u8 = 2;
pub const SCOPE_TRIG_FREE_RUN: u8 = 3;

pub const SCOPE_EDGE_RISING: u8 = 0;
pub const SCOPE_EDGE_FALLING: u8 = 1;

pub const SCOPE_FLAG_TRIGGERED: u8 = 0x01;
pub const SCOPE_FLAG_OVERCLOCKED: u8 = 0x02;
pub const SCOPE_FLAG_CALIBRATED_MV: u8 = 0x04;
pub const SCOPE_FLAG_ADC_SATURATED: u8 = 0x08;
pub const SCOPE_FLAG_PACKED_12BIT: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeRatePreset {
    pub code: u8,
    pub label: &'static str,
    pub total_rate_hz: u32,
    pub overclocked: bool,
    pub s3_only: bool,
}

pub const SCOPE_RATE_PRESETS: [ScopeRatePreset; 7] = [
    ScopeRatePreset {
        code: 0,
        label: "10 kSPS",
        total_rate_hz: 10_000,
        overclocked: false,
        s3_only: false,
    },
    ScopeRatePreset {
        code: 1,
        label: "20 kSPS",
        total_rate_hz: 20_000,
        overclocked: false,
        s3_only: false,
    },
    ScopeRatePreset {
        code: 2,
        label: "50 kSPS",
        total_rate_hz: 50_000,
        overclocked: false,
        s3_only: false,
    },
    ScopeRatePreset {
        code: 3,
        label: "83.3 kSPS (Spec)",
        total_rate_hz: 83_333,
        overclocked: false,
        s3_only: false,
    },
    ScopeRatePreset {
        code: 4,
        label: "200 kSPS (S3 Turbo)",
        total_rate_hz: 200_000,
        overclocked: true,
        s3_only: true,
    },
    ScopeRatePreset {
        code: 5,
        label: "500 kSPS (S3 OC)",
        total_rate_hz: 500_000,
        overclocked: true,
        s3_only: true,
    },
    ScopeRatePreset {
        code: 6,
        label: "1 MSPS (S3 OC Max)",
        total_rate_hz: 1_000_000,
        overclocked: true,
        s3_only: true,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeAttenPreset {
    pub code: u8,
    pub label: &'static str,
    pub full_scale_mv: u16,
}

pub const SCOPE_ATTEN_PRESETS: [ScopeAttenPreset; 4] = [
    ScopeAttenPreset {
        code: 0,
        label: "0 dB (0–0.95 V)",
        full_scale_mv: 950,
    },
    ScopeAttenPreset {
        code: 1,
        label: "2.5 dB (0–1.25 V)",
        full_scale_mv: 1250,
    },
    ScopeAttenPreset {
        code: 2,
        label: "6 dB (0–1.75 V)",
        full_scale_mv: 1750,
    },
    ScopeAttenPreset {
        code: 3,
        label: "12 dB (0–3.1 V)",
        full_scale_mv: 3100,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeDepthPreset {
    pub code: u8,
    pub samples: usize,
    pub label: &'static str,
}

pub const SCOPE_DEPTH_PRESETS: [ScopeDepthPreset; 4] = [
    ScopeDepthPreset {
        code: 0,
        samples: 512,
        label: "512",
    },
    ScopeDepthPreset {
        code: 1,
        samples: 1024,
        label: "1024",
    },
    ScopeDepthPreset {
        code: 2,
        samples: 2048,
        label: "2048",
    },
    ScopeDepthPreset {
        code: 3,
        samples: 4096,
        label: "4096",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestGenPreset {
    pub code: u8,
    pub label: &'static str,
    pub freq_hz: u32,
}

pub const TEST_GEN_PRESETS: [TestGenPreset; 5] = [
    TestGenPreset {
        code: 0,
        label: "Off",
        freq_hz: 0,
    },
    TestGenPreset {
        code: 1,
        label: "1 kHz",
        freq_hz: 1_000,
    },
    TestGenPreset {
        code: 2,
        label: "5 kHz",
        freq_hz: 5_000,
    },
    TestGenPreset {
        code: 3,
        label: "10 kHz",
        freq_hz: 10_000,
    },
    TestGenPreset {
        code: 4,
        label: "50 kHz",
        freq_hz: 50_000,
    },
];

pub const CHANNEL_COLORS: [&str; 4] = ["#facc15", "#38bdf8", "#f472b6", "#4ade80"];
pub const CHANNEL_NAMES: [&str; 4] = ["CH1 (D0)", "CH2 (D1)", "CH3 (D2)", "CH4 (D3)"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceMode {
    Off,
    Decay500ms,
    Decay2s,
    Decay5s,
    Infinite,
}

impl PersistenceMode {
    pub fn as_code(self) -> &'static str {
        match self {
            PersistenceMode::Off => "off",
            PersistenceMode::Decay500ms => "500ms",
            PersistenceMode::Decay2s => "2s",
            PersistenceMode::Decay5s => "5s",
            PersistenceMode::Infinite => "inf",
        }
    }

    pub fn from_code(s: &str) -> Self {
        match s {
            "500ms" => PersistenceMode::Decay500ms,
            "2s" => PersistenceMode::Decay2s,
            "5s" => PersistenceMode::Decay5s,
            "inf" => PersistenceMode::Infinite,
            _ => PersistenceMode::Off,
        }
    }

    pub fn decay_ms(self) -> Option<f64> {
        match self {
            PersistenceMode::Decay500ms => Some(500.0),
            PersistenceMode::Decay2s => Some(2000.0),
            PersistenceMode::Decay5s => Some(5000.0),
            _ => None,
        }
    }

    pub fn badge(self) -> Option<&'static str> {
        match self {
            PersistenceMode::Off => None,
            PersistenceMode::Decay500ms => Some("PERSIST 0.5s"),
            PersistenceMode::Decay2s => Some("PERSIST 2.0s"),
            PersistenceMode::Decay5s => Some("PERSIST 5.0s"),
            PersistenceMode::Infinite => Some("PERSIST ∞"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardCapabilitiesFrame {
    pub board: String,
    pub firmware: String,
    pub scope_capable: bool,
    pub max_channels: u8,
    pub max_rate_hz: u32,
    pub adc_bits: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScopeFrame {
    pub sample_rate_hz: u32,
    pub channel_mask: u8,
    pub atten_code: u8,
    pub status_flags: u8,
    pub triggered: bool,
    pub overclocked: bool,
    pub calibrated_mv: bool,
    pub adc_saturated: bool,
    pub trig_channel: u8,
    pub samples_per_ch: usize,
    pub trigger_index: usize,
    pub trigger_level_mv: u16,
    pub vref_mv: u16,
    /// Planar channel data: `channels[ch_idx]` holds millivolt samples (`f32`) when active, or empty.
    pub channels_mv: [Vec<f32>; 4],
    pub timestamp_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedFrame {
    Capabilities(BoardCapabilitiesFrame),
    Scope(ScopeFrame),
}
