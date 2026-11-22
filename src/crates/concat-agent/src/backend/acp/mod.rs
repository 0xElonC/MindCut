// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The Agent Client Protocol: JSON-RPC lines over a program's standard
//! input and output, for any agent that speaks it.
//!
//! This is a protocol, not a product. What differs between the programs
//! that speak it (how they are started, where the instructions go, which
//! models are wanted, what their tools are called) is an [`AcpProfile`],
//! written once for each program in its own module (`antigravity`), and
//! nothing here names one.
//!
//! What the protocol does not give is a conversation's history, so a
//! backend on it is `remote_history: false` and the app keeps its own
//! record (`store::transcript`). A session taken up again replays what was
//! said before; that is dropped, since the record already holds it.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Backend, BackendId, Client, Connected, Descriptor, Done, Event, Turn};
use crate::peer::{Incoming, Peer, Spec, Streams, Writer};
use crate::vocab::{Item, Skill, ThreadSetup, Update};

pub mod wire;

/// How one program that speaks the protocol differs from another.
#[derive(Debug, Clone, Copy)]
pub struct AcpProfile {
    /// What follows the program's name on the command line.
    pub arguments: &'static [&'static str],
    /// Where in a session's `_meta` the program takes instructions to add
    /// to its own: `["vendor", "systemPrompt", "append"]`. Empty for none.
    pub system_prompt_meta: &'static [&'static str],
    /// Whether a model it lists is one to offer.
    pub model_filter: fn(&str) -> bool,
    /// A tool's name as the chat should show it, from the name the program
    /// gives (which may be marked as an MCP server's).
    pub tool_name: for<'a> fn(&'a str) -> &'a str,
}

/// What is shared between the caller's threads and the one that reads the
/// program.
struct Shared {
    on_event: Box<dyn Fn(Event) + Send + Sync>,
    profile: AcpProfile,
    state: Mutex<State>,
    /// Numbers a turn, which the protocol does not.
    turns: AtomicU64,
}

#[derive(Default)]
struct State {
    readers: HashMap<String, wire::Reader>,
    /// Sessions being taken up again: what they replay is not new.
    loading: HashSet<String>,
    /// The model and depth a session was last set to.
    chosen: HashMap<String, (Option<String>, Option<String>)>,
    /// Numbers the items of words, across every session.
    seq: u64,
}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn emit(&self, update: Update) {
        (self.on_event)(Event::Update(update));
    }
}

/// Starts `program` and takes up the protocol.
pub fn connect(
    descriptor: &'static Descriptor,
    profile: &AcpProfile,
    program: &Path,
    client: &Client,
    on_event: impl Fn(Event) + Send + Sync + 'static,
) -> Result<Connected, String> {
    let shared = shared(profile, on_event);
    let peer = Peer::spawn(program, spec(descriptor, profile), answering(&shared))?;
    finish(descriptor, peer, shared, client)
}

/// [`connect`] over `streams` that stand in for the program.
pub fn connect_over(
    descriptor: &'static Descriptor,
    profile: &AcpProfile,
    streams: Streams,
    client: &Client,
    on_event: impl Fn(Event) + Send + Sync + 'static,
) -> Result<Connected, String> {
    let shared = shared(profile, on_event);
    let peer = Peer::attach(streams, spec(descriptor, profile), answering(&shared));
    finish(descriptor, peer, shared, client)
}

fn spec(descriptor: &Descriptor, profile: &AcpProfile) -> Spec {
    Spec {
        name: descriptor.name,
        arguments: profile.arguments,
        probe: &[],
        jsonrpc: true,
    }
}

fn shared(profile: &AcpProfile, on_event: impl Fn(Event) + Send + Sync + 'static) -> Arc<Shared> {
    Arc::new(Shared {
        on_event: Box::new(on_event),
        profile: *profile,
        state: Mutex::new(State {
            // Items are named for the record and found again by name, so a
            // name must not come round again in another run: the count
            // starts at the time, which does not.
            seq: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_millis() as u64)
                .unwrap_or(0),
            ..State::default()
        }),
        turns: AtomicU64::new(0),
    })
}

/// What is done with what the program says unasked: events are read and
/// passed on, except what a session taken up again replays, and what asks
/// leave to run is let run, since the person let the agent act on its own.
fn answering(shared: &Arc<Shared>) -> impl Fn(Incoming, &Writer) + Send + 'static {
    let shared = Arc::clone(shared);
    move |incoming, writer| match incoming {
        Incoming::Notification { method, params } if method == "session/update" => {
            let (Some(session), Some(update)) = (
                params.get("sessionId").and_then(Value::as_str),
                params.get("update"),
            ) else {
                return;
            };
            let updates = {
                let mut state = shared.state();
                if state.loading.contains(session) {
                    return;
                }
                let State { readers, seq, .. } = &mut *state;
                readers
                    .entry(session.to_owned())
                    .or_default()
                    .read(seq, session, update, &shared.profile)
            };
            for update in updates {
                shared.emit(update);
            }
        }
        Incoming::Notification { .. } => {}
        Incoming::Request { id, method, params } => {
            let answer = if method == "session/request_permission" {
                wire::permission_answer(&params)
            } else {
                json!({})
            };
            writer.respond(id, answer);
        }
        Incoming::Closed(reason) => (shared.on_event)(Event::Closed(reason)),
    }
}

fn finish(
    descriptor: &'static Descriptor,
    peer: Peer,
    shared: Arc<Shared>,
    client: &Client,
) -> Result<Connected, String> {
    peer.call_blocking(
        "initialize",
        wire::initialize_params(client),
        Duration::from_secs(20),
    )?;
    // The models and depths are on offer in a session, so one is opened to
    // read them and closed again. A session in which nothing was said is
    // not kept by the program.
    let folder = std::env::temp_dir().to_string_lossy().into_owned();
    let opened = peer.call_blocking(
        "session/new",
        json!({ "cwd": folder, "mcpServers": [] }),
        Duration::from_secs(30),
    )?;
    let models = wire::read_models(&opened, &shared.profile);
    if let Ok(session) = wire::read_session(&opened) {
        peer.call("session/close", json!({ "sessionId": session }), |_| {});
    }
    Ok(Connected {
        backend: Arc::new(Acp {
            id: descriptor.id,
            peer: Arc::new(peer),
            shared,
        }),
        models,
    })
}

/// A connected program that speaks the protocol.
struct Acp {
    id: BackendId,
    peer: Arc<Peer>,
    shared: Arc<Shared>,
}

/// What a turn needs done before the message goes, then the message and the
/// end of the turn. Each step waits for the one before: the program takes
/// its requests as they come, and a depth set after the message began would
/// not be the one the message was answered at.
struct Sending {
    peer: Weak<Peer>,
    shared: Arc<Shared>,
    thread: String,
    turn: String,
    /// `(option, value)` still to be set.
    changes: Vec<(&'static str, String)>,
    prompt: Value,
}

impl Sending {
    fn run(mut self) {
        let Some(peer) = self.peer.upgrade() else { return };
        if self.changes.is_empty() {
            let thread = self.thread.clone();
            let shared = Arc::clone(&self.shared);
            let turn = self.turn.clone();
            peer.call("session/prompt", self.prompt, move |reply| {
                let (status, error) = wire::turn_end(&reply);
                shared.emit(Update::TurnEnded { thread, turn, status, error });
            });
            return;
        }
        let (option, value) = self.changes.remove(0);
        let params = wire::config_params(&self.thread, option, &value);
        peer.call("session/set_config_option", params, move |reply| {
            match reply {
                Ok(_) => {
                    let mut state = self.shared.state();
                    let chosen = state.chosen.entry(self.thread.clone()).or_default();
                    if option == "model" {
                        chosen.0 = Some(value);
                    } else {
                        chosen.1 = Some(value);
                    }
                }
                // The message goes anyway, answered as the program was set.
                Err(error) => log::warn!("agent: setting {option} failed: {error}"),
            }
            self.run();
        });
    }
}

impl Backend for Acp {
    fn id(&self) -> BackendId {
        self.id
    }

    fn start(&self, setup: &ThreadSetup, done: Done<String>) {
        let params = wire::new_params(setup, &self.shared.profile);
        self.peer.call("session/new", params, move |reply| {
            done(reply.and_then(|result| wire::read_session(&result)));
        });
    }

    fn resume(&self, thread: &str, setup: &ThreadSetup, done: Done<Vec<Item>>) {
        let params = wire::load_params(setup, &self.shared.profile, thread);
        // What the session replays arrives before the answer, on the thread
        // that reads the program: it is dropped until the answer is in.
        {
            let mut state = self.shared.state();
            state.loading.insert(thread.to_owned());
        }
        let shared = Arc::clone(&self.shared);
        let thread = thread.to_owned();
        self.peer.call("session/load", params, move |reply| {
            {
                let mut state = shared.state();
                state.loading.remove(&thread);
                state.readers.remove(&thread);
                // What the session is set to is not known after a load.
                state.chosen.remove(&thread);
            }
            // The history is the app's own record, not the program's.
            done(reply.map(|_| Vec::new()));
        });
    }

    fn send(&self, turn: Turn, done: Done<Option<String>>) {
        let id = format!("turn-{}", self.shared.turns.fetch_add(1, Ordering::Relaxed) + 1);
        let mut changes = Vec::new();
        {
            let state = self.shared.state();
            let (model, effort) = state.chosen.get(&turn.thread).cloned().unwrap_or_default();
            if let Some(wanted) = turn.model.as_ref().filter(|wanted| Some(*wanted) != model.as_ref()) {
                changes.push(("model", wanted.clone()));
            }
            if let Some(wanted) = turn.effort.as_ref().filter(|wanted| Some(*wanted) != effort.as_ref()) {
                changes.push(("thought_level", wanted.clone()));
            }
        }
        // The protocol has no word for a turn beginning: this is it.
        self.shared.emit(Update::TurnStarted {
            thread: turn.thread.clone(),
            turn: id.clone(),
        });
        done(Ok(Some(id.clone())));
        let blocks = wire::prompt_blocks(&turn.inputs, wire::read_image);
        Sending {
            peer: Arc::downgrade(&self.peer),
            shared: Arc::clone(&self.shared),
            prompt: wire::prompt_params(&turn.thread, blocks),
            thread: turn.thread,
            turn: id,
            changes,
        }
        .run();
    }

    fn interrupt(&self, thread: &str, _turn: &str) {
        self.peer.notify("session/cancel", wire::cancel_params(thread));
    }

    fn rename(&self, _thread: &str, _title: &str) {}

    fn archive(&self, thread: &str) {
        self.peer
            .call("session/close", json!({ "sessionId": thread }), |_| {});
    }

    fn skills(&self, _cwd: &str, done: Done<Vec<Skill>>) {
        done(Ok(Vec::new()));
    }
}

#[cfg(test)]
mod tests {
    //! The protocol through the shared checks, against a stand-in that
    //! speaks it over pipes, and the parts that are the protocol's own.

    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    use super::*;
    use crate::backend::{Caps, Install, Protocol};
    use crate::testing::{self, Harness};

    const PROFILE: AcpProfile = AcpProfile {
        arguments: &[],
        system_prompt_meta: &["vendor", "systemPrompt", "append"],
        model_filter: |id| id.starts_with("gemini-"),
        tool_name: |name| name.strip_prefix("mcp_eloncut_").unwrap_or(name),
    };

    static DESCRIPTOR: Descriptor = Descriptor {
        id: BackendId("fake-acp"),
        name: "Fake",
        locate: || None,
        install: Install { command: None, url: "" },
        protocol: Protocol::Acp(PROFILE),
        caps: Caps {
            efforts: true,
            skills: false,
            remote_history: false,
            rename: false,
            setup_on_resume: true,
        },
    };

    fn client() -> Client {
        Client { name: "test".into(), title: "Test".into(), version: "0".into() }
    }

    /// What the fake was asked and answered, for a test to look at.
    #[derive(Default)]
    struct Seen {
        /// `(method, params)` of every request and notification, in order.
        calls: Vec<(String, Value)>,
        /// What the client answered when asked leave to run something.
        granted: Vec<Value>,
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
            let update = |session: &str, update: Value| {
                json!({ "method": "session/update", "params": { "sessionId": session, "update": update } })
            };
            // The prompt that waits for a cancel, by its id.
            let mut waiting: Option<Value> = None;
            loop {
                if end.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(line) = line_reader.recv_timeout(Duration::from_millis(10)) else {
                    continue;
                };
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                assert_eq!(message["jsonrpc"], "2.0", "lines carry the version: {line}");
                let id = message.get("id").cloned();
                let Some(method) = message.get("method").and_then(Value::as_str) else {
                    if let Some(result) = message.get("result") {
                        seen.lock().unwrap().granted.push(result.clone());
                    }
                    continue;
                };
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                seen.lock().unwrap().calls.push((method.to_owned(), params.clone()));
                let reply = |result: Value| json!({ "id": id, "result": result });
                match method {
                    "initialize" => say(reply(json!({ "protocolVersion": 1, "agentCapabilities": {} }))),
                    "session/new" => say(reply(json!({ "sessionId": "s1", "configOptions": options() }))),
                    "session/load" => {
                        let session = params["sessionId"].as_str().unwrap_or("").to_owned();
                        say(update(&session, json!({ "sessionUpdate": "user_message_chunk",
                            "content": { "type": "text", "text": "the past" } })));
                        say(update(&session, json!({ "sessionUpdate": "agent_message_chunk",
                            "content": { "type": "text", "text": "what was said" } })));
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
                            _ => {
                                say(update(&session, json!({ "sessionUpdate": "something_new" })));
                                say(json!({ "id": "ask-1", "method": "session/request_permission",
                                    "params": { "sessionId": session, "options": [
                                        { "optionId": "deny", "kind": "reject_once" },
                                        { "optionId": "allow", "kind": "allow_once" } ] } }));
                                say(update(&session, json!({ "sessionUpdate": "agent_thought_chunk",
                                    "content": { "type": "text", "text": "thinking" } })));
                                say(update(&session, json!({ "sessionUpdate": "agent_message_chunk",
                                    "content": { "type": "text", "text": "Hel" } })));
                                say(update(&session, json!({ "sessionUpdate": "tool_call", "toolCallId": "c1",
                                    "name": "mcp_eloncut_concat_version", "status": "in_progress",
                                    "rawInput": { "toolSummary": "Check the version" } })));
                                say(update(&session, json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1",
                                    "name": "mcp_eloncut_concat_version", "status": "completed" })));
                                say(update(&session, json!({ "sessionUpdate": "agent_message_chunk",
                                    "content": { "type": "text", "text": "lo" } })));
                                say(reply(json!({ "stopReason": "end_turn" })));
                            }
                        }
                    }
                    _ => say(reply(json!({}))),
                }
            }
        });
    }

    struct Rig {
        connected: Connected,
        events: mpsc::Receiver<Event>,
        end: Arc<AtomicBool>,
        seen: Arc<Mutex<Seen>>,
    }

    fn rig() -> Rig {
        let (requests_in, requests_out) = std::io::pipe().unwrap();
        let (replies_in, replies_out) = std::io::pipe().unwrap();
        let end = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Seen::default()));
        fake(requests_in, replies_out, Arc::clone(&end), Arc::clone(&seen));
        let (send, events) = mpsc::channel();
        let send = Mutex::new(send);
        let connected = connect_over(
            &DESCRIPTOR,
            &PROFILE,
            Streams {
                input: Box::new(requests_out),
                output: Box::new(replies_in),
                errors: None,
                child: None,
                program: None,
            },
            &client(),
            move |event| {
                let _ = send.lock().unwrap().send(event);
            },
        )
        .expect("connects");
        Rig { connected, events, end, seen }
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
        receive.recv_timeout(Duration::from_secs(5)).expect("answered")
    }

    fn setup() -> ThreadSetup {
        testing::setup()
    }

    fn say(thread: &str, text: &str, model: Option<&str>, effort: Option<&str>) -> Turn {
        Turn {
            thread: thread.into(),
            inputs: vec![crate::vocab::Input::Text(text.into())],
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        }
    }

    #[test]
    fn the_protocol_does_what_a_backend_owes_the_window() {
        let rig = rig();
        // The models on offer are read when connecting, and filtered.
        let ids: Vec<&str> = rig.connected.models.list.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gemini-fake", "gemini-other"]);
        assert_eq!(rig.connected.models.default_model.as_deref(), Some("gemini-fake"));
        assert_eq!(rig.connected.models.default_effort.as_deref(), Some("low"));
        assert_eq!(rig.connected.models.list[0].efforts, ["low", "high"]);
        let end = Arc::clone(&rig.end);
        testing::conformance(Harness {
            connected: rig.connected,
            events: rig.events,
            end_program: Box::new(move || end.store(true, Ordering::Relaxed)),
        });
        // What asked to run was let run, with the option that allows it.
        let granted = rig.seen.lock().unwrap().granted.clone();
        assert!(
            granted.iter().any(|answer| answer["outcome"]["optionId"] == "allow"),
            "the request to run something is granted: {granted:?}"
        );
    }

    #[test]
    fn a_session_opened_to_read_the_models_is_closed_again() {
        let rig = rig();
        std::thread::sleep(Duration::from_millis(100));
        let calls: Vec<String> =
            rig.seen.lock().unwrap().calls.iter().map(|(method, _)| method.clone()).collect();
        assert_eq!(calls, ["initialize", "session/new", "session/close"]);
    }

    #[test]
    fn a_session_is_started_in_the_folder_with_the_tools_and_instructions() {
        let rig = rig();
        let backend = rig.connected.backend.as_ref();
        let thread = ask(|done| backend.start(&setup(), done)).unwrap();
        assert_eq!(thread, "s1");
        let calls = rig.seen.lock().unwrap().calls.clone();
        let (_, params) = calls.iter().rfind(|(method, _)| method == "session/new").unwrap();
        assert_eq!(params["cwd"], "/tmp");
        assert_eq!(params["mcpServers"][0]["command"], "/app/concat");
        assert_eq!(params["_meta"]["vendor"]["systemPrompt"]["append"], "edit video");
    }

    #[test]
    fn the_model_and_depth_are_set_once_and_before_the_message() {
        let rig = rig();
        let backend = rig.connected.backend.as_ref();
        let thread = ask(|done| backend.start(&setup(), done)).unwrap();
        ask(|done| backend.send(say(&thread, "hello", Some("gemini-other"), Some("high")), done)).unwrap();
        wait_for(&rig.events, |u| matches!(u, Update::TurnEnded { .. }));
        // The same choices again set nothing; a new depth sets only that.
        ask(|done| backend.send(say(&thread, "hello", Some("gemini-other"), Some("low")), done)).unwrap();
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
                    "session/set_config_option" => format!("{}={}", params["configId"], params["value"]),
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
            !seen.iter().any(|update| matches!(update, Update::AgentWords { words, .. } if words == "what was said")),
            "the replay is not heard: {seen:?}"
        );
        assert!(seen.iter().any(|update| matches!(update, Update::AgentWords { words, .. } if words == "Hel")));
        // The taken-up session was given the folder and the tools again.
        let calls = rig.seen.lock().unwrap().calls.clone();
        let (_, params) = calls.iter().find(|(method, _)| method == "session/load").unwrap();
        assert_eq!(params["sessionId"], "s1");
        assert_eq!(params["mcpServers"][0]["name"], "eloncut");
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
        assert_ne!(words[0].0, words[1].0, "the tool call ends the first message");
        assert!(seen.iter().any(|update| matches!(update, Update::ThoughtWords { words, .. } if words == "thinking")));
        assert!(seen.iter().any(|update| matches!(
            update,
            Update::ItemStarted { item: Item::Tool { summary, .. }, .. } if summary == "Check the version"
        )));
    }
}
