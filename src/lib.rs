//! routwatch library: the same data path the CLI and the TUI share.
//!
//! Keeping this as a library (not only a binary) lets the render tests build an
//! `App` from a fixture and assert on the produced frame.

pub mod app;
pub mod completion;
pub mod config;
/// The design contracts in `docs/contracts.md`, checked against the source.
#[cfg(test)]
mod contracts;
pub mod error;
pub mod headless;
/// A fake firmware on a real socket, so the transport half has tests too.
#[cfg(test)]
mod mock_cpe;
pub mod model;
pub mod preflight;
pub mod record;
pub mod report;
pub mod router;
pub mod scrub;
pub mod share;
pub mod theme;
pub mod ui;
pub mod worker;
pub mod write;
pub mod xml;
