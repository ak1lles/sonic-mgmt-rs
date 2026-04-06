use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sonic_core::{Connection, ConnectionType, Credentials, SonicError, Result};
use tokio::sync::Mutex;
use tracing::{debug, instrument};

use crate::connection::ssh::SshConnection;
use crate::connection::telnet::TelnetConnection;

// -----------------------------------------------------------------------
// Pool entry
// -----------------------------------------------------------------------

struct PoolEntry {
    conn: Box<dyn Connection>,
    #[allow(dead_code)]
    created_at: Instant,
    last_used: Instant,
}

impl PoolEntry {
    fn idle_duration(&self) -> Duration {
        self.last_used.elapsed()
    }
}

/// Composite key for pool lookup.
#[derive(Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    host: String,
    port: u16,
    conn_type: ConnectionType,
}

// -----------------------------------------------------------------------
// Public API
// -----------------------------------------------------------------------

/// A simple connection pool that caches and reuses open connections keyed by
/// `(host, port, connection_type)`.
///
/// The pool enforces:
/// * A maximum number of connections per host.
/// * An idle timeout after which connections are automatically discarded.
///
/// All access is behind an `Arc<Mutex<..>>` so the pool itself is `Send +
/// Sync` and can be shared across tasks.
pub struct ConnectionPool {
    max_per_host: usize,
    idle_timeout: Duration,
    state: Arc<Mutex<PoolState>>,
}

struct PoolState {
    connections: HashMap<PoolKey, Vec<PoolEntry>>,
}

impl ConnectionPool {
    /// Create a new pool.
    ///
    /// * `max_per_host` -- maximum cached connections per `(host, conn_type)`.
    /// * `idle_timeout`  -- connections idle longer than this are dropped.
    pub fn new(max_per_host: usize, idle_timeout: Duration) -> Self {
        Self {
            max_per_host,
            idle_timeout,
            state: Arc::new(Mutex::new(PoolState {
                connections: HashMap::new(),
            })),
        }
    }

    /// Acquire a connection.  If a live, idle connection for the requested
    /// `(host, port, conn_type)` is available it is returned directly.
    /// Otherwise a new one is created, opened, and returned.
    #[instrument(skip(self, credentials), fields(%host, %port, ?conn_type))]
    pub async fn get_connection(
        &self,
        host: &str,
        port: u16,
        conn_type: ConnectionType,
        credentials: Credentials,
    ) -> Result<Box<dyn Connection>> {
        let key = PoolKey {
            host: host.to_string(),
            port,
            conn_type,
        };

        // 1) Try to reuse an existing idle connection.
        {
            let mut state = self.state.lock().await;
            if let Some(entries) = state.connections.get_mut(&key) {
                // Walk from the back so we can pop efficiently.
                while let Some(mut entry) = entries.pop() {
                    if entry.idle_duration() > self.idle_timeout {
                        debug!("discarding idle connection to {}:{}", host, port);
                        entry.conn.close().await.ok();
                        continue;
                    }
                    if entry.conn.is_alive().await {
                        debug!("reusing pooled connection to {}:{}", host, port);
                        entry.last_used = Instant::now();
                        return Ok(entry.conn);
                    }
                    debug!("pooled connection to {}:{} is dead, discarding", host, port);
                    entry.conn.close().await.ok();
                }
            }
        }

        // 2) Create a fresh connection.
        debug!("creating new {} connection to {}:{}", conn_type, host, port);
        let mut conn: Box<dyn Connection> = match conn_type {
            ConnectionType::Ssh => {
                Box::new(SshConnection::new(host, port, credentials))
            }
            ConnectionType::Telnet => {
                Box::new(TelnetConnection::new(host, port, credentials))
            }
            ConnectionType::Console => {
                // Console connections require a ConsoleInfo which is not
                // available here.  Callers that need console connections
                // should create them directly.
                return Err(SonicError::other(
                    "console connections cannot be created from the pool without ConsoleInfo; \
                     create ConsoleConnection directly",
                ));
            }
            _ => {
                return Err(SonicError::other(format!(
                    "unsupported connection type for pool: {conn_type}"
                )));
            }
        };

        conn.open().await?;
        Ok(conn)
    }

    /// Return a connection to the pool for later reuse.
    #[instrument(skip(self, conn), fields(%host, %port, ?conn_type))]
    pub async fn release_connection(
        &self,
        host: &str,
        port: u16,
        conn_type: ConnectionType,
        mut conn: Box<dyn Connection>,
    ) {
        if !conn.is_alive().await {
            debug!("released connection to {}:{} is dead, dropping", host, port);
            conn.close().await.ok();
            return;
        }

        let key = PoolKey {
            host: host.to_string(),
            port,
            conn_type,
        };

        let mut state = self.state.lock().await;
        let entries = state.connections.entry(key).or_insert_with(Vec::new);

        if entries.len() >= self.max_per_host {
            debug!(
                "pool at capacity ({}) for {}:{}, dropping oldest",
                self.max_per_host, host, port
            );
            if let Some(mut oldest) = entries.pop() {
                oldest.conn.close().await.ok();
            }
        }

        entries.push(PoolEntry {
            conn,
            created_at: Instant::now(),
            last_used: Instant::now(),
        });
    }

    /// Remove all dead or idle-timed-out connections from the pool.
    #[instrument(skip(self))]
    pub async fn cleanup(&self) {
        let mut state = self.state.lock().await;
        let idle_timeout = self.idle_timeout;

        for (key, entries) in state.connections.iter_mut() {
            let mut i = 0;
            while i < entries.len() {
                let too_old = entries[i].idle_duration() > idle_timeout;
                let dead = !entries[i].conn.is_alive().await;
                if too_old || dead {
                    let reason = if too_old { "idle timeout" } else { "dead" };
                    debug!(
                        "removing {} connection to {}:{} ({})",
                        key.conn_type, key.host, key.port, reason
                    );
                    let mut removed = entries.swap_remove(i);
                    removed.conn.close().await.ok();
                    // Don't increment `i` -- swap_remove moved the last
                    // element into position `i`.
                } else {
                    i += 1;
                }
            }
        }

        // Drop empty buckets.
        state.connections.retain(|_, v| !v.is_empty());
    }

    /// Close and discard all pooled connections.
    #[instrument(skip(self))]
    pub async fn drain(&self) {
        let mut state = self.state.lock().await;
        for (_, entries) in state.connections.drain() {
            for mut entry in entries {
                entry.conn.close().await.ok();
            }
        }
    }

    /// Returns the total number of connections currently held in the pool.
    pub async fn size(&self) -> usize {
        let state = self.state.lock().await;
        state.connections.values().map(|v| v.len()).sum()
    }
}
