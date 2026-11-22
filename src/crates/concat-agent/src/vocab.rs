// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What the chat is made of, whichever agent is behind it.
//!
//! Every backend (`backend/`) reads what its program says into these types,
//! and the window draws only these: a message, some words arriving, a tool
//! call and how it went. Nothing here names a program or a protocol.

use std::path::PathBuf;

use serde_json::{Value, json};

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

/// The name the Concat MCP server goes by in the agent, and so in the tool
/// calls a conversation shows.
pub const SERVER_NAME: &str = "eloncut";

/// One thing sent with a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Text(String),
    /// A picture the model looks at.
    Image(PathBuf),
    /// A skill the person chose to bring into the message.
    Skill { name: String, path: String },
}

/// A skill the agent can be pointed at.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The `SKILL.md` Codex reads it from.
    pub path: String,
}

/// A model an agent offers, and the depths of thought it takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub efforts: Vec<String>,
    pub default_effort: String,
}

/// The depths a model that does not say gets: what every model is taken to take.
pub const COMMON_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

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
        /// Paths of files that went with it.
        attachments: Vec<String>,
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

    /// The item as one line of a conversation's record.
    pub fn to_json(&self) -> Value {
        match self {
            Item::User { id, text, attachments } => {
                json!({ "type": "user", "id": id, "text": text, "attachments": attachments })
            }
            Item::Agent { id, text } => json!({ "type": "agent", "id": id, "text": text }),
            Item::Reasoning { id, text } => json!({ "type": "reasoning", "id": id, "text": text }),
            Item::Tool { id, name, summary, input, output, state } => json!({
                "type": "tool", "id": id, "name": name, "summary": summary,
                "input": input, "output": output,
                "state": match state {
                    ToolState::Running => "running",
                    ToolState::Done => "done",
                    ToolState::Failed => "failed",
                },
            }),
        }
    }

    /// An item read back from [`Item::to_json`]. `None` for what is not one.
    pub fn from_json(value: &Value) -> Option<Item> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let id = text("id")?;
        match value.get("type")?.as_str()? {
            "user" => Some(Item::User {
                id,
                text: text("text")?,
                attachments: value
                    .get("attachments")
                    .or_else(|| value.get("images"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|path| path.as_str().map(str::to_owned))
                    .collect(),
            }),
            "agent" => Some(Item::Agent { id, text: text("text")? }),
            "reasoning" => Some(Item::Reasoning { id, text: text("text")? }),
            "tool" => Some(Item::Tool {
                id,
                name: text("name")?,
                summary: text("summary")?,
                input: text("input")?,
                output: text("output")?,
                state: match value.get("state")?.as_str()? {
                    "running" => ToolState::Running,
                    "done" => ToolState::Done,
                    "failed" => ToolState::Failed,
                    _ => return None,
                },
            }),
            _ => None,
        }
    }
}

/// `text` on one line and no longer than `longest` characters, marked when cut.
pub fn shorten(text: &str, longest: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= longest {
        flat
    } else {
        let head: String = flat.chars().take(longest).collect();
        format!("{head}…")
    }
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_user_records_with_image_paths_still_read_back() {
        let item = Item::from_json(&json!({ "type": "user", "id": "u", "text": "", "images": ["/old.png"] }));
        assert!(matches!(item, Some(Item::User { attachments, .. }) if attachments == ["/old.png"]));
    }

    #[test]
    fn every_kind_of_item_reads_back_as_it_was_written() {
        let items = [
            Item::User { id: "u".into(), text: "cut it\nshort".into(), attachments: vec!["/a b/c.png".into(), "/clip.mov".into()] },
            Item::Agent { id: "a".into(), text: "ok \"quoted\" 剪好了".into() },
            Item::Reasoning { id: "r".into(), text: "thinking".into() },
            Item::Tool {
                id: "t".into(), name: "clip_split".into(), summary: "split".into(),
                input: "{\"at\":1}".into(), output: String::new(), state: ToolState::Failed,
            },
        ];
        for item in items {
            let line = item.to_json().to_string();
            assert!(!line.contains('\n'), "one line: {line}");
            let back = serde_json::from_str::<Value>(&line).ok().and_then(|v| Item::from_json(&v));
            assert_eq!(back, Some(item));
        }
    }

    #[test]
    fn what_is_not_an_item_is_not_read() {
        assert_eq!(Item::from_json(&json!({ "type": "sleep", "id": "x" })), None);
        assert_eq!(Item::from_json(&json!({ "type": "agent" })), None);
        assert_eq!(Item::from_json(&json!([1, 2])), None);
    }
}
