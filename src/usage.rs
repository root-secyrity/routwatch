//! Local usage ledger and budget baselines.
//!
//! The router exposes a month counter on some firmwares and nothing at all on
//! others. This module keeps the parts that must survive a restart (a per-day
//! byte ledger and the value the budget bar counts from) in one small JSON file
//! under `$XDG_DATA_HOME/routwatch/`. It never touches the network.

pub const VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {
        assert_eq!(super::VERSION, 1);
    }
}
