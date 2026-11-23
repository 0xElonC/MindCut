// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The protocol against a stand-in agent that speaks it over pipes, and the
//! parts that are the protocol's own.

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use std::path::PathBuf;

use super::*;
use crate::backend::NamedMcpServer;
use crate::peer::Streams;
use crate::wire::{Input, McpServer, SERVER_NAME, ToolState};

/// A connected backend and the models it offered, as the tests read them.
struct Connected {
    backend: Arc<AcpBackend>,
    models: Models,
}

struct Turn {
    thread: String,
    inputs: Vec<Input>,
    model: Option<String>,
    effort: Option<String>,
}

fn session_setup(setup: &ThreadSetup) -> SessionSetup {
    SessionSetup {
        cwd: setup.cwd.clone(),
        model: setup.model.clone(),
        instructions: setup.instructions.clone(),
        mcp: NamedMcpServer {
            name: SERVER_NAME.into(),
            server: setup.mcp.clone(),
        },
    }
}

/// The calls a window makes, in the words the tests are written in.
trait Window {
    fn start(&self, setup: &ThreadSetup, done: Done<String>);
    fn resume(&self, thread: &str, setup: &ThreadSetup, done: Done<Vec<Item>>);
    fn send(&self, turn: Turn, done: Done<Option<String>>);
    fn stop(&self, thread: &str, turn: &str);
}

impl Window for AcpBackend {
    fn start(&self, setup: &ThreadSetup, done: Done<String>) {
        self.start_session(
            session_setup(setup),
            Box::new(move |reply| done(reply.map(|session| session.id))),
        );
    }

    fn resume(&self, thread: &str, setup: &ThreadSetup, done: Done<Vec<Item>>) {
        self.resume_session(
            session_setup(setup),
            thread,
            Box::new(move |reply| done(reply.map(|session| session.history))),
        );
    }

    fn send(&self, turn: Turn, done: Done<Option<String>>) {
        self.send_turn(
            &turn.thread,
            turn.inputs,
            turn.model,
            turn.effort,
            Box::new(move |reply| done(reply.map(Some))),
        );
    }

    fn stop(&self, thread: &str, turn: &str) {
        self.interrupt(thread, turn, Box::new(|_| {}));
    }
}

fn folder(_name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "concat-acp-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

const PROFILE: AcpProfile = AcpProfile {
    arguments: &[],
    system_prompt_meta: &["vendor", "systemPrompt", "append"],
    model_filter: |id| id.starts_with("gemini-"),
    tool_name: |name| name.strip_prefix("mcp_eloncut_").unwrap_or(name),
};

fn client() -> Client {
    Client {
        name: "test".into(),
        title: "Test".into(),
        version: "0".into(),
    }
}

/// What the fake was asked and answered, for a test to look at.
#[derive(Default)]
struct Seen {
    /// `(method, params)` of every request and notification, in order.
    calls: Vec<(String, Value)>,
    /// What the client answered when asked leave to run something.
    granted: Vec<Value>,
}

struct Pause {
    method: &'static str,
    arrived: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

fn options() -> Value {
    json!([
        { "id": "model", "currentValue": "gemini-fake", "options": [
            { "value": "gemini-fake", "name": "Gemini Fake" },
            { "value": "gemini-other", "name": "Gemini Other" },
            { "value": "claude-x", "name": "Claude X" } ] },
        { "id": "thought_level", "currentValue": "low", "options": [
            { "value": "low", "name": "Low" }, { "value": "high", "name": "High" } ] },
    ])
}

/// An agent that follows the script in `testing`. The words `replay` in
/// a message make a loaded session say its past first.
fn fake(
    requests: std::io::PipeReader,
    mut replies: std::io::PipeWriter,
    end: Arc<AtomicBool>,
    seen: Arc<Mutex<Seen>>,
    mut pause: Option<Pause>,
) {
    std::thread::spawn(move || {
        let (lines, line_reader) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(requests).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let mut say = |value: Value| {
            let mut value = value;
            value["jsonrpc"] = json!("2.0");
            let _ = writeln!(replies, "{value}");
            let _ = replies.flush();
        };
        let update = |session: &str, update: Value| json!({ "method": "session/update", "params": { "sessionId": session, "update": update } });
        // The prompt that waits for a cancel, by its id.
        let mut waiting: Option<Value> = None;
        loop {
            if end.load(Ordering::Relaxed) {
                return;
            }
            let Ok(line) = line_reader.recv_timeout(Duration::from_millis(10)) else {
                continue;
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            assert_eq!(message["jsonrpc"], "2.0", "lines carry the version: {line}");
            let id = message.get("id").cloned();
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                if let Some(result) = message.get("result") {
                    seen.lock().unwrap().granted.push(result.clone());
                }
                continue;
            };
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            seen.lock()
                .unwrap()
                .calls
                .push((method.to_owned(), params.clone()));
            if pause.as_ref().is_some_and(|pause| pause.method == method) {
                let pause = pause.take().unwrap();
                pause.arrived.send(()).unwrap();
                pause.release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            let reply = |result: Value| json!({ "id": id, "result": result });
            match method {
                "initialize" => say(reply(
                    json!({ "protocolVersion": 1, "agentCapabilities": {} }),
                )),
                "session/new" => say(reply(
                    json!({ "sessionId": "s1", "configOptions": options() }),
                )),
                "session/load" => {
                    let session = params["sessionId"].as_str().unwrap_or("").to_owned();
                    say(update(
                        &session,
                        json!({ "sessionUpdate": "user_message_chunk",
                        "content": { "type": "text", "text": "the past" } }),
                    ));
                    say(update(
                        &session,
                        json!({ "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": "what was said" } }),
                    ));
                    say(reply(json!({ "configOptions": options() })));
                }
                "session/set_config_option" => say(reply(json!({ "configOptions": options() }))),
                "session/cancel" => {
                    if let Some(prompt_id) = waiting.take() {
                        say(json!({ "id": prompt_id, "result": { "stopReason": "cancelled" } }));
                    }
                }
                "session/prompt" => {
                    let words = params["prompt"][0]["text"].as_str().unwrap_or("");
                    let session = params["sessionId"].as_str().unwrap_or("s1").to_owned();
                    match words {
                        "stall" => {}
                        "slow" => waiting = id,
                        "plain" => {
                            say(update(
                                &session,
                                json!({ "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": "plain answer" } }),
                            ));
                            say(reply(json!({ "stopReason": "end_turn" })));
                        }
                        _ => {
                            say(update(
                                &session,
                                json!({ "sessionUpdate": "something_new" }),
                            ));
                            say(
                                json!({ "id": "ask-1", "method": "session/request_permission",
                                "params": { "sessionId": session, "options": [
                                    { "optionId": "deny", "kind": "reject_once" },
                                    { "optionId": "allow", "kind": "allow_once" } ] } }),
                            );
                            say(update(
                                &session,
                                json!({ "sessionUpdate": "agent_thought_chunk",
                                "content": { "type": "text", "text": "thinking" } }),
                            ));
                            say(update(
                                &session,
                                json!({ "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": "Hel" } }),
                            ));
                            say(update(
                                &session,
                                json!({ "sessionUpdate": "tool_call", "toolCallId": "c1",
                                "name": "mcp_eloncut_concat_version", "status": "in_progress",
                                "rawInput": { "toolSummary": "Check the version", "detail": true } }),
                            ));
                            say(update(
                                &session,
                                json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1",
                                "status": "completed" }),
                            ));
                            say(update(
                                &session,
                                json!({ "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": "lo" } }),
                            ));
                            say(reply(json!({ "stopReason": "end_turn" })));
                        }
                    }
                }
                _ => say(reply(json!({}))),
            }
        }
    });
}

#[allow(dead_code)]
struct Rig {
    connected: Connected,
    events: mpsc::Receiver<Event>,
    end: Arc<AtomicBool>,
    seen: Arc<Mutex<Seen>>,
}

fn rig() -> Rig {
    rig_with_pause(None)
}

fn paused_rig(method: &'static str) -> (Rig, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (arrived, notice) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    (
        rig_with_pause(Some(Pause {
            method,
            arrived,
            release: proceed,
        })),
        notice,
        release,
    )
}

fn rig_with_pause(pause: Option<Pause>) -> Rig {
    let (requests_in, requests_out) = std::io::pipe().unwrap();
    let (replies_in, replies_out) = std::io::pipe().unwrap();
    let end = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Seen::default()));
    fake(
        requests_in,
        replies_out,
        Arc::clone(&end),
        Arc::clone(&seen),
        pause,
    );
    let (send, events) = mpsc::channel();
    let send = Mutex::new(send);
    let shared = shared(
        &PROFILE,
        Transcripts::in_folder(&folder("rig")),
        Arc::new(move |event| {
            let _ = send.lock().unwrap().send(event);
        }),
    );
    let peer = Peer::attach(
        Streams {
            input: Box::new(requests_out),
            output: Box::new(replies_in),
            errors: None,
            child: None,
            program: None,
        },
        spec("Fake", &PROFILE),
        answering(&shared),
    );
    let connected = finish(Provider::Antigravity, peer, shared, &client()).expect("connects");
    let connected = Connected {
        models: connected.models.clone(),
        backend: Arc::new(connected),
    };
    Rig {
        connected,
        events,
        end,
        seen,
    }
}

fn wait_for(events: &mpsc::Receiver<Event>, wanted: impl Fn(&Update) -> bool) -> Vec<Update> {
    let mut seen = Vec::new();
    loop {
        match events.recv_timeout(Duration::from_secs(5)) {
            Ok(Event::Update(update)) => {
                let done = wanted(&update);
                seen.push(update);
                if done {
                    return seen;
                }
            }
            other => panic!("waiting: {other:?} after {seen:?}"),
        }
    }
}

fn ask<T: Send + 'static>(call: impl FnOnce(Done<T>)) -> Result<T, String> {
    let (send, receive) = mpsc::channel();
    call(Box::new(move |reply| {
        let _ = send.send(reply);
    }));
    receive
        .recv_timeout(Duration::from_secs(5))
        .expect("answered")
}

fn setup() -> ThreadSetup {
    ThreadSetup {
        cwd: "/tmp".into(),
        model: None,
        instructions: "edit video".into(),
        mcp: McpServer {
            command: "/app/concat".into(),
            args: vec!["--mcp".into(), "window".into()],
            env: vec![("CONCAT_API_TOKEN".into(), "t".into())],
        },
    }
}

fn say(thread: &str, text: &str, model: Option<&str>, effort: Option<&str>) -> Turn {
    Turn {
        thread: thread.into(),
        inputs: vec![crate::wire::Input::Text(text.into())],
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
    }
}

#[test]
fn a_turn_is_streamed_let_run_and_written_down() {
    let rig = rig();
    // The models on offer are read when connecting, and filtered.
    let ids: Vec<&str> = rig
        .connected
        .models
        .list
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, ["gemini-fake", "gemini-other"]);
    assert_eq!(
        rig.connected.models.default_model.as_deref(),
        Some("gemini-fake")
    );
    assert_eq!(rig.connected.models.default_effort.as_deref(), Some("low"));
    assert_eq!(rig.connected.models.list[0].efforts, ["low", "high"]);

    let backend = rig.connected.backend.as_ref();
    let thread = ask(|done| backend.start(&setup(), done)).unwrap();
    ask(|done| backend.send(say(&thread, "hello", None, None), done)).unwrap();
    let updates = wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
    assert!(matches!(updates.first(), Some(Update::TurnStarted { .. })));
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, Update::AgentWords { .. }))
    );
    assert!(updates.iter().any(|u| matches!(
        u,
        Update::ItemDone { item: Item::Tool { name, state: ToolState::Done, .. }, .. }
            if name == "concat_version"
    )));
    assert!(matches!(
        updates.last(),
        Some(Update::TurnEnded { status, .. }) if status == "completed"
    ));
    // What asked to run was let run, with the option that allows it.
    let granted = rig.seen.lock().unwrap().granted.clone();
    assert!(
        granted
            .iter()
            .any(|answer| answer["outcome"]["optionId"] == "allow"),
        "the request to run something is granted: {granted:?}"
    );
    // The program keeps no history, so the app's own record gives it back:
    // what the person said, then the answer, in the order they came.
    let history = ask(|done| backend.resume(&thread, &setup(), done)).unwrap();
    assert!(matches!(history.first(), Some(Item::User { text, .. }) if text == "hello"));
    // The tool ran between the two parts of the answer, so it is two items.
    let said: Vec<&str> = history
        .iter()
        .filter_map(|item| match item {
            Item::Agent { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(said, ["Hel", "lo"]);
    assert!(history.iter().any(|item| matches!(item, Item::Tool { .. })));
}

#[test]
fn a_session_opened_to_read_the_models_is_closed_again() {
    let rig = rig();
    std::thread::sleep(Duration::from_millis(100));
    let calls: Vec<String> = rig
        .seen
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|(method, _)| method.clone())
        .collect();
    assert_eq!(calls, ["initialize", "session/new", "session/close"]);
}

#[test]
fn a_session_is_started_in_the_folder_with_the_tools_and_instructions() {
    let rig = rig();
    let backend = rig.connected.backend.as_ref();
    let thread = ask(|done| backend.start(&setup(), done)).unwrap();
    assert_eq!(thread, "s1");
    let calls = rig.seen.lock().unwrap().calls.clone();
    let (_, params) = calls
        .iter()
        .rfind(|(method, _)| method == "session/new")
        .unwrap();
    assert_eq!(params["cwd"], "/tmp");
    assert_eq!(params["mcpServers"][0]["command"], "/app/concat");
    assert_eq!(
        params["_meta"]["vendor"]["systemPrompt"]["append"],
        "edit video"
    );
}

#[test]
fn the_model_and_depth_are_set_once_and_before_the_message() {
    let rig = rig();
    let backend = rig.connected.backend.as_ref();
    let thread = ask(|done| backend.start(&setup(), done)).unwrap();
    ask(|done| {
        backend.send(
            say(&thread, "hello", Some("gemini-other"), Some("high")),
            done,
        )
    })
    .unwrap();
    wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
    // The same choices again set nothing; a new depth sets only that.
    ask(|done| {
        backend.send(
            say(&thread, "hello", Some("gemini-other"), Some("low")),
            done,
        )
    })
    .unwrap();
    wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
    let calls: Vec<(String, String)> = rig
        .seen
        .lock()
        .unwrap()
        .calls
        .iter()
        .skip(3) // initialize, the reading session and its close
        .filter(|(method, _)| method != "session/new")
        .map(|(method, params)| {
            let detail = match method.as_str() {
                "session/set_config_option" => {
                    format!("{}={}", params["configId"], params["value"])
                }
                _ => String::new(),
            };
            (method.clone(), detail)
        })
        .collect();
    let methods: Vec<&str> = calls.iter().map(|(method, _)| method.as_str()).collect();
    assert_eq!(
        methods,
        [
            "session/set_config_option",
            "session/set_config_option",
            "session/prompt",
            "session/set_config_option",
            "session/prompt"
        ]
    );
    assert_eq!(calls[0].1, "\"model\"=\"gemini-other\"");
    assert_eq!(calls[1].1, "\"thought_level\"=\"high\"");
    assert_eq!(calls[3].1, "\"thought_level\"=\"low\"");
}

#[test]
fn what_a_session_replays_when_taken_up_again_is_dropped() {
    let rig = rig();
    let backend = rig.connected.backend.as_ref();
    let items = ask(|done| backend.resume("s1", &setup(), done)).unwrap();
    assert!(items.is_empty(), "the history is the app's own record");
    // The replay was before the answer; after it, words are heard.
    ask(|done| backend.send(say("s1", "hello", None, None), done)).unwrap();
    let seen = wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
    assert!(
        !seen.iter().any(
            |update| matches!(update, Update::AgentWords { words, .. } if words == "what was said")
        ),
        "the replay is not heard: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|update| matches!(update, Update::AgentWords { words, .. } if words == "Hel"))
    );
    // The taken-up session was given the folder and the tools again.
    let calls = rig.seen.lock().unwrap().calls.clone();
    let (_, params) = calls
        .iter()
        .find(|(method, _)| method == "session/load")
        .unwrap();
    assert_eq!(params["sessionId"], "s1");
    assert_eq!(params["mcpServers"][0]["name"], "eloncut");
}

#[test]
fn simultaneous_resumes_share_one_load_and_suppress_its_replay() {
    let (rig, arrived, release) = paused_rig("session/load");
    let backend = rig.connected.backend.as_ref();
    let (done, replies) = mpsc::channel();
    let first = done.clone();
    backend.resume(
        "s1",
        &setup(),
        Box::new(move |reply| first.send(reply).unwrap()),
    );
    arrived.recv_timeout(Duration::from_secs(5)).unwrap();
    backend.resume(
        "s1",
        &setup(),
        Box::new(move |reply| done.send(reply).unwrap()),
    );
    release.send(()).unwrap();
    for _ in 0..2 {
        assert!(
            replies
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }
    ask(|done| backend.send(say("s1", "plain", None, None), done)).unwrap();
    let seen = wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
    let words: Vec<_> = seen
        .iter()
        .filter_map(|u| match u {
            Update::AgentWords { words, .. } => Some(words.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(words, ["plain answer"]);
    let calls = rig.seen.lock().unwrap();
    assert_eq!(
        calls
            .calls
            .iter()
            .filter(|(method, _)| method == "session/load")
            .count(),
        1
    );
}

#[test]
fn stopping_during_configuration_skips_the_prompt() {
    let (rig, arrived, release) = paused_rig("session/set_config_option");
    let backend = rig.connected.backend.as_ref();
    let turn =
        ask(|done| backend.send(say("s1", "stall", Some("gemini-other"), Some("high")), done))
            .unwrap()
            .unwrap();
    arrived.recv_timeout(Duration::from_secs(5)).unwrap();
    backend.stop("s1", &turn);
    release.send(()).unwrap();
    let seen = wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
    assert!(
        matches!(seen.last(), Some(Update::TurnEnded { turn: id, status, .. })
        if id == &turn && status == "interrupted")
    );

    // Complete another prompt so any earlier calls have reached the fake.
    ask(|done| backend.send(say("s1", "plain", None, None), done)).unwrap();
    wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
    let seen = rig.seen.lock().unwrap();
    let methods: Vec<_> = seen
        .calls
        .iter()
        .skip(3)
        .map(|(method, _)| method.as_str())
        .collect();
    assert_eq!(methods, ["session/set_config_option", "session/prompt"]);
}

#[test]
fn consecutive_prompts_start_separate_message_items() {
    let rig = rig();
    let backend = rig.connected.backend.as_ref();
    let mut items = Vec::new();
    for _ in 0..2 {
        ask(|done| backend.send(say("s1", "plain", None, None), done)).unwrap();
        let seen = wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
        items.extend(seen.into_iter().filter_map(|u| match u {
            Update::AgentWords { item, .. } => Some(item),
            _ => None,
        }));
    }
    assert_eq!(items.len(), 2);
    assert_ne!(items[0], items[1]);
}

#[test]
fn a_thought_a_tool_and_the_answer_are_items_of_their_own() {
    let rig = rig();
    let backend = rig.connected.backend.as_ref();
    let thread = ask(|done| backend.start(&setup(), done)).unwrap();
    ask(|done| backend.send(say(&thread, "hello", None, None), done)).unwrap();
    let seen = wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
    let words: Vec<(&str, &str)> = seen
        .iter()
        .filter_map(|update| match update {
            Update::AgentWords { item, words, .. } => Some((item.as_str(), words.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(words.len(), 2);
    assert_ne!(
        words[0].0, words[1].0,
        "the tool call ends the first message"
    );
    assert!(
        seen.iter().any(
            |update| matches!(update, Update::ThoughtWords { words, .. } if words == "thinking")
        )
    );
    assert!(seen.iter().any(|update| matches!(
        update,
        Update::ItemStarted { item: Item::Tool { summary, .. }, .. } if summary == "Check the version"
    )));
    assert!(seen.iter().any(|update| matches!(
        update,
        Update::ItemDone { item: Item::Tool { name, summary, input, .. }, .. }
            if name == "concat_version" && summary == "Check the version" && input == r#"{"detail":true}"#
    )));
}
