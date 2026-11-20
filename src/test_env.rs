//! Test-only isolation for the unit tests that run a real worker.
//!
//! The worker collects through `headless::collect_traced`, which stamps one row
//! into the signal log at `signal_history::path()`. That path comes from
//! `dirs::data_dir()`, so a unit test that runs a worker without pointing
//! `XDG_DATA_HOME` at a scratch directory appends to the real
//! `~/.local/share/routwatch/signal-history.jsonl`.
//!
//! The variable is one for the whole process and the tests run in parallel
//! threads, so the guard takes a process-wide lock, points both XDG variables at
//! a directory of its own, and restores them when it drops.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

static XDG_LOCK: Mutex<()> = Mutex::new(());
static SEQ: AtomicU64 = AtomicU64::new(0);

pub struct DataHome {
    dir: PathBuf,
    prev_data: Option<OsString>,
    prev_config: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
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
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Point both XDG variables at a scratch directory for the guard's lifetime.
pub fn isolate_data_home(tag: &str) -> DataHome {
    let lock = XDG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rw-lib-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    let prev_data = std::env::var_os("XDG_DATA_HOME");
    let prev_config = std::env::var_os("XDG_CONFIG_HOME");
    std::env::set_var("XDG_DATA_HOME", &dir);
    std::env::set_var("XDG_CONFIG_HOME", &dir);
    DataHome {
        dir,
        prev_data,
        prev_config,
        _lock: lock,
    }
}
