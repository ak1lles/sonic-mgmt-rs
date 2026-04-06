//! Device abstraction layer for SONiC network management.
//!
//! Provides SSH/Telnet/Console connections, host abstractions for SONiC DUTs,
//! EOS/Cisco/Fanout neighbors, PTF containers, a connection pool, and CLI
//! output parsers that populate the `sonic_core` fact types.

pub mod connection;
pub mod facts;
pub mod hosts;

pub use connection::pool::ConnectionPool;
pub use connection::ssh::SshConnection;
pub use connection::telnet::TelnetConnection;
pub use connection::console::ConsoleConnection;

pub use hosts::create_host;
pub use hosts::sonic::SonicHost;
pub use hosts::eos::EosHost;
pub use hosts::fanout::FanoutHost;
pub use hosts::ptf::PtfHost;
pub use hosts::cisco::CiscoHost;

pub use facts::cache::FactsCache;
pub use facts::parser;
