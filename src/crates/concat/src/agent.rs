// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The Agent pane's controller.
//!
//! The pane (`ui/workspace/agent-pane.slint`) draws whatever is on the
//! `AgentChat` global and reports what the person does through its
//! callbacks. This is the other end: it starts the Codex on this computer
//! (`concat-agent`), keeps the list of conversations, turns what Codex says
//! into the messages the pane draws, and starts the agent with the Concat
//! MCP server so that what it does is editing - on the project open in the
//! window, as it is on screen, when live mode is on (`live.rs`).
//!
//! Everything here runs on the event-loop thread. What arrives from Codex
//! is posted there; nothing in this file waits for it.
//!
//! `CONCAT_AGENT_STATUS=missing|connecting|failed` opens the pane on one of
//! its status pages without starting anything, since a computer with Codex
//! on it never shows them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use concat_agent::backend::{self, Backend, Client, Descriptor, Event, Turn};
use concat_agent::store::transcript::{Recorder, Transcripts};
use concat_agent::store::{Chat, Store};
use concat_agent::vocab::{
    COMMON_EFFORTS, Input, Item, McpServer, Model, Skill, ThreadSetup, ToolState, Update,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::host::Shell;
use crate::i18n::{t, tf};
use crate::ui::{
    AgentAttachment, AgentChat, AgentMessage, AgentRole, AgentSession, AgentSkill, AgentStatus,
    AgentToolCall, App,
};

/// A file waiting to go with the next message.
struct Attached {
    path: PathBuf,
    shown: AgentAttachment,
}

/// One line of the conversation on screen.
#[derive(Clone)]
struct Line {
    id: String,
    role: AgentRole,
    text: String,
    images: Vec<AgentAttachment>,
    reasoning: String,
    tool: String,
    tool_input: String,
    tool_output: String,
    tool_state: i32,
    streaming: bool,
}

impl Line {
    fn new(id: &str, role: AgentRole) -> Line {
        Line {
            id: id.to_owned(),
            role,
            text: String::new(),
            images: Vec::new(),
            reasoning: String::new(),
            tool: String::new(),
            tool_input: String::new(),
            tool_output: String::new(),
            tool_state: 0,
            streaming: false,
        }
    }

    fn notice(id: &str, text: &str) -> Line {
        Line {
            text: text.to_owned(),
            ..Line::new(id, AgentRole::Notice)
        }
    }
}

/// How one backend's program is running.
enum Link {
    /// Not started: nothing has needed it yet.
    Idle,
    Starting,
    Missing,
    Failed(String),
    Ready,
}

/// What is known of one backend's program. Each backend has one, and the
/// conversation on screen is with the backend whose slot is `descriptor`'s.
struct Slot {
    link: Link,
    /// The program, once it has started.
    backend: Option<Arc<dyn Backend>>,
    models: Vec<Model>,
    /// What the person's own settings for the program choose.
    defaults: (Option<String>, Option<String>),
}

impl Default for Slot {
    fn default() -> Slot {
        Slot {
            link: Link::Idle,
            backend: None,
            models: Vec::new(),
            defaults: (None, None),
        }
    }
}

struct Controller {
    /// Every backend there is, by id.
    slots: HashMap<&'static str, Slot>,
    /// The backend of the conversation on screen, or of the next one: what
    /// is found, started and asked about.
    descriptor: &'static Descriptor,
    model: usize,
    effort: usize,
    store: Store,
    /// The record of the conversations whose program does not keep it.
    transcripts: Transcripts,
    recorder: Recorder,
    query: String,
    /// The thread on screen, once it exists: the first message creates it.
    current: Option<String>,
    lines: Vec<Line>,
    turn: Option<String>,
    busy: bool,
    /// What each thread was last started or resumed for, so a change of
    /// project or of live mode gives the agent its new surroundings first.
    loaded: HashMap<String, String>,
    attachments: Vec<Attached>,
    /// Live mode: the agent edits the project open in the window.
    live: Arc<AtomicBool>,
    /// The API the agent's MCP server reaches the window's project through.
    bridge: Option<concat_server::Server>,
    /// Lines made here, numbered.
    minted: u64,
    /// What the pane last said the project was called.
    shown_project: String,
    /// The skills the current backend has, as last asked, and the search
    /// over them.
    skills: Vec<Skill>,
    skill_query: String,
    /// The skills chosen for the next message.
    picked: Vec<Skill>,
    /// The person is at the composer: a file dropped on the window is
    /// meant for the agent. Ended by a press anywhere else in the editor.
    engaged: bool,
}

thread_local! {
    static CONTROLLER: RefCell<Option<Controller>> = const { RefCell::new(None) };
}

/// Runs `body` on the controller, with the shell and the window.
fn with(body: impl FnOnce(&mut Controller, &Shell, &App)) {
    Shell::with(|shell, app| {
        CONTROLLER.with(|slot| {
            if let Some(controller) = slot.borrow_mut().as_mut() {
                body(controller, shell, &app);
            }
        });
    });
}

/// Runs `body` on the event-loop thread, from anywhere.
fn post(body: impl FnOnce(&mut Controller, &Shell, &App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with(body));
}

/// The person's home folder: `HOME`, or `USERPROFILE` on Windows.
fn home_dir() -> Option<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|home| !home.is_empty())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// What a person is told the agent is: its job, and the project it works in.
fn instructions(project: Option<&str>, live: bool) -> String {
    let mut text = String::from(
        "You are the editing agent inside ElonCut, a desktop video editor. You edit video by \
         calling the eloncut MCP tools (project_*, timeline_read, clip_*, title_add, captions_add, \
         effect_*, export_*, preview_frame and the rest); do not edit project files by hand. \
         Before starting an edit, call guide_read without arguments for the operation manual's \
         directory, then call guide_read with topic to read the relevant topics. \
         Read the timeline with timeline_read before changing it, make the edits asked for, and \
         look at the result with preview_frame when it matters. Times are in seconds. \
         Files the person attaches are given as paths: bring a video, photo or sound into the \
         project with media_import, and look at a video with media_probe and preview_frame. \
         Answer briefly, in the language the person writes in.",
    );
    match (project, live) {
        (Some(path), true) => text.push_str(&format!(
            " Live mode is on: the person has the project at {path} open and is watching it. \
             Call project_open with exactly that path and edit it directly; each edit appears on \
             their screen as you make it, and they can undo it. The editor saves the project \
             itself, so save only when asked."
        )),
        (Some(path), false) => text.push_str(&format!(
            " Live mode is off: the person has the project at {path} open and does not want it \
             changed under them. Do not open or edit that folder. If they want work done on it, \
             copy the folder to a new place first and edit the copy."
        )),
        (None, _) => text.push_str(
            " No project is open in the window. Create one with project_create if the person \
             wants a new edit.",
        ),
    }
    text
}

/// The first `longest` characters of `text`, marked when there were more:
/// a tool's whole answer can be a screenful of JSON.
fn clip(text: &str, longest: usize) -> String {
    if text.chars().count() <= longest {
        return text.to_owned();
    }
    let head: String = text.chars().take(longest).collect();
    format!("{head}…")
}

/// Words as the chat draws them: the model writes Markdown, and the pane
/// draws plain text, so the marks that would show as stray characters
/// are taken off - emphasis and heading marks - and bullets are bulleted.
fn plain(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let body = trimmed.trim_start_matches('#');
            let body = if body.len() != trimmed.len() {
                body.trim_start()
            } else {
                trimmed
            };
            let indent = &line[..line.len() - trimmed.len()];
            let body = body
                .strip_prefix("- ")
                .or_else(|| body.strip_prefix("* "))
                .map(|rest| format!("• {rest}"))
                .unwrap_or_else(|| body.to_owned());
            format!("{indent}{}", body.replace("**", "").replace("__", ""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn effort_label(effort: &str) -> String {
    match effort {
        "low" => t("agent.effortLow"),
        "medium" => t("agent.effortMedium"),
        "high" => t("agent.effortHigh"),
        "xhigh" => t("agent.effortXhigh"),
        "max" => t("agent.effortMax"),
        "ultra" => t("agent.effortUltra"),
        other => other.to_owned(),
    }
}

fn kind_of(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => "video",
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" => "audio",
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" => "image",
        _ => "file",
    }
}

fn attached(id: String, path: &Path) -> Attached {
    let kind = kind_of(path);
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let detail = if size >= 1 << 20 {
        format!("{:.1} MB", size as f64 / f64::from(1 << 20))
    } else {
        format!("{} KB", size.div_ceil(1024))
    };
    let thumb = (kind == "image")
        .then(|| slint::Image::load_from_path(path).ok())
        .flatten();
    Attached {
        path: path.to_owned(),
        shown: AgentAttachment {
            id: id.into(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into(),
            kind: kind.into(),
            detail: detail.into(),
            has_thumb: thumb.is_some(),
            thumb: thumb.unwrap_or_default(),
        },
    }
}

/// "3 minutes ago", in the language of the window.
fn ago(then: u64) -> String {
    let seconds = now().saturating_sub(then);
    match seconds {
        0..=59 => t("agent.justNow"),
        60..=3599 => tf("agent.minutesAgo", &[&(seconds / 60)]),
        3600..=86399 => tf("agent.hoursAgo", &[&(seconds / 3600)]),
        _ => tf("agent.daysAgo", &[&(seconds / 86400)]),
    }
}

fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Controller {
    fn mint(&mut self, kind: &str) -> String {
        self.minted += 1;
        format!("{kind}{}", self.minted)
    }

    /// The slot of the backend the conversation on screen is with.
    fn slot(&self) -> &Slot {
        self.slots
            .get(self.descriptor.id.0)
            .expect("every backend has a slot")
    }

    fn model_id(&self) -> Option<String> {
        self.slot().models.get(self.model).map(|model| model.id.clone())
    }

    fn efforts(&self) -> Vec<String> {
        self.slot()
            .models
            .get(self.model)
            .map(|model| model.efforts.clone())
            .unwrap_or_else(|| COMMON_EFFORTS.map(str::to_owned).to_vec())
    }

    fn effort_id(&self) -> Option<String> {
        self.efforts().get(self.effort).cloned()
    }

    /// Picks the model and depth the current backend would start with, from
    /// what the person's own settings say.
    fn choose_defaults(&mut self) {
        let slot = self
            .slots
            .get_mut(self.descriptor.id.0)
            .expect("every backend has a slot");
        let (model, effort) = slot.defaults.clone();
        if let Some(model) = &model {
            // A model the settings name that the program does not list - a
            // provider of the person's own - is offered all the same.
            if !slot.models.iter().any(|known| &known.id == model) {
                slot.models.insert(
                    0,
                    Model {
                        id: model.clone(),
                        name: model.clone(),
                        efforts: COMMON_EFFORTS.map(str::to_owned).to_vec(),
                        default_effort: effort.clone().unwrap_or_else(|| "medium".into()),
                    },
                );
            }
        }
        self.model = model
            .and_then(|model| slot.models.iter().position(|known| known.id == model))
            .unwrap_or(0);
        let wanted = effort.or_else(|| slot.models.get(self.model).map(|m| m.default_effort.clone()));
        let efforts = self.efforts();
        self.effort = wanted
            .and_then(|wanted| efforts.iter().position(|known| *known == wanted))
            .unwrap_or(0);
    }

    // ── what the pane shows ──

    /// What the agent is doing, in words, while a turn runs.
    fn activity(&self) -> String {
        if !self.busy {
            return String::new();
        }
        match self.lines.last() {
            Some(line) if matches!(line.role, AgentRole::Tool) && line.tool_state == 0 => {
                tf("agent.runningTool", &[&line.tool])
            }
            Some(line)
                if matches!(line.role, AgentRole::Assistant)
                    && line.streaming
                    && !line.text.is_empty() =>
            {
                t("agent.writing")
            }
            _ => t("agent.thinking"),
        }
    }

    fn publish(&mut self, shell: &Shell, app: &App) {
        let chat = app.global::<AgentChat>();
        let name = self.descriptor.name;
        let link = &self.slot().link;
        chat.set_status(match link {
            Link::Ready => AgentStatus::Ready,
            Link::Idle | Link::Starting => AgentStatus::Connecting,
            Link::Missing => AgentStatus::Missing,
            Link::Failed(_) => AgentStatus::Failed,
        });
        let (title, body) = match link {
            Link::Ready => (String::new(), String::new()),
            Link::Idle | Link::Starting => {
                (tf("agent.connecting", &[&name]), tf("agent.connectingBody", &[&name]))
            }
            Link::Missing => (tf("agent.missingTitle", &[&name]), tf("agent.missingBody", &[&name])),
            Link::Failed(reason) if reason.is_empty() => {
                (tf("agent.failedTitle", &[&name]), t("agent.failedBody"))
            }
            Link::Failed(reason) => (tf("agent.failedTitle", &[&name]), reason.clone()),
        };
        chat.set_status_title(title.into());
        chat.set_status_body(body.into());
        let hint = match (link, self.descriptor.install.command) {
            (Link::Missing, Some(command)) => tf("agent.installWith", &[&command]),
            _ => String::new(),
        };
        chat.set_install_hint(hint.into());
        // Which program the conversation is with, and whether it may still
        // be changed: it may until the first message is sent.
        chat.set_backends(ModelRc::new(VecModel::from(
            backend::REGISTRY
                .iter()
                .map(|descriptor| SharedString::from(descriptor.name))
                .collect::<Vec<_>>(),
        )));
        chat.set_backend_index(
            backend::REGISTRY
                .iter()
                .position(|descriptor| descriptor.id == self.descriptor.id)
                .unwrap_or(0) as i32,
        );
        chat.set_backend_locked(self.current.is_some());
        chat.set_has_efforts(self.descriptor.caps.efforts && !self.efforts().is_empty());
        chat.set_has_skills(self.descriptor.caps.skills);
        chat.set_busy(self.busy);
        chat.set_activity(self.activity().into());
        chat.set_live(self.live.load(Ordering::Relaxed));
        chat.set_models(ModelRc::new(VecModel::from(
            self.slot()
                .models
                .iter()
                .map(|model| SharedString::from(model.name.as_str()))
                .collect::<Vec<_>>(),
        )));
        chat.set_model_index(self.model as i32);
        chat.set_efforts(ModelRc::new(VecModel::from(
            self.efforts()
                .iter()
                .map(|effort| SharedString::from(effort_label(effort)))
                .collect::<Vec<_>>(),
        )));
        chat.set_effort_index(self.effort as i32);

        let message = |line: &Line| AgentMessage {
            id: line.id.as_str().into(),
            role: line.role,
            text: if matches!(line.role, AgentRole::Assistant) {
                plain(&line.text).into()
            } else {
                line.text.as_str().into()
            },
            attachments: ModelRc::new(VecModel::from(line.images.clone())),
            reasoning: plain(&line.reasoning).into(),
            tool: line.tool.as_str().into(),
            tool_input: SharedString::new(),
            tool_output: SharedString::new(),
            tool_state: line.tool_state,
            calls: ModelRc::default(),
            streaming: line.streaming,
        };
        // A run of tool calls made one after another is one line, which
        // opens to them: an agent that looks at twenty frames would
        // otherwise fill the conversation with twenty cards.
        let mut messages: Vec<AgentMessage> = Vec::new();
        let mut run: Vec<&Line> = Vec::new();
        let close = |run: &mut Vec<&Line>, messages: &mut Vec<AgentMessage>| {
            let Some(last) = run.last() else { return };
            let mut group = message(run[0]);
            group.tool = last.tool.as_str().into();
            group.text = last.text.as_str().into();
            group.tool_state = if run.iter().any(|line| line.tool_state == 0) {
                0
            } else if run.iter().any(|line| line.tool_state == 2) {
                2
            } else {
                1
            };
            group.calls = ModelRc::new(VecModel::from(
                run.iter()
                    .map(|line| AgentToolCall {
                        id: line.id.as_str().into(),
                        name: line.tool.as_str().into(),
                        summary: line.text.as_str().into(),
                        input: line.tool_input.as_str().into(),
                        output: line.tool_output.as_str().into(),
                        state: line.tool_state,
                    })
                    .collect::<Vec<_>>(),
            ));
            messages.push(group);
            run.clear();
        };
        for line in &self.lines {
            if matches!(line.role, AgentRole::Tool) {
                run.push(line);
            } else {
                close(&mut run, &mut messages);
                messages.push(message(line));
            }
        }
        close(&mut run, &mut messages);
        chat.set_messages(ModelRc::new(VecModel::from(messages)));
        chat.set_attachments(ModelRc::new(VecModel::from(
            self.attachments
                .iter()
                .map(|file| file.shown.clone())
                .collect::<Vec<_>>(),
        )));

        let query = self.skill_query.trim().to_lowercase();
        chat.set_skills(ModelRc::new(VecModel::from(
            self.skills
                .iter()
                .filter(|skill| {
                    !self.picked.iter().any(|picked| picked.name == skill.name)
                        && (query.is_empty()
                            || skill.name.to_lowercase().contains(&query)
                            || skill.description.to_lowercase().contains(&query))
                })
                .map(|skill| AgentSkill {
                    name: skill.name.as_str().into(),
                    description: skill.description.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )));
        chat.set_picked_skills(ModelRc::new(VecModel::from(
            self.picked
                .iter()
                .map(|skill| SharedString::from(skill.name.as_str()))
                .collect::<Vec<_>>(),
        )));
        let title = self
            .current
            .as_deref()
            .and_then(|id| self.store.get(id))
            .map(|chat| chat.title.clone())
            .unwrap_or_default();
        chat.set_title(title.into());
        self.shown_project = shell.studio.borrow().project_name.clone();
        let project = if shell.studio.borrow().session.is_some() {
            self.shown_project.clone()
        } else {
            String::new()
        };
        chat.set_project(project.into());

        let sessions: Vec<AgentSession> = self
            .store
            .list(&self.query)
            .into_iter()
            .map(|entry| AgentSession {
                id: entry.id.as_str().into(),
                title: entry.title.as_str().into(),
                detail: if backend::REGISTRY.len() > 1 {
                    let name = backend::find(&entry.backend).map_or(entry.backend.as_str(), |d| d.name);
                    format!("{name} · {} · {}", folder_name(&entry.project), ago(entry.updated))
                } else {
                    format!("{} · {}", folder_name(&entry.project), ago(entry.updated))
                }
                .into(),
                count: entry.messages as i32,
                running: self.busy && self.current.as_deref() == Some(entry.id.as_str()),
                current: self.current.as_deref() == Some(entry.id.as_str()),
            })
            .collect();
        chat.set_sessions(ModelRc::new(VecModel::from(sessions)));
    }

    // ── starting the agent ──

    /// Starts `descriptor`'s program if nothing has yet.
    fn ensure(&mut self, descriptor: &'static Descriptor, shell: &Shell, app: &App) {
        if matches!(self.slots.entry(descriptor.id.0).or_default().link, Link::Idle) {
            self.begin(descriptor, shell, app);
        }
    }

    fn begin(&mut self, descriptor: &'static Descriptor, shell: &Shell, app: &App) {
        let slot = self.slots.entry(descriptor.id.0).or_default();
        slot.link = Link::Starting;
        slot.backend = None;
        self.publish(shell, app);
        std::thread::spawn(move || {
            let Some(program) = (descriptor.locate)() else {
                post(move |c, shell, app| {
                    c.slots.entry(descriptor.id.0).or_default().link = Link::Missing;
                    c.publish(shell, app);
                });
                return;
            };
            log::info!("agent: starting {}", program.display());
            let client = Client {
                name: "eloncut".into(),
                title: "ElonCut".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            };
            let connected = backend::connect(descriptor, &program, &client, move |event| {
                post(move |c, shell, app| c.heard(descriptor, event, shell, app));
            });
            match connected {
                Err(error) => {
                    log::warn!("agent: {} did not start: {error}", descriptor.name);
                    post(move |c, shell, app| {
                        c.slots.entry(descriptor.id.0).or_default().link = Link::Failed(error);
                        c.publish(shell, app);
                    });
                }
                Ok(connected) => post(move |c, shell, app| {
                    let slot = c.slots.entry(descriptor.id.0).or_default();
                    slot.models = connected.models.list;
                    slot.defaults = (connected.models.default_model, connected.models.default_effort);
                    slot.backend = Some(connected.backend);
                    slot.link = Link::Ready;
                    if c.descriptor.id == descriptor.id {
                        c.choose_defaults();
                        c.load_skills(shell);
                    }
                    // A conversation of this program opened while it was
                    // starting is taken up now. One of another program's is
                    // left alone: it is not waiting for this.
                    let waiting = c
                        .current
                        .as_deref()
                        .and_then(|thread| c.store.get(thread))
                        .is_some_and(|chat| chat.backend == descriptor.id.0);
                    if waiting {
                        c.take_up(shell, app);
                    }
                    c.publish(shell, app);
                }),
            }
        });
    }

    /// What a program said without being asked.
    fn heard(&mut self, from: &'static Descriptor, event: Event, shell: &Shell, app: &App) {
        match event {
            Event::Update(update) => {
                self.update(update);
                self.publish(shell, app);
            }
            Event::Closed(reason) => {
                log::warn!("agent: {} ended: {reason}", from.name);
                let slot = self.slots.entry(from.id.0).or_default();
                slot.backend = None;
                slot.link = Link::Failed(reason);
                // What was running in the conversation on screen is over,
                // and every conversation of that program is to be taken up
                // again when it is next used.
                if self.descriptor.id == from.id {
                    self.busy = false;
                    self.turn = None;
                }
                let store = &self.store;
                self.loaded
                    .retain(|thread, _| store.get(thread).is_none_or(|chat| chat.backend != from.id.0));
                self.publish(shell, app);
            }
        }
    }

    // ── the conversation ──

    fn put_item(&mut self, item: &Item, done: bool) {
        let at = self.lines.iter().position(|line| line.id == item.id());
        // A thought with no words yet is not drawn as an empty line.
        if at.is_none() && matches!(item, Item::Reasoning { text, .. } if text.is_empty()) {
            return;
        }
        let mut line = match at {
            Some(at) => self.lines[at].clone(),
            None => Line::new(item.id(), AgentRole::Notice),
        };
        match item {
            Item::Agent { text, .. } => {
                line.role = AgentRole::Assistant;
                // Words that arrived are never cut back to fewer by an
                // item that has not caught up.
                if text.len() >= line.text.len() {
                    line.text = text.clone();
                }
                line.streaming = !done;
            }
            Item::Reasoning { text, .. } => {
                line.role = AgentRole::Assistant;
                if text.len() >= line.reasoning.len() {
                    line.reasoning = text.clone();
                }
            }
            Item::Tool {
                name,
                summary,
                input,
                output,
                state,
                ..
            } => {
                line.role = AgentRole::Tool;
                line.tool = name.clone();
                line.text = summary.clone();
                line.tool_input = clip(input, 2000);
                line.tool_output = clip(output, 2000);
                line.tool_state = match state {
                    ToolState::Running => 0,
                    ToolState::Done => 1,
                    ToolState::Failed => 2,
                };
            }
            Item::User { text, images, .. } => {
                line.role = AgentRole::User;
                line.text = text.clone();
                line.images = images
                    .iter()
                    .map(|path| attached(String::new(), Path::new(path)).shown)
                    .collect();
            }
        }
        match at {
            Some(at) => self.lines[at] = line,
            None => self.lines.push(line),
        }
    }

    fn update(&mut self, update: Update) {
        // What is said in a conversation whose program does not keep it is
        // kept here, whichever conversation is on screen.
        self.recorder.feed(&update);
        let ours = |thread: &str, current: &Option<String>| current.as_deref() == Some(thread);
        match update {
            Update::TurnStarted { thread, turn } if ours(&thread, &self.current) => {
                self.turn = Some(turn);
                self.busy = true;
            }
            Update::TurnEnded {
                thread,
                status,
                error,
                ..
            } => {
                if ours(&thread, &self.current) {
                    self.busy = false;
                    self.turn = None;
                    for line in &mut self.lines {
                        line.streaming = false;
                    }
                    let notice = match (status.as_str(), error) {
                        ("failed", Some(error)) => Some(error),
                        ("failed", None) => Some(t("agent.turnFailed")),
                        ("interrupted", _) => Some(t("agent.stopped")),
                        _ => None,
                    };
                    if let Some(notice) = notice {
                        let id = self.mint("n");
                        self.lines.push(Line::notice(&id, &notice));
                    }
                }
                let count = self
                    .lines
                    .iter()
                    .filter(|line| matches!(line.role, AgentRole::User | AgentRole::Assistant))
                    .count() as u32;
                let is_current = ours(&thread, &self.current);
                self.store.edit(&thread, |chat| {
                    chat.updated = now();
                    if is_current {
                        chat.messages = count;
                    }
                });
            }
            // Our own message is on screen already.
            Update::ItemStarted { item: Item::User { .. }, .. }
            | Update::ItemDone { item: Item::User { .. }, .. } => {}
            Update::ItemStarted { thread, item } if ours(&thread, &self.current) => {
                self.put_item(&item, false);
            }
            Update::ItemDone { thread, item } if ours(&thread, &self.current) => {
                self.put_item(&item, true);
            }
            Update::AgentWords { thread, item, words } if ours(&thread, &self.current) => {
                match self.lines.iter_mut().find(|line| line.id == item) {
                    Some(line) => line.text.push_str(&words),
                    None => {
                        let mut line = Line::new(&item, AgentRole::Assistant);
                        line.text = words;
                        line.streaming = true;
                        self.lines.push(line);
                    }
                }
            }
            Update::ThoughtWords { thread, item, words } if ours(&thread, &self.current) => {
                match self.lines.iter_mut().find(|line| line.id == item) {
                    Some(line) => line.reasoning.push_str(&words),
                    None => {
                        let mut line = Line::new(&item, AgentRole::Assistant);
                        line.reasoning = words;
                        line.streaming = true;
                        self.lines.push(line);
                    }
                }
            }
            Update::Trouble { thread, message } => {
                if thread.as_deref().is_none_or(|thread| ours(thread, &self.current)) {
                    let id = self.mint("n");
                    self.lines.push(Line::notice(&id, &message));
                }
            }
            _ => {}
        }
    }

    /// The agent's MCP server, reaching the window's project through an API
    /// of its own that this process serves.
    fn mcp(&mut self, shell: &Shell) -> Result<McpServer, String> {
        if self.bridge.is_none() {
            let (exporter, open_projects, dirs) = {
                let studio = shell.studio.borrow();
                (
                    studio.host.exporter.clone(),
                    studio.host.open_projects.clone(),
                    studio.host.dirs.clone(),
                )
            };
            let live = Arc::clone(&self.live);
            let config = concat_server::Config {
                json: Some("127.0.0.1:0".parse().map_err(|_| "no address")?),
                ..concat_server::Config::default()
            };
            let server = concat_server::Server::start(config, move |events| {
                let mut api = concat_api::Api::with_dirs(dirs, events);
                api.share_exporter(exporter);
                api.share_open_projects(open_projects);
                api.set_live(Arc::new(crate::live::WindowLive { on: live }));
                Ok(api)
            })?;
            self.bridge = Some(server);
        }
        let server = self.bridge.as_ref().ok_or("the editor's API is not running")?;
        let address = server.json_addr().ok_or("the editor's API has no address")?;
        let program = std::env::current_exe()
            .map_err(|error| format!("this program cannot be found: {error}"))?;
        Ok(McpServer {
            command: program.to_string_lossy().into_owned(),
            args: vec!["--mcp".into(), "window".into()],
            env: vec![
                ("CONCAT_API_ADDRESS".into(), address.to_string()),
                ("CONCAT_API_TOKEN".into(), server.token().to_owned()),
            ],
        })
    }

    /// The setup for a thread, and what it is a setup for.
    fn setup(&mut self, shell: &Shell) -> Result<(ThreadSetup, String), String> {
        let mcp = self.mcp(shell)?;
        let project = shell
            .studio
            .borrow()
            .session
            .as_ref()
            .map(|session| session.path().to_owned());
        let live = self.live.load(Ordering::Relaxed);
        let cwd = project
            .clone()
            .or_else(home_dir)
            .unwrap_or_else(|| ".".into());
        let key = format!("{live}:{}", project.clone().unwrap_or_default());
        Ok((
            ThreadSetup {
                cwd,
                model: self.model_id(),
                instructions: instructions(project.as_deref(), live),
                mcp,
            },
            key,
        ))
    }

    /// Asks the program which skills there are for the project the window has.
    fn load_skills(&mut self, shell: &Shell) {
        let Some(backend) = self.slot().backend.clone() else {
            return;
        };
        if !self.descriptor.caps.skills {
            return;
        }
        let cwd = shell
            .studio
            .borrow()
            .session
            .as_ref()
            .map(|session| session.path().to_owned())
            .or_else(home_dir)
            .unwrap_or_else(|| ".".into());
        backend.skills(
            &cwd,
            Box::new(|reply| {
                post(move |c, shell, app| {
                    if let Ok(skills) = reply {
                        c.skills = skills;
                        c.publish(shell, app);
                    }
                });
            }),
        );
    }

    fn add_files(&mut self, paths: Vec<PathBuf>) {
        for path in paths.into_iter().filter(|path| path.is_file()) {
            let id = self.mint("a");
            self.attachments.push(attached(id, &path));
        }
    }

    fn fail_turn(&mut self, reason: &str) {
        self.busy = false;
        self.turn = None;
        let id = self.mint("n");
        self.lines.push(Line::notice(&id, reason));
    }

    fn send(&mut self, shell: &Shell, app: &App, text: String) {
        let Some(backend) = self.slot().backend.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        let files = std::mem::take(&mut self.attachments);
        let text = text.trim().to_owned();
        if text.is_empty() && files.is_empty() && self.picked.is_empty() {
            return;
        }
        // What goes to the model: the words, the pictures as pictures, and
        // every other file as a path to bring in with the tools.
        let mut words = text.clone();
        let mut inputs: Vec<Input> = Vec::new();
        for file in &files {
            if file.shown.kind == "image" {
                inputs.push(Input::Image(file.path.clone()));
            } else {
                words.push_str(&format!(
                    "\n\nAttached {}: {}",
                    file.shown.kind,
                    file.path.display()
                ));
            }
        }
        inputs.insert(0, Input::Text(words));
        let skills = std::mem::take(&mut self.picked);
        for skill in &skills {
            inputs.push(Input::Skill {
                name: skill.name.clone(),
                path: skill.path.clone(),
            });
        }

        // What the person said, as the record keeps it.
        let id = self.mint("u");
        let said_item = Item::User {
            id: format!("{id}-{}", now()),
            text: text.clone(),
            images: files
                .iter()
                .filter(|file| file.shown.kind == "image")
                .map(|file| file.path.to_string_lossy().into_owned())
                .collect(),
        };
        let mut said = Line::new(&id, AgentRole::User);
        said.text = text.clone();
        said.images = skills
            .iter()
            .map(|skill| AgentAttachment {
                name: skill.name.as_str().into(),
                kind: "skill".into(),
                ..AgentAttachment::default()
            })
            .chain(files.iter().map(|file| file.shown.clone()))
            .collect();
        self.lines.push(said);
        self.busy = true;

        let (setup, key) = match self.setup(shell) {
            Ok(done) => done,
            Err(error) => {
                self.fail_turn(&error);
                self.publish(shell, app);
                return;
            }
        };
        let model = self.model_id();
        let effort = self.effort_id();
        let title: String = if text.is_empty() {
            files
                .first()
                .map(|file| file.shown.name.to_string())
                .unwrap_or_default()
        } else {
            text.chars().take(36).collect()
        };
        let project = setup.cwd.clone();
        let backend_id = self.descriptor.id.0;

        let asking = Arc::clone(&backend);
        let keeps_record = !self.descriptor.caps.remote_history;
        let go = move |c: &mut Controller, thread: String, key: String| {
            c.loaded.insert(thread.clone(), key);
            if keeps_record {
                c.recorder.watch(&thread);
                c.recorder.said(&thread, said_item);
            }
            c.begin_turn(&asking, &thread, inputs, model, effort);
        };
        match self.current.clone() {
            Some(thread) if self.loaded.get(&thread) == Some(&key) => {
                go(self, thread, key);
            }
            Some(thread) => {
                backend.resume(
                    &thread.clone(),
                    &setup,
                    Box::new(move |reply| {
                        post(move |c, shell, app| {
                            match reply {
                                Ok(_) => go(c, thread, key),
                                Err(error) => c.fail_turn(&error),
                            }
                            c.publish(shell, app);
                        });
                    }),
                );
            }
            None => {
                self.remember(shell);
                backend.start(
                    &setup,
                    Box::new(move |reply| {
                        post(move |c, shell, app| {
                            match reply {
                                Ok(thread) => {
                                    c.current = Some(thread.clone());
                                    c.store.put(Chat {
                                        id: thread.clone(),
                                        title,
                                        project,
                                        updated: now(),
                                        messages: 1,
                                        backend: backend_id.to_owned(),
                                    });
                                    go(c, thread, key);
                                }
                                Err(error) => c.fail_turn(&error),
                            }
                            c.publish(shell, app);
                        });
                    }),
                );
            }
        }
        self.publish(shell, app);
    }

    fn begin_turn(
        &mut self,
        backend: &Arc<dyn Backend>,
        thread: &str,
        inputs: Vec<Input>,
        model: Option<String>,
        effort: Option<String>,
    ) {
        let turn = Turn {
            thread: thread.to_owned(),
            inputs,
            model,
            effort,
        };
        backend.send(
            turn,
            Box::new(move |reply| {
                post(move |c, shell, app| {
                    match reply {
                        Ok(turn) => c.turn = turn,
                        Err(error) => c.fail_turn(&error),
                    }
                    c.publish(shell, app);
                });
            }),
        );
    }

    fn stop(&mut self) {
        let (Some(backend), Some(thread), Some(turn)) =
            (&self.slot().backend, &self.current, &self.turn)
        else {
            return;
        };
        backend.interrupt(thread, turn);
    }

    fn new_chat(&mut self) {
        self.current = None;
        self.lines.clear();
        self.attachments.clear();
        self.picked.clear();
        self.turn = None;
        self.busy = false;
    }

    /// Makes `descriptor`'s program the one the conversation on screen is
    /// with: its models and depths, its skills, and its program started if
    /// nothing has needed it yet.
    fn enter(&mut self, descriptor: &'static Descriptor, shell: &Shell, app: &App) {
        self.descriptor = descriptor;
        self.skills.clear();
        self.picked.clear();
        self.choose_defaults();
        self.ensure(descriptor, shell, app);
        self.load_skills(shell);
    }

    /// Remembers which program the person last began a conversation with, so
    /// that the next one is with it too.
    fn remember(&self, shell: &Shell) {
        let mut studio = shell.studio.borrow_mut();
        let studio = &mut *studio;
        if studio.prefs.agent_backend.as_deref() != Some(self.descriptor.id.0) {
            studio.prefs.agent_backend = Some(self.descriptor.id.0.to_owned());
            studio.prefs.save(&studio.host.dirs);
        }
    }

    fn open_chat(&mut self, shell: &Shell, app: &App, id: &str) {
        let Some(chat) = self.store.get(id).cloned() else {
            return;
        };
        self.new_chat();
        // The conversation goes on with the program it began with.
        let descriptor = backend::find(&chat.backend).unwrap_or(self.descriptor);
        self.enter(descriptor, shell, app);
        self.current = Some(chat.id.clone());
        if !descriptor.caps.remote_history {
            // What this program does not keep is read from the record.
            self.recorder.watch(&chat.id);
            let (items, _damaged) = self.transcripts.read(&chat.id);
            if items.is_empty() && chat.messages > 0 {
                let id = self.mint("n");
                self.lines
                    .push(Line::notice(&id, &tf("agent.recordMissing", &[&descriptor.name])));
            }
            for item in &items {
                self.put_item(item, true);
            }
        }
        self.take_up(shell, app);
        self.publish(shell, app);
    }

    /// Takes the conversation on screen up again in its program, once the
    /// program is there: its history is read back when the program keeps it,
    /// and what it needs to go on is given it again.
    fn take_up(&mut self, shell: &Shell, app: &App) {
        let Some(thread) = self.current.clone() else {
            return;
        };
        let Some(descriptor) = self
            .store
            .get(&thread)
            .and_then(|chat| backend::find(&chat.backend))
        else {
            return;
        };
        let Some(backend) = self
            .slots
            .get(descriptor.id.0)
            .and_then(|slot| slot.backend.clone())
        else {
            return;
        };
        // What is already taken up, and shown from the record, is left.
        if !descriptor.caps.remote_history && self.loaded.contains_key(&thread) {
            return;
        }
        let (setup, key) = match self.setup(shell) {
            Ok(done) => done,
            Err(error) => {
                self.fail_turn(&error);
                self.publish(shell, app);
                return;
            }
        };
        let remote_history = descriptor.caps.remote_history;
        backend.resume(
            &thread.clone(),
            &setup,
            Box::new(move |reply| {
                post(move |c, shell, app| {
                    if c.current.as_deref() != Some(thread.as_str()) {
                        return;
                    }
                    match reply {
                        Ok(items) => {
                            c.loaded.insert(thread, key);
                            if remote_history {
                                c.lines.clear();
                                for item in &items {
                                    c.put_item(item, true);
                                }
                            }
                        }
                        Err(error) => c.fail_turn(&error),
                    }
                    c.publish(shell, app);
                });
            }),
        );
    }

    fn delete_chat(&mut self, id: &str) {
        if let Some(chat) = self.store.get(id)
            && let Some(backend) = self.slots.get(chat.backend.as_str()).and_then(|slot| slot.backend.as_ref())
        {
            backend.archive(id);
        }
        self.transcripts.remove(id);
        self.recorder.forget(id);
        self.store.remove(id);
        self.loaded.remove(id);
        if self.current.as_deref() == Some(id) {
            self.new_chat();
        }
    }
}

/// Wires the pane's global to the controller and starts the agent.
pub fn install(app: &App) {
    let dirs = concat_host::AppDirs::locate().ok();
    let store = dirs
        .as_ref()
        .map(|dirs| Store::open(&dirs.config.join("agent-chats.json")))
        .unwrap_or_else(Store::in_memory);
    // Where a record is kept for the programs that do not keep their own.
    let transcripts = Transcripts::in_folder(
        &dirs
            .as_ref()
            .map(|dirs| dirs.config.join("agent-chats"))
            .unwrap_or_else(|| std::env::temp_dir().join("eloncut-agent-chats")),
    );
    // A new conversation is with the program the last one was with.
    let descriptor = dirs
        .as_ref()
        .map(crate::prefs::Preferences::load)
        .and_then(|prefs| prefs.agent_backend)
        .and_then(|id| backend::find(&id))
        .unwrap_or(backend::REGISTRY[0]);
    let mut slots: HashMap<&'static str, Slot> = backend::REGISTRY
        .iter()
        .map(|descriptor| (descriptor.id.0, Slot::default()))
        .collect();
    let preview = std::env::var("CONCAT_AGENT_STATUS").ok();
    if let Some(slot) = slots.get_mut(descriptor.id.0) {
        slot.link = match preview.as_deref() {
            Some("missing") => Link::Missing,
            Some("failed") => Link::Failed(String::new()),
            _ => Link::Idle,
        };
    }
    let controller = Controller {
        slots,
        descriptor,
        model: 0,
        effort: 0,
        store,
        recorder: Recorder::new(transcripts.clone()),
        transcripts,
        query: String::new(),
        current: None,
        lines: Vec::new(),
        turn: None,
        busy: false,
        loaded: HashMap::new(),
        attachments: Vec::new(),
        live: Arc::new(AtomicBool::new(true)),
        bridge: None,
        minted: 0,
        shown_project: String::new(),
        engaged: false,
        skills: Vec::new(),
        skill_query: String::new(),
        picked: Vec::new(),
    };
    CONTROLLER.with(|slot| *slot.borrow_mut() = Some(controller));

    let chat = app.global::<AgentChat>();
    chat.set_suggestions(ModelRc::new(VecModel::from(vec![
        SharedString::from(t("agent.suggestSilence")),
        SharedString::from(t("agent.suggestHighlight")),
        SharedString::from(t("agent.suggestCaptions")),
        SharedString::from(t("agent.suggestBeat")),
    ])));
    chat.set_live(true);

    chat.on_send(|text| with(|c, shell, app| c.send(shell, app, text.to_string())));
    chat.on_stop(|| with(|c, _, _| c.stop()));
    chat.on_new_session(|| {
        with(|c, shell, app| {
            c.new_chat();
            c.publish(shell, app);
        });
    });
    chat.on_open_session(|id| with(|c, shell, app| c.open_chat(shell, app, id.as_str())));
    chat.on_rename_session(|id, title| {
        with(|c, shell, app| {
            let title = title.trim().to_owned();
            if !title.is_empty() {
                c.store.edit(id.as_str(), |chat| chat.title = title.clone());
                // Told to the program the conversation is with, if it keeps names.
                if let Some(descriptor) = c
                    .store
                    .get(id.as_str())
                    .and_then(|chat| backend::find(&chat.backend))
                    && descriptor.caps.rename
                    && let Some(backend) = c.slots.get(descriptor.id.0).and_then(|slot| slot.backend.as_ref())
                {
                    backend.rename(id.as_str(), &title);
                }
            }
            c.publish(shell, app);
        });
    });
    chat.on_delete_session(|id| {
        with(|c, shell, app| {
            c.delete_chat(id.as_str());
            c.publish(shell, app);
        });
    });
    chat.on_search_sessions(|text| {
        with(|c, shell, app| {
            c.query = text.to_string();
            c.publish(shell, app);
        });
    });
    chat.on_attach_remove(|id| {
        with(|c, shell, app| {
            c.attachments.retain(|file| file.shown.id != id);
            c.publish(shell, app);
        });
    });
    chat.on_attach_pick(|| {
        // The dialog is asked with nothing borrowed, as every dialog is.
        let picked = crate::platform::pick_files("Attach", None).unwrap_or_default();
        with(move |c, shell, app| {
            c.add_files(picked);
            c.publish(shell, app);
        });
    });
    chat.on_skills_open(|| {
        with(|c, shell, _| {
            c.skill_query.clear();
            c.load_skills(shell);
        });
    });
    chat.on_skills_search(|text| {
        with(|c, shell, app| {
            c.skill_query = text.to_string();
            c.publish(shell, app);
        });
    });
    chat.on_skill_add(|name| {
        with(|c, shell, app| {
            if let Some(skill) = c.skills.iter().find(|skill| skill.name == name.as_str()).cloned()
                && !c.picked.iter().any(|picked| picked.name == skill.name)
            {
                c.picked.push(skill);
            }
            c.publish(shell, app);
        });
    });
    chat.on_skill_remove(|name| {
        with(|c, shell, app| {
            c.picked.retain(|skill| skill.name != name.as_str());
            c.publish(shell, app);
        });
    });
    chat.on_composer_focused(|| with(|c, _, _| c.engaged = true));
    chat.on_model_changed(|index| {
        with(|c, shell, app| {
            let kept = c.effort_id();
            c.model = (index.max(0) as usize).min(c.slot().models.len().saturating_sub(1));
            let efforts = c.efforts();
            c.effort = kept
                .and_then(|kept| efforts.iter().position(|known| *known == kept))
                .or_else(|| {
                    c.slot()
                        .models
                        .get(c.model)
                        .and_then(|model| efforts.iter().position(|e| *e == model.default_effort))
                })
                .unwrap_or(0);
            c.publish(shell, app);
        });
    });
    chat.on_effort_changed(|index| {
        with(|c, shell, app| {
            c.effort = (index.max(0) as usize).min(c.efforts().len().saturating_sub(1));
            c.publish(shell, app);
        });
    });
    chat.on_live_changed(|on| {
        with(|c, shell, app| {
            c.live.store(on, Ordering::Relaxed);
            c.publish(shell, app);
        });
    });
    chat.on_backend_changed(|index| {
        with(|c, shell, app| {
            // Only before the first message: a conversation stays with the
            // program it began with.
            if c.current.is_some() || c.busy {
                return;
            }
            let Some(descriptor) = backend::REGISTRY.get(index.max(0) as usize).copied() else {
                return;
            };
            c.enter(descriptor, shell, app);
            c.publish(shell, app);
        });
    });
    chat.on_retry(|| {
        with(|c, shell, app| {
            let descriptor = c.descriptor;
            c.begin(descriptor, shell, app);
        });
    });
    chat.on_open_help(|| {
        with(|c, _, _| {
            let _ = opener::open(c.descriptor.install.url);
        });
    });

    // The project's name in the pane's strip follows the window's: no event
    // says it changed, so it is looked at now and then.
    let watch = slint::Timer::default();
    watch.start(slint::TimerMode::Repeated, Duration::from_secs(2), || {
        with(|c, shell, app| {
            let studio = shell.studio.borrow();
            let now = if studio.session.is_some() {
                studio.project_name.clone()
            } else {
                String::new()
            };
            drop(studio);
            if now != c.shown_project {
                c.publish(shell, app);
            }
        });
    });
    std::mem::forget(watch);

    // Started once the event loop is running, so that what the worker posts
    // has somewhere to land.
    if preview.is_none() {
        let _ = slint::invoke_from_event_loop(|| {
            with(|c, shell, app| {
                let descriptor = c.descriptor;
                c.begin(descriptor, shell, app);
            });
        });
    } else {
        let _ = slint::invoke_from_event_loop(|| with(|c, shell, app| c.publish(shell, app)));
    }
}

/// Ends the agent with the window: Codex is stopped, and so is the API its
/// MCP server reached the project through.
pub fn shutdown() {
    CONTROLLER.with(|slot| {
        if let Some(controller) = slot.borrow_mut().as_mut() {
            for slot in controller.slots.values_mut() {
                slot.backend = None;
            }
            if let Some(server) = controller.bridge.take() {
                server.stop_detached();
            }
        }
    });
}

/// A press landed somewhere else in the editor: files dropped next belong
/// to the library again.
pub fn disengage() {
    CONTROLLER.with(|slot| {
        if let Some(controller) = slot.borrow_mut().as_mut() {
            controller.engaged = false;
        }
    });
}

/// Whether files dropped on the window now are for the agent: its pane is
/// on screen and the person was last at its composer.
pub fn wants_drop(shell: &Shell) -> bool {
    let engaged = CONTROLLER.with(|slot| slot.borrow().as_ref().is_some_and(|c| c.engaged));
    engaged && {
        let studio = shell.studio.borrow();
        !studio.on_start && studio.dock.contains(crate::ui::PaneKind::Agent)
    }
}

/// Puts dropped files in the composer, to go with the next message.
pub fn attach(shell: &Shell, app: &App, paths: Vec<PathBuf>) {
    CONTROLLER.with(|slot| {
        if let Some(controller) = slot.borrow_mut().as_mut() {
            controller.add_files(paths);
            controller.publish(shell, app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_read_the_embedded_guide_before_editing_in_every_mode() {
        for project in [None, Some("/tmp/ElonCut Project")] {
            for live in [false, true] {
                let text = instructions(project, live);
                let guide = text
                    .find("call guide_read without arguments")
                    .expect("directory first");
                assert!(text.contains("call guide_read with topic"));
                assert!(
                    guide
                        < text
                            .find("Read the timeline")
                            .expect("editing instructions")
                );
            }
        }
        assert!(instructions(Some("/tmp/project"), true).contains("edit it directly"));
        assert!(
            instructions(Some("/tmp/project"), false).contains("Do not open or edit that folder")
        );
    }

    #[test]
    fn markdown_marks_do_not_show_as_stray_characters() {
        assert_eq!(plain("已经剪成 **5.5 秒** 的片子"), "已经剪成 5.5 秒 的片子");
        assert_eq!(plain("## 步骤\n- 一\n* 二"), "步骤\n• 一\n• 二");
        assert_eq!(plain("plain"), "plain");
    }

    #[test]
    fn a_long_answer_is_cut_and_marked() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 3), "abc…");
    }
}
