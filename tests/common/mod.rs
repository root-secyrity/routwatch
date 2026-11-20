//! Shared isolation for the integration tests.
//!
//! Every persistent log routwatch writes resolves its directory from
//! `dirs::data_dir()`, which honours `XDG_DATA_HOME` on Linux. A test that
//! reaches a write path without pointing that variable at a scratch directory
//! appends a row to the real `~/.local/share/routwatch`, and the evidence the
//! signal log and the incident detector are built on stops being reproducible.
//!
//! `XDG_DATA_HOME` is one variable for the whole process while tests run in
//! parallel threads, so the isolation is a lock plus a directory: `isolate_data_home`
//! takes the process-wide lock, creates a directory of its own, points both XDG
//! variables at it, and restores the previous values when the guard drops.
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

static XDG_LOCK: Mutex<()> = Mutex::new(());
static SEQ: AtomicU64 = AtomicU64::new(0);

/// A scratch directory that removes itself on drop, so a test run leaves no
/// `rw-*` directories behind in the scratch tree.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("rw-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create the scratch directory");
        TempDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The signal log inside this directory, for tests that write a row.
    pub fn signal_log(&self) -> PathBuf {
        self.0.join("signal-history.jsonl")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The process-wide XDG lock held for as long as the isolated data home lives,
/// with the previous environment saved so the next test sees what it expected.
pub struct DataHome {
    dir: TempDir,
    prev_data: Option<OsString>,
    prev_config: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl DataHome {
    pub fn take(tag: &str) -> DataHome {
        let lock = XDG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new(tag);
        let prev_data = std::env::var_os("XDG_DATA_HOME");
        let prev_config = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_DATA_HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        DataHome {
            dir,
            prev_data,
            prev_config,
            _lock: lock,
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The `routwatch` directory the app would create under this data home.
    pub fn data_dir(&self) -> PathBuf {
        self.dir.path().join("routwatch")
    }
}

impl Drop for DataHome {
    fn drop(&mut self) {
        match &self.prev_data {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match &self.prev_config {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}

/// Point `XDG_DATA_HOME` and `XDG_CONFIG_HOME` at a scratch directory for the
/// lifetime of the returned guard, holding the process-wide lock meanwhile.
pub fn isolate_data_home(tag: &str) -> DataHome {
    DataHome::take(tag)
}
