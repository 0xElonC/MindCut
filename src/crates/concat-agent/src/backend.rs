use std::path::Path;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use serde_json::json;

use crate::rpc::{Codex, Incoming};
use crate::wire::{self, Input, Item, McpServer, Model, Skill, Update};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Provider {
    #[default]
    Codex,
    Claude,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    NotInstalled,
    Unavailable(String),
    Available,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedMcpServer {
    pub name: String,
    pub server: McpServer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSetup {
    pub cwd: String,
    pub model: Option<String>,
    pub instructions: String,
    pub mcp: NamedMcpServer,
}

impl From<wire::ThreadSetup> for SessionSetup {
    fn from(setup: wire::ThreadSetup) -> Self {
        Self {
            cwd: setup.cwd,
            model: setup.model,
            instructions: setup.instructions,
            mcp: NamedMcpServer {
                name: wire::SERVER_NAME.into(),
                server: setup.mcp,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub history: Vec<Item>,
}

pub enum Event {
    Update(Update),
    Closed(String),
}

pub type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;
pub type Done<T> = Box<dyn FnOnce(Result<T, String>) + Send>;
pub type Defaults = (Option<String>, Option<String>);

pub trait Backend: Send + Sync {
    fn provider(&self) -> Provider;
    fn start_session(&self, setup: SessionSetup, done: Done<Session>);
    fn resume_session(&self, setup: SessionSetup, id: &str, done: Done<Session>);
    fn send_turn(
        &self,
        id: &str,
        inputs: Vec<Input>,
        model: Option<String>,
        effort: Option<String>,
        done: Done<String>,
    );
    fn interrupt(&self, id: &str, turn: &str, done: Done<()>);
    fn models(&self, done: Done<Vec<Model>>);
    fn defaults(&self, done: Done<Defaults>);
    fn skills(&self, _cwd: &str, done: Done<Vec<Skill>>) {
        done(Ok(Vec::new()));
    }
    fn rename(&self, _id: &str, _title: &str, done: Done<()>) {
        done(Ok(()));
    }
    fn archive(&self, _id: &str, done: Done<()>) {
        done(Ok(()));
    }
}

pub struct CodexBackend(Arc<Codex>);

impl CodexBackend {
    pub fn connect(
        program: &Path,
        name: &str,
        title: &str,
        version: &str,
        on_event: OnEvent,
    ) -> Result<Self, String> {
        let link = Arc::new(OnceLock::<Weak<Codex>>::new());
        let reader = Arc::clone(&link);
        let codex = Arc::new(Codex::spawn(program, move |message| match message {
            Incoming::Notification { method, params } => {
                if let Some(update) = wire::read_update(&method, &params) {
                    on_event(Event::Update(update));
                }
            }
            Incoming::Request { id, method, .. } => {
                if let Some(codex) = reader.get().and_then(Weak::upgrade) {
                    codex.respond(id, wire::auto_answer(&method));
                }
            }
            Incoming::Closed(reason) => on_event(Event::Closed(reason)),
        })?);
        let _ = link.set(Arc::downgrade(&codex));
        codex.handshake(name, title, version)?;
        Ok(Self(codex))
    }

    pub fn status() -> Availability {
        if crate::locate::codex().is_some() {
            Availability::Available
        } else {
            Availability::NotInstalled
        }
    }

    fn session(&self, setup: SessionSetup, id: Option<&str>, done: Done<Session>) {
        let thread = wire::ThreadSetup {
            cwd: setup.cwd,
            model: setup.model,
            instructions: setup.instructions,
            mcp: setup.mcp.server,
        };
        let name = &setup.mcp.name;
        let (method, params) = if let Some(id) = id {
            ("thread/resume", thread.resume_params_for(id, name))
        } else {
            ("thread/start", thread.start_params_for(name))
        };
        let id = id.map(str::to_owned);
        self.0.call(method, params, move |reply| {
            done(reply.and_then(|result| {
                let thread = &result["thread"];
                let id = thread["id"]
                    .as_str()
                    .map(str::to_owned)
                    .or(id)
                    .ok_or("Codex did not start a conversation")?;
                Ok(Session {
                    id,
                    history: wire::read_history(thread),
                })
            }));
        });
    }
}

impl Backend for CodexBackend {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn start_session(&self, setup: SessionSetup, done: Done<Session>) {
        self.session(setup, None, done);
    }

    fn resume_session(&self, setup: SessionSetup, id: &str, done: Done<Session>) {
        self.session(setup, Some(id), done);
    }

    fn send_turn(
        &self,
        id: &str,
        inputs: Vec<Input>,
        model: Option<String>,
        effort: Option<String>,
        done: Done<String>,
    ) {
        let params = wire::turn_params(id, &inputs, model.as_deref(), effort.as_deref());
        self.0.call("turn/start", params, |reply| {
            done(reply.and_then(|result| {
                result["turn"]["id"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "Codex did not return a turn ID".into())
            }));
        });
    }

    fn interrupt(&self, id: &str, turn: &str, done: Done<()>) {
        self.0.call(
            "turn/interrupt",
            wire::interrupt_params(id, turn),
            |reply| done(reply.map(drop)),
        );
    }

    fn models(&self, done: Done<Vec<Model>>) {
        let codex = Arc::clone(&self.0);
        std::thread::spawn(move || {
            let result = codex.call_blocking("model/list", json!({}), Duration::from_secs(20));
            done(Ok(result
                .map(|result| wire::read_models(&result))
                .unwrap_or_default()));
        });
    }

    fn defaults(&self, done: Done<Defaults>) {
        let codex = Arc::clone(&self.0);
        std::thread::spawn(move || {
            let result = codex.call_blocking("config/read", json!({}), Duration::from_secs(20));
            done(Ok(result
                .map(|result| wire::read_defaults(&result))
                .unwrap_or_default()));
        });
    }

    fn skills(&self, cwd: &str, done: Done<Vec<Skill>>) {
        self.0
            .call("skills/list", wire::skills_params(cwd), move |reply| {
                done(reply.map(|result| wire::read_skills(&result)))
            });
    }

    fn rename(&self, id: &str, title: &str, done: Done<()>) {
        self.0.call(
            "thread/name/set",
            json!({ "threadId": id, "name": title }),
            |reply| done(reply.map(drop)),
        );
    }

    fn archive(&self, id: &str, done: Done<()>) {
        self.0
            .call("thread/archive", json!({ "threadId": id }), |reply| {
                done(reply.map(drop))
            });
    }
}
