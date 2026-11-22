// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The Agent pane's controller.
//!
//! The pane (`ui/workspace/agent-pane.slint`) draws whatever is on the
//! `AgentChat` global and reports what the person does through its
//! callbacks. This is the other end: it starts the chosen local backend
//! (`concat-agent`), keeps the list of conversations, turns backend updates
//! into the messages the pane draws, and starts the agent with the Concat
//! MCP server so that what it does is editing - on the project open in the
//! window, as it is on screen, when live mode is on (`live.rs`).
//!
//! The controller runs on the event-loop thread. What arrives from a backend
//! is posted there; nothing in this file waits for it.
//!
//! `CONCAT_AGENT_STATUS=missing|connecting|failed` opens the pane on one of
//! its status pages without starting anything, since a computer with Codex
//! on it never shows them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use concat_agent::backend::{
    Availability, Backend, CodexBackend, Defaults, Done, Event, NamedMcpServer, OnEvent, Provider,
    SessionSetup,
};
use concat_agent::claude::ClaudeBackend;
use concat_agent::store::{Chat, Store};
use concat_agent::wire::{self, Input, Item, McpServer, Model, Skill, ToolState, Update};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::host::Shell;
use crate::i18n::{t, tf};
use crate::ui::{
    AgentAttachment, AgentChat, AgentMessage, AgentProviderOption, AgentRole, AgentSession,
    AgentSkill, AgentStatus, AgentToolCall, App,
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

const PROVIDERS: [Provider; 2] = [Provider::Codex, Provider::Claude];

fn provider_index(provider: Provider) -> usize {
    match provider {
        Provider::Codex => 0,
        Provider::Claude => 1,
    }
}

fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "Codex",
        Provider::Claude => "Claude",
    }
}

fn status_view(provider: Provider, link: &Link) -> (AgentStatus, String, String) {
    let (status, reason) = match link {
        Link::Starting => (AgentStatus::Connecting, ""),
        Link::Missing => (AgentStatus::Missing, ""),
        Link::Failed(reason) => (AgentStatus::Failed, reason.as_str()),
        Link::Ready => (AgentStatus::Ready, ""),
    };
    if provider == Provider::Codex || status == AgentStatus::Ready {
        return (status, String::new(), reason.into());
    }
    let (title, details) = match status {
        AgentStatus::Connecting => ("agent.claudeConnecting", "agent.claudeConnectingBody"),
        AgentStatus::Missing => ("agent.claudeMissingTitle", "agent.claudeMissingBody"),
        _ => (
            "agent.claudeUnavailableTitle",
            "agent.claudeUnavailableBody",
        ),
    };
    let mut detail = t(details);
    if !reason.is_empty() {
        detail.push_str(&format!("\n{reason}"));
    }
    if matches!(status, AgentStatus::Missing | AgentStatus::Failed) {
        detail.push_str(&format!("\n{}", t("agent.claudeTerms")));
    }
    (status, t(title), detail)
}

fn connect_backend(
    provider: Provider,
    history: Option<PathBuf>,
    on_event: OnEvent,
) -> Result<Arc<dyn Backend>, Link> {
    match provider {
        Provider::Codex => {
            let program = concat_agent::locate::codex().ok_or(Link::Missing)?;
            log::info!("agent: starting {}", program.display());
            let backend = CodexBackend::connect(
                &program,
                "eloncut",
                "MindCut",
                env!("CARGO_PKG_VERSION"),
                on_event,
            )
            .map_err(Link::Failed)?;
            Ok(Arc::new(backend))
        }
        Provider::Claude => {
            let program = concat_agent::locate::claude().ok_or(Link::Missing)?;
            match ClaudeBackend::status_for(Some(&program)) {
                Availability::Available => {}
                Availability::NotInstalled => return Err(Link::Missing),
                Availability::Unavailable(reason) => return Err(Link::Failed(reason)),
            }
            let history = history
                .ok_or_else(|| Link::Failed("The settings folder is unavailable.".into()))?;
            Ok(Arc::new(ClaudeBackend::new(program, history, on_event)))
        }
    }
}

fn probe_backend(provider: Provider) -> Availability {
    match provider {
        Provider::Codex => CodexBackend::status(),
        Provider::Claude => ClaudeBackend::status(),
    }
}

struct Controller {
    link: Link,
    provider: Provider,
    availability: [Option<Availability>; 2],
    backend: Option<Arc<dyn Backend>>,
    history: Option<PathBuf>,
    generation: u64,
    view: u64,
    models: Vec<Model>,
    /// What the person's own Codex settings choose.
    defaults: Defaults,
    choices: [Defaults; 2],
    model: usize,
    effort: usize,
    store: Store,
    query: String,
    /// The thread on screen, once it exists: the first message creates it.
    current: Option<String>,
    lines: Vec<Line>,
    turn: Option<String>,
    busy: bool,
    in_flight: HashSet<String>,
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
    fn new(store: Store, history: Option<PathBuf>) -> Self {
        Self {
            link: Link::Starting,
            provider: Provider::default(),
            availability: [None, None],
            backend: None,
            history,
            generation: 0,
            view: 0,
            models: Vec::new(),
            defaults: Defaults::default(),
            choices: Default::default(),
            model: 0,
            effort: 0,
            store,
            query: String::new(),
            current: None,
            lines: Vec::new(),
            turn: None,
            busy: false,
            in_flight: HashSet::new(),
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
        }
    }

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
            .unwrap_or_else(|| match self.provider {
                Provider::Codex => wire::COMMON_EFFORTS.map(str::to_owned).to_vec(),
                Provider::Claude => Vec::new(),
            })
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
        let (last_model, last_effort) = &self.choices[provider_index(self.provider)];
        self.model = last_model
            .as_ref()
            .or(model.as_ref())
            .and_then(|model| self.models.iter().position(|known| &known.id == model))
            .unwrap_or(0);
        let efforts = self.efforts();
        self.effort = last_effort
            .iter()
            .chain(effort.iter())
            .chain(
                self.models
                    .get(self.model)
                    .map(|model| &model.default_effort),
            )
            .find_map(|wanted| efforts.iter().position(|known| known == wanted))
            .unwrap_or(0);
    }

    fn provider_options(&self) -> Vec<AgentProviderOption> {
        PROVIDERS
            .into_iter()
            .map(|provider| {
                let status = &self.availability[provider_index(provider)];
                let reason = match status {
                    Some(Availability::Available) => String::new(),
                    Some(Availability::Unavailable(reason)) => reason.clone(),
                    Some(Availability::NotInstalled) if provider == Provider::Claude => {
                        t("agent.claudeMissingTitle")
                    }
                    Some(Availability::NotInstalled) => t("agent.missingTitle"),
                    None if provider == Provider::Claude => t("agent.claudeConnecting"),
                    None => t("agent.connecting"),
                };
                AgentProviderOption {
                    id: provider.as_str().into(),
                    label: provider_label(provider).into(),
                    available: status == &Some(Availability::Available),
                    reason: reason.into(),
                }
            })
            .collect()
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
        let (status, title, reason) = status_view(self.provider, &self.link);
        chat.set_status(status);
        chat.set_status_title(title.into());
        chat.set_status_detail(reason.into());
        chat.set_provider(self.provider.as_str().into());
        chat.set_providers(ModelRc::new(VecModel::from(self.provider_options())));
        chat.set_can_switch_provider(!self.reply_active());
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
                provider: self
                    .store
                    .provider(&entry.id)
                    .unwrap_or_default()
                    .as_str()
                    .into(),
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
        self.generation += 1;
        let generation = self.generation;
        let provider = self.provider;
        let history = self.history.clone();
        self.set_link(Link::Starting);
        self.backend = None;
        self.loaded.clear();
        self.in_flight.clear();
        self.models.clear();
        self.model = 0;
        self.effort = 0;
        self.skills.clear();
        self.publish(shell, app);
        std::thread::spawn(move || {
            let event = Arc::new(move |event| {
                post(move |c, shell, app| {
                    if c.heard(generation, event) {
                        c.publish(shell, app);
                    }
                });
            });
            let backend = match connect_backend(provider, history, event) {
                Ok(backend) => backend,
                Err(link) => {
                    post(move |c, shell, app| {
                        if c.generation == generation {
                            c.set_link(link);
                            c.publish(shell, app);
                        }
                    });
                    return;
                }
            };
            let config = Arc::clone(&backend);
            let connected = Arc::clone(&backend);
            backend.models(Box::new(move |models| {
                config.defaults(Box::new(move |defaults| {
                    post(move |c, shell, app| {
                        if c.generation != generation {
                            return;
                        }
                        c.models = models.unwrap_or_default();
                        c.defaults = defaults.unwrap_or_default();
                        c.choose_defaults();
                        c.backend = Some(connected);
                        c.set_link(Link::Ready);
                        c.load_skills(shell);
                        c.restore_current(shell, app);
                        c.publish(shell, app);
                    });
                }));
            }));
        });
    }

    fn set_link(&mut self, link: Link) {
        self.availability[provider_index(self.provider)] = match &link {
            Link::Starting => None,
            Link::Missing => Some(Availability::NotInstalled),
            Link::Failed(reason) => Some(Availability::Unavailable(reason.clone())),
            Link::Ready => Some(Availability::Available),
        };
        self.link = link;
    }

    fn probe_other(&self) {
        let provider = match self.provider {
            Provider::Codex => Provider::Claude,
            Provider::Claude => Provider::Codex,
        };
        std::thread::spawn(move || {
            let status = probe_backend(provider);
            post(move |c, shell, app| {
                if c.provider != provider {
                    c.availability[provider_index(provider)] = Some(status);
                    c.publish(shell, app);
                }
            });
        });
    }

    fn heard(&mut self, generation: u64, event: Event) -> bool {
        if self.generation != generation {
            return false;
        }
        match event {
            Event::Update(update) => self.update(update),
            Event::Closed(reason) => {
                log::warn!("agent: {} ended: {reason}", provider_label(self.provider));
                self.generation += 1;
                self.backend = None;
                self.busy = false;
                self.turn = None;
                self.in_flight.clear();
                self.loaded.clear();
                self.set_link(Link::Failed(reason));
            }
        }
        true
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
                self.in_flight.remove(&thread);
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
                if thread
                    .as_deref()
                    .is_none_or(|thread| ours(thread, &self.current))
                {
                    let id = self.mint("n");
                    self.lines.push(Line::notice(&id, &message));
                }
            }
            Update::Named { thread, name } if self.provider == Provider::Claude => {
                self.store.edit(&thread, |chat| chat.title = name);
            }
            _ => {}
        }
    }

    /// The agent's MCP server, reaching the window's project through an API
    /// of its own that this process serves.
    fn mcp(&mut self, shell: &Shell) -> Result<McpServer, String> {
        if self.bridge.is_none() {
            let (exporter, transcriber, speech, open_projects, dirs) = {
                let studio = shell.studio.borrow();
                (
                    studio.host.exporter.clone(),
                    Arc::clone(&studio.host.transcriber),
                    Arc::clone(&studio.host.speech),
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
                api.share_speech(speech);
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
    fn setup(&mut self, shell: &Shell) -> Result<(SessionSetup, String), String> {
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
            SessionSetup {
                cwd,
                model: self.model_id(),
                instructions: instructions(project.as_deref(), live),
                mcp: NamedMcpServer {
                    name: wire::SERVER_NAME.into(),
                    server: mcp,
                },
            },
            key,
        ))
    }

    /// Asks Codex which skills there are for the project the window has.
    fn load_skills(&mut self, shell: &Shell) {
        let Some(backend) = &self.backend else {
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
        let generation = self.generation;
        backend.skills(
            &cwd,
            Box::new(move |reply| {
                post(move |c, shell, app| {
                    if c.generation == generation
                        && let Ok(skills) = reply
                    {
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
        let Some(backend) = self.backend.clone() else {
            return;
        };
        if self.busy
            || self
                .current
                .as_ref()
                .is_some_and(|id| self.in_flight.contains(id))
        {
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

        let generation = self.generation;
        let view = self.view;
        let go = move |c: &mut Controller, thread: String, key: String| {
            c.loaded.insert(thread.clone(), key);
            c.begin_turn(&thread, inputs, model, effort);
        };
        match self.current.clone() {
            Some(thread) if self.loaded.get(&thread) == Some(&key) => {
                go(self, thread, key);
            }
            Some(thread) => {
                let resumed = thread.clone();
                backend.resume_session(
                    setup,
                    &thread,
                    Box::new(move |reply| {
                        post(move |c, shell, app| {
                            if c.generation != generation || c.view != view {
                                return;
                            }
                            match reply {
                                Ok(_) => go(c, resumed, key),
                                Err(error) => c.fail_turn(&error),
                            }
                            c.publish(shell, app);
                        });
                    }),
                );
            }
            None => {
                backend.start_session(
                    setup,
                    Box::new(move |reply| {
                        post(move |c, shell, app| {
                            if c.generation != generation || c.view != view {
                                return;
                            }
                            match reply {
                                Ok(session) => {
                                    let thread = session.id;
                                    c.remember_session(&thread, title, project);
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
        thread: &str,
        inputs: Vec<Input>,
        model: Option<String>,
        effort: Option<String>,
    ) {
        let generation = self.generation;
        let view = self.view;
        let id = thread.to_owned();
        self.dispatch_turn(
            thread,
            inputs,
            model,
            effort,
            Box::new(move |reply| {
                post(move |c, shell, app| {
                    if c.generation != generation {
                        return;
                    }
                    if reply.is_err() {
                        c.in_flight.remove(&id);
                    }
                    if c.view != view || !c.busy {
                        c.publish(shell, app);
                        return;
                    }
                    match reply {
                        Ok(turn) => c.turn = Some(turn),
                        Err(error) => c.fail_turn(&error),
                    }
                    c.publish(shell, app);
                });
            }),
        );
    }

    fn dispatch_turn(
        &mut self,
        thread: &str,
        inputs: Vec<Input>,
        model: Option<String>,
        effort: Option<String>,
        done: Done<String>,
    ) {
        if let Some(backend) = &self.backend {
            self.in_flight.insert(thread.into());
            backend.send_turn(thread, inputs, model, effort, done);
        } else {
            done(Err(t("agent.failedBody")));
        }
    }

    fn stop(&mut self) {
        let (Some(backend), Some(thread), Some(turn)) = (&self.backend, &self.current, &self.turn)
        else {
            return;
        };
        backend.interrupt(
            thread,
            turn,
            Box::new(|reply| {
                if let Err(error) = reply {
                    log::warn!("agent: stopping the turn failed: {error}");
                }
            }),
        );
    }

    fn remember_session(&mut self, thread: &str, title: String, project: String) {
        self.current = Some(thread.into());
        self.store.put_with_provider(
            Chat {
                id: thread.into(),
                title,
                project,
                updated: now(),
                messages: 1,
            },
            self.provider,
        );
    }

    fn change_provider(&mut self, provider: Provider) {
        if matches!(self.link, Link::Ready) {
            self.choices[provider_index(self.provider)] = (self.model_id(), self.effort_id());
        }
        self.provider = provider;
        self.picked.clear();
        self.skill_query.clear();
    }

    fn reply_active(&self) -> bool {
        self.busy || !self.in_flight.is_empty()
    }

    fn can_select_provider(&self, provider: Provider) -> bool {
        !self.reply_active()
            && self.availability[provider_index(provider)] == Some(Availability::Available)
    }

    fn select_provider(&mut self, shell: &Shell, app: &App, provider: Provider) {
        if !self.can_select_provider(provider) {
            return;
        }
        {
            let mut studio = shell.studio.borrow_mut();
            studio.prefs.agent_provider = Some(provider.as_str().into());
            studio.prefs.save(&studio.host.dirs);
        }
        if self.switch_conversation_provider(provider) {
            self.begin(shell, app);
        }
    }

    fn switch_conversation_provider(&mut self, provider: Provider) -> bool {
        if self.provider == provider {
            return false;
        }
        if self.current.is_some() || self.lines.iter().any(|line| line.role == AgentRole::User) {
            self.new_chat();
        }
        self.change_provider(provider);
        true
    }

    fn new_chat(&mut self) {
        self.view += 1;
        self.current = None;
        self.lines.clear();
        self.attachments.clear();
        self.picked.clear();
        self.turn = None;
        self.busy = false;
    }

    fn open_chat(&mut self, shell: &Shell, app: &App, id: &str) {
        match self.prepare_open_chat(id) {
            Some(true) => self.begin(shell, app),
            Some(false) => {
                self.restore_current(shell, app);
                self.publish(shell, app);
            }
            None => {}
        }
    }

    fn prepare_open_chat(&mut self, id: &str) -> Option<bool> {
        let provider = self.store.provider(id)?;
        if self.reply_active() && (provider != self.provider || provider == Provider::Claude) {
            return None;
        }
        self.new_chat();
        self.current = Some(id.into());
        if self.provider != provider {
            self.change_provider(provider);
            return Some(true);
        }
        Some(false)
    }

    fn restore_current(&mut self, shell: &Shell, app: &App) {
        let (Some(backend), Some(thread)) = (self.backend.clone(), self.current.clone()) else {
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
        let generation = self.generation;
        let view = self.view;
        let id = thread.clone();
        backend.resume_session(
            setup,
            &thread,
            Box::new(move |reply| {
                post(move |c, shell, app| {
                    if c.generation != generation || c.view != view || c.busy {
                        return;
                    }
                    match reply {
                        Ok(session) => {
                            c.restore_items(id, key, &session.history);
                        }
                        Err(error) => c.fail_turn(&error),
                    }
                    c.publish(shell, app);
                });
            }),
        );
    }

    fn restore_items(&mut self, id: String, key: String, items: &[Item]) {
        self.loaded.insert(id, key);
        self.lines.clear();
        for item in items {
            self.put_item(item, true);
        }
    }

    fn chat_backend(&self, id: &str) -> Option<&dyn Backend> {
        let backend = self.backend.as_deref()?;
        (self.store.provider(id) == Some(backend.provider())).then_some(backend)
    }

    fn delete_chat(&mut self, id: &str) {
        if self.provider == Provider::Claude
            && self.current.as_deref() == Some(id)
            && self.reply_active()
        {
            return;
        }
        if let Some(backend) = self.chat_backend(id) {
            backend.archive(id, Box::new(|_| {}));
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
        .as_ref()
        .map(|dirs| Store::open(&dirs.config.join("agent-chats.json")))
        .unwrap_or_else(Store::in_memory);
    let preview = std::env::var("CONCAT_AGENT_STATUS").ok();
    let mut controller = Controller::new(store, dirs.map(|dirs| dirs.config.join("agent-claude")));
    controller.set_link(match preview.as_deref() {
        Some("missing") => Link::Missing,
        Some("failed") => Link::Failed(String::new()),
        _ => Link::Starting,
    });
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
            if c.provider == Provider::Claude && c.reply_active() {
                return;
            }
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
                if let Some(backend) = c.chat_backend(id.as_str()) {
                    backend.rename(id.as_str(), &title, Box::new(|_| {}));
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
    chat.on_provider_changed(|provider| {
        if let Some(provider) = Provider::from_id(provider.as_str()) {
            with(|c, shell, app| c.select_provider(shell, app, provider));
        }
    });
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
    chat.on_retry(|| {
        with(|c, shell, app| {
            c.begin(shell, app);
            c.probe_other();
        })
    });
    chat.on_open_help(|| {
        with(|c, _, _| {
            let link = match c.provider {
                Provider::Codex => "https://developers.openai.com/codex/cli",
                Provider::Claude => "https://code.claude.com/docs/en/quickstart",
            };
            let _ = opener::open(link);
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
                c.provider = shell.studio.borrow().prefs.agent_provider();
                c.begin(shell, app);
                c.probe_other();
            })
        });
    } else {
        let _ = slint::invoke_from_event_loop(|| with(|c, shell, app| c.publish(shell, app)));
    }
}

/// Ends the agent with the window: the backend is stopped, and so is the API its
/// MCP server reached the project through.
pub fn shutdown() {
    CONTROLLER.with(|slot| {
        if let Some(controller) = slot.borrow_mut().as_mut() {
            controller.generation += 1;
            controller.backend = None;
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
    use concat_agent::backend::Session;

    struct TestBackend {
        provider: Provider,
        events: OnEvent,
    }

    impl Backend for TestBackend {
        fn provider(&self) -> Provider {
            self.provider
        }

        fn start_session(&self, _setup: SessionSetup, done: Done<Session>) {
            done(Ok(Session {
                id: self.provider.as_str().into(),
                history: Vec::new(),
            }));
        }

        fn resume_session(&self, _setup: SessionSetup, id: &str, done: Done<Session>) {
            if id != self.provider.as_str() {
                return done(Err("Wrong history provider".into()));
            }
            done(Ok(Session {
                id: id.into(),
                history: vec![Item::Agent {
                    id: "past".into(),
                    text: provider_label(self.provider).into(),
                }],
            }));
        }

        fn send_turn(
            &self,
            id: &str,
            inputs: Vec<Input>,
            model: Option<String>,
            effort: Option<String>,
            done: Done<String>,
        ) {
            assert_eq!(self.provider, Provider::Claude);
            assert_eq!(inputs, [Input::Text("Hello".into())]);
            assert_eq!(model.as_deref(), Some("sonnet"));
            assert_eq!(effort.as_deref(), Some("medium"));
            done(Ok("turn".into()));
            for update in [
                Update::TurnStarted {
                    thread: id.into(),
                    turn: "turn".into(),
                },
                Update::AgentWords {
                    thread: id.into(),
                    item: "reply".into(),
                    words: "Hi!".into(),
                },
                Update::TurnEnded {
                    thread: id.into(),
                    turn: "turn".into(),
                    status: "completed".into(),
                    error: None,
                },
            ] {
                (self.events)(Event::Update(update));
            }
        }

        fn interrupt(&self, _id: &str, _turn: &str, done: Done<()>) {
            done(Ok(()));
        }
        fn models(&self, done: Done<Vec<Model>>) {
            done(Ok(Vec::new()));
        }
        fn defaults(&self, done: Done<Defaults>) {
            done(Ok(Defaults::default()));
        }
    }

    fn test_setup() -> SessionSetup {
        SessionSetup {
            cwd: "/project".into(),
            model: None,
            instructions: "edit this project".into(),
            mcp: NamedMcpServer {
                name: "editor".into(),
                server: McpServer {
                    command: "fake".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                },
            },
        }
    }

    fn test_call<T: Send + 'static>(call: impl FnOnce(Done<T>)) -> T {
        let (send, receive) = std::sync::mpsc::channel();
        call(Box::new(move |reply| {
            send.send(reply).expect("receiver is alive")
        }));
        receive
            .recv_timeout(Duration::from_secs(2))
            .expect("callback")
            .expect("success")
    }

    #[test]
    fn switching_provider_resets_only_a_conversation_with_messages() {
        let mut controller = Controller::new(Store::in_memory(), None);
        let empty_view = controller.view;
        assert!(controller.switch_conversation_provider(Provider::Claude));
        assert_eq!(controller.provider, Provider::Claude);
        assert_eq!(controller.view, empty_view);

        controller.remember_session("saved", "Saved chat".into(), "project".into());
        controller.lines.push(Line::new("message", AgentRole::User));
        assert!(!controller.switch_conversation_provider(Provider::Claude));
        assert_eq!(controller.current.as_deref(), Some("saved"));
        assert!(controller.switch_conversation_provider(Provider::Codex));
        assert_eq!(controller.provider, Provider::Codex);
        assert!(controller.current.is_none());
        assert!(controller.lines.is_empty());
        assert_ne!(controller.view, empty_view);
        assert_eq!(controller.store.provider("saved"), Some(Provider::Claude));
        assert_eq!(controller.store.list("")[0].id, "saved");
    }

    #[test]
    fn each_provider_restores_its_own_model_and_effort() {
        let mut controller = Controller::new(Store::in_memory(), None);
        let models = |first: &str, second: &str, effort: &str| {
            vec![
                Model {
                    id: first.into(),
                    name: first.into(),
                    efforts: vec!["low".into()],
                    default_effort: "low".into(),
                },
                Model {
                    id: second.into(),
                    name: second.into(),
                    efforts: vec!["medium".into(), effort.into()],
                    default_effort: "medium".into(),
                },
            ]
        };
        let codex = models("codex-default", "codex-other", "high");
        let claude = models("claude-default", "claude-other", "max");
        controller.set_link(Link::Ready);
        controller.models = codex.clone();
        controller.model = 1;
        controller.effort = 1;

        assert!(controller.switch_conversation_provider(Provider::Claude));
        controller.models = claude.clone();
        controller.choose_defaults();
        assert_eq!(controller.model_id().as_deref(), Some("claude-default"));
        assert_eq!(controller.efforts(), ["low"]);
        controller.model = 1;
        controller.effort = 1;

        assert!(controller.switch_conversation_provider(Provider::Codex));
        controller.models = codex;
        controller.choose_defaults();
        assert_eq!(controller.model_id().as_deref(), Some("codex-other"));
        assert_eq!(controller.effort_id().as_deref(), Some("high"));

        assert!(controller.switch_conversation_provider(Provider::Claude));
        controller.models = claude;
        controller.choose_defaults();
        assert_eq!(controller.model_id().as_deref(), Some("claude-other"));
        assert_eq!(controller.effort_id().as_deref(), Some("max"));
    }

    #[test]
    fn claude_turn_drives_controller_updates_and_blocks_provider_switching() {
        let (send, receive) = std::sync::mpsc::channel();
        let mut controller = Controller::new(Store::in_memory(), None);
        controller.availability = [Some(Availability::Available), Some(Availability::Available)];
        assert!(controller.can_select_provider(Provider::Claude));
        controller.change_provider(Provider::Claude);
        let backend = Arc::new(TestBackend {
            provider: controller.provider,
            events: Arc::new(move |event| send.send(event).expect("controller receiver")),
        });
        let session = test_call(|done| backend.start_session(test_setup(), done));
        controller.remember_session(&session.id, "Test".into(), "/project".into());
        controller.backend = Some(backend);
        assert_eq!(
            test_call(|done| controller.dispatch_turn(
                &session.id,
                vec![Input::Text("Hello".into())],
                Some("sonnet".into()),
                Some("medium".into()),
                done
            )),
            "turn"
        );
        assert!(!controller.can_select_provider(Provider::Codex));
        let generation = controller.generation;
        for event in receive.try_iter() {
            assert!(controller.heard(generation, event));
        }
        assert_eq!(
            controller.lines.last().map(|line| line.text.as_str()),
            Some("Hi!")
        );
        assert_eq!(
            controller.store.provider(&session.id),
            Some(Provider::Claude)
        );
        assert!(!controller.reply_active());
        assert!(controller.can_select_provider(Provider::Codex));
        assert!(!controller.heard(generation + 1, Event::Closed("stale".into())));
    }

    #[test]
    fn saved_conversations_reopen_with_their_own_backend_after_restart() {
        let root =
            std::env::temp_dir().join(format!("concat-provider-history-{}", std::process::id()));
        let file = root.join("agent-chats.json");
        let mut controller = Controller::new(Store::open(&file), None);
        for provider in PROVIDERS {
            controller.change_provider(provider);
            controller.remember_session(
                provider.as_str(),
                provider_label(provider).into(),
                "/project".into(),
            );
        }
        drop(controller);

        let mut restarted = Controller::new(Store::open(&file), None);
        for provider in [Provider::Claude, Provider::Codex] {
            let id = provider.as_str();
            assert_eq!(restarted.prepare_open_chat(id), Some(true));
            assert_eq!(restarted.provider, provider);
            assert_eq!(restarted.current.as_deref(), Some(id));
            let backend = TestBackend {
                provider: restarted.provider,
                events: Arc::new(|_| unreachable!()),
            };
            let session = test_call(|done| backend.resume_session(test_setup(), id, done));
            restarted.restore_items(session.id, "project".into(), &session.history);
            assert_eq!(restarted.lines[0].text, provider_label(provider));
        }
        std::fs::remove_dir_all(root).expect("clean test history");
    }

    #[test]
    fn claude_status_prompts_users_to_check_the_terms_when_unavailable() {
        let mut controller = Controller::new(Store::in_memory(), None);
        controller.change_provider(Provider::Claude);
        for (link, expected) in [
            (Link::Missing, AgentStatus::Missing),
            (Link::Failed("Not signed in".into()), AgentStatus::Failed),
        ] {
            controller.set_link(link);
            let (status, title, detail) = status_view(controller.provider, &controller.link);
            assert_eq!(status, expected);
            assert!(title.contains("Claude Code"));
            assert!(detail.contains("Claude Code") && detail.contains("Anthropic"));
            let option = &controller.provider_options()[provider_index(Provider::Claude)];
            assert!(!option.available && !option.reason.is_empty());
            assert!(!controller.can_select_provider(Provider::Claude));
        }
        assert!(include_str!("../locales/zh-Hans.json").contains("请自行确认 Anthropic 使用条款"));
        let (status, title, detail) = status_view(Provider::Codex, &Link::Missing);
        assert_eq!(status, AgentStatus::Missing);
        assert!(title.is_empty() && detail.is_empty());
    }

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
