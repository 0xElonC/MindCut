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

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{Value, json};

use crate::backend::{Backend, Defaults, Done, Event, OnEvent, Provider, Session, SessionSetup};
use crate::peer::{Incoming, Peer, Spec, Writer};
use crate::transcript::{Recorder, Transcripts};
use crate::wire::{Input, Item, Model, Skill, ThreadSetup, Update};

pub mod wire;

/// Who is asking, for the program's own records.
#[derive(Debug, Clone)]
pub struct Client {
    pub name: String,
    pub title: String,
    pub version: String,
}

/// The models a program offers, and what the person's own settings choose.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Models {
    pub list: Vec<Model>,
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
}

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
    on_event: OnEvent,
    /// The record of each conversation, kept here because the program
    /// does not give its history back.
    recorder: Mutex<Recorder>,
    profile: AcpProfile,
    state: Mutex<State>,
    /// Numbers a turn, which the protocol does not.
    turns: AtomicU64,
}

#[derive(Default)]
struct State {
    readers: HashMap<String, wire::Reader>,
    /// Sessions being taken up again: what they replay is not new.
    loading: HashMap<String, Vec<Done<Vec<Item>>>>,
    active: HashMap<String, ActiveTurn>,
    /// The model and depth a session was last set to.
    chosen: HashMap<String, (Option<String>, Option<String>)>,
    /// Numbers the items of words, across every session.
    seq: u64,
}

struct ActiveTurn {
    id: String,
    submission: Arc<Mutex<Submission>>,
}

enum Submission {
    Queued,
    Submitted,
    Cancelled,
}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn emit(&self, update: Update) {
        self.recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .feed(&update);
        (self.on_event)(Event::Update(update));
    }

    fn recorder(&self) -> std::sync::MutexGuard<'_, Recorder> {
        self.recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn end_turn(&self, thread: String, turn: String, status: String, error: Option<String>) {
        self.state().active.remove(&thread);
        self.emit(Update::TurnEnded {
            thread,
            turn,
            status,
            error,
        });
    }
}

fn shared(profile: &AcpProfile, transcripts: Transcripts, on_event: OnEvent) -> Arc<Shared> {
    Arc::new(Shared {
        on_event,
        recorder: Mutex::new(Recorder::new(transcripts)),
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
                if state.loading.contains_key(session) {
                    return;
                }
                let State { readers, seq, .. } = &mut *state;
                readers.entry(session.to_owned()).or_default().read(
                    seq,
                    session,
                    update,
                    &shared.profile,
                )
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

/// What a turn needs done before the message goes, then the message and the
/// end of the turn. Each step waits for the one before: the program takes
/// its requests as they come, and a depth set after the message began would
/// not be the one the message was answered at.
struct Sending {
    peer: Weak<Peer>,
    shared: Arc<Shared>,
    thread: String,
    turn: String,
    submission: Arc<Mutex<Submission>>,
    /// `(option, value)` still to be set.
    changes: Vec<(&'static str, String)>,
    prompt: Value,
}

impl Sending {
    fn run(mut self) {
        let Some(peer) = self.peer.upgrade() else {
            return;
        };
        let gate = Arc::clone(&self.submission);
        let mut submission = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(*submission, Submission::Cancelled) {
            drop(submission);
            self.shared
                .end_turn(self.thread, self.turn, "interrupted".into(), None);
            return;
        }
        if self.changes.is_empty() {
            let thread = self.thread.clone();
            let shared = Arc::clone(&self.shared);
            let turn = self.turn.clone();
            // Hold the gate until the prompt is written so a concurrent
            // Stop either skips this prompt or writes cancel after it.
            *submission = Submission::Submitted;
            self.shared.state().readers.remove(&thread);
            peer.call("session/prompt", self.prompt, move |reply| {
                let (status, error) = wire::turn_end(&reply);
                shared.end_turn(thread, turn, status, error);
            });
            return;
        }
        // A write failure can run this callback inline.
        drop(submission);
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

/// Starts `program` and takes up the protocol. Waits for the program, so: a
/// worker thread, never the window's.
pub fn connect(
    provider: Provider,
    name: &'static str,
    profile: &AcpProfile,
    program: &Path,
    client: &Client,
    transcripts: Transcripts,
    on_event: OnEvent,
) -> Result<AcpBackend, String> {
    let shared = shared(profile, transcripts, on_event);
    let peer = Peer::spawn(program, spec(name, profile), answering(&shared))?;
    finish(provider, peer, shared, client)
}

fn spec(name: &'static str, profile: &AcpProfile) -> Spec {
    Spec {
        name,
        arguments: profile.arguments,
        probe: &[],
        jsonrpc: true,
    }
}

/// Takes up the protocol over a connected `peer`.
fn finish(
    provider: Provider,
    peer: Peer,
    shared: Arc<Shared>,
    client: &Client,
) -> Result<AcpBackend, String> {
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
    Ok(AcpBackend {
        provider,
        peer: Arc::new(peer),
        shared,
        models,
    })
}

/// A connected program that speaks the protocol.
pub struct AcpBackend {
    provider: Provider,
    peer: Arc<Peer>,
    shared: Arc<Shared>,
    models: Models,
}

fn thread_setup(setup: SessionSetup) -> ThreadSetup {
    ThreadSetup {
        cwd: setup.cwd,
        model: setup.model,
        instructions: setup.instructions,
        mcp: setup.mcp.server,
    }
}

impl Backend for AcpBackend {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn start_session(&self, setup: SessionSetup, done: Done<Session>) {
        let params = wire::new_params(&thread_setup(setup), &self.shared.profile);
        let shared = Arc::clone(&self.shared);
        self.peer.call("session/new", params, move |reply| {
            done(
                reply
                    .and_then(|result| wire::read_session(&result))
                    .map(|id| {
                        shared.recorder().watch(&id);
                        Session {
                            id,
                            history: Vec::new(),
                        }
                    }),
            );
        });
    }

    fn resume_session(&self, setup: SessionSetup, id: &str, done: Done<Session>) {
        let params = wire::load_params(&thread_setup(setup), &self.shared.profile, id);
        let thread = id.to_owned();
        // What the session replays arrives before the answer, on the thread
        // that reads the program: it is dropped until the answer is in.
        let done: Done<Vec<Item>> = {
            let shared = Arc::clone(&self.shared);
            let thread = thread.clone();
            Box::new(move |result| {
                done(result.map(|_| Session {
                    // The history is the app's own record, not the program's.
                    history: shared.recorder().files().read(&thread).0,
                    id: thread.clone(),
                }))
            })
        };
        {
            let mut state = self.shared.state();
            if let Some(waiters) = state.loading.get_mut(id) {
                waiters.push(done);
                return;
            }
            if state.active.contains_key(id) {
                drop(state);
                done(Err("Cannot load a session during a turn".into()));
                return;
            }
            state.loading.insert(thread.clone(), vec![done]);
        }
        let shared = Arc::clone(&self.shared);
        self.peer.call("session/load", params, move |reply| {
            let waiters = {
                let mut state = shared.state();
                state.readers.remove(&thread);
                // What the session is set to is not known after a load.
                state.chosen.remove(&thread);
                state.loading.remove(&thread).unwrap_or_default()
            };
            if reply.is_ok() {
                shared.recorder().watch(&thread);
            }
            for done in waiters {
                done(reply.as_ref().map(|_| Vec::new()).map_err(String::clone));
            }
        });
    }

    fn send_turn(
        &self,
        id: &str,
        inputs: Vec<Input>,
        model: Option<String>,
        effort: Option<String>,
        done: Done<String>,
    ) {
        let thread = id.to_owned();
        let turn_id = format!(
            "turn-{}",
            self.shared.turns.fetch_add(1, Ordering::Relaxed) + 1
        );
        let submission = Arc::new(Mutex::new(Submission::Queued));
        let mut changes = Vec::new();
        {
            let mut state = self.shared.state();
            if state.active.contains_key(&thread) || state.loading.contains_key(&thread) {
                drop(state);
                done(Err("The session is busy".into()));
                return;
            }
            state.active.insert(
                thread.clone(),
                ActiveTurn {
                    id: turn_id.clone(),
                    submission: Arc::clone(&submission),
                },
            );
            let (chosen_model, chosen_effort) =
                state.chosen.get(&thread).cloned().unwrap_or_default();
            if let Some(wanted) = model
                .as_ref()
                .filter(|wanted| Some(*wanted) != chosen_model.as_ref())
            {
                changes.push(("model", wanted.clone()));
            }
            if let Some(wanted) = effort
                .as_ref()
                .filter(|wanted| Some(*wanted) != chosen_effort.as_ref())
            {
                changes.push(("thought_level", wanted.clone()));
            }
        }
        // What the person said is written down at once.
        let words: Vec<&str> = inputs
            .iter()
            .filter_map(|input| match input {
                Input::Text(text) if !text.trim().is_empty() => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let images = inputs
            .iter()
            .filter_map(|input| match input {
                Input::Image(path) => Some(path.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        self.shared.recorder().said(
            &thread,
            Item::User {
                id: format!("{turn_id}-user"),
                text: words.join("\n"),
                images,
            },
        );
        // The protocol has no word for a turn beginning: this is it.
        self.shared.emit(Update::TurnStarted {
            thread: thread.clone(),
            turn: turn_id.clone(),
        });
        done(Ok(turn_id.clone()));
        let blocks = wire::prompt_blocks(&inputs, wire::read_image);
        Sending {
            peer: Arc::downgrade(&self.peer),
            shared: Arc::clone(&self.shared),
            prompt: wire::prompt_params(&thread, blocks),
            thread,
            turn: turn_id,
            submission,
            changes,
        }
        .run();
    }

    fn interrupt(&self, id: &str, turn: &str, done: Done<()>) {
        let submission = self
            .shared
            .state()
            .active
            .get(id)
            .filter(|active| active.id == turn)
            .map(|active| Arc::clone(&active.submission));
        if let Some(submission) = submission {
            let mut submission = submission
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match *submission {
                Submission::Queued => *submission = Submission::Cancelled,
                Submission::Submitted => {
                    // The prompt may have ended while Stop waited for the gate.
                    let state = self.shared.state();
                    if state.active.get(id).is_some_and(|active| active.id == turn) {
                        self.peer.notify("session/cancel", wire::cancel_params(id));
                    }
                }
                Submission::Cancelled => {}
            }
        }
        done(Ok(()));
    }

    fn models(&self, done: Done<Vec<Model>>) {
        done(Ok(self.models.list.clone()));
    }

    fn defaults(&self, done: Done<Defaults>) {
        done(Ok((
            self.models.default_model.clone(),
            self.models.default_effort.clone(),
        )));
    }

    fn skills(&self, _cwd: &str, done: Done<Vec<Skill>>) {
        done(Ok(Vec::new()));
    }

    fn archive(&self, id: &str, done: Done<()>) {
        self.shared.recorder().forget(id);
        self.shared.recorder().files().remove(id);
        self.peer
            .call("session/close", json!({ "sessionId": id }), |_| {});
        done(Ok(()));
    }
}

#[cfg(test)]
mod tests;
