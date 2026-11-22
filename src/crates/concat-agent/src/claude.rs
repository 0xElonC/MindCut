mod events;
mod history;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use self::events::{TurnEvents, valid_session};
use self::history::History;
use crate::backend::{
    Availability, Backend, Defaults, Done, Event, OnEvent, Provider, Session, SessionSetup,
};
use crate::wire::{Input, Item, Model, Update, shorten};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
const IMAGE_LIMIT: usize = 5 * 1024 * 1024;

pub struct ClaudeBackend {
    program: PathBuf,
    history_dir: PathBuf,
    on_event: OnEvent,
    sessions: Mutex<HashMap<String, SessionSetup>>,
    active: Arc<Mutex<Option<Active>>>,
}

struct Active {
    id: String,
    turn: String,
    child: Option<Child>,
    interrupted: bool,
}

impl Active {
    fn stop(&mut self) -> Result<(), String> {
        if let Some(child) = &mut self.child {
            child
                .kill()
                .map_err(|error| format!("Could not stop Claude Code: {error}"))?;
        }
        self.interrupted = true;
        Ok(())
    }
}

impl ClaudeBackend {
    pub fn status() -> Availability {
        Self::status_for(crate::locate::claude().as_deref())
    }

    pub fn status_for(program: Option<&Path>) -> Availability {
        let Some(program) = program else {
            return auth_status(None);
        };
        let Ok(output) = launch(program)
            .args(["auth", "status"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            return Availability::Unavailable(
                "Could not start Claude Code to check sign-in.".into(),
            );
        };
        auth_status(Some((output.status.code(), &output.stdout)))
    }

    pub fn new(program: PathBuf, history_dir: PathBuf, on_event: OnEvent) -> Self {
        let program = program.canonicalize().unwrap_or(program);
        Self {
            program,
            history_dir,
            on_event,
            sessions: Mutex::default(),
            active: Arc::default(),
        }
    }

    fn open(&self, setup: SessionSetup, id: String) -> Result<Session, String> {
        validate_server(&setup.mcp.name)?;
        let history = History::read(&History::path(&self.history_dir, &id)?)?;
        self.sessions
            .lock()
            .expect("Claude sessions")
            .insert(id.clone(), setup);
        Ok(Session {
            id,
            history: history.items,
        })
    }
}

impl Backend for ClaudeBackend {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn start_session(&self, setup: SessionSetup, done: Done<Session>) {
        done(self.open(setup, format!("claude-{}", unique_id())));
    }

    fn resume_session(&self, setup: SessionSetup, id: &str, done: Done<Session>) {
        done(self.open(setup, id.into()));
    }

    fn send_turn(
        &self,
        id: &str,
        inputs: Vec<Input>,
        model: Option<String>,
        effort: Option<String>,
        done: Done<String>,
    ) {
        let id = id.to_owned();
        let setup = self
            .sessions
            .lock()
            .expect("Claude sessions")
            .get(&id)
            .cloned();
        let Some(setup) = setup else {
            return done(Err(
                "Open the Claude conversation before sending a message.".into(),
            ));
        };
        let path = match History::path(&self.history_dir, &id) {
            Ok(path) => path,
            Err(error) => return done(Err(error)),
        };
        let turn = unique_id();
        {
            let mut active = self.active.lock().expect("active Claude turn");
            if active.is_some() {
                drop(active);
                return done(Err("Claude Code is already responding.".into()));
            }
            *active = Some(Active {
                id: id.clone(),
                turn: turn.clone(),
                child: None,
                interrupted: false,
            });
        }
        let active = Arc::clone(&self.active);
        let on_event = Arc::clone(&self.on_event);
        let program = self.program.clone();
        std::thread::spawn(move || {
            let emit = |update| on_event(Event::Update(update));
            let prepared = (|| {
                let history = History::read(&path)?;
                let args = arguments(
                    &setup,
                    model.as_deref(),
                    effort.as_deref(),
                    history.session_id.as_deref(),
                )?;
                let (input, item) = input_message(&turn, &inputs, history.session_id.as_deref())?;
                Ok((history, args, input, item))
            })();
            let (mut history, args, input, item) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    active.lock().expect("active Claude turn").take();
                    return done(Err(error));
                }
            };
            done(Ok(turn.clone()));
            emit(Update::TurnStarted {
                thread: id.clone(),
                turn: turn.clone(),
            });
            if history.items.is_empty()
                && let Item::User { text, .. } = &item
                && !text.trim().is_empty()
            {
                emit(Update::Named {
                    thread: id.clone(),
                    name: shorten(text, 60),
                });
            }
            let mut events = TurnEvents::new(&id, &turn);
            let mut command = launch(&program);
            command
                .current_dir(&setup.cwd)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut error = exchange(&mut command, &active, &input, &mut events, &emit).err();
            let interrupted = active
                .lock()
                .expect("active Claude turn")
                .as_ref()
                .is_some_and(|active| active.interrupted);
            let completed = error.is_none()
                && events
                    .result
                    .as_ref()
                    .is_some_and(|result| result.failed.is_none());
            let session_id = events.session_id.take();
            if let Some(outcome) = events.result.take() {
                error = outcome.failed.or(error);
            }
            let items = events.finish(&emit);
            if let (Some(previous), Some(returned)) = (&history.session_id, &session_id)
                && previous != returned
            {
                error = Some("Claude Code returned a different session ID from the one it was asked to resume.".into());
            } else if (completed || interrupted && !items.is_empty()) && session_id.is_some() {
                history.session_id = session_id;
            } else if completed && history.session_id.is_none() {
                error = Some(
                    "Claude Code did not return a session ID for resuming this conversation."
                        .into(),
                );
            }
            history.items.push(item);
            history.items.extend(items);
            if let Err(problem) = history.save(&path) {
                if interrupted || error.is_some() {
                    emit(Update::Trouble {
                        thread: Some(id.clone()),
                        message: problem,
                    });
                } else {
                    error = Some(problem);
                }
            }
            let state = active.lock().expect("active Claude turn").take();
            let status = if state.is_some_and(|state| state.interrupted) {
                error = None;
                "interrupted"
            } else if error.is_some() {
                "failed"
            } else {
                "completed"
            };
            emit(Update::TurnEnded {
                thread: id,
                turn,
                status: status.into(),
                error,
            });
        });
    }

    fn interrupt(&self, id: &str, turn: &str, done: Done<()>) {
        let result = self
            .active
            .lock()
            .expect("active Claude turn")
            .as_mut()
            .filter(|active| active.id == id && active.turn == turn)
            .map_or(Ok(()), Active::stop);
        done(result);
    }

    fn models(&self, done: Done<Vec<Model>>) {
        done(Ok([
            ("sonnet", "Sonnet"),
            ("opus", "Opus"),
            ("haiku", "Haiku"),
        ]
        .into_iter()
        .map(|(id, name)| Model {
            id: id.into(),
            name: name.into(),
            efforts: if id == "haiku" {
                Vec::new()
            } else {
                ["low", "medium", "high"].map(str::to_owned).to_vec()
            },
            default_effort: if id == "haiku" {
                String::new()
            } else {
                "medium".into()
            },
        })
        .collect()));
    }

    fn defaults(&self, done: Done<Defaults>) {
        done(Ok((None, None)));
    }

    fn archive(&self, id: &str, done: Done<()>) {
        let active = self.active.lock().expect("active Claude turn");
        if active.as_ref().is_some_and(|active| active.id == id) {
            drop(active);
            return done(Err(
                "Stop the Claude response before deleting the conversation.".into(),
            ));
        }
        self.sessions.lock().expect("Claude sessions").remove(id);
        let result =
            History::path(&self.history_dir, id).and_then(|path| {
                match std::fs::remove_file(path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(format!("Could not remove Claude history: {error}")),
                }
            });
        drop(active);
        done(result);
    }
}

impl Drop for ClaudeBackend {
    fn drop(&mut self) {
        if let Some(active) = self.active.lock().expect("active Claude turn").as_mut() {
            let _ = active.stop();
        }
    }
}

fn unique_id() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{time:x}-{:x}-{:x}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn auth_status(output: Option<(Option<i32>, &[u8])>) -> Availability {
    let Some((code, stdout)) = output else {
        return Availability::NotInstalled;
    };
    let parsed = serde_json::from_slice::<Value>(stdout).ok();
    match (
        code,
        parsed
            .as_ref()
            .and_then(|value| value["loggedIn"].as_bool()),
    ) {
        (Some(0), Some(true)) => Availability::Available,
        (Some(1), Some(false)) => Availability::Unavailable(
            "Claude Code is not signed in. Launch `claude` to sign in.".into(),
        ),
        _ => Availability::Unavailable(
            "Could not verify Claude Code sign-in. Check `claude auth status` in your terminal."
                .into(),
        ),
    }
}

fn launch(program: &Path) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if program
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd"))
            && let Some(folder) = program.parent()
        {
            let script = folder.join("node_modules/@anthropic-ai/claude-code/cli.js");
            if script.is_file() {
                let node = folder.join("node.exe");
                command = Command::new(if node.is_file() {
                    node
                } else {
                    PathBuf::from("node.exe")
                });
                command.arg(script);
            }
        }
        command.creation_flags(0x0800_0000);
    }
    command.env_clear();
    for name in [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "SYSTEMROOT",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "TERM",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "CLAUDE_CONFIG_DIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

fn validate_server(name: &str) -> Result<(), String> {
    if !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(
            "The Claude MCP server name must use only letters, digits, hyphens and underscores."
                .into(),
        )
    }
}

fn arguments(
    setup: &SessionSetup,
    model: Option<&str>,
    effort: Option<&str>,
    resume: Option<&str>,
) -> Result<Vec<String>, String> {
    let name = &setup.mcp.name;
    validate_server(name)?;
    let server = &setup.mcp.server;
    let env: serde_json::Map<String, Value> = server
        .env
        .iter()
        .map(|(name, value)| (name.clone(), json!(value)))
        .collect();
    let config = json!({ "mcpServers": { name: { "command": server.command, "args": server.args, "env": env } } });
    let allowed = format!("mcp__{name}__*");
    let config = config.to_string();
    let mut args = [
        "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages",
        "--tools", "", "--strict-mcp-config", "--mcp-config", &config, "--allowedTools", &allowed,
        "--permission-mode", "dontAsk", "--setting-sources", "", "--settings", r#"{"disableAllHooks":true}"#,
        "--disable-slash-commands", "--system-prompt", "You are the assistant for the application that launched you. Use only the tools provided for this session.",
        "--append-system-prompt", &setup.instructions, "--system-prompt-snapshot", "off",
    ].map(str::to_owned).to_vec();
    if let Some(resume) = resume {
        if !valid_session(resume) {
            return Err("The saved Claude Code session ID is invalid.".into());
        }
        args.extend(["--resume".into(), resume.into()]);
    }
    if let Some(model) = model
        .or(setup.model.as_deref())
        .filter(|model| !model.is_empty())
    {
        args.push(format!("--model={model}"));
    }
    if let Some(effort) = effort.filter(|effort| !effort.is_empty()) {
        args.push(format!("--effort={effort}"));
    }
    Ok(args)
}

fn exchange(
    command: &mut Command,
    active: &Mutex<Option<Active>>,
    input: &Value,
    events: &mut TurnEvents<'_>,
    emit: &impl Fn(Update),
) -> Result<(), String> {
    let (mut stdin, stdout) = {
        let mut state = active.lock().expect("active Claude turn");
        let state = state.as_mut().expect("the current worker owns the turn");
        if state.interrupted {
            return Ok(());
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("Could not start Claude Code: {error}"))?;
        let handles = (
            child.stdin.take().expect("piped Claude input"),
            child.stdout.take().expect("piped Claude output"),
        );
        state.child = Some(child);
        handles
    };
    let reader = std::thread::scope(|scope| {
        let writer = scope.spawn(move || writeln!(stdin, "{input}"));
        let reader = BufReader::new(stdout).lines().try_for_each(|line| {
            events.read(&line?, emit);
            Ok::<_, std::io::Error>(())
        });
        if let Err(error) = reader {
            let _ = active
                .lock()
                .expect("active Claude turn")
                .as_mut()
                .and_then(|active| active.child.as_mut())
                .map(Child::kill);
            return Err(format!("Could not read Claude Code output: {error}"));
        }
        writer
            .join()
            .expect("Claude input writer")
            .map_err(|error| format!("Could not send the Claude message: {error}"))
    });
    let mut child = active
        .lock()
        .expect("active Claude turn")
        .as_mut()
        .and_then(|active| active.child.take())
        .expect("Claude process started");
    let exit = child
        .wait()
        .map_err(|error| format!("Could not wait for Claude Code: {error}"))?;
    reader?;
    if !exit.success() || events.result.is_none() {
        return Err(format!(
            "Claude Code stopped before completing the response ({exit})."
        ));
    }
    Ok(())
}

fn input_message(
    turn: &str,
    inputs: &[Input],
    session: Option<&str>,
) -> Result<(Value, Item), String> {
    let mut content = Vec::new();
    let mut words = Vec::new();
    let mut images = Vec::new();
    for input in inputs {
        match input {
            Input::Text(text) if !text.trim().is_empty() => {
                content.push(json!({ "type": "text", "text": text }));
                words.push(text.as_str());
            }
            Input::Text(_) => {}
            Input::Image(path) => {
                content.push(image_block(path)?);
                images.push(path.to_string_lossy().into_owned());
            }
            Input::Skill { .. } => {
                return Err("Codex skills are not available in Claude Code conversations.".into());
            }
        }
    }
    if content.is_empty() {
        return Err("Send some text or an image to Claude Code.".into());
    }
    let message = json!({ "type": "user", "session_id": session.unwrap_or_default(), "parent_tool_use_id": null,
        "message": { "role": "user", "content": content } });
    Ok((
        message,
        Item::User {
            id: format!("{turn}-user"),
            text: words.join("\n"),
            images,
        },
    ))
}

fn image_block(path: &Path) -> Result<Value, String> {
    let mut data = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(IMAGE_LIMIT as u64 + 1).read_to_end(&mut data))
        .map_err(|error| format!("Could not read the Claude image attachment: {error}"))?;
    if data.len() > IMAGE_LIMIT {
        return Err("Claude image attachments are limited to 5 MiB each.".into());
    }
    let mime = match data.as_slice() {
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => "image/png",
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        _ => return Err("Claude image attachments must be PNG, JPEG, GIF or WebP.".into()),
    };
    Ok(
        json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": base64(&data) } }),
    )
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        encoded.push(char::from(ALPHABET[usize::from(first >> 2)]));
        encoded.push(char::from(
            ALPHABET[usize::from((first & 3) << 4 | second >> 4)],
        ));
        encoded.push(if chunk.len() > 1 {
            char::from(ALPHABET[usize::from((second & 15) << 2 | third >> 6)])
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            char::from(ALPHABET[usize::from(third & 63)])
        } else {
            '='
        });
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::backend::NamedMcpServer;
    use crate::wire::{McpServer, ToolState};

    const TURN: &str = include_str!("claude/fixtures/turn.jsonl");
    const SESSION_ID: &str = "12345678-1234-4234-8234-123456789abc";

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("concat-claude-{}", unique_id()));
            std::fs::create_dir(&dir).expect("test directory");
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(cwd: &Path) -> SessionSetup {
        SessionSetup {
            cwd: cwd.to_string_lossy().into_owned(),
            model: Some("sonnet".into()),
            instructions: "Only edit the open project.".into(),
            mcp: NamedMcpServer {
                name: "editing".into(),
                server: McpServer {
                    command: "project-mcp".into(),
                    args: vec!["--stdio".into()],
                    env: vec![("SOCKET".into(), "local socket".into())],
                },
            },
        }
    }

    #[test]
    fn only_the_configured_mcp_server_is_allowed_and_turn_choices_are_applied() {
        let mut setup = setup(Path::new("."));
        let args =
            arguments(&setup, Some("opus"), Some("high"), Some(SESSION_ID)).expect("valid flags");
        let after = |name| {
            let index = args
                .iter()
                .position(|arg| arg == name)
                .expect("flag is present");
            args[index + 1].as_str()
        };
        assert_eq!(after("--tools"), "");
        assert!(args.contains(&"--strict-mcp-config".into()));
        assert_eq!(after("--allowedTools"), "mcp__editing__*");
        assert_eq!(after("--permission-mode"), "dontAsk");
        assert_eq!(after("--append-system-prompt"), setup.instructions);
        assert_eq!(after("--input-format"), "stream-json");
        assert_eq!(after("--output-format"), "stream-json");
        assert_eq!(after("--resume"), SESSION_ID);
        assert!(args.contains(&"--model=opus".into()));
        assert!(args.contains(&"--effort=high".into()));
        assert!(!args.contains(&"--no-session-persistence".into()));
        let config: Value = serde_json::from_str(after("--mcp-config")).expect("MCP config JSON");
        assert_eq!(
            config,
            json!({ "mcpServers": { "editing": {
            "command": "project-mcp", "args": ["--stdio"], "env": { "SOCKET": "local socket" },
        } } })
        );
        let fresh = arguments(&setup, None, None, None).expect("new session flags");
        assert!(fresh.contains(&"--model=sonnet".into()));
        assert!(!fresh.contains(&"--resume".into()));
        setup.mcp.name = "editing* Bash".into();
        assert!(arguments(&setup, None, None, None).is_err());
        setup.mcp.name = "editing".into();
        assert!(arguments(&setup, None, None, Some("--continue")).is_err());
    }

    #[test]
    fn availability_uses_observed_exit_codes_and_only_the_boolean_status() {
        assert_eq!(ClaudeBackend::status_for(None), Availability::NotInstalled);
        assert_eq!(
            auth_status(Some((
                Some(0),
                br#"{"loggedIn":true,"authMethod":"claude.ai"}"#
            ))),
            Availability::Available
        );
        assert!(
            matches!(auth_status(Some((Some(1), br#"{"loggedIn":false}"#))), Availability::Unavailable(reason) if reason.contains("not signed in"))
        );
        assert!(
            matches!(auth_status(Some((Some(2), b"unsupported command"))), Availability::Unavailable(reason) if reason.contains("Could not verify"))
        );
        assert!(
            matches!(auth_status(Some((Some(0), br#"{"loggedIn":false}"#))), Availability::Unavailable(reason) if reason.contains("Could not verify"))
        );
    }

    #[test]
    fn launcher_has_no_credential_environment_entries() {
        let command = launch(Path::new("claude"));
        let names = command
            .get_envs()
            .map(|(name, _)| name.to_string_lossy())
            .collect::<Vec<_>>();
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            assert!(!names.contains(&name.into()));
        }
    }

    #[test]
    fn text_and_images_are_sent_as_content_blocks_without_image_paths() {
        let dir = Temp::new();
        let image = dir.0.join("attachment");
        std::fs::write(&image, b"\x89PNG\r\n\x1a\n").expect("test image header");
        let (message, item) = input_message(
            "turn",
            &[Input::Text("Hi".into()), Input::Image(image.clone())],
            Some(SESSION_ID),
        )
        .expect("content blocks");
        assert_eq!(
            message,
            json!({ "type": "user", "session_id": SESSION_ID, "parent_tool_use_id": null,
            "message": { "role": "user", "content": [
                { "type": "text", "text": "Hi" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } },
            ] } })
        );
        assert!(
            matches!(item, Item::User { images, .. } if images == [image.to_string_lossy().into_owned()])
        );
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert!(input_message("turn", &[], None).is_err());
    }

    fn fixture_updates() -> (Vec<Update>, Vec<Item>) {
        let mut events = TurnEvents::new("thread", "turn");
        let updates = RefCell::new(Vec::new());
        let emit = |update| updates.borrow_mut().push(update);
        for line in TURN.lines() {
            events.read(line, &emit);
        }
        assert_eq!(events.session_id.as_deref(), Some(SESSION_ID));
        assert!(
            events
                .result
                .as_ref()
                .is_some_and(|result| result.failed.is_none())
        );
        let items = events.finish(&emit);
        (updates.into_inner(), items)
    }

    #[test]
    fn fixture_streams_text_and_reasoning_without_duplicating_the_final_message() {
        let (updates, items) = fixture_updates();
        assert_eq!(
            items
                .iter()
                .filter(|item| matches!(item, Item::Agent { .. }))
                .count(),
            1
        );
        assert!(updates.iter().any(|update| matches!(update, Update::AgentWords { thread, words, .. } if thread == "thread" && words == "Hi!")));
        assert!(updates.iter().any(|update| matches!(update, Update::ThoughtWords { words, .. } if words == "Checking the timeline.")));
        assert!(updates.iter().any(|update| matches!(update, Update::ItemDone { item: Item::Agent { text, .. }, .. } if text == "Hi!")));
        assert!(updates.iter().any(|update| matches!(update, Update::ItemDone { item: Item::Reasoning { text, .. }, .. } if text == "Checking the timeline.")));
    }

    #[test]
    fn fixture_matches_tools_to_success_or_failure_by_id() {
        let (updates, _) = fixture_updates();
        assert_eq!(
            updates
                .iter()
                .filter(|update| matches!(
                    update,
                    Update::ItemStarted {
                        item: Item::Tool {
                            state: ToolState::Running,
                            ..
                        },
                        ..
                    }
                ))
                .count(),
            2
        );
        assert!(updates.iter().any(|update| matches!(update, Update::ItemDone { item: Item::Tool { id, input, output, state: ToolState::Done, .. }, .. }
            if id == "turn-tool_fixture" && input == r#"{"range":1}"# && output == "One clip.")));
        assert!(updates.iter().any(|update| matches!(update, Update::ItemDone { item: Item::Tool { id, output, state: ToolState::Failed, .. }, .. }
            if id == "turn-tool_failed" && output == "No clip.")));
    }

    #[test]
    fn fixture_errors_are_reported_and_unknown_messages_are_ignored() {
        let mut events = TurnEvents::new("thread", "turn");
        let updates = RefCell::new(Vec::new());
        let emit = |update| updates.borrow_mut().push(update);
        for line in [
            "not JSON",
            r#"{"type":"unknown"}"#,
            r#"{"type":"stream_event","event":{"type":"unknown"}}"#,
        ] {
            events.read(line, &emit);
        }
        assert!(updates.borrow().is_empty());
        events.read(
            r#"{"type":"assistant","error":"authentication_failed"}"#,
            &emit,
        );
        events.read(
            r#"{"type":"result","subtype":"success","is_error":true}"#,
            &emit,
        );
        assert!(
            matches!(&updates.borrow()[0], Update::Trouble { thread: Some(thread), message } if thread == "thread" && message.contains("authentication failed"))
        );
        assert!(
            events
                .result
                .as_ref()
                .and_then(|result| result.failed.as_deref())
                .is_some_and(|message| message.contains("authentication failed"))
        );
    }

    #[cfg(unix)]
    mod process {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::mpsc::{self, Receiver};
        use std::time::Duration;

        use super::*;

        fn script(dir: &Temp, body: &str) -> PathBuf {
            let script = dir.0.join("fake-claude");
            std::fs::write(&script, format!("#!/bin/sh\nset -eu\n{body}\n")).expect("script");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
                .expect("executable script");
            script
        }

        fn listen(program: &Path, dir: &Temp) -> (ClaudeBackend, Receiver<Update>) {
            let (sender, receiver) = mpsc::channel();
            let backend = ClaudeBackend::new(
                program.into(),
                dir.0.join("history"),
                Arc::new(move |event| {
                    if let Event::Update(update) = event {
                        let _ = sender.send(update);
                    }
                }),
            );
            (backend, receiver)
        }

        fn call<T: Send + 'static>(start: impl FnOnce(Done<T>)) -> T {
            let (sender, receiver) = mpsc::channel();
            start(Box::new(move |result| {
                let _ = sender.send(result);
            }));
            receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("callback")
                .expect("successful operation")
        }

        fn finish(receiver: &Receiver<Update>) -> Vec<Update> {
            let mut events = Vec::new();
            loop {
                let update = receiver
                    .recv_timeout(Duration::from_secs(5))
                    .expect("update");
                let ended = matches!(update, Update::TurnEnded { .. });
                events.push(update);
                if ended {
                    return events;
                }
            }
        }

        #[test]
        fn offline_turn_emits_updates_and_reuses_the_cli_id_after_restart() {
            let dir = Temp::new();
            let program = script(
                &dir,
                &format!(
                    r#"
IFS= read -r message
case "$message" in
  *First*) ;;
  *Next*)
    case " $* " in *' --resume {SESSION_ID} '*) ;; *) exit 3;; esac
    case "$message" in *'{SESSION_ID}'*) ;; *) exit 4;; esac ;;
  *) exit 2 ;;
esac
cat <<'END_FIXTURE'
{TURN}END_FIXTURE
"#
                ),
            );
            let (backend, updates) = listen(&program, &dir);
            let session = call(|done| backend.start_session(setup(&dir.0), done));
            let turn = call(|done| {
                backend.send_turn(
                    &session.id,
                    vec![Input::Text("First question".into())],
                    None,
                    None,
                    done,
                )
            });
            let events = finish(&updates);
            assert!(
                matches!(&events[0], Update::TurnStarted { thread, turn: id } if id == &turn && thread == &session.id)
            );
            assert!(events.iter().any(
                |update| matches!(update, Update::Named { name, .. } if name == "First question")
            ));
            assert!(
                matches!(events.last(), Some(Update::TurnEnded { status, error: None, .. }) if status == "completed")
            );
            drop(backend);

            let (backend, updates) = listen(&program, &dir);
            let opened = call(|done| backend.resume_session(setup(&dir.0), &session.id, done));
            assert!(
                matches!(&opened.history[0], Item::User { text, .. } if text == "First question")
            );
            assert!(
                opened
                    .history
                    .iter()
                    .any(|item| matches!(item, Item::Agent { text, .. } if text == "Hi!"))
            );
            assert!(opened.history.iter().any(|item| matches!(
                item,
                Item::Tool {
                    state: ToolState::Failed,
                    ..
                }
            )));
            call(|done| {
                backend.send_turn(
                    &opened.id,
                    vec![Input::Text("Next question".into())],
                    None,
                    None,
                    done,
                )
            });
            assert!(
                matches!(finish(&updates).last(), Some(Update::TurnEnded { status, error: None, .. }) if status == "completed")
            );
        }

        #[test]
        fn failed_result_and_interrupt_end_the_correct_turn() {
            let dir = Temp::new();
            let program = script(
                &dir,
                r#"
IFS= read -r message
case "$message" in
  *Fail*) printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true}' ;;
  *) touch worker-ready; exec sleep 30 ;;
esac
"#,
            );
            let (backend, updates) = listen(&program, &dir);
            let session = call(|done| backend.start_session(setup(&dir.0), done));
            call(|done| {
                backend.send_turn(
                    &session.id,
                    vec![Input::Text("Fail".into())],
                    None,
                    None,
                    done,
                )
            });
            assert!(
                matches!(finish(&updates).last(), Some(Update::TurnEnded { status, error: Some(error), .. }) if status == "failed" && error.contains("turn limit"))
            );
            let turn = call(|done| {
                backend.send_turn(
                    &session.id,
                    vec![Input::Text("Wait".into())],
                    None,
                    None,
                    done,
                )
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !dir.0.join("worker-ready").is_file() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "mock Claude process started"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            call(|done| backend.interrupt(&session.id, &turn, done));
            assert!(
                matches!(finish(&updates).last(), Some(Update::TurnEnded { status, error: None, turn: ended, .. }) if status == "interrupted" && ended == &turn)
            );
        }
    }
}
