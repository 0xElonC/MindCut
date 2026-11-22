// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The programs the chat can be backed by, one module each.
//!
//! A backend is a protocol (how to talk to a program) and a product (which
//! program, where it is found, what it can do). A product is a
//! [`Descriptor`]: data, listed in [`REGISTRY`]. The window draws its
//! choices from that list and asks a connected [`Backend`] for everything
//! else, so that it never needs to know which program is behind a chat.
//!
//! Rules that keep it so:
//!
//! 1. This crate depends on nothing of the window or of video.
//! 2. The window names no backend: it reads a [`Descriptor`] and its
//!    [`Caps`], never "if this is Codex".
//! 3. A protocol module names no product: what differs between products on
//!    one protocol is data in that protocol's profile.
//! 4. What a backend can or cannot do is a [`Caps`] field, not a test of
//!    which backend it is.
//!
//! To add a program, see `docs/AGENT_BACKENDS.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::vocab::{Input, Item, Model, Skill, ThreadSetup, Update};

pub mod acp;
pub mod antigravity;
pub mod codex;

/// What a backend answers a request with, on whatever thread it answers.
pub type Done<T> = Box<dyn FnOnce(Result<T, String>) + Send>;

/// A backend's name as it is kept in the list of conversations: stable, and
/// never renamed once released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BackendId(pub &'static str);

/// What to tell a person who does not have the program.
#[derive(Debug, Clone, Copy)]
pub struct Install {
    /// A command that installs it, if there is one.
    pub command: Option<&'static str>,
    /// Where to read more.
    pub url: &'static str,
}

/// How a backend's program talks.
#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    /// Codex's own: `codex app-server`.
    Codex,
    /// The Agent Client Protocol, as this program speaks it.
    Acp(acp::AcpProfile),
}

/// What a backend can do. The window shows and hides by these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// It has depths of thought to choose between.
    pub efforts: bool,
    /// It has skills to bring into a message.
    pub skills: bool,
    /// The program keeps a conversation's history and gives it back on
    /// resume. When it does not, the app keeps its own record.
    pub remote_history: bool,
    /// It can be told a conversation's new name.
    pub rename: bool,
    /// A conversation resumed can be given another folder, instructions and
    /// MCP server than it was started with.
    pub setup_on_resume: bool,
}

/// One product: a program the chat can be backed by.
#[derive(Debug)]
pub struct Descriptor {
    pub id: BackendId,
    /// What the person calls it. A proper name: not translated.
    pub name: &'static str,
    /// Finds the program on this computer.
    pub locate: fn() -> Option<PathBuf>,
    pub install: Install,
    pub protocol: Protocol,
    pub caps: Caps,
}

/// Every backend there is, in the order they are offered.
pub static REGISTRY: &[&Descriptor] = &[&codex::DESCRIPTOR, &antigravity::DESCRIPTOR];

/// The backend that goes by `id`, if there is one.
pub fn find(id: &str) -> Option<&'static Descriptor> {
    REGISTRY.iter().copied().find(|descriptor| descriptor.id.0 == id)
}

/// What a connected backend says unasked.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Something happened in a conversation.
    Update(Update),
    /// The program ended, with how and the last thing it said.
    Closed(String),
}

/// Who is asking, for the program's own records.
#[derive(Debug, Clone)]
pub struct Client {
    pub name: String,
    pub title: String,
    pub version: String,
}

/// The models a backend offers, and what the person's own settings choose.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Models {
    pub list: Vec<Model>,
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
}

/// One message, and how to answer it.
#[derive(Debug, Clone)]
pub struct Turn {
    pub thread: String,
    pub inputs: Vec<Input>,
    /// `None` takes the program's own choice.
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// A connected program. Every method answers on the thread that reads the
/// program, never on the caller's, and none waits.
pub trait Backend: Send + Sync {
    fn id(&self) -> BackendId;

    /// Starts a conversation; answers with its id.
    fn start(&self, setup: &ThreadSetup, done: Done<String>);

    /// Takes up a conversation again under `setup`. Answers with its history
    /// when the program keeps it ([`Caps::remote_history`]), else with none.
    fn resume(&self, thread: &str, setup: &ThreadSetup, done: Done<Vec<Item>>);

    /// Sends a message; answers with the id of the turn it started.
    fn send(&self, turn: Turn, done: Done<Option<String>>);

    /// Stops the turn running in `thread`.
    fn interrupt(&self, thread: &str, turn: &str);

    /// Tells the program a conversation's new name ([`Caps::rename`]).
    fn rename(&self, thread: &str, title: &str);

    /// Tells the program a conversation is gone.
    fn archive(&self, thread: &str);

    /// The skills there are for `cwd` ([`Caps::skills`]).
    fn skills(&self, cwd: &str, done: Done<Vec<Skill>>);
}

/// A running backend, and the models it offered on connecting.
pub struct Connected {
    pub backend: Arc<dyn Backend>,
    pub models: Models,
}

/// Starts `program` as the product `descriptor` describes, and takes up its
/// protocol. Waits for the program, so: a worker thread, never the window's.
/// `on_event` is called on the thread that reads the program.
pub fn connect(
    descriptor: &'static Descriptor,
    program: &Path,
    client: &Client,
    on_event: impl Fn(Event) + Send + Sync + 'static,
) -> Result<Connected, String> {
    match descriptor.protocol {
        Protocol::Codex => codex::connect(program, client, on_event),
        Protocol::Acp(profile) => acp::connect(descriptor, &profile, program, client, on_event),
    }
}
