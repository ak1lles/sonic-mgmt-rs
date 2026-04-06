use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use russh::{client, ChannelMsg, Disconnect};
use russh_keys::load_secret_key;
use sonic_core::{CommandResult, ConnectionType, Credentials, SonicError, Result};
use tokio::sync::Mutex;
use tracing::{debug, instrument, warn};

// -----------------------------------------------------------------------
// SSH client handler (required by the russh async protocol machine)
// -----------------------------------------------------------------------

/// Minimal handler for the russh client.  We accept all host keys and
/// otherwise defer everything to the library defaults.
#[derive(Clone)]
pub struct SshClientHandler;

#[async_trait]
impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        // In a production environment with a known-hosts database we would
        // verify the key here.  For lab / testbed usage we accept all keys.
        Ok(true)
    }
}

// -----------------------------------------------------------------------
// Connection state kept behind a Mutex so that `&self` methods work.
// -----------------------------------------------------------------------

struct SshInner {
    handle: client::Handle<SshClientHandler>,
}

// -----------------------------------------------------------------------
// Public SSH connection struct
// -----------------------------------------------------------------------

/// An SSH connection backed by the `russh` crate.
pub struct SshConnection {
    host: String,
    port: u16,
    credentials: Credentials,
    connect_timeout: Duration,
    command_timeout: Duration,
    inner: Arc<Mutex<Option<SshInner>>>,
}

impl SshConnection {
    /// Create a new, *disconnected* SSH connection descriptor.
    pub fn new(host: impl Into<String>, port: u16, credentials: Credentials) -> Self {
        Self {
            host: host.into(),
            port,
            credentials,
            connect_timeout: Duration::from_secs(30),
            command_timeout: Duration::from_secs(120),
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Override the TCP + authentication timeout (default 30 s).
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Override the per-command timeout (default 120 s).
    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    // -- internal helpers ------------------------------------------------

    /// Authenticate the session handle, trying key-based auth first.
    #[instrument(skip(self, handle), fields(host = %self.host, user = %self.credentials.username))]
    async fn authenticate(
        &self,
        handle: &mut client::Handle<SshClientHandler>,
    ) -> Result<()> {
        let user = &self.credentials.username;

        // 1) Try key-based auth when a path is configured.
        if let Some(ref key_path) = self.credentials.key_path {
            let passphrase = self.credentials.passphrase.as_deref();
            match load_secret_key(key_path, passphrase) {
                Ok(key_pair) => {
                    let auth_result = handle
                        .authenticate_publickey(user.clone(), Arc::new(key_pair))
                        .await
                        .map_err(|_e| SonicError::Authentication {
                            user: user.clone(),
                            host: self.host.clone(),
                        })?;
                    if auth_result {
                        debug!("public-key auth succeeded for {}@{}", user, self.host);
                        return Ok(());
                    }
                    debug!("public-key auth rejected, falling through to password");
                }
                Err(e) => {
                    warn!("failed to load private key {}: {}", key_path, e);
                }
            }
        }

        // 2) Fall back to password auth.
        if let Some(ref password) = self.credentials.password {
            let auth_result = handle
                .authenticate_password(user.clone(), password.clone())
                .await
                .map_err(|_e| SonicError::Authentication {
                    user: user.clone(),
                    host: self.host.clone(),
                })?;
            if auth_result {
                debug!("password auth succeeded for {}@{}", user, self.host);
                return Ok(());
            }
        }

        Err(SonicError::Authentication {
            user: user.clone(),
            host: self.host.clone(),
        })
    }
}

// -----------------------------------------------------------------------
// `Connection` trait implementation
// -----------------------------------------------------------------------

#[async_trait]
impl sonic_core::Connection for SshConnection {
    #[instrument(skip(self), fields(host = %self.host, port = %self.port))]
    async fn open(&mut self) -> Result<()> {
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(600)),
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            ..Default::default()
        });
        let handler = SshClientHandler;
        let addr = format!("{}:{}", self.host, self.port);

        let mut handle = tokio::time::timeout(self.connect_timeout, async {
            client::connect(config, &addr, handler).await
        })
        .await
        .map_err(|_| SonicError::Timeout {
            seconds: self.connect_timeout.as_secs(),
            operation: format!("SSH connect to {}", addr),
        })?
        .map_err(|e| SonicError::connection(&self.host, e.to_string()))?;

        self.authenticate(&mut handle).await?;

        let mut guard = self.inner.lock().await;
        *guard = Some(SshInner { handle });
        Ok(())
    }

    #[instrument(skip(self), fields(host = %self.host))]
    async fn close(&mut self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if let Some(inner) = guard.take() {
            inner
                .handle
                .disconnect(Disconnect::ByApplication, "", "English")
                .await
                .ok();
        }
        Ok(())
    }

    async fn send(&self, data: &str) -> Result<String> {
        let guard = self.inner.lock().await;
        let inner = guard.as_ref().ok_or_else(|| {
            SonicError::connection(&self.host, "not connected")
        })?;
        let mut channel = inner.handle.channel_open_session().await.map_err(|e| {
            SonicError::connection(&self.host, e.to_string())
        })?;
        channel.data(data.as_bytes()).await.map_err(|e| {
            SonicError::connection(&self.host, e.to_string())
        })?;
        channel.eof().await.map_err(|e| {
            SonicError::connection(&self.host, e.to_string())
        })?;

        let mut stdout = Vec::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => {
                    stdout.extend_from_slice(data);
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    #[instrument(skip(self), fields(host = %self.host, command = %command))]
    async fn send_command(&self, command: &str) -> Result<CommandResult> {
        let start = Instant::now();
        let guard = self.inner.lock().await;
        let inner = guard.as_ref().ok_or_else(|| {
            SonicError::connection(&self.host, "not connected")
        })?;

        let mut channel = inner.handle.channel_open_session().await.map_err(|e| {
            SonicError::connection(&self.host, e.to_string())
        })?;

        channel.exec(true, command).await.map_err(|e| {
            SonicError::connection(&self.host, e.to_string())
        })?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: i32 = -1;

        let read_result = tokio::time::timeout(self.command_timeout, async {
            while let Some(msg) = channel.wait().await {
                match msg {
                    ChannelMsg::Data { ref data } => {
                        stdout.extend_from_slice(data);
                    }
                    ChannelMsg::ExtendedData { ref data, ext } => {
                        if ext == 1 {
                            stderr.extend_from_slice(data);
                        }
                    }
                    ChannelMsg::ExitStatus { exit_status } => {
                        exit_code = exit_status as i32;
                    }
                    // Eof means no more data, but ExitStatus may still follow.
                    // Only break on Close (channel fully done).
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
        })
        .await;

        if read_result.is_err() {
            return Err(SonicError::Timeout {
                seconds: self.command_timeout.as_secs(),
                operation: format!("command `{}`", command),
            });
        }

        let duration = start.elapsed();
        Ok(CommandResult {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_code,
            duration,
            command: command.to_string(),
        })
    }

    async fn is_alive(&self) -> bool {
        let guard = self.inner.lock().await;
        if let Some(ref inner) = *guard {
            // Try opening a channel as a liveness probe.  The handle itself
            // does not expose a simple "is open" predicate, so a lightweight
            // channel open is the most reliable check.
            inner
                .handle
                .channel_open_session()
                .await
                .is_ok()
        } else {
            false
        }
    }

    fn connection_type(&self) -> ConnectionType {
        ConnectionType::Ssh
    }
}
