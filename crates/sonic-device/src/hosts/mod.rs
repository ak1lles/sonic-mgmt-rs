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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use sonic_core::Credentials;

    fn make_info(device_type: DeviceType) -> DeviceInfo {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        DeviceInfo::new("test-host", ip, device_type, Credentials::new("admin"))
    }

    #[test]
    fn create_host_sonic() {
        let host = create_host(make_info(DeviceType::Sonic));
        assert_eq!(host.info().device_type, DeviceType::Sonic);
        assert_eq!(host.info().hostname, "test-host");
    }

    #[test]
    fn create_host_eos() {
        let host = create_host(make_info(DeviceType::Eos));
        assert_eq!(host.info().device_type, DeviceType::Eos);
    }

    #[test]
    fn create_host_fanout() {
        let host = create_host(make_info(DeviceType::Fanout));
        assert_eq!(host.info().device_type, DeviceType::Fanout);
    }

    #[test]
    fn create_host_ptf() {
        let host = create_host(make_info(DeviceType::Ptf));
        assert_eq!(host.info().device_type, DeviceType::Ptf);
    }

    #[test]
    fn create_host_cisco() {
        let host = create_host(make_info(DeviceType::Cisco));
        assert_eq!(host.info().device_type, DeviceType::Cisco);
    }

    #[test]
    fn create_host_unknown_falls_back_to_sonic() {
        // Types without explicit arms fall back to SonicHost
        let host = create_host(make_info(DeviceType::Cumulus));
        assert_eq!(host.info().device_type, DeviceType::Cumulus);
    }
}
