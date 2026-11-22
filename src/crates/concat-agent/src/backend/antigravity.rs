// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Antigravity: Google's `agy`, run as its ACP server, with the Gemini
//! models it offers.
//!
//! Everything about the protocol is in `acp`; this is only what is
//! Antigravity's: where the program is, how it is started, which of its
//! models are wanted, and what it calls a tool.

use std::path::{Path, PathBuf};

use super::acp::AcpProfile;
use super::{BackendId, Caps, Descriptor, Install, Protocol};
use crate::vocab::SERVER_NAME;

/// The environment variable that names the program outright, for a person
/// whose `agy` lives somewhere unusual.
pub const OVERRIDE: &str = "CONCAT_AGY";

/// What Antigravity is, to the registry.
pub static DESCRIPTOR: Descriptor = Descriptor {
    id: BackendId("antigravity"),
    name: "Antigravity",
    locate,
    install: Install {
        command: Some("curl -fsSL https://antigravity.google/cli/install.sh | bash"),
        url: "https://antigravity.google/product/antigravity-cli",
    },
    protocol: Protocol::Acp(PROFILE),
    caps: Caps {
        // The depth is a setting of its own beside the model.
        efforts: true,
        skills: false,
        remote_history: false,
        rename: false,
        setup_on_resume: true,
    },
};

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
    if cfg!(windows) { &["agy.exe", "agy.cmd"] } else { &["agy"] }
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
            find(Some(forced.clone()), None, None, UNIX, |path| path == forced),
            Some(forced)
        );
        assert_eq!(find(Some("/elsewhere/agy".into()), None, None, UNIX, |_| false), None);
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

    /// Against the `agy` on this computer, if there is one and it is signed
    /// in: `cargo test -p concat-agent -- --ignored`.
    #[test]
    #[ignore = "needs Antigravity installed and signed in"]
    fn the_agy_on_this_computer_answers_a_message() {
        use crate::backend::{Client, Event, Turn};
        use crate::vocab::{Input, McpServer, ThreadSetup, Update};
        use std::sync::{Mutex, mpsc};
        use std::time::Duration;

        let Some(program) = locate() else {
            eprintln!("no agy here: nothing checked");
            return;
        };
        let (send, events) = mpsc::channel();
        let send = Mutex::new(send);
        let client = Client { name: "test".into(), title: "Test".into(), version: "0".into() };
        let connected = crate::backend::connect(&DESCRIPTOR, &program, &client, move |event| {
            let _ = send.lock().unwrap().send(event);
        })
        .expect("connects");
        let ids: Vec<&str> = connected.models.list.iter().map(|m| m.id.as_str()).collect();
        eprintln!("models: {ids:?} default {:?} {:?}", connected.models.default_model, connected.models.default_effort);
        assert!(!ids.is_empty() && ids.iter().all(|id| id.starts_with("gemini-")));
        let setup = ThreadSetup {
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            model: None,
            instructions: "Answer in one word.".into(),
            mcp: match std::env::var("CONCAT_TEST_MCP") {
                // A built `concat`, with a home of its own so that what the
                // agent does is not done to the person's projects.
                Ok(concat) => McpServer {
                    command: concat,
                    args: vec!["--mcp".into()],
                    env: vec![("HOME".into(), std::env::temp_dir().join("agent-test-home").to_string_lossy().into_owned())],
                },
                Err(_) => McpServer { command: "/bin/echo".into(), args: vec![], env: vec![] },
            },
        };
        let with_tools = std::env::var("CONCAT_TEST_MCP").is_ok();
        let (done, started) = mpsc::channel();
        connected.backend.start(&setup, Box::new(move |reply| { let _ = done.send(reply); }));
        let thread = started.recv_timeout(Duration::from_secs(30)).unwrap().expect("starts");
        let turn = Turn {
            thread: thread.clone(),
            inputs: vec![Input::Text(if with_tools {
                "Call the eloncut tool concat_version once, then reply with the single word: pong".into()
            } else {
                "Reply with the single word: pong".into()
            })],
            model: connected.models.default_model.clone(),
            effort: Some("low".into()),
        };
        connected.backend.send(turn, Box::new(|_| {}));
        let mut said = String::new();
        let mut tool_done = false;
        loop {
            match events.recv_timeout(Duration::from_secs(90)).expect("an event") {
                Event::Update(Update::AgentWords { words, .. }) => said.push_str(&words),
                Event::Update(Update::ItemDone { item: crate::vocab::Item::Tool { name, state, .. }, .. })
                    if name == "concat_version" =>
                {
                    tool_done |= state == crate::vocab::ToolState::Done;
                }
                Event::Update(Update::TurnEnded { status, .. }) => {
                    assert_eq!(status, "completed");
                    break;
                }
                Event::Closed(reason) => panic!("closed: {reason}"),
                _ => {}
            }
        }
        eprintln!("said: {said:?}");
        assert!(said.to_lowercase().contains("pong"));
        assert!(!with_tools || tool_done, "the eloncut tool was called and ended");
    }
}
