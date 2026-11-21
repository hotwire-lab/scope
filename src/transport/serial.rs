pub use hotwire_core::transport::{
    HotWireTransport as SerialTransport, SerialConfig, TransportSnapshot,
};

use crate::transport::demo::DemoSimulator;

pub fn create_scope_transport() -> SerialTransport {
    SerialTransport::with_demo(
        SerialConfig::hotwire_cdc("hotwire_scope_last_device"),
        DemoSimulator::new(),
    )
}
