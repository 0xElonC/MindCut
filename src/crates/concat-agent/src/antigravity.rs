// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Antigravity: Google's `agy`, run as its ACP server, with the Gemini
//! models it offers.
//!
//! Everything about the protocol is in `acp`; this is only what is
//! Antigravity's: where the program is, how it is started, which of its
//! models are wanted, and what it calls a tool.

use std::path::{Path, PathBuf};

use crate::acp::AcpProfile;
use crate::wire::SERVER_NAME;

/// What the person calls it. A proper name: not translated.
pub const NAME: &str = "Antigravity";

/// A command that installs it.
pub const INSTALL_COMMAND: &str = "curl -fsSL https://antigravity.google/cli/install.sh | bash";

/// Where to read more.
pub const INSTALL_URL: &str = "https://antigravity.google/product/antigravity-cli";

/// The environment variable that names the program outright, for a person
/// whose `agy` lives somewhere unusual.
pub const OVERRIDE: &str = "CONCAT_AGY";

/// How `agy acp` differs from any other ACP agent.
pub const PROFILE: AcpProfile = AcpProfile {
    // The agent is let act on its own, as the person chose.
    arguments: &["acp", "--dangerously-skip-permissions"],
    system_prompt_meta: &["antigravity", "systemPrompt", "append"],
    // `agy` also lists models from other makers; the ones wanted are Gemini.
    model_filter: |id| id.starts_with("gemini-"),
    tool_name,
};

/// A tool of the Concat MCP server comes marked `mcp_eloncut_`.
fn tool_name(name: &str) -> &str {
    name.strip_prefix("mcp_")
        .and_then(|rest| rest.strip_prefix(SERVER_NAME))
        .and_then(|rest| rest.strip_prefix('_'))
        .unwrap_or(name)
}

fn names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["agy.exe", "agy.cmd"]
    } else {
        &["agy"]
    }
}

/// The `agy` to run, if there is one on this computer.
pub fn locate() -> Option<PathBuf> {
    let var = |key: &str| {
        std::env::var_os(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = var("HOME").or_else(|| var("USERPROFILE"));
    find(
        std::env::var_os(OVERRIDE).map(PathBuf::from),
        std::env::var_os("PATH").as_deref(),
        home.as_deref(),
        names(),
        |candidate| candidate.is_file(),
    )
}

/// [`locate`] over what it reads from the machine, so a test can give it
/// another. A program named outright is the only one tried. Next the PATH,
/// then where the installer puts it, which a program started from the Dock
/// or the Start menu does not have on its PATH.
pub fn find(
    forced: Option<PathBuf>,
    path: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
    names: &[&str],
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(forced) = forced.filter(|forced| !forced.as_os_str().is_empty()) {
        return exists(&forced).then_some(forced);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = path {
        for dir in std::env::split_paths(path) {
            for name in names {
                candidates.push(dir.join(name));
            }
        }
    }
    if let Some(home) = home {
        for name in names {
            candidates.push(home.join(".local").join("bin").join(name));
        }
    }
    for name in names {
        candidates.push(Path::new("/opt/homebrew/bin").join(name));
        candidates.push(Path::new("/usr/local/bin").join(name));
    }
    candidates.into_iter().find(|candidate| exists(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIX: &[&str] = &["agy"];

    #[test]
    fn a_named_program_is_the_only_one_tried() {
        let forced = PathBuf::from("/elsewhere/agy");
        assert_eq!(
            find(Some(forced.clone()), None, None, UNIX, |path| path
                == forced),
            Some(forced)
        );
        assert_eq!(
            find(Some("/elsewhere/agy".into()), None, None, UNIX, |_| false),
            None
        );
    }

    #[test]
    fn path_comes_before_the_installers_folder() {
        let on_path = PathBuf::from("/tools/bin/agy");
        let found = find(
            None,
            Some(std::ffi::OsStr::new("/tools/bin")),
            Some(Path::new("/home/me")),
            UNIX,
            |path| path == on_path || path == Path::new("/home/me/.local/bin/agy"),
        );
        assert_eq!(found, Some(on_path));
        let found = find(None, None, Some(Path::new("/home/me")), UNIX, |path| {
            path == Path::new("/home/me/.local/bin/agy")
        });
        assert_eq!(found, Some(PathBuf::from("/home/me/.local/bin/agy")));
        assert_eq!(find(None, None, None, UNIX, |_| false), None);
    }

    #[test]
    fn the_concat_servers_tools_lose_their_mark_and_others_keep_their_names() {
        assert_eq!(tool_name("mcp_eloncut_clips_split"), "clips_split");
        assert_eq!(tool_name("mcp_eloncut_x_y"), "x_y");
        assert_eq!(tool_name("run_command"), "run_command");
        assert_eq!(tool_name("mcp_other_thing"), "mcp_other_thing");
        assert_eq!(tool_name("mcp_eloncut"), "mcp_eloncut");
    }

    #[test]
    fn only_gemini_models_are_wanted() {
        assert!((PROFILE.model_filter)("gemini-3.8-flash"));
        assert!(!(PROFILE.model_filter)("claude-sonnet-4-6"));
        assert!(!(PROFILE.model_filter)("gpt-oss-120b-medium"));
    }
}
