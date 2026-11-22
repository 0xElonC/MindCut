// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What this app asks Codex, and what Codex's answers and events mean.
//!
//! Requests are built here as plain JSON so that each is testable without a
//! process. Events are read into [`Update`], whose parts are the ones the
//! chat draws: a message, some words arriving, a tool call and how it went.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

/// The Concat MCP server the agent is started with: how it edits.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// What a thread (one conversation) is started or resumed with.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSetup {
    /// The project folder, which is also where the agent's own commands run.
    pub cwd: String,
    /// `None` takes the model from the person's Codex settings.
    pub model: Option<String>,
    /// What the agent is told about its job and the project it is in.
    pub instructions: String,
    pub mcp: McpServer,
}

/// The name the Concat MCP server goes by in Codex, and so in the tool
/// calls a conversation shows.
pub const SERVER_NAME: &str = "eloncut";

impl ThreadSetup {
    fn fields(&self, server_name: &str) -> Map<String, Value> {
        let mut env = Map::new();
        for (key, value) in &self.mcp.env {
            env.insert(key.clone(), json!(value));
        }
        let mut object = Map::new();
        object.insert("cwd".into(), json!(self.cwd));
        // The person chose to let the agent act on its own: nothing waits
        // for an approval, and the commands it runs are not fenced in.
        object.insert("approvalPolicy".into(), json!("never"));
        object.insert("sandbox".into(), json!("danger-full-access"));
        object.insert("developerInstructions".into(), json!(self.instructions));
        if let Some(model) = &self.model {
            object.insert("model".into(), json!(model));
        }
        object.insert(
            "config".into(),
            json!({
                "mcp_servers": {
                    server_name: {
                        "command": self.mcp.command,
                        "args": self.mcp.args,
                        "env": env,
                    }
                }
            }),
        );
        object
    }

    /// `thread/start`.
    pub fn start_params(&self) -> Value {
        self.start_params_for(SERVER_NAME)
    }

    /// `thread/resume`.
    pub fn resume_params(&self, thread: &str) -> Value {
        self.resume_params_for(thread, SERVER_NAME)
    }

    pub(crate) fn start_params_for(&self, server_name: &str) -> Value {
        Value::Object(self.fields(server_name))
    }

    pub(crate) fn resume_params_for(&self, thread: &str, server_name: &str) -> Value {
        let mut object = self.fields(server_name);
        object.insert("threadId".into(), json!(thread));
        Value::Object(object)
    }
}

/// One thing sent with a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Text(String),
    /// A picture the model looks at.
    Image(PathBuf),
    /// A skill the person chose to bring into the message.
    Skill { name: String, path: String },
}

/// `turn/start`: one message, and how hard to think about it.
pub fn turn_params(
    thread: &str,
    inputs: &[Input],
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    let input: Vec<Value> = inputs
        .iter()
        .map(|input| match input {
            Input::Text(text) => json!({ "type": "text", "text": text, "text_elements": [] }),
            Input::Image(path) => {
                json!({ "type": "localImage", "path": path.to_string_lossy() })
            }
            Input::Skill { name, path } => json!({ "type": "skill", "name": name, "path": path }),
        })
        .collect();
    let mut object = Map::new();
    object.insert("threadId".into(), json!(thread));
    object.insert("input".into(), Value::Array(input));
    if let Some(model) = model {
        object.insert("model".into(), json!(model));
    }
    if let Some(effort) = effort {
        object.insert("effort".into(), json!(effort));
    }
    Value::Object(object)
}

/// A skill the agent can be pointed at.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The `SKILL.md` Codex reads it from.
    pub path: String,
}

/// `skills/list`, for what is available in `cwd`.
pub fn skills_params(cwd: &str) -> Value {
    json!({ "cwds": [cwd], "forceReload": false })
}

/// Reads `skills/list`: the skills that are turned on, by name, each once.
pub fn read_skills(result: &Value) -> Vec<Skill> {
    let mut skills: Vec<Skill> = result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|entry| entry.get("skills").and_then(Value::as_array).into_iter().flatten())
        .filter(|skill| skill.get("enabled").and_then(Value::as_bool).unwrap_or(true))
        .filter_map(|skill| {
            Some(Skill {
                name: skill.get("name")?.as_str()?.to_owned(),
                description: skill
                    .get("shortDescription")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .or_else(|| skill.get("description").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_owned(),
                path: skill.get("path")?.as_str()?.to_owned(),
            })
        })
        .collect();
    skills.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    skills.dedup_by(|a, b| a.name == b.name);
    skills
}

/// `turn/interrupt`.
pub fn interrupt_params(thread: &str, turn: &str) -> Value {
    json!({ "threadId": thread, "turnId": turn })
}

/// A model Codex offers, and the depths of thought it takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub efforts: Vec<String>,
    pub default_effort: String,
}

/// The depths a model that does not say gets: what every Codex model takes.
pub const COMMON_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

/// Reads `model/list`.
pub fn read_models(result: &Value) -> Vec<Model> {
    result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| !model.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|model| {
            let id = model.get("model").or_else(|| model.get("id"))?.as_str()?;
            let efforts: Vec<String> = model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| option.get("reasoningEffort")?.as_str().map(str::to_owned))
                .collect();
            Some(Model {
                id: id.to_owned(),
                name: model
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned(),
                efforts: if efforts.is_empty() {
                    COMMON_EFFORTS.map(str::to_owned).to_vec()
                } else {
                    efforts
                },
                default_effort: model
                    .get("defaultReasoningEffort")
                    .and_then(Value::as_str)
                    .unwrap_or("medium")
                    .to_owned(),
            })
        })
        .collect()
}

/// The model and depth the person's own Codex settings choose, from
/// `config/read`.
pub fn read_defaults(result: &Value) -> (Option<String>, Option<String>) {
    let config = result.get("config").unwrap_or(result);
    let text = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    (text("model"), text("model_reasoning_effort"))
}

/// How a tool call stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

/// One entry of a conversation, as the chat draws it.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User {
        id: String,
        text: String,
        /// Pictures that went with it.
        images: Vec<String>,
    },
    Agent {
        id: String,
        text: String,
    },
    Reasoning {
        id: String,
        text: String,
    },
    Tool {
        id: String,
        name: String,
        /// One line: what it was given, shortened.
        summary: String,
        input: String,
        output: String,
        state: ToolState,
    },
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::User { id, .. }
            | Item::Agent { id, .. }
            | Item::Reasoning { id, .. }
            | Item::Tool { id, .. } => id,
        }
    }
}

pub(crate) fn shorten(text: &str, longest: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= longest {
        flat
    } else {
        let head: String = flat.chars().take(longest).collect();
        format!("{head}…")
    }
}

fn state_of(status: Option<&str>) -> ToolState {
    match status {
        Some("completed") => ToolState::Done,
        Some("failed") | Some("declined") => ToolState::Failed,
        _ => ToolState::Running,
    }
}

/// The text a tool answered with: the words of its content, else its error.
fn tool_output(item: &Value) -> String {
    if let Some(error) = item
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        return error.to_owned();
    }
    let Some(result) = item.get("result") else {
        return String::new();
    };
    let words: Vec<&str> = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect();
    words.join("\n")
}

/// Reads a thread item. Items the chat has nothing to say about are `None`.
pub fn read_item(item: &Value) -> Option<Item> {
    let id = item.get("id")?.as_str()?.to_owned();
    let text_of = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    match item.get("type")?.as_str()? {
        "userMessage" => {
            let mut text = Vec::new();
            let mut images = Vec::new();
            for part in item.get("content").and_then(Value::as_array).into_iter().flatten() {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(words) = part.get("text").and_then(Value::as_str) {
                            text.push(words.to_owned());
                        }
                    }
                    Some("localImage") => {
                        if let Some(path) = part.get("path").and_then(Value::as_str) {
                            images.push(path.to_owned());
                        }
                    }
                    _ => {}
                }
            }
            Some(Item::User {
                id,
                text: text.join("\n"),
                images,
            })
        }
        "agentMessage" => Some(Item::Agent {
            id,
            text: text_of("text"),
        }),
        "reasoning" => {
            let summary: Vec<&str> = item
                .get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            Some(Item::Reasoning {
                id,
                text: summary.join("\n"),
            })
        }
        "mcpToolCall" | "dynamicToolCall" => {
            let input = item
                .get("arguments")
                .filter(|arguments| !arguments.is_null())
                .map(|arguments| arguments.to_string())
                .unwrap_or_default();
            Some(Item::Tool {
                id,
                name: text_of("tool"),
                summary: shorten(&input, 80),
                input,
                output: tool_output(item),
                state: state_of(item.get("status").and_then(Value::as_str)),
            })
        }
        "commandExecution" => {
            let command = text_of("command");
            Some(Item::Tool {
                id,
                name: "shell".into(),
                summary: shorten(&command, 80),
                input: command,
                output: text_of("aggregatedOutput"),
                state: state_of(item.get("status").and_then(Value::as_str)),
            })
        }
        "fileChange" => {
            let paths: Vec<&str> = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .collect();
            Some(Item::Tool {
                id,
                name: "apply_patch".into(),
                summary: shorten(&paths.join(", "), 80),
                input: paths.join("\n"),
                output: String::new(),
                state: state_of(item.get("status").and_then(Value::as_str)),
            })
        }
        "webSearch" => Some(Item::Tool {
            id,
            name: "web_search".into(),
            summary: shorten(&text_of("query"), 80),
            input: text_of("query"),
            output: String::new(),
            state: ToolState::Done,
        }),
        _ => None,
    }
}

/// Every item of a thread as `thread/resume` returns it, in order.
pub fn read_history(thread: &Value) -> Vec<Item> {
    thread
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|turn| turn.get("items").and_then(Value::as_array).into_iter().flatten())
        .filter_map(read_item)
        .collect()
}

/// What an event means to the chat.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    TurnStarted {
        thread: String,
        turn: String,
    },
    TurnEnded {
        thread: String,
        turn: String,
        /// "completed", "interrupted" or "failed".
        status: String,
        error: Option<String>,
    },
    ItemStarted {
        thread: String,
        item: Item,
    },
    ItemDone {
        thread: String,
        item: Item,
    },
    /// Words of an answer arriving.
    AgentWords {
        thread: String,
        item: String,
        words: String,
    },
    /// Words of a thought arriving.
    ThoughtWords {
        thread: String,
        item: String,
        words: String,
    },
    /// The agent's name for the conversation.
    Named {
        thread: String,
        name: String,
    },
    /// Something the person should hear, on a thread or on none.
    Trouble {
        thread: Option<String>,
        message: String,
    },
}

/// Reads a notification. The many that the chat has no use for are `None`.
pub fn read_update(method: &str, params: &Value) -> Option<Update> {
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let thread = || text(params, "threadId").unwrap_or_default();
    match method {
        "turn/started" => Some(Update::TurnStarted {
            thread: thread(),
            turn: text(params.get("turn")?, "id")?,
        }),
        "turn/completed" => {
            let turn = params.get("turn")?;
            Some(Update::TurnEnded {
                thread: thread(),
                turn: text(turn, "id")?,
                status: text(turn, "status").unwrap_or_else(|| "completed".into()),
                error: turn
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        }
        "item/started" => Some(Update::ItemStarted {
            thread: thread(),
            item: read_item(params.get("item")?)?,
        }),
        "item/completed" => Some(Update::ItemDone {
            thread: thread(),
            item: read_item(params.get("item")?)?,
        }),
        "item/agentMessage/delta" => Some(Update::AgentWords {
            thread: thread(),
            item: text(params, "itemId")?,
            words: text(params, "delta")?,
        }),
        "item/reasoning/summaryTextDelta" => Some(Update::ThoughtWords {
            thread: thread(),
            item: text(params, "itemId")?,
            words: text(params, "delta")?,
        }),
        "thread/name/updated" => Some(Update::Named {
            thread: thread(),
            name: text(params, "threadName").or_else(|| text(params, "name"))?,
        }),
        "error" => Some(Update::Trouble {
            thread: text(params, "threadId"),
            message: params
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .or_else(|| params.get("message").and_then(Value::as_str))
                .unwrap_or("Codex reported an error")
                .to_owned(),
        }),
        _ => None,
    }
}

/// The answer to something Codex asks that the person is not asked about:
/// they let the agent act on its own, so what asks to run is let run.
pub fn auto_answer(method: &str) -> Value {
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            json!({ "decision": "accept" })
        }
        "applyPatchApproval" | "execCommandApproval" => json!({ "decision": "approved" }),
        "item/permissions/requestApproval" => json!({ "permissions": {}, "scope": "session" }),
        "mcpServer/elicitation/request" => json!({ "action": "accept" }),
        "item/tool/requestUserInput" => json!({ "answers": {} }),
        "item/tool/call" => json!({ "contentItems": [], "success": false }),
        _ => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> ThreadSetup {
        ThreadSetup {
            cwd: "/p".into(),
            model: Some("gpt-x".into()),
            instructions: "edit video".into(),
            mcp: McpServer {
                command: "/app/concat".into(),
                args: vec!["--mcp".into(), "window".into()],
                env: vec![("CONCAT_API_TOKEN".into(), "t".into())],
            },
        }
    }

    #[test]
    fn a_thread_starts_with_the_mcp_server_and_no_waiting_for_approval() {
        let params = setup().start_params();
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["sandbox"], "danger-full-access");
        assert_eq!(params["model"], "gpt-x");
        let server = &params["config"]["mcp_servers"][SERVER_NAME];
        assert_eq!(server["command"], "/app/concat");
        assert_eq!(server["args"], json!(["--mcp", "window"]));
        assert_eq!(server["env"]["CONCAT_API_TOKEN"], "t");
        let resumed = setup().resume_params("t1");
        assert_eq!(resumed["threadId"], "t1");
        assert_eq!(resumed["config"], params["config"]);
    }

    #[test]
    fn a_model_left_to_the_settings_is_not_named() {
        let mut setup = setup();
        setup.model = None;
        assert!(setup.start_params().get("model").is_none());
    }

    #[test]
    fn a_turn_carries_words_pictures_and_the_choices() {
        let params = turn_params(
            "t1",
            &[Input::Text("cut".into()), Input::Image("/a/b.png".into())],
            Some("m"),
            Some("high"),
        );
        assert_eq!(params["input"][0]["type"], "text");
        assert_eq!(params["input"][1], json!({ "type": "localImage", "path": "/a/b.png" }));
        assert_eq!(params["model"], "m");
        assert_eq!(params["effort"], "high");
        let bare = turn_params("t1", &[], None, None);
        assert!(bare.get("model").is_none() && bare.get("effort").is_none());
    }

    #[test]
    fn models_come_with_their_depths_and_hidden_ones_stay_hidden() {
        let result = json!({ "data": [
            { "model": "a", "displayName": "A", "hidden": false, "defaultReasoningEffort": "low",
              "supportedReasoningEfforts": [{ "reasoningEffort": "low" }, { "reasoningEffort": "max" }] },
            { "model": "b", "hidden": true },
            { "model": "c" },
        ]});
        let models = read_models(&result);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].efforts, ["low", "max"]);
        assert_eq!(models[0].name, "A");
        assert_eq!(models[1].efforts, COMMON_EFFORTS);
        assert_eq!(models[1].name, "c");
    }

    #[test]
    fn the_persons_own_choice_is_read_from_their_settings() {
        let result = json!({ "config": { "model": "gpt-6.2-sol", "model_reasoning_effort": "xhigh" } });
        assert_eq!(
            read_defaults(&result),
            (Some("gpt-6.2-sol".into()), Some("xhigh".into()))
        );
        assert_eq!(read_defaults(&json!({})), (None, None));
    }

    #[test]
    fn a_tool_call_reads_with_its_arguments_and_its_answer() {
        let item = json!({
            "type": "mcpToolCall", "id": "c1", "server": "eloncut", "tool": "clips_split",
            "status": "completed", "arguments": { "at": [1.5, 3.0] },
            "result": { "content": [{ "type": "text", "text": "split 2" }] }
        });
        let Some(Item::Tool { name, summary, output, state, .. }) = read_item(&item) else {
            panic!("a tool");
        };
        assert_eq!(name, "clips_split");
        assert_eq!(summary, r#"{"at":[1.5,3.0]}"#);
        assert_eq!(output, "split 2");
        assert_eq!(state, ToolState::Done);

        let failed = json!({ "type": "mcpToolCall", "id": "c2", "tool": "t", "status": "failed",
            "arguments": {}, "error": { "message": "no such clip" } });
        let Some(Item::Tool { output, state, .. }) = read_item(&failed) else {
            panic!("a tool");
        };
        assert_eq!((output.as_str(), state), ("no such clip", ToolState::Failed));
    }

    #[test]
    fn what_the_chat_does_not_draw_is_skipped() {
        assert_eq!(read_item(&json!({ "type": "sleep", "id": "x" })), None);
        assert_eq!(read_update("account/rateLimits/updated", &json!({})), None);
    }

    #[test]
    fn a_history_is_every_item_of_every_turn() {
        let thread = json!({ "turns": [
            { "items": [
                { "type": "userMessage", "id": "u", "content": [
                    { "type": "text", "text": "hello" },
                    { "type": "localImage", "path": "/i.png" } ] },
                { "type": "agentMessage", "id": "a", "text": "hi" } ] },
            { "items": [ { "type": "reasoning", "id": "r", "summary": ["thinking"], "content": [] } ] },
        ]});
        let items = read_history(&thread);
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[0],
            Item::User { id: "u".into(), text: "hello".into(), images: vec!["/i.png".into()] }
        );
        assert_eq!(items[2].id(), "r");
    }

    #[test]
    fn events_read_into_updates() {
        let delta = read_update(
            "item/agentMessage/delta",
            &json!({ "threadId": "t", "turnId": "u", "itemId": "m", "delta": "Hel" }),
        );
        assert_eq!(
            delta,
            Some(Update::AgentWords { thread: "t".into(), item: "m".into(), words: "Hel".into() })
        );
        let ended = read_update(
            "turn/completed",
            &json!({ "threadId": "t", "turn": { "id": "u", "status": "failed",
                "error": { "message": "quota" } } }),
        );
        assert_eq!(
            ended,
            Some(Update::TurnEnded {
                thread: "t".into(), turn: "u".into(), status: "failed".into(),
                error: Some("quota".into())
            })
        );
        assert!(matches!(
            read_update("error", &json!({ "error": { "message": "x" } })),
            Some(Update::Trouble { thread: None, .. })
        ));
    }

    #[test]
    fn skills_are_listed_by_name_once_and_only_when_on() {
        let result = json!({ "data": [
            { "cwd": "/a", "skills": [
                { "name": "zoom", "description": "long", "shortDescription": "short",
                  "path": "/s/zoom/SKILL.md", "enabled": true },
                { "name": "Audit", "description": "check", "path": "/s/audit/SKILL.md", "enabled": true },
                { "name": "off", "description": "x", "path": "/s/off/SKILL.md", "enabled": false } ] },
            { "cwd": "/b", "skills": [
                { "name": "zoom", "description": "again", "path": "/t/zoom/SKILL.md", "enabled": true } ] },
        ]});
        let skills = read_skills(&result);
        let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(names, ["Audit", "zoom"]);
        assert_eq!(skills[1].description, "short");
        let params = turn_params(
            "t",
            &[Input::Skill { name: "zoom".into(), path: "/s/zoom/SKILL.md".into() }],
            None,
            None,
        );
        assert_eq!(
            params["input"][0],
            json!({ "type": "skill", "name": "zoom", "path": "/s/zoom/SKILL.md" })
        );
    }

    #[test]
    fn what_asks_to_run_is_let_run() {
        assert_eq!(auto_answer("item/commandExecution/requestApproval")["decision"], "accept");
        assert_eq!(auto_answer("execCommandApproval")["decision"], "approved");
    }
}
