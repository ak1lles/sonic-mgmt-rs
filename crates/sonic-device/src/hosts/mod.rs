pub mod sonic;
pub mod eos;
pub mod fanout;
pub mod ptf;
pub mod cisco;

use sonic_core::{DeviceInfo, DeviceType};

use self::cisco::CiscoHost;
use self::eos::EosHost;
use self::fanout::FanoutHost;
use self::ptf::PtfHost;
use self::sonic::SonicHost;

/// Factory: create the appropriate host type from a [`DeviceInfo`].
pub fn create_host(info: DeviceInfo) -> Box<dyn sonic_core::Device> {
    match info.device_type {
        DeviceType::Sonic => Box::new(SonicHost::new(info)),
        DeviceType::Eos => Box::new(EosHost::new(info)),
        DeviceType::Fanout => Box::new(FanoutHost::new(info)),
        DeviceType::Ptf => Box::new(PtfHost::new(info)),
        DeviceType::Cisco => Box::new(CiscoHost::new(info)),
        // Fallback: treat unrecognised types as a generic SONiC host.
        _ => Box::new(SonicHost::new(info)),
    }
}
