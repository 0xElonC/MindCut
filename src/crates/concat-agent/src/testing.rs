// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What every backend must do, and a stand-in program to check it with.
//!
//! [`conformance`] is the one list of what a [`Backend`](crate::backend::Backend)
//! owes the window. A backend's test starts a fake program that speaks the
//! backend's protocol, connects to it, and hands [`conformance`] what it
//! needs. The fake follows this script, keyed on the words of the message:
//!
//! - `hello`: the turn starts, an unknown event arrives (to be ignored), the
//!   program asks leave to run something (to be granted), words of an answer
//!   arrive, a tool called `concat_version` runs and ends, and the turn
//!   completes.
//! - `slow`: the turn starts and does not end until it is interrupted, when
//!   it ends as interrupted.
//! - `stall`: the message is never answered, so that a program that ends
//!   with it in flight can be told from one that answers.
//!
//! A new backend adds a fake and one `#[test]` that calls [`conformance`].

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::backend::{Backend, Connected, Done, Event, Turn};
use crate::vocab::{Input, Item, McpServer, ThreadSetup, ToolState, Update};

/// How long a step may take before the test gives up on it.
const PATIENCE: Duration = Duration::from_secs(5);

/// What [`conformance`] needs of a backend under test.
pub struct Harness {
    pub connected: Connected,
    /// What the backend said unasked, from `connect`'s `on_event`.
    pub events: Receiver<Event>,
    /// Makes the fake program end, as a crash does.
    pub end_program: Box<dyn Fn()>,
}

pub fn setup() -> ThreadSetup {
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

/// Asks a backend something and waits for the answer.
fn ask<T: Send + 'static>(call: impl FnOnce(Done<T>)) -> Result<T, String> {
    let (send, receive) = mpsc::channel();
    call(Box::new(move |reply| {
        let _ = send.send(reply);
    }));
    receive
        .recv_timeout(PATIENCE)
        .unwrap_or_else(|_| Err("no answer in time".into()))
}

fn say(thread: &str, text: &str) -> Turn {
    Turn {
        thread: thread.to_owned(),
        inputs: vec![Input::Text(text.to_owned())],
        model: None,
        effort: None,
    }
}

/// Reads events until one satisfies `wanted`, and returns everything read.
fn until(events: &Receiver<Event>, what: &str, wanted: impl Fn(&Update) -> bool) -> Vec<Update> {
    let mut seen = Vec::new();
    loop {
        match events.recv_timeout(PATIENCE) {
            Ok(Event::Update(update)) => {
                let done = wanted(&update);
                seen.push(update);
                if done {
                    return seen;
                }
            }
            Ok(Event::Closed(reason)) => panic!("closed while waiting for {what}: {reason}"),
            Err(_) => panic!("timed out waiting for {what}; saw {seen:?}"),
        }
    }
}

/// Everything a backend owes the window.
pub fn conformance(harness: Harness) {
    let Harness {
        connected,
        events,
        end_program,
    } = harness;
    let backend: &dyn Backend = connected.backend.as_ref();

    // It offers models, each with some depths to choose between.
    assert!(!connected.models.list.is_empty(), "no models offered");
    assert!(connected.models.list.iter().all(|model| !model.id.is_empty()));

    // A conversation starts and answers a message.
    let thread = ask(|done| backend.start(&setup(), done)).expect("a conversation starts");
    assert!(!thread.is_empty());
    let turn = ask(|done| backend.send(say(&thread, "hello"), done)).expect("the message is taken");
    assert!(turn.is_some(), "the turn has an id");
    let seen = until(&events, "the turn to end", |update| {
        matches!(update, Update::TurnEnded { .. })
    });
    assert!(
        seen.iter().any(|update| matches!(update, Update::TurnStarted { .. })),
        "the turn starts: {seen:?}"
    );
    assert!(
        seen.iter().any(|update| matches!(update, Update::AgentWords { .. })),
        "words of the answer arrive: {seen:?}"
    );
    assert!(
        matches!(seen.last(), Some(Update::TurnEnded { status, .. }) if status == "completed"),
        "the turn completes: {seen:?}"
    );

    // A tool call is seen starting and ending under its own name, not the
    // program's name for it.
    let tools: Vec<(&str, ToolState)> = seen
        .iter()
        .filter_map(|update| match update {
            Update::ItemStarted { item: Item::Tool { name, state, .. }, .. }
            | Update::ItemDone { item: Item::Tool { name, state, .. }, .. } => {
                Some((name.as_str(), *state))
            }
            _ => None,
        })
        .collect();
    assert!(
        tools.iter().any(|(name, state)| *name == "concat_version" && *state == ToolState::Running),
        "the tool call starts: {seen:?}"
    );
    assert!(
        tools.iter().any(|(name, state)| *name == "concat_version" && *state == ToolState::Done),
        "the tool call ends: {seen:?}"
    );

    // A turn that is stopped ends as stopped, and the conversation goes on.
    let turn = ask(|done| backend.send(say(&thread, "slow"), done))
        .expect("the message is taken")
        .expect("the turn has an id");
    until(&events, "the slow turn to start", |update| {
        matches!(update, Update::TurnStarted { .. })
    });
    backend.interrupt(&thread, &turn);
    let seen = until(&events, "the stopped turn to end", |update| {
        matches!(update, Update::TurnEnded { .. })
    });
    assert!(
        matches!(seen.last(), Some(Update::TurnEnded { status, .. }) if status == "interrupted"),
        "the stopped turn ends as interrupted: {seen:?}"
    );
    ask(|done| backend.send(say(&thread, "hello"), done)).expect("the conversation goes on");
    until(&events, "the next turn to end", |update| {
        matches!(update, Update::TurnEnded { .. })
    });

    // A program that ends answers what waits, and says it ended. A message
    // in flight is reported as failed one way or the other: its own answer
    // is an error, or the turn it began ends as failed.
    let (send, receive) = mpsc::channel();
    backend.send(
        say(&thread, "stall"),
        Box::new(move |reply| {
            let _ = send.send(reply);
        }),
    );
    std::thread::sleep(Duration::from_millis(100));
    end_program();
    let mut turn_failed = false;
    loop {
        match events.recv_timeout(PATIENCE) {
            Ok(Event::Closed(_)) => break,
            Ok(Event::Update(Update::TurnEnded { status, .. })) => turn_failed |= status == "failed",
            Ok(Event::Update(_)) => {}
            Err(_) => panic!("the end of the program is not reported"),
        }
    }
    let answered_with_error = matches!(receive.recv_timeout(PATIENCE), Ok(Err(_)));
    assert!(
        answered_with_error || turn_failed,
        "a message in flight when the program ends is reported as failed"
    );
}
