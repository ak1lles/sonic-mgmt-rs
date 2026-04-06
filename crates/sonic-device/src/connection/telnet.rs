use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use sonic_core::{CommandResult, ConnectionType, Credentials, SonicError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, instrument};

// -----------------------------------------------------------------------
// Telnet protocol constants (RFC 854 / 855)
// -----------------------------------------------------------------------

const IAC: u8 = 255;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;
const SB: u8 = 250;
const SE: u8 = 240;

// Common option codes.
const OPT_ECHO: u8 = 1;
const OPT_SUPPRESS_GO_AHEAD: u8 = 3;

// -----------------------------------------------------------------------
// Internal state
// -----------------------------------------------------------------------

struct TelnetInner {
    stream: TcpStream,
}

// -----------------------------------------------------------------------
// Public struct
// -----------------------------------------------------------------------

/// A Telnet connection implemented on top of a raw TCP stream with minimal
/// in-band telnet option negotiation.
pub struct TelnetConnection {
    host: String,
    port: u16,
    credentials: Credentials,
    prompt: Regex,
    connect_timeout: Duration,
    command_timeout: Duration,
    inner: Arc<Mutex<Option<TelnetInner>>>,
}

impl TelnetConnection {
    pub fn new(host: impl Into<String>, port: u16, credentials: Credentials) -> Self {
        Self {
            host: host.into(),
            port,
            credentials,
            // Default prompt regex: a line ending in `$`, `#`, or `>`
            prompt: Regex::new(r"[#$>]\s*$").expect("built-in regex"),
            connect_timeout: Duration::from_secs(30),
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

    // -- helpers ---------------------------------------------------------

    /// Read bytes from the stream until `predicate` returns `true` on the
    /// accumulated buffer, or until the timeout fires.
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
                    operation: "telnet read".into(),
                });
            }

            let n = match tokio::time::timeout(remaining, stream.read(&mut tmp)).await {
                Ok(Ok(0)) => {
                    break; // EOF
                }
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(SonicError::Io(e)),
                Err(_) => {
                    return Err(SonicError::Timeout {
                        seconds: timeout.as_secs(),
                        operation: "telnet read".into(),
                    });
                }
            };

            // Process telnet IAC sequences inline.
            let clean = Self::strip_telnet_sequences(&tmp[..n], stream).await?;
            buf.extend_from_slice(&clean);

            let text = String::from_utf8_lossy(&buf);
            if predicate(&text) {
                return Ok(text.into_owned());
            }
        }

        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Strip in-band telnet option negotiation bytes, sending appropriate
    /// refusals back to the server for options we do not support.
    async fn strip_telnet_sequences(
        data: &[u8],
        stream: &mut TcpStream,
    ) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if data[i] == IAC && i + 1 < data.len() {
                let cmd = data[i + 1];
                match cmd {
                    WILL | WONT | DO | DONT if i + 2 < data.len() => {
                        let option = data[i + 2];
                        let response = Self::negotiate_option(cmd, option);
                        stream.write_all(&response).await.map_err(SonicError::Io)?;
                        i += 3;
                    }
                    SB => {
                        // Skip until IAC SE.
                        while i < data.len() {
                            if data[i] == IAC
                                && i + 1 < data.len()
                                && data[i + 1] == SE
                            {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    IAC => {
                        // Escaped 0xFF byte.
                        out.push(IAC);
                        i += 2;
                    }
                    _ => {
                        i += 2;
                    }
                }
            } else {
                out.push(data[i]);
                i += 1;
            }
        }
        Ok(out)
    }

    /// Build a 3-byte reply for a WILL/WONT/DO/DONT option request.
    fn negotiate_option(command: u8, option: u8) -> [u8; 3] {
        match command {
            DO => {
                // The server asks us to enable an option.
                // We agree to SUPPRESS-GO-AHEAD; refuse everything else.
                if option == OPT_SUPPRESS_GO_AHEAD {
                    [IAC, WILL, option]
                } else {
                    [IAC, WONT, option]
                }
            }
            WILL => {
                // The server tells us it will enable an option.
                // We accept ECHO and SUPPRESS-GO-AHEAD.
                if option == OPT_ECHO || option == OPT_SUPPRESS_GO_AHEAD {
                    [IAC, DO, option]
                } else {
                    [IAC, DONT, option]
                }
            }
            DONT => [IAC, WONT, option],
            WONT => [IAC, DONT, option],
            _ => [IAC, WONT, option],
        }
    }

    /// After TCP connect, wait for a login prompt, authenticate, and wait
    /// for the shell prompt.
    #[instrument(skip(self, stream), fields(host = %self.host))]
    async fn login(&self, stream: &mut TcpStream) -> Result<()> {
        let timeout = self.connect_timeout;

        // Wait for login/username prompt.
        let login_output = Self::read_until(stream, timeout, |s| {
            let lower = s.to_lowercase();
            lower.contains("login:") || lower.contains("username:")
        })
        .await?;
        debug!("received login banner: {} bytes", login_output.len());

        let username = format!("{}\r\n", self.credentials.username);
        stream.write_all(username.as_bytes()).await.map_err(SonicError::Io)?;

        // Wait for password prompt.
        let _pw_prompt = Self::read_until(stream, timeout, |s| {
            s.to_lowercase().contains("password:")
        })
        .await?;

        let password = self
            .credentials
            .password
            .as_deref()
            .unwrap_or("");
        let pw_line = format!("{}\r\n", password);
        stream.write_all(pw_line.as_bytes()).await.map_err(SonicError::Io)?;

        // Wait for the shell prompt.
        let prompt = self.prompt.clone();
        let post_login = Self::read_until(stream, timeout, |s| prompt.is_match(s))
            .await?;

        // Detect authentication failures.
        let lower = post_login.to_lowercase();
        if lower.contains("login incorrect")
            || lower.contains("access denied")
            || lower.contains("authentication failed")
        {
            return Err(SonicError::Authentication {
                user: self.credentials.username.clone(),
                host: self.host.clone(),
            });
        }

        debug!("telnet login succeeded for {}@{}", self.credentials.username, self.host);
        Ok(())
    }
}

// -----------------------------------------------------------------------
// `Connection` trait
// -----------------------------------------------------------------------

#[async_trait]
impl sonic_core::Connection for TelnetConnection {
    #[instrument(skip(self), fields(host = %self.host, port = %self.port))]
    async fn open(&mut self) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);

        let mut stream = tokio::time::timeout(
            self.connect_timeout,
            TcpStream::connect(&addr),
        )
        .await
        .map_err(|_| SonicError::Timeout {
            seconds: self.connect_timeout.as_secs(),
            operation: format!("telnet connect to {}", addr),
        })?
        .map_err(|e| SonicError::connection(&self.host, e.to_string()))?;

        stream.set_nodelay(true).ok();

        self.login(&mut stream).await?;

        let mut guard = self.inner.lock().await;
        *guard = Some(TelnetInner { stream });
        Ok(())
    }

    #[instrument(skip(self), fields(host = %self.host))]
    async fn close(&mut self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if let Some(mut inner) = guard.take() {
            inner.stream.write_all(b"exit\r\n").await.ok();
            inner.stream.shutdown().await.ok();
        }
        Ok(())
    }

    async fn send(&self, data: &str) -> Result<String> {
        let mut guard = self.inner.lock().await;
        let inner = guard.as_mut().ok_or_else(|| {
            SonicError::connection(&self.host, "not connected")
        })?;
        inner.stream.write_all(data.as_bytes()).await.map_err(SonicError::Io)?;
        inner.stream.write_all(b"\r\n").await.map_err(SonicError::Io)?;

        let prompt = self.prompt.clone();
        let timeout = self.command_timeout;
        let output = Self::read_until(&mut inner.stream, timeout, |s| prompt.is_match(s))
            .await?;
        Ok(output)
    }

    #[instrument(skip(self), fields(host = %self.host, command = %command))]
    async fn send_command(&self, command: &str) -> Result<CommandResult> {
        let start = Instant::now();
        let mut guard = self.inner.lock().await;
        let inner = guard.as_mut().ok_or_else(|| {
            SonicError::connection(&self.host, "not connected")
        })?;

        // Send the command.
        let line = format!("{}\r\n", command);
        inner.stream.write_all(line.as_bytes()).await.map_err(SonicError::Io)?;

        // Read until we see the prompt again.
        let prompt = self.prompt.clone();
        let timeout = self.command_timeout;
        let raw = Self::read_until(&mut inner.stream, timeout, |s| prompt.is_match(s))
            .await?;

        let duration = start.elapsed();

        // The raw output normally starts with the echoed command line and ends
        // with the prompt.  Strip both to get clean output.
        let mut lines: Vec<&str> = raw.lines().collect();

        // Remove the first line if it is the echoed command.
        if let Some(first) = lines.first() {
            if first.trim() == command.trim() {
                lines.remove(0);
            }
        }
        // Remove the last line if it matches the prompt.
        if let Some(last) = lines.last() {
            if self.prompt.is_match(last) {
                lines.pop();
            }
        }

        let stdout = lines.join("\n");

        // Telnet does not provide a native exit code.  We use 0 for a
        // successful read; callers that need a real exit code should wrap
        // commands with `echo $?`.
        Ok(CommandResult {
            stdout,
            stderr: String::new(),
            exit_code: 0,
            duration,
            command: command.to_string(),
        })
    }

    async fn is_alive(&self) -> bool {
        let mut guard = self.inner.lock().await;
        if let Some(ref mut inner) = *guard {
            // A zero-length read on a TCP stream returns Ok(0) when the peer
            // has closed.  We peek instead, which returns WouldBlock on a
            // healthy connection.
            let mut probe = [0u8; 1];
            match inner.stream.try_read(&mut probe) {
                Ok(0) => false,           // EOF
                Ok(_) => true,            // data available (unexpected but alive)
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    fn connection_type(&self) -> ConnectionType {
        ConnectionType::Telnet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_core::Connection;

    // -- negotiate_option ---------------------------------------------------

    #[test]
    fn negotiate_do_suppress_go_ahead_accepts() {
        assert_eq!(
            TelnetConnection::negotiate_option(DO, OPT_SUPPRESS_GO_AHEAD),
            [IAC, WILL, OPT_SUPPRESS_GO_AHEAD]
        );
    }

    #[test]
    fn negotiate_do_unknown_refuses() {
        let opt = 99;
        assert_eq!(
            TelnetConnection::negotiate_option(DO, opt),
            [IAC, WONT, opt]
        );
    }

    #[test]
    fn negotiate_will_echo_accepts() {
        assert_eq!(
            TelnetConnection::negotiate_option(WILL, OPT_ECHO),
            [IAC, DO, OPT_ECHO]
        );
    }

    #[test]
    fn negotiate_will_suppress_go_ahead_accepts() {
        assert_eq!(
            TelnetConnection::negotiate_option(WILL, OPT_SUPPRESS_GO_AHEAD),
            [IAC, DO, OPT_SUPPRESS_GO_AHEAD]
        );
    }

    #[test]
    fn negotiate_will_unknown_refuses() {
        let opt = 42;
        assert_eq!(
            TelnetConnection::negotiate_option(WILL, opt),
            [IAC, DONT, opt]
        );
    }

    #[test]
    fn negotiate_dont_replies_wont() {
        assert_eq!(
            TelnetConnection::negotiate_option(DONT, OPT_ECHO),
            [IAC, WONT, OPT_ECHO]
        );
    }

    #[test]
    fn negotiate_wont_replies_dont() {
        assert_eq!(
            TelnetConnection::negotiate_option(WONT, OPT_ECHO),
            [IAC, DONT, OPT_ECHO]
        );
    }

    // -- strip_telnet_sequences (needs a mock stream, so test the sync parts) --

    #[test]
    fn default_prompt_matches_common_prompts() {
        let creds = Credentials::new("user").with_password("pass");
        let conn = TelnetConnection::new("host", 23, creds);
        assert!(conn.prompt.is_match("admin@sonic:~$ "));
        assert!(conn.prompt.is_match("sonic# "));
        assert!(conn.prompt.is_match("Router> "));
    }

    #[test]
    fn with_prompt_overrides_default() {
        let creds = Credentials::new("user").with_password("pass");
        let conn = TelnetConnection::new("host", 23, creds)
            .with_prompt(r"custom-prompt>>\s*$")
            .unwrap();
        assert!(conn.prompt.is_match("custom-prompt>> "));
        assert!(!conn.prompt.is_match("sonic# "));
    }

    #[test]
    fn connection_type_is_telnet() {
        let creds = Credentials::new("user");
        let conn = TelnetConnection::new("host", 23, creds);
        assert_eq!(conn.connection_type(), ConnectionType::Telnet);
    }

    #[test]
    fn with_timeouts() {
        let creds = Credentials::new("user");
        let conn = TelnetConnection::new("host", 23, creds)
            .with_connect_timeout(Duration::from_secs(5))
            .with_command_timeout(Duration::from_secs(10));
        assert_eq!(conn.connect_timeout, Duration::from_secs(5));
        assert_eq!(conn.command_timeout, Duration::from_secs(10));
    }
}
