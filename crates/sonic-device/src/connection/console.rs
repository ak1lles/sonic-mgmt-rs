use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use sonic_core::{CommandResult, ConnectionType, ConsoleInfo, Credentials, SonicError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, instrument};

// -----------------------------------------------------------------------
// Conserver protocol helpers
// -----------------------------------------------------------------------

/// Conserver escape: `Ctrl-E c` sequences are used to interact with the
/// console-server daemon (conserver).  The escape character is 0x05 by
/// default.
const CONSERVER_ESCAPE: u8 = 0x05;

/// "force attach" after connecting to conserver.
const CONSERVER_FORCE: &[u8] = &[CONSERVER_ESCAPE, b'c', b'f'];

/// "send break" to the remote device.
const CONSERVER_BREAK: &[u8] = &[CONSERVER_ESCAPE, b'c', b'l', b'0'];

// -----------------------------------------------------------------------
// Internal state
// -----------------------------------------------------------------------

struct ConsoleInner {
    stream: TcpStream,
}

// -----------------------------------------------------------------------
// Public struct
// -----------------------------------------------------------------------

/// Console connection via a *conserver* TCP socket.
///
/// The typical flow:
/// 1. TCP connect to the console server (e.g., `conserver-host:7782`).
/// 2. Authenticate with conserver (plain text `login` / `passwd` exchange).
/// 3. Request the target device's console line.
/// 4. Interact as if attached to a serial port.
pub struct ConsoleConnection {
    console_info: ConsoleInfo,
    device_name: String,
    credentials: Credentials,
    prompt: Regex,
    connect_timeout: Duration,
    command_timeout: Duration,
    inner: Arc<Mutex<Option<ConsoleInner>>>,
}

impl ConsoleConnection {
    pub fn new(
        console_info: ConsoleInfo,
        device_name: impl Into<String>,
        credentials: Credentials,
    ) -> Self {
        Self {
            console_info,
            device_name: device_name.into(),
            credentials,
            prompt: Regex::new(r"[#$>]\s*$").expect("built-in regex"),
            connect_timeout: Duration::from_secs(60),
            command_timeout: Duration::from_secs(120),
            inner: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_prompt(mut self, pattern: &str) -> Result<Self> {
        self.prompt = Regex::new(pattern)?;
        Ok(self)
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    /// Send a break signal through the console server.
    pub async fn send_break(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let inner = guard.as_mut().ok_or_else(|| {
            SonicError::connection(&self.device_name, "console not connected")
        })?;
        inner.stream.write_all(CONSERVER_BREAK).await.map_err(SonicError::Io)?;
        Ok(())
    }

    /// Send the conserver force-attach escape.
    pub async fn force_attach(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let inner = guard.as_mut().ok_or_else(|| {
            SonicError::connection(&self.device_name, "console not connected")
        })?;
        inner.stream.write_all(CONSERVER_FORCE).await.map_err(SonicError::Io)?;
        Ok(())
    }

    // -- internal --------------------------------------------------------

    /// Read from the stream until `predicate` matches the accumulated text.
    async fn read_until(
        stream: &mut TcpStream,
        timeout: Duration,
        predicate: impl Fn(&str) -> bool,
    ) -> Result<String> {
        let mut buf = Vec::with_capacity(4096);
        let mut tmp = [0u8; 1024];
        let deadline = Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SonicError::Timeout {
                    seconds: timeout.as_secs(),
                    operation: "console read".into(),
                });
            }

            let n = match tokio::time::timeout(remaining, stream.read(&mut tmp)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(SonicError::Io(e)),
                Err(_) => {
                    return Err(SonicError::Timeout {
                        seconds: timeout.as_secs(),
                        operation: "console read".into(),
                    });
                }
            };

            buf.extend_from_slice(&tmp[..n]);
            let text = String::from_utf8_lossy(&buf);
            if predicate(&text) {
                return Ok(text.into_owned());
            }
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Negotiate with conserver: send credentials, request the device line,
    /// then force-attach if required.
    #[instrument(skip(self, stream), fields(device = %self.device_name))]
    async fn negotiate_conserver(&self, stream: &mut TcpStream) -> Result<()> {
        let timeout = self.connect_timeout;

        // Wait for conserver greeting.
        let greeting = Self::read_until(stream, timeout, |s| {
            let lower = s.to_lowercase();
            lower.contains("ok") || lower.contains("login") || lower.contains("username")
        })
        .await?;
        debug!("conserver greeting: {}", greeting.trim());

        // If the server asks for authentication, provide it.
        let lower = greeting.to_lowercase();
        if lower.contains("login") || lower.contains("username") {
            let login = format!("{}\n", self.credentials.username);
            stream.write_all(login.as_bytes()).await.map_err(SonicError::Io)?;

            let _pw_prompt = Self::read_until(stream, timeout, |s| {
                s.to_lowercase().contains("password")
            })
            .await?;

            let password = self.credentials.password.as_deref().unwrap_or("");
            let pw_line = format!("{}\n", password);
            stream.write_all(pw_line.as_bytes()).await.map_err(SonicError::Io)?;

            let _ok = Self::read_until(stream, timeout, |s| {
                let l = s.to_lowercase();
                l.contains("ok") || l.contains("[") || l.contains("connected")
            })
            .await?;
        }

        // Request the console line for our device.
        let console_cmd = format!("console {}\n", self.device_name);
        stream.write_all(console_cmd.as_bytes()).await.map_err(SonicError::Io)?;

        let response = Self::read_until(stream, timeout, |s| {
            let l = s.to_lowercase();
            l.contains("attached")
                || l.contains("spy")
                || l.contains("ok")
                || l.contains("refused")
                || l.contains("error")
        })
        .await?;

        let rl = response.to_lowercase();
        if rl.contains("refused") || rl.contains("error") {
            return Err(SonicError::connection(
                &self.device_name,
                format!("conserver refused: {}", response.trim()),
            ));
        }

        // If we are in spy mode, force-attach.
        if rl.contains("spy") {
            stream.write_all(CONSERVER_FORCE).await.map_err(SonicError::Io)?;
            let _ = Self::read_until(stream, timeout, |s| {
                s.to_lowercase().contains("attached")
            })
            .await?;
        }

        // Send a carriage return to trigger a fresh prompt from the device.
        stream.write_all(b"\r\n").await.map_err(SonicError::Io)?;

        let prompt = self.prompt.clone();
        let _ = Self::read_until(stream, timeout, move |s| prompt.is_match(s)).await;

        debug!("console attached to {}", self.device_name);
        Ok(())
    }
}

// -----------------------------------------------------------------------
// `Connection` trait
// -----------------------------------------------------------------------

#[async_trait]
impl sonic_core::Connection for ConsoleConnection {
    #[instrument(skip(self), fields(device = %self.device_name, server = %self.console_info.server))]
    async fn open(&mut self) -> Result<()> {
        let addr = format!("{}:{}", self.console_info.server, self.console_info.port);

        let mut stream = tokio::time::timeout(
            self.connect_timeout,
            TcpStream::connect(&addr),
        )
        .await
        .map_err(|_| SonicError::Timeout {
            seconds: self.connect_timeout.as_secs(),
            operation: format!("console connect to {}", addr),
        })?
        .map_err(|e| SonicError::connection(&self.device_name, e.to_string()))?;

        stream.set_nodelay(true).ok();

        self.negotiate_conserver(&mut stream).await?;

        let mut guard = self.inner.lock().await;
        *guard = Some(ConsoleInner { stream });
        Ok(())
    }

    #[instrument(skip(self), fields(device = %self.device_name))]
    async fn close(&mut self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if let Some(mut inner) = guard.take() {
            // Disconnect from conserver with escape-c-d.
            inner
                .stream
                .write_all(&[CONSERVER_ESCAPE, b'c', b'd'])
                .await
                .ok();
            inner.stream.shutdown().await.ok();
        }
        Ok(())
    }

    async fn send(&self, data: &str) -> Result<String> {
        let mut guard = self.inner.lock().await;
        let inner = guard.as_mut().ok_or_else(|| {
            SonicError::connection(&self.device_name, "console not connected")
        })?;
        inner.stream.write_all(data.as_bytes()).await.map_err(SonicError::Io)?;
        inner.stream.write_all(b"\r\n").await.map_err(SonicError::Io)?;

        let prompt = self.prompt.clone();
        let timeout = self.command_timeout;
        Self::read_until(&mut inner.stream, timeout, |s| prompt.is_match(s)).await
    }

    #[instrument(skip(self), fields(device = %self.device_name, command = %command))]
    async fn send_command(&self, command: &str) -> Result<CommandResult> {
        let start = Instant::now();
        let mut guard = self.inner.lock().await;
        let inner = guard.as_mut().ok_or_else(|| {
            SonicError::connection(&self.device_name, "console not connected")
        })?;

        let line = format!("{}\r\n", command);
        inner.stream.write_all(line.as_bytes()).await.map_err(SonicError::Io)?;

        let prompt = self.prompt.clone();
        let timeout = self.command_timeout;
        let raw =
            Self::read_until(&mut inner.stream, timeout, |s| prompt.is_match(s)).await?;

        let duration = start.elapsed();

        // Strip echoed command and trailing prompt.
        let mut lines: Vec<&str> = raw.lines().collect();
        if let Some(first) = lines.first() {
            if first.trim() == command.trim() {
                lines.remove(0);
            }
        }
        if let Some(last) = lines.last() {
            if self.prompt.is_match(last) {
                lines.pop();
            }
        }

        Ok(CommandResult {
            stdout: lines.join("\n"),
            stderr: String::new(),
            exit_code: 0,
            duration,
            command: command.to_string(),
        })
    }

    async fn is_alive(&self) -> bool {
        let mut guard = self.inner.lock().await;
        if let Some(ref mut inner) = *guard {
            let mut probe = [0u8; 1];
            match inner.stream.try_read(&mut probe) {
                Ok(0) => false,
                Ok(_) => true,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    fn connection_type(&self) -> ConnectionType {
        ConnectionType::Console
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use sonic_core::Connection;

    fn test_console_info() -> ConsoleInfo {
        ConsoleInfo {
            server: "console-server".into(),
            port: 7782,
            protocol: ConnectionType::Ssh,
        }
    }

    #[test]
    fn conserver_escape_is_ctrl_e() {
        assert_eq!(CONSERVER_ESCAPE, 0x05);
    }

    #[test]
    fn conserver_force_sequence() {
        assert_eq!(CONSERVER_FORCE, &[0x05, b'c', b'f']);
    }

    #[test]
    fn conserver_break_sequence() {
        assert_eq!(CONSERVER_BREAK, &[0x05, b'c', b'l', b'0']);
    }

    #[test]
    fn default_prompt_matches_common_prompts() {
        let creds = Credentials::new("admin").with_password("pass");
        let conn = ConsoleConnection::new(test_console_info(), "dut-1", creds);
        assert!(conn.prompt.is_match("admin@sonic:~$ "));
        assert!(conn.prompt.is_match("sonic# "));
        assert!(conn.prompt.is_match("Router> "));
    }

    #[test]
    fn with_prompt_overrides_default() {
        let creds = Credentials::new("admin").with_password("pass");
        let conn = ConsoleConnection::new(test_console_info(), "dut-1", creds)
            .with_prompt(r"MYDEVICE#\s*$")
            .unwrap();
        assert!(conn.prompt.is_match("MYDEVICE# "));
        assert!(!conn.prompt.is_match("sonic# "));
    }

    #[test]
    fn connection_type_is_console() {
        let creds = Credentials::new("admin");
        let conn = ConsoleConnection::new(test_console_info(), "dut-1", creds);
        assert_eq!(conn.connection_type(), ConnectionType::Console);
    }

    #[test]
    fn with_timeouts() {
        let creds = Credentials::new("admin");
        let conn = ConsoleConnection::new(test_console_info(), "dut-1", creds)
            .with_connect_timeout(Duration::from_secs(10))
            .with_command_timeout(Duration::from_secs(30));
        assert_eq!(conn.connect_timeout, Duration::from_secs(10));
        assert_eq!(conn.command_timeout, Duration::from_secs(30));
    }

    #[test]
    fn device_name_stored() {
        let creds = Credentials::new("admin");
        let conn = ConsoleConnection::new(test_console_info(), "my-switch", creds);
        assert_eq!(conn.device_name, "my-switch");
    }
}
