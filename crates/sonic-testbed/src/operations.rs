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

        let dut_hostnames: Vec<String> = testbed
            .get_all_duts()
            .iter()
            .map(|d| d.hostname.clone())
            .collect();

        for hostname in &dut_hostnames {
            if !testbed.device_manager().is_connected(hostname) {
                warn!(
                    dut = %hostname,
                    "DUT not connected, skipping minigraph push"
                );
                continue;
            }

            // Write the minigraph XML to /etc/sonic/minigraph.xml via heredoc.
            let write_cmd = format!(
                "cat > /etc/sonic/minigraph.xml << 'XMLEOF'\n{}\nXMLEOF",
                minigraph_xml
            );
            info!(dut = %hostname, "writing minigraph to device");
            if let Err(e) = testbed.device_manager().execute_on(hostname, &write_cmd).await {
                error!(dut = %hostname, error = %e, "failed to write minigraph");
                return Err(e);
            }

            // Apply the minigraph configuration.
            info!(dut = %hostname, "loading minigraph configuration");
            if let Err(e) = testbed
                .device_manager()
                .execute_on(hostname, "sudo config load_minigraph -y")
                .await
            {
                error!(dut = %hostname, error = %e, "config load_minigraph failed");
                return Err(e);
            }

            // Allow time for config to apply.
            debug!(dut = %hostname, "waiting for config to apply");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }

        info!(testbed = %testbed.name(), "minigraph deployed");
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
            if !testbed.device_manager().is_connected(&dut.hostname) {
                warn!(dut = %dut.hostname, "DUT not connected, skipping config reload");
                continue;
            }
            info!(dut = %dut.hostname, "issuing config reload");
            testbed
                .device_manager()
                .execute_on(&dut.hostname, "sudo config reload -y")
                .await?;
        }

        info!(
            testbed = %testbed.name(),
            dut_count = duts.len(),
            "DUT config reloaded"
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

        testbed.connect_all().await?;

        info!(
            testbed = %testbed.name(),
            connected = testbed.device_manager().connected_hosts().len(),
            "device connections established"
        );
        Ok(())
    }

    /// Tears down management connections to all devices.
    pub async fn disconnect_devices(testbed: &mut Testbed) -> sonic_core::Result<()> {
        info!(testbed = %testbed.name(), "disconnecting devices");
        testbed.disconnect_all().await?;
        info!(testbed = %testbed.name(), "all device connections closed");
        Ok(())
    }

    // -- upgrade / recovery ------------------------------------------------

    /// Upgrades the SONiC image on every DUT in the testbed.
    ///
    /// Steps:
    /// 1. Download the image to each DUT.
    /// 2. Install the image.
    /// 3. Reboot and wait for the DUT to come back up.
    /// 4. Reconnect.
    pub async fn upgrade_sonic(
        testbed: &mut Testbed,
        image_url: &str,
    ) -> sonic_core::Result<()> {
        info!(
            testbed = %testbed.name(),
            image_url,
            "upgrading SONiC image"
        );

        let dut_infos: Vec<sonic_core::DeviceInfo> = testbed
            .get_all_duts()
            .iter()
            .map(|d| (*d).clone())
            .collect();
        if dut_infos.is_empty() {
            return Err(SonicError::testbed("no DUTs to upgrade"));
        }

        testbed.set_state(TestbedState::Maintenance);

        for info in &dut_infos {
            let hostname = &info.hostname;

            if !testbed.device_manager().is_connected(hostname) {
                warn!(dut = %hostname, "DUT not connected, skipping upgrade");
                continue;
            }

            // Download the image.
            let download_cmd = format!("curl -o /tmp/sonic.bin {image_url}");
            info!(dut = %hostname, "downloading image");
            testbed
                .device_manager()
                .execute_on(hostname, &download_cmd)
                .await?;

            // Install the image.
            info!(dut = %hostname, "installing image");
            testbed
                .device_manager()
                .execute_on(hostname, "sudo sonic-installer install /tmp/sonic.bin -y")
                .await?;

            // Reboot. The execute call may return an error because the
            // connection drops during reboot -- that is expected.
            info!(dut = %hostname, "rebooting");
            let _ = testbed
                .device_manager()
                .execute_on(hostname, "sudo reboot")
                .await;

            // Disconnect the now-dead session.
            testbed
                .device_manager_mut()
                .disconnect_device(hostname)
                .await?;

            // Wait for the DUT to come back online via TCP probe loop.
            info!(dut = %hostname, "waiting for DUT to come back up");
            let checker = crate::health::HealthChecker::new()
                .with_timeout(std::time::Duration::from_secs(10))
                .with_retries(30)
                .with_retry_delay(std::time::Duration::from_secs(10));
            let health = checker.check_device(info).await;
            if !health.reachable {
                error!(
                    dut = %hostname,
                    "DUT did not come back after reboot"
                );
                testbed.set_state(TestbedState::Error);
                return Err(SonicError::testbed(format!(
                    "DUT {hostname} did not come back after reboot"
                )));
            }

            // Reconnect.
            info!(dut = %hostname, "reconnecting after reboot");
            testbed
                .device_manager_mut()
                .connect_device(info)
                .await?;
        }

        testbed.set_state(TestbedState::Available);
        info!(
            testbed = %testbed.name(),
            duts_upgraded = dut_infos.len(),
            "SONiC upgrade complete"
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
            for dev_health in &unreachable {
                // Look up the DeviceInfo for this hostname.
                if let Some(info) = devices.iter().find(|d| d.hostname == dev_health.hostname) {
                    debug!(hostname = %dev_health.hostname, "attempting reconnection");
                    match testbed.device_manager_mut().connect_device(info).await {
                        Ok(()) => {
                            info!(hostname = %dev_health.hostname, "reconnected successfully");
                        }
                        Err(e) => {
                            warn!(
                                hostname = %dev_health.hostname,
                                error = %e,
                                "reconnection failed"
                            );
                        }
                    }
                }
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

    /// Triggers BGP route announcement on neighbor VMs by restarting ExaBGP.
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
            if !testbed.device_manager().is_connected(&nbr.hostname) {
                debug!(
                    neighbor = %nbr.hostname,
                    "neighbor not connected, skipping route announcement"
                );
                continue;
            }
            debug!(neighbor = %nbr.hostname, "restarting ExaBGP for route announcement");
            if let Err(e) = testbed
                .device_manager()
                .execute_on(&nbr.hostname, "supervisorctl restart exabgp")
                .await
            {
                warn!(
                    neighbor = %nbr.hostname,
                    error = %e,
                    "ExaBGP restart failed, continuing with remaining neighbors"
                );
            }
        }

        Ok(())
    }
}
