//! routwatch library: the same data path the CLI and the TUI share.
//!
//! Keeping this as a library (not only a binary) lets the render tests build an
//! `App` from a fixture and assert on the produced frame.

pub mod advisor;
pub mod antenna;
pub mod app;
pub mod capabilities;
pub mod client_detail;
pub mod completion;
pub mod config;
pub mod config_snapshot;
/// The design contracts in `docs/contracts.md`, checked against the source.
#[cfg(test)]
mod contracts;
pub mod error;
pub mod events;
pub mod headless;
pub mod history;
pub mod incidents;
/// A fake firmware on a real socket, so the transport half has tests too.
#[cfg(test)]
mod mock_cpe;
pub mod model;
pub mod output;
pub mod preflight;
pub mod privacy;
pub mod privacy_drift;
pub mod record;
pub mod report;
pub mod router;
pub mod scan;
pub mod scrub;
pub mod share;
pub mod signal_history;
pub mod speedtest;
/// Test-only isolation: a unit test that runs a worker must not write the real
/// signal log.
#[cfg(test)]
mod test_env;
pub mod theme;
pub mod ui;
pub mod usage;
pub mod watch;
pub mod worker;
pub mod write;
pub mod xml;
