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

use concat_agent::rpc::{Codex, Incoming};
use concat_agent::store::{Chat, Store};
use concat_agent::wire::{
    self, Input, Item, McpServer, Model, Skill, ThreadSetup, ToolState, Update,
};
use serde_json::{Value, json};
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

/// How the agent is running.
enum Link {
    Starting,
    Missing,
    Failed(String),
    Ready,
}

struct Controller {
    link: Link,
    codex: Option<Arc<Codex>>,
    models: Vec<Model>,
    /// What the person's own Codex settings choose.
    defaults: (Option<String>, Option<String>),
    model: usize,
    effort: usize,
    store: Store,
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
    /// The skills Codex has, as last asked, and the search over them.
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
        "You are the editing agent inside MindCut, a desktop video editor. You edit video by \
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

    fn model_id(&self) -> Option<String> {
        self.models.get(self.model).map(|model| model.id.clone())
    }

    fn efforts(&self) -> Vec<String> {
        self.models
            .get(self.model)
            .map(|model| model.efforts.clone())
            .unwrap_or_else(|| wire::COMMON_EFFORTS.map(str::to_owned).to_vec())
    }

    fn effort_id(&self) -> Option<String> {
        self.efforts().get(self.effort).cloned()
    }

    /// Picks the model and depth Codex would start with, from what the
    /// person's own settings say.
    fn choose_defaults(&mut self) {
        let (model, effort) = self.defaults.clone();
        if let Some(model) = &model {
            // A model the settings name that Codex does not list - a
            // provider of the person's own - is offered all the same.
            if !self.models.iter().any(|known| &known.id == model) {
                self.models.insert(
                    0,
                    Model {
                        id: model.clone(),
                        name: model.clone(),
                        efforts: wire::COMMON_EFFORTS.map(str::to_owned).to_vec(),
                        default_effort: effort.clone().unwrap_or_else(|| "medium".into()),
                    },
                );
            }
        }
        self.model = model
            .and_then(|model| self.models.iter().position(|known| known.id == model))
            .unwrap_or(0);
        let efforts = self.efforts();
        let wanted = effort.or_else(|| self.models.get(self.model).map(|m| m.default_effort.clone()));
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
        match &self.link {
            Link::Ready => {
                chat.set_status(AgentStatus::Ready);
                chat.set_status_detail(SharedString::new());
            }
            Link::Starting => {
                chat.set_status(AgentStatus::Connecting);
                chat.set_status_detail(SharedString::new());
            }
            Link::Missing => {
                chat.set_status(AgentStatus::Missing);
                chat.set_status_detail(SharedString::new());
            }
            Link::Failed(reason) => {
                chat.set_status(AgentStatus::Failed);
                chat.set_status_detail(reason.as_str().into());
            }
        }
        chat.set_busy(self.busy);
        chat.set_activity(self.activity().into());
        chat.set_live(self.live.load(Ordering::Relaxed));
        chat.set_models(ModelRc::new(VecModel::from(
            self.models
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
                detail: format!("{} · {}", folder_name(&entry.project), ago(entry.updated)).into(),
                count: entry.messages as i32,
                running: self.busy && self.current.as_deref() == Some(entry.id.as_str()),
                current: self.current.as_deref() == Some(entry.id.as_str()),
            })
            .collect();
        chat.set_sessions(ModelRc::new(VecModel::from(sessions)));
    }

    // ── starting the agent ──

    fn begin(&mut self, shell: &Shell, app: &App) {
        self.link = Link::Starting;
        self.codex = None;
        self.publish(shell, app);
        std::thread::spawn(|| {
            let Some(program) = concat_agent::locate::codex() else {
                post(|c, shell, app| {
                    c.link = Link::Missing;
                    c.publish(shell, app);
                });
                return;
            };
            log::info!("agent: starting {}", program.display());
            let spawned = Codex::spawn(&program, |incoming| {
                post(move |c, shell, app| c.heard(incoming, shell, app));
            });
            let codex = match spawned.and_then(|codex| {
                codex.handshake("eloncut", "MindCut", env!("CARGO_PKG_VERSION"))?;
                Ok(codex)
            }) {
                Ok(codex) => codex,
                Err(error) => {
                    log::warn!("agent: Codex did not start: {error}");
                    post(move |c, shell, app| {
                        c.link = Link::Failed(error);
                        c.publish(shell, app);
                    });
                    return;
                }
            };
            let patience = Duration::from_secs(20);
            let models = codex
                .call_blocking("model/list", json!({}), patience)
                .map(|result| wire::read_models(&result))
                .unwrap_or_default();
            let defaults = codex
                .call_blocking("config/read", json!({}), patience)
                .map(|result| wire::read_defaults(&result))
                .unwrap_or_default();
            let codex = Arc::new(codex);
            post(move |c, shell, app| {
                c.models = models;
                c.defaults = defaults;
                c.choose_defaults();
                c.codex = Some(codex);
                c.link = Link::Ready;
                c.load_skills(shell);
                c.publish(shell, app);
            });
        });
    }

    /// What Codex said without being asked.
    fn heard(&mut self, incoming: Incoming, shell: &Shell, app: &App) {
        match incoming {
            Incoming::Notification { method, params } => {
                if let Some(update) = wire::read_update(&method, &params) {
                    self.update(update);
                    self.publish(shell, app);
                }
            }
            Incoming::Request { id, method, .. } => {
                if let Some(codex) = &self.codex {
                    codex.respond(id, wire::auto_answer(&method));
                }
            }
            Incoming::Closed(reason) => {
                log::warn!("agent: Codex ended: {reason}");
                self.codex = None;
                self.busy = false;
                self.turn = None;
                self.loaded.clear();
                self.link = Link::Failed(reason);
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
            let (exporter, transcriber, open_projects, dirs) = {
                let studio = shell.studio.borrow();
                (
                    studio.host.exporter.clone(),
                    Arc::clone(&studio.host.transcriber),
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
                api.share_transcriber(transcriber);
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

    /// Asks Codex which skills there are for the project the window has.
    fn load_skills(&mut self, shell: &Shell) {
        let Some(codex) = self.codex.clone() else {
            return;
        };
        let cwd = shell
            .studio
            .borrow()
            .session
            .as_ref()
            .map(|session| session.path().to_owned())
            .or_else(home_dir)
            .unwrap_or_else(|| ".".into());
        codex.call("skills/list", wire::skills_params(&cwd), |reply| {
            post(move |c, shell, app| {
                if let Ok(result) = reply {
                    c.skills = wire::read_skills(&result);
                    c.publish(shell, app);
                }
            });
        });
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
        let Some(codex) = self.codex.clone() else {
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

        let id = self.mint("u");
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

        let asking = Arc::clone(&codex);
        let go = move |c: &mut Controller, thread: String, key: String| {
            c.loaded.insert(thread.clone(), key);
            c.begin_turn(&asking, &thread, inputs, model, effort);
        };
        match self.current.clone() {
            Some(thread) if self.loaded.get(&thread) == Some(&key) => {
                go(self, thread, key);
            }
            Some(thread) => {
                let params = setup.resume_params(&thread);
                codex.call("thread/resume", params, move |reply| {
                    post(move |c, shell, app| {
                        match reply {
                            Ok(_) => go(c, thread, key),
                            Err(error) => c.fail_turn(&error),
                        }
                        c.publish(shell, app);
                    });
                });
            }
            None => {
                codex.call("thread/start", setup.start_params(), move |reply| {
                    post(move |c, shell, app| {
                        match reply {
                            Ok(result) => {
                                let id = result
                                    .get("thread")
                                    .and_then(|thread| thread.get("id"))
                                    .and_then(Value::as_str)
                                    .map(str::to_owned);
                                match id {
                                    Some(thread) => {
                                        c.current = Some(thread.clone());
                                        c.store.put(Chat {
                                            id: thread.clone(),
                                            title,
                                            project,
                                            updated: now(),
                                            messages: 1,
                                        });
                                        go(c, thread, key);
                                    }
                                    None => c.fail_turn("Codex did not start a conversation"),
                                }
                            }
                            Err(error) => c.fail_turn(&error),
                        }
                        c.publish(shell, app);
                    });
                });
            }
        }
        self.publish(shell, app);
    }

    fn begin_turn(
        &mut self,
        codex: &Arc<Codex>,
        thread: &str,
        inputs: Vec<Input>,
        model: Option<String>,
        effort: Option<String>,
    ) {
        let params = wire::turn_params(thread, &inputs, model.as_deref(), effort.as_deref());
        codex.call("turn/start", params, move |reply| {
            post(move |c, shell, app| {
                match reply {
                    Ok(result) => {
                        c.turn = result
                            .get("turn")
                            .and_then(|turn| turn.get("id"))
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                    Err(error) => c.fail_turn(&error),
                }
                c.publish(shell, app);
            });
        });
    }

    fn stop(&mut self) {
        let (Some(codex), Some(thread), Some(turn)) =
            (&self.codex, &self.current, &self.turn)
        else {
            return;
        };
        codex.call("turn/interrupt", wire::interrupt_params(thread, turn), |reply| {
            if let Err(error) = reply {
                log::warn!("agent: stopping the turn failed: {error}");
            }
        });
    }

    fn new_chat(&mut self) {
        self.current = None;
        self.lines.clear();
        self.attachments.clear();
        self.picked.clear();
        self.turn = None;
        self.busy = false;
    }

    fn open_chat(&mut self, shell: &Shell, app: &App, id: &str) {
        let Some(codex) = self.codex.clone() else {
            return;
        };
        let Some(chat) = self.store.get(id).cloned() else {
            return;
        };
        let (setup, key) = match self.setup(shell) {
            Ok(done) => done,
            Err(error) => {
                self.new_chat();
                self.fail_turn(&error);
                self.publish(shell, app);
                return;
            }
        };
        self.new_chat();
        self.current = Some(chat.id.clone());
        self.publish(shell, app);
        let thread = chat.id;
        codex.call("thread/resume", setup.resume_params(&thread), move |reply| {
            post(move |c, shell, app| {
                if c.current.as_deref() != Some(thread.as_str()) {
                    return;
                }
                match reply {
                    Ok(result) => {
                        c.loaded.insert(thread, key);
                        let items = result.get("thread").map(wire::read_history).unwrap_or_default();
                        c.lines.clear();
                        for item in &items {
                            c.put_item(item, true);
                        }
                    }
                    Err(error) => c.fail_turn(&error),
                }
                c.publish(shell, app);
            });
        });
    }

    fn delete_chat(&mut self, id: &str) {
        if let Some(codex) = &self.codex {
            codex.call("thread/archive", json!({ "threadId": id }), |_| {});
        }
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
        .map(|dirs| Store::open(&dirs.config.join("agent-chats.json")))
        .unwrap_or_else(Store::in_memory);
    let preview = std::env::var("CONCAT_AGENT_STATUS").ok();
    let controller = Controller {
        link: match preview.as_deref() {
            Some("missing") => Link::Missing,
            Some("failed") => Link::Failed(String::new()),
            Some("connecting") => Link::Starting,
            _ => Link::Starting,
        },
        codex: None,
        models: Vec::new(),
        defaults: (None, None),
        model: 0,
        effort: 0,
        store,
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
                if let Some(codex) = &c.codex {
                    codex.call(
                        "thread/name/set",
                        json!({ "threadId": id.as_str(), "name": title }),
                        |_| {},
                    );
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
            c.model = (index.max(0) as usize).min(c.models.len().saturating_sub(1));
            let efforts = c.efforts();
            c.effort = kept
                .and_then(|kept| efforts.iter().position(|known| *known == kept))
                .or_else(|| {
                    c.models
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
    chat.on_retry(|| with(|c, shell, app| c.begin(shell, app)));
    chat.on_open_help(|| {
        let _ = opener::open("https://developers.openai.com/codex/cli");
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
        let _ = slint::invoke_from_event_loop(|| with(|c, shell, app| c.begin(shell, app)));
    } else {
        let _ = slint::invoke_from_event_loop(|| with(|c, shell, app| c.publish(shell, app)));
    }
}

/// Ends the agent with the window: Codex is stopped, and so is the API its
/// MCP server reached the project through.
pub fn shutdown() {
    CONTROLLER.with(|slot| {
        if let Some(controller) = slot.borrow_mut().as_mut() {
            controller.codex = None;
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
        for project in [None, Some("/tmp/MindCut Project")] {
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
