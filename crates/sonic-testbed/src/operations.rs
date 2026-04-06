//! Testbed operations.
//!
//! These functions mirror the commands provided by the legacy `testbed-cli.sh`
//! script: adding / removing topologies, deploying configurations, upgrading
//! SONiC images, and recovering failed testbeds.

use tracing::{debug, error, info, warn};

use sonic_core::{
    SonicError, TestbedManager, TestbedState, TopologyGenerator, TopologyType,
};

use crate::manager::Testbed;

/// Stateless operations layer.
///
/// Methods accept a mutable reference to a [`Testbed`] and orchestrate
/// high-level workflows.
pub struct TestbedOps;

impl TestbedOps {
    // -- topology lifecycle -------------------------------------------------

    /// Deploys the given topology on the testbed.
    ///
    /// Steps:
    /// 1. Generate the topology definition.
    /// 2. Transition state to `Deploying`.
    /// 3. Provision VMs and set up the PTF container.
    /// 4. Announce routes.
    /// 5. Transition state to `Available`.
    pub async fn add_topology(
        testbed: &mut Testbed,
        topo_type: TopologyType,
    ) -> sonic_core::Result<()> {
        info!(
            testbed = %testbed.name(),
            topo = %topo_type,
            "adding topology"
        );

        if testbed.state() == TestbedState::Deploying {
            return Err(SonicError::testbed("testbed is already deploying"));
        }

        testbed.set_state(TestbedState::Deploying);

        // Generate the topology.
        let generator = sonic_topology::DefaultTopologyGenerator::veos();
        let topo_def = generator.generate(topo_type).map_err(|e| {
            testbed.set_state(TestbedState::Error);
            e
        })?;

        info!(
            vms = topo_def.vms.len(),
            vlans = topo_def.vlans.len(),
            "topology definition generated"
        );

        testbed.set_topology(topo_def);

        // In a full implementation we would:
        //   - create VMs via libvirt / KVM
        //   - start the PTF container
        //   - wait for VM management interfaces to come up
        //   - push base configs

        debug!(testbed = %testbed.name(), "VM provisioning step (simulated)");
        debug!(testbed = %testbed.name(), "PTF container start step (simulated)");

        // Announce routes from neighbor VMs.
        Self::announce_routes(testbed).await.map_err(|e| {
            warn!(
                testbed = %testbed.name(),
                error = %e,
                "route announcement failed during topology add"
            );
            testbed.set_state(TestbedState::Error);
            e
        })?;

        testbed.set_state(TestbedState::Available);
        info!(testbed = %testbed.name(), "topology deployed successfully");
        Ok(())
    }

    /// Removes the current topology from the testbed.
    ///
    /// Steps:
    /// 1. Transition state to `Deploying` (teardown in progress).
    /// 2. Stop VMs.
    /// 3. Tear down PTF container.
    /// 4. Clear topology definition.
    /// 5. Transition state to `Destroyed`.
    pub async fn remove_topology(testbed: &mut Testbed) -> sonic_core::Result<()> {
        info!(testbed = %testbed.name(), "removing topology");

        if testbed.topology().is_none() {
            warn!(testbed = %testbed.name(), "no topology to remove");
            return Ok(());
        }

        testbed.set_state(TestbedState::Deploying);

        // Disconnect devices first (best-effort).
        if let Err(e) = Self::disconnect_devices(testbed).await {
            warn!(error = %e, "disconnect during teardown failed (continuing)");
        }

        debug!(testbed = %testbed.name(), "stopping VMs (simulated)");
        debug!(testbed = %testbed.name(), "removing PTF container (simulated)");

        testbed.clear_topology();
        testbed.set_state(TestbedState::Destroyed);

        info!(testbed = %testbed.name(), "topology removed");
        Ok(())
    }

    // -- config management -------------------------------------------------

    /// Pushes the generated minigraph to all DUTs and triggers a config reload.
    pub async fn deploy_minigraph(testbed: &mut Testbed) -> sonic_core::Result<()> {
        let topo = testbed
            .topology()
            .ok_or_else(|| SonicError::testbed("no topology set"))?;

        info!(testbed = %testbed.name(), "deploying minigraph");

        let renderer = sonic_topology::TopologyRenderer::default();
        let minigraph_xml = renderer.render_minigraph(topo)?;

        debug!(
            bytes = minigraph_xml.len(),
            "minigraph rendered, pushing to DUT(s)"
        );

        // In production:
        //   for dut in testbed.get_all_duts() {
        //       scp minigraph_xml -> /etc/sonic/minigraph.xml
        //       ssh dut "config load_minigraph -y"
        //   }

        info!(testbed = %testbed.name(), "minigraph deployed (simulated)");
        Ok(())
    }

    /// Reloads the DUT configuration from its current minigraph / config_db.
    pub async fn refresh_dut(testbed: &Testbed) -> sonic_core::Result<()> {
        info!(testbed = %testbed.name(), "refreshing DUT config");

        let duts = testbed.get_all_duts();
        if duts.is_empty() {
            return Err(SonicError::testbed("no DUTs in testbed"));
        }

        for dut in &duts {
            debug!(dut = %dut.hostname, "issuing config reload");
            // In production: ssh dut "sudo config reload -y"
        }

        info!(
            testbed = %testbed.name(),
            dut_count = duts.len(),
            "DUT config reloaded (simulated)"
        );
        Ok(())
    }

    // -- device connectivity ------------------------------------------------

    /// Establishes management connections to all devices in the testbed.
    pub async fn connect_devices(testbed: &mut Testbed) -> sonic_core::Result<()> {
        info!(testbed = %testbed.name(), "connecting to devices");

        let all_devices = testbed.all_device_names();
        if all_devices.is_empty() {
            return Err(SonicError::testbed("no devices in testbed"));
        }

        for hostname in &all_devices {
            debug!(hostname = %hostname, "opening connection (simulated)");
            // In production: open SSH session via sonic_device::SshConnection
        }

        info!(
            testbed = %testbed.name(),
            connected = all_devices.len(),
            "all device connections established (simulated)"
        );
        Ok(())
    }

    /// Tears down management connections to all devices.
    pub async fn disconnect_devices(testbed: &mut Testbed) -> sonic_core::Result<()> {
        info!(testbed = %testbed.name(), "disconnecting devices");

        let all_devices = testbed.all_device_names();
        for hostname in &all_devices {
            debug!(hostname = %hostname, "closing connection (simulated)");
        }

        info!(
            testbed = %testbed.name(),
            disconnected = all_devices.len(),
            "all device connections closed (simulated)"
        );
        Ok(())
    }

    // -- upgrade / recovery ------------------------------------------------

    /// Upgrades the SONiC image on every DUT in the testbed.
    ///
    /// Steps:
    /// 1. Download the image to each DUT.
    /// 2. Install the image.
    /// 3. Set it as the next-boot image.
    /// 4. Reboot and wait for the DUT to come back up.
    pub async fn upgrade_sonic(
        testbed: &mut Testbed,
        image_url: &str,
    ) -> sonic_core::Result<()> {
        info!(
            testbed = %testbed.name(),
            image_url,
            "upgrading SONiC image"
        );

        let dut_hostnames: Vec<String> = testbed
            .get_all_duts()
            .iter()
            .map(|d| d.hostname.clone())
            .collect();
        if dut_hostnames.is_empty() {
            return Err(SonicError::testbed("no DUTs to upgrade"));
        }

        testbed.set_state(TestbedState::Maintenance);

        for hostname in &dut_hostnames {
            info!(dut = %hostname, "downloading image (simulated)");
            // ssh dut "curl -o /tmp/sonic.bin $image_url"

            info!(dut = %hostname, "installing image (simulated)");
            // ssh dut "sudo sonic-installer install /tmp/sonic.bin -y"

            info!(dut = %hostname, "rebooting (simulated)");
            // ssh dut "sudo reboot"
            // wait_ready(dut, 300)
        }

        testbed.set_state(TestbedState::Available);
        info!(
            testbed = %testbed.name(),
            duts_upgraded = dut_hostnames.len(),
            "SONiC upgrade complete (simulated)"
        );
        Ok(())
    }

    /// Attempts to recover a testbed in `Error` or `Maintenance` state.
    ///
    /// Recovery strategy:
    /// 1. Health-check all devices.
    /// 2. Reconnect unreachable devices.
    /// 3. If the topology is partially deployed, tear it down and redeploy.
    /// 4. If recovery succeeds, set state to `Available`.
    pub async fn recover_testbed(testbed: &mut Testbed) -> sonic_core::Result<()> {
        info!(
            testbed = %testbed.name(),
            current_state = %testbed.state(),
            "attempting testbed recovery"
        );

        let state = testbed.state();
        if state != TestbedState::Error && state != TestbedState::Maintenance {
            return Err(SonicError::testbed(format!(
                "recovery only valid in Error or Maintenance state, current state is {state}"
            )));
        }

        testbed.set_state(TestbedState::Maintenance);

        // Step 1: health check.
        let checker = crate::health::HealthChecker::new();
        let devices: Vec<sonic_core::DeviceInfo> =
            testbed.get_all_duts().into_iter().cloned().collect();
        let health = checker.check_testbed(&devices).await?;

        info!(
            overall = %health.overall,
            "recovery health check complete"
        );

        // Step 2: for any unreachable devices, attempt reconnection.
        let unreachable: Vec<_> = health
            .devices
            .iter()
            .filter(|d| !d.reachable)
            .collect();

        if !unreachable.is_empty() {
            warn!(
                count = unreachable.len(),
                "unreachable devices found, attempting reconnection"
            );
            // In production: for each unreachable device attempt SSH reconnect,
            // potentially with console fallback.
            for dev in &unreachable {
                debug!(hostname = %dev.hostname, "reconnection attempt (simulated)");
            }
        }

        // Step 3: topology repair.
        if let Some(topo) = testbed.topology() {
            let topo_type = topo.topo_type;
            info!(
                topo = %topo_type,
                "redeploying topology as part of recovery"
            );
            // Tear down whatever partial state remains.
            if let Err(e) = Self::remove_topology(testbed).await {
                warn!(error = %e, "teardown during recovery failed, forcing clean state");
                testbed.clear_topology();
            }
            // Redeploy.
            Self::add_topology(testbed, topo_type).await.map_err(|e| {
                error!(error = %e, "redeployment during recovery failed");
                testbed.set_state(TestbedState::Error);
                e
            })?;
        }

        testbed.set_state(TestbedState::Available);
        info!(testbed = %testbed.name(), "recovery complete");
        Ok(())
    }

    // -- internal helpers ---------------------------------------------------

    /// Triggers BGP route announcement on neighbor VMs.
    async fn announce_routes(testbed: &Testbed) -> sonic_core::Result<()> {
        let neighbors = testbed.get_neighbors();
        if neighbors.is_empty() {
            debug!(testbed = %testbed.name(), "no neighbors to announce routes from");
            return Ok(());
        }

        info!(
            testbed = %testbed.name(),
            neighbor_count = neighbors.len(),
            "announcing routes"
        );

        for nbr in &neighbors {
            debug!(
                neighbor = %nbr.hostname,
                "announcing routes from neighbor (simulated)"
            );
            // In production: ssh nbr "exabgp announce ..."
        }

        Ok(())
    }
}
