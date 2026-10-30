//! Shell completion, written by hand so the tool keeps its dependency budget.
//!
//! Each script lists the real subcommands and the global flags they accept.

use crate::error::ApiError;

/// Every subcommand the CLI exposes. `restart` is spelled out here because the
/// completion must match `--help`, not the historical word.
pub const SUBCOMMANDS: &[&str] = &[
    "snapshot", "doctor", "bands", "dns", "sms", "device", "restart", "ping", "dump", "status",
    "record", "replay", "preview",
];

pub const GLOBAL_FLAGS: &[&str] = &[
    "--config",
    "--password-stdin",
    "--theme",
    "--view",
    "--graph",
    "--tab",
    "--completion",
    "--generate-config",
    "--help",
    "--version",
];

const PREVIEW_SIZES: &[&str] = &["80x24", "100x30", "120x34"];

/// The same eight names `--theme` validates against, so a typo is completed
/// away instead of ending in `Invalid theme. Use: …`.
const THEMES: &[&str] = crate::theme::THEME_NAMES;

const PREVIEW_FLAGS: &[&str] = &["--size", "--fixture", "--ansi"];

pub fn fish() -> String {
    let mut out = String::from("# fish completion for routwatch\n");
    out.push_str("complete -c routwatch -f\n");
    for cmd in SUBCOMMANDS {
        out.push_str(&format!(
            "complete -c routwatch -n __fish_use_subcommand -a {cmd}\n"
        ));
    }
    for flag in GLOBAL_FLAGS {
        let name = flag.trim_start_matches("--");
        out.push_str(&format!("complete -c routwatch -l {name}\n"));
    }
    out.push_str("complete -c routwatch -n '__fish_seen_subcommand_from preview' -l size -a '");
    out.push_str(&PREVIEW_SIZES.join(" "));
    out.push_str("'\n");
    out.push_str(
        "complete -c routwatch -n '__fish_seen_subcommand_from bands' -l mode -a 'auto 4g 5g'\n",
    );
    out.push_str(
        "complete -c routwatch -n '__fish_seen_subcommand_from --completion' -a 'bash zsh fish'\n",
    );
    out.push_str(&format!(
        "complete -c routwatch -l theme -a '{}'\n",
        THEMES.join(" ")
    ));
    out.push_str(&format!(
        "complete -c routwatch -n '__fish_seen_subcommand_from preview' -a '{}'\n",
        PREVIEW_FLAGS.join(" ")
    ));
    out
}

pub fn bash() -> String {
    let mut out = String::from("# bash completion for routwatch\n");
    out.push_str("_routwatch() {\n");
    out.push_str("  local cur prev\n");
    out.push_str("  cur=\"${COMP_WORDS[COMP_CWORD]}\"\n");
    out.push_str("  prev=\"${COMP_WORDS[COMP_CWORD-1]}\"\n");
    out.push_str("  case \"$prev\" in\n");
    out.push_str(
        "    --completion) COMPREPLY=( $(compgen -W \"bash zsh fish\" -- \"$cur\") ); return ;;\n",
    );
    out.push_str(&format!(
        "    --size) COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") ); return ;;\n",
        PREVIEW_SIZES.join(" ")
    ));
    out.push_str("    --mode) COMPREPLY=( $(compgen -W \"auto 4g 5g\" -- \"$cur\") ); return ;;\n");
    out.push_str(&format!(
        "    --theme) COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") ); return ;;\n",
        THEMES.join(" ")
    ));
    out.push_str("  esac\n");
    out.push_str("  if [[ \"$cur\" == -* ]]; then\n");
    out.push_str(&format!(
        "    COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )\n",
        GLOBAL_FLAGS.join(" ")
    ));
    out.push_str("  else\n");
    out.push_str(&format!(
        "    COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )\n",
        SUBCOMMANDS.join(" ")
    ));
    out.push_str("  fi\n");
    out.push_str("}\n");
    out.push_str("complete -F _routwatch routwatch\n");
    out
}

pub fn zsh() -> String {
    let mut out = String::from("#compdef routwatch\n# zsh completion for routwatch\n");
    out.push_str("_routwatch() {\n");
    out.push_str("  local -a subcommands flags\n");
    out.push_str(&format!("  subcommands=({})\n", SUBCOMMANDS.join(" ")));
    out.push_str(&format!("  flags=({})\n", GLOBAL_FLAGS.join(" ")));
    out.push_str("  _arguments \\\n");
    out.push_str("    '1: :->cmd' \\\n");
    out.push_str(&format!(
        "    '--theme[color theme]:theme:({})' \\\n",
        THEMES.join(" ")
    ));
    out.push_str("    '*: :->args'\n");
    out.push_str("  case $state in\n");
    out.push_str("    cmd) _describe 'command' subcommands ;;\n");
    out.push_str("    args) _describe 'flag' flags ;;\n");
    out.push_str("  esac\n");
    out.push_str("}\n");
    out.push_str("compdef _routwatch routwatch\n");
    out
}

/// Print the script for `shell`; an unknown shell is a usage error (exit 3).
pub fn for_shell(shell: &str) -> Result<String, ApiError> {
    match shell.trim().to_ascii_lowercase().as_str() {
        "fish" => Ok(fish()),
        "bash" => Ok(bash()),
        "zsh" => Ok(zsh()),
        other => Err(ApiError::Usage(format!(
            "unknown shell {other:?} - expected bash, zsh or fish"
        ))),
    }
}

pub fn print(shell: &str) -> Result<(), ApiError> {
    println!("{}", for_shell(shell)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_script_names_every_subcommand() {
        for (name, script) in [("fish", fish()), ("bash", bash()), ("zsh", zsh())] {
            assert!(script.lines().count() > 10, "{name} script too short");
            for cmd in SUBCOMMANDS {
                assert!(script.contains(cmd), "{name} is missing {cmd}");
            }
            assert!(
                script.contains("routwatch"),
                "{name} must register the command"
            );
        }
    }

    #[test]
    fn every_shell_completes_the_theme_names_and_preview_flags() {
        for (name, script) in [("fish", fish()), ("bash", bash()), ("zsh", zsh())] {
            for theme in THEMES {
                assert!(script.contains(theme), "{name} cannot complete {theme}");
            }
        }
        let fish = fish();
        for flag in PREVIEW_FLAGS {
            assert!(fish.contains(flag), "fish cannot complete {flag}");
        }
    }

    #[test]
    fn unknown_shell_is_a_usage_error() {
        assert!(matches!(for_shell("tcsh"), Err(ApiError::Usage(_))));
        assert!(for_shell("FISH ").is_ok(), "case and padding are tolerated");
        assert_eq!(for_shell("tcsh").unwrap_err().exit_code(), 3);
    }
}
