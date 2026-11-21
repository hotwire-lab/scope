use hotwire_core::protocol::build_get_caps_cmd;
pub use hotwire_core::transport::{
    HotWireTransport as SerialTransport, SerialConfig, TransportSnapshot,
};

use crate::protocol::codec::build_scope_run_cmd;
use crate::protocol::types::SCOPE_RUN_STOP;
use crate::transport::demo::DemoSimulator;

pub fn create_scope_transport() -> SerialTransport {
    let transport = SerialTransport::with_demo(
        SerialConfig::hotwire_cdc("hotwire_scope_last_device"),
        DemoSimulator::new(),
    );
    // Stop any previously active continuous stream before requesting 0x83 capabilities
    // so Android WebUSB reconnects never stall behind an ongoing 20 FPS waveform flood.
    let mut init_cmds = Vec::with_capacity(16);
    init_cmds.extend_from_slice(&build_scope_run_cmd(SCOPE_RUN_STOP, 20, 0, true));
    init_cmds.extend_from_slice(&build_get_caps_cmd());
    transport.set_init_commands(init_cmds);
    transport
}
