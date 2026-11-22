// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Codex: the program run as its `app-server`, with its own models,
//! reasoning depths, sign-in and settings.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::{
    Backend, BackendId, Caps, Client, Connected, Descriptor, Done, Event, Install, Models,
    Protocol, Turn,
};
use crate::peer::{Incoming, Peer, Reply, Spec, Streams, Writer};
use crate::vocab::{Item, Skill, ThreadSetup};

pub mod locate;
pub mod wire;

/// What Codex is, to the registry.
pub static DESCRIPTOR: Descriptor = Descriptor {
    id: BackendId("codex"),
    name: "Codex",
    locate: locate::codex,
    install: Install {
        command: Some("npm i -g @openai/codex"),
        url: "https://developers.openai.com/codex/cli",
    },
    protocol: Protocol::Codex,
    caps: Caps {
        efforts: true,
        skills: true,
        remote_history: true,
        rename: true,
        setup_on_resume: true,
    },
};

/// How Codex is run and how it talks: no `"jsonrpc"` member on the lines.
pub const SPEC: Spec = Spec {
    name: "Codex",
    arguments: &["app-server"],
    probe: &["app-server", "--print-startup-config"],
    jsonrpc: false,
};

/// The opening exchange: who is asking, then the word that says the asking
/// may begin. The reply says where Codex keeps its home.
pub fn handshake(peer: &Peer, name: &str, title: &str, version: &str) -> Reply {
    let reply = peer.call_blocking(
        "initialize",
        json!({
            "clientInfo": { "name": name, "title": title, "version": version },
            "capabilities": { "experimentalApi": true },
        }),
        Duration::from_secs(20),
    )?;
    peer.notify("initialized", Value::Null);
    Ok(reply)
}

/// Starts Codex and takes up its protocol.
pub fn connect(
    program: &Path,
    client: &Client,
    on_event: impl Fn(Event) + Send + Sync + 'static,
) -> Result<Connected, String> {
    let peer = Peer::spawn(program, SPEC, answering(on_event))?;
    finish(peer, client)
}

/// [`connect`] over `streams` that stand in for the program.
pub fn connect_over(
    streams: Streams,
    client: &Client,
    on_event: impl Fn(Event) + Send + Sync + 'static,
) -> Result<Connected, String> {
    finish(Peer::attach(streams, SPEC, answering(on_event)), client)
}

/// What is done with what Codex says unasked: events are passed on, and
/// what asks to run is let run, since the person let the agent act on its
/// own.
fn answering(
    on_event: impl Fn(Event) + Send + Sync + 'static,
) -> impl Fn(Incoming, &Writer) + Send + 'static {
    move |incoming, writer| match incoming {
        Incoming::Notification { method, params } => {
            if let Some(update) = wire::read_update(&method, &params) {
                on_event(Event::Update(update));
            }
        }
        Incoming::Request { id, method, .. } => {
            writer.respond(id, wire::auto_answer(&method));
        }
        Incoming::Closed(reason) => on_event(Event::Closed(reason)),
    }
}

fn finish(peer: Peer, client: &Client) -> Result<Connected, String> {
    handshake(&peer, &client.name, &client.title, &client.version)?;
    let patience = Duration::from_secs(20);
    let list = peer
        .call_blocking("model/list", json!({}), patience)
        .map(|result| wire::read_models(&result))
        .unwrap_or_default();
    let (default_model, default_effort) = peer
        .call_blocking("config/read", json!({}), patience)
        .map(|result| wire::read_defaults(&result))
        .unwrap_or_default();
    Ok(Connected {
        backend: Arc::new(Codex { peer }),
        models: Models {
            list,
            default_model,
            default_effort,
        },
    })
}

/// A connected Codex.
struct Codex {
    peer: Peer,
}

impl Backend for Codex {
    fn id(&self) -> BackendId {
        DESCRIPTOR.id
    }

    fn start(&self, setup: &ThreadSetup, done: Done<String>) {
        self.peer
            .call("thread/start", wire::start_params(setup), move |reply| {
                done(reply.and_then(|result| {
                    result
                        .get("thread")
                        .and_then(|thread| thread.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| "Codex did not start a conversation".to_owned())
                }));
            });
    }

    fn resume(&self, thread: &str, setup: &ThreadSetup, done: Done<Vec<Item>>) {
        self.peer.call(
            "thread/resume",
            wire::resume_params(setup, thread),
            move |reply| {
                done(reply.map(|result| {
                    result
                        .get("thread")
                        .map(wire::read_history)
                        .unwrap_or_default()
                }));
            },
        );
    }

    fn send(&self, turn: Turn, done: Done<Option<String>>) {
        let params = wire::turn_params(
            &turn.thread,
            &turn.inputs,
            turn.model.as_deref(),
            turn.effort.as_deref(),
        );
        self.peer.call("turn/start", params, move |reply| {
            done(reply.map(|result| {
                result
                    .get("turn")
                    .and_then(|turn| turn.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }));
        });
    }

    fn interrupt(&self, thread: &str, turn: &str) {
        self.peer
            .call("turn/interrupt", wire::interrupt_params(thread, turn), |reply| {
                if let Err(error) = reply {
                    log::warn!("agent: stopping the turn failed: {error}");
                }
            });
    }

    fn rename(&self, thread: &str, title: &str) {
        self.peer.call(
            "thread/name/set",
            json!({ "threadId": thread, "name": title }),
            |_| {},
        );
    }

    fn archive(&self, thread: &str) {
        self.peer
            .call("thread/archive", json!({ "threadId": thread }), |_| {});
    }

    fn skills(&self, cwd: &str, done: Done<Vec<Skill>>) {
        self.peer
            .call("skills/list", wire::skills_params(cwd), move |reply| {
                done(reply.map(|result| wire::read_skills(&result)));
            });
    }
}

#[cfg(test)]
mod tests {
    //! Codex through the shared checks, against a stand-in that speaks its
    //! protocol over pipes.

    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;
    use crate::testing::{self, Harness};

    fn client() -> Client {
        Client {
            name: "test".into(),
            title: "Test".into(),
            version: "0".into(),
        }
    }

    /// A Codex that follows the script in `testing`. Returns what it was
    /// answered when it asked for leave to run something.
    fn fake(
        requests: std::io::PipeReader,
        mut replies: std::io::PipeWriter,
        end: Arc<AtomicBool>,
        granted: Arc<Mutex<Vec<Value>>>,
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
                let _ = writeln!(replies, "{value}");
                let _ = replies.flush();
            };
            loop {
                if end.load(Ordering::Relaxed) {
                    return; // the pipe closes: the program has ended
                }
                let Ok(line) = line_reader.recv_timeout(Duration::from_millis(10)) else {
                    continue;
                };
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let id = message.get("id").cloned();
                let Some(method) = message.get("method").and_then(Value::as_str) else {
                    // An answer to something this fake asked.
                    if let Some(result) = message.get("result") {
                        granted.lock().unwrap().push(result.clone());
                    }
                    continue;
                };
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let reply = |result: Value| json!({ "id": id, "result": result });
                match method {
                    "initialize" => say(reply(json!({ "userAgent": "fake" }))),
                    "model/list" => say(reply(json!({ "data": [{
                        "model": "fake-1", "displayName": "Fake",
                        "supportedReasoningEfforts": [
                            { "reasoningEffort": "low" }, { "reasoningEffort": "high" }],
                        "defaultReasoningEffort": "low",
                    }]}))),
                    "config/read" => say(reply(json!({ "config": { "model": "fake-1" } }))),
                    "thread/start" => say(reply(json!({ "thread": { "id": "t1" } }))),
                    "turn/start" => {
                        let words = params["input"][0]["text"].as_str().unwrap_or("");
                        if words == "stall" {
                            continue;
                        }
                        say(reply(json!({ "turn": { "id": "u1" } })));
                        say(json!({ "method": "turn/started",
                            "params": { "threadId": "t1", "turn": { "id": "u1" } } }));
                        if words == "hello" {
                            say(json!({ "method": "something/new", "params": {} }));
                            say(json!({ "id": "ask-1",
                                "method": "item/commandExecution/requestApproval", "params": {} }));
                            say(json!({ "method": "item/agentMessage/delta", "params": {
                                "threadId": "t1", "itemId": "m1", "delta": "Hel" } }));
                            say(json!({ "method": "item/started", "params": { "threadId": "t1",
                                "item": { "type": "mcpToolCall", "id": "c1", "tool": "concat_version",
                                    "status": "inProgress", "arguments": {} } } }));
                            say(json!({ "method": "item/completed", "params": { "threadId": "t1",
                                "item": { "type": "mcpToolCall", "id": "c1", "tool": "concat_version",
                                    "status": "completed", "arguments": {},
                                    "result": { "content": [{ "type": "text", "text": "ok" }] } } } }));
                            say(json!({ "method": "turn/completed", "params": {
                                "threadId": "t1", "turn": { "id": "u1", "status": "completed" } } }));
                        }
                    }
                    "turn/interrupt" => {
                        say(reply(json!({})));
                        say(json!({ "method": "turn/completed", "params": {
                            "threadId": "t1", "turn": { "id": "u1", "status": "interrupted" } } }));
                    }
                    _ => say(reply(json!({}))),
                }
            }
        });
    }

    #[test]
    fn codex_does_what_a_backend_owes_the_window() {
        let (requests_in, requests_out) = std::io::pipe().unwrap();
        let (replies_in, replies_out) = std::io::pipe().unwrap();
        let end = Arc::new(AtomicBool::new(false));
        let granted = Arc::new(Mutex::new(Vec::new()));
        fake(requests_in, replies_out, Arc::clone(&end), Arc::clone(&granted));

        let (send, events) = mpsc::channel();
        let send = Mutex::new(send);
        let connected = connect_over(
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
        // What the person's own settings choose is read on connecting.
        assert_eq!(connected.models.default_model.as_deref(), Some("fake-1"));
        assert_eq!(connected.models.list[0].efforts, ["low", "high"]);

        testing::conformance(Harness {
            connected,
            events,
            end_program: Box::new(move || end.store(true, Ordering::Relaxed)),
        });
        // What asked to run was let run.
        assert!(
            granted.lock().unwrap().iter().any(|answer| answer["decision"] == "accept"),
            "the request to run something is granted"
        );
    }

    /// Against the Codex on this computer, if there is one:
    /// `cargo test -p concat-agent -- --ignored`.
    #[test]
    #[ignore = "needs Codex installed"]
    fn the_codex_on_this_computer_connects_and_lists_models() {
        let Some(program) = locate::codex() else {
            eprintln!("no Codex here: nothing checked");
            return;
        };
        let connected = connect(&program, &client(), |_| {}).expect("connects");
        assert!(!connected.models.list.is_empty(), "models are listed");
        eprintln!(
            "models: {:?}; defaults: {:?} {:?}",
            connected.models.list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            connected.models.default_model,
            connected.models.default_effort
        );
    }
}
