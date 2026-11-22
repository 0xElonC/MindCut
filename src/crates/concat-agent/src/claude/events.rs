use std::collections::HashMap;

use serde_json::Value;

use crate::wire::{Item, ToolState, Update, shorten};

#[derive(Default)]
pub(super) struct Outcome {
    pub failed: Option<String>,
}

pub(super) struct TurnEvents<'a> {
    thread: &'a str,
    turn: &'a str,
    items: Vec<Item>,
    blocks: HashMap<u64, usize>,
    streamed_messages: std::collections::HashSet<String>,
    pub session_id: Option<String>,
    pub result: Option<Outcome>,
    problem: Option<String>,
}

impl<'a> TurnEvents<'a> {
    pub fn new(thread: &'a str, turn: &'a str) -> Self {
        Self {
            thread,
            turn,
            items: Vec::new(),
            blocks: HashMap::new(),
            streamed_messages: Default::default(),
            session_id: None,
            result: None,
            problem: None,
        }
    }

    pub fn read(&mut self, line: &str, emit: &impl Fn(Update)) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match value["type"].as_str() {
            Some("system") if value["subtype"] == "init" => self.capture_session(&value),
            Some("stream_event") => self.stream(&value["event"], emit),
            Some("assistant") => self.assistant(&value, emit),
            Some("user") => {
                if let Some(blocks) = value["message"]["content"].as_array() {
                    for block in blocks.iter().filter(|block| block["type"] == "tool_result") {
                        self.tool_result(block, emit);
                    }
                }
            }
            Some("result") => {
                self.capture_session(&value);
                let failed = self.problem.clone().or_else(|| {
                    (value["is_error"].as_bool() == Some(true) || value["subtype"] != "success")
                        .then(|| failure(&value["subtype"]).to_owned())
                });
                self.result = Some(Outcome { failed });
            }
            Some("error") => self.trouble(failure(&value["error"]["type"]), emit),
            _ => {}
        }
    }

    fn capture_session(&mut self, value: &Value) {
        if let Some(id) = value["session_id"].as_str().filter(|id| valid_session(id)) {
            self.session_id = Some(id.to_owned());
        }
    }

    fn stream(&mut self, event: &Value, emit: &impl Fn(Update)) {
        let index = event["index"].as_u64();
        match event["type"].as_str() {
            Some("message_start") => {
                self.blocks.clear();
            }
            Some("content_block_start") => {
                if let Some(index) = index
                    && !self.blocks.contains_key(&index)
                    && let Some(at) = self.new_block(&event["content_block"], emit)
                {
                    self.blocks.insert(index, at);
                }
            }
            Some("content_block_delta") => {
                if let Some(at) = index.and_then(|index| self.blocks.get(&index)).copied() {
                    self.delta(at, &event["delta"], emit);
                }
            }
            Some("content_block_stop") => {
                if let Some(at) = index.and_then(|index| self.blocks.remove(&index)) {
                    let item = &mut self.items[at];
                    if let Item::Tool { input, summary, .. } = item {
                        *summary = shorten(input, 80);
                    } else {
                        emit(Update::ItemDone {
                            thread: self.thread.into(),
                            item: item.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
        if event["type"] == "message_start"
            && let Some(id) = event["message"]["id"].as_str()
        {
            self.streamed_messages.insert(id.into());
        }
    }

    fn new_block(&mut self, block: &Value, emit: &impl Fn(Update)) -> Option<usize> {
        let id = || format!("{}-{}", self.turn, self.items.len());
        let item = match block["type"].as_str()? {
            "text" => Item::Agent {
                id: id(),
                text: block["text"].as_str().unwrap_or_default().into(),
            },
            "thinking" => Item::Reasoning {
                id: id(),
                text: block["thinking"].as_str().unwrap_or_default().into(),
            },
            "tool_use" => {
                let tool_id = block["id"].as_str()?;
                let tool_id = format!("{}-{tool_id}", self.turn);
                let input = block
                    .get("input")
                    .filter(|value| !value.is_null() && **value != serde_json::json!({}));
                if let Some(at) = self.items.iter().position(|item| item.id() == tool_id) {
                    if let (
                        Some(value),
                        Item::Tool {
                            input,
                            summary,
                            state: ToolState::Running,
                            ..
                        },
                    ) = (input, &mut self.items[at])
                    {
                        *input = value.to_string();
                        *summary = shorten(input, 80);
                    }
                    return Some(at);
                }
                let input = input.map(Value::to_string).unwrap_or_default();
                Item::Tool {
                    id: tool_id,
                    name: block["name"].as_str()?.into(),
                    summary: shorten(&input, 80),
                    input,
                    output: String::new(),
                    state: ToolState::Running,
                }
            }
            _ => return None,
        };
        emit(Update::ItemStarted {
            thread: self.thread.into(),
            item: item.clone(),
        });
        let at = self.items.len();
        self.items.push(item);
        Some(at)
    }

    fn delta(&mut self, at: usize, delta: &Value, emit: &impl Fn(Update)) {
        match (&mut self.items[at], delta["type"].as_str()) {
            (Item::Agent { id, text }, Some("text_delta")) => {
                if let Some(words) = delta["text"].as_str().filter(|words| !words.is_empty()) {
                    text.push_str(words);
                    emit(Update::AgentWords {
                        thread: self.thread.into(),
                        item: id.clone(),
                        words: words.into(),
                    });
                }
            }
            (Item::Reasoning { id, text }, Some("thinking_delta")) => {
                if let Some(words) = delta["thinking"].as_str().filter(|words| !words.is_empty()) {
                    text.push_str(words);
                    emit(Update::ThoughtWords {
                        thread: self.thread.into(),
                        item: id.clone(),
                        words: words.into(),
                    });
                }
            }
            (Item::Tool { input, .. }, Some("input_json_delta")) => {
                if let Some(json) = delta["partial_json"].as_str() {
                    input.push_str(json);
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, value: &Value, emit: &impl Fn(Update)) {
        if let Some(error) = value["error"].as_str() {
            self.trouble(failure(&Value::String(error.into())), emit);
            return;
        }
        let message = &value["message"];
        let streamed = message["id"]
            .as_str()
            .is_some_and(|id| self.streamed_messages.contains(id));
        for block in message["content"].as_array().into_iter().flatten() {
            if streamed && matches!(block["type"].as_str(), Some("text" | "thinking")) {
                continue;
            }
            if let Some(at) = self.new_block(block, emit)
                && !matches!(self.items[at], Item::Tool { .. })
            {
                emit(Update::ItemDone {
                    thread: self.thread.into(),
                    item: self.items[at].clone(),
                });
            }
        }
    }

    fn tool_result(&mut self, block: &Value, emit: &impl Fn(Update)) {
        let Some(id) = block["tool_use_id"].as_str() else {
            return;
        };
        let id = format!("{}-{id}", self.turn);
        if let Some(item @ Item::Tool { .. }) = self.items.iter_mut().find(|item| item.id() == id)
            && let Item::Tool { state, output, .. } = item
        {
            let content = &block["content"];
            *output = if let Some(text) = content.as_str() {
                text.into()
            } else if let Some(parts) = content.as_array() {
                parts
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                String::new()
            };
            *state = if block["is_error"].as_bool() == Some(true) {
                ToolState::Failed
            } else {
                ToolState::Done
            };
            emit(Update::ItemDone {
                thread: self.thread.into(),
                item: item.clone(),
            });
        }
    }

    fn trouble(&mut self, message: &str, emit: &impl Fn(Update)) {
        self.problem = Some(message.into());
        emit(Update::Trouble {
            thread: Some(self.thread.into()),
            message: message.into(),
        });
    }

    pub fn finish(mut self, emit: &impl Fn(Update)) -> Vec<Item> {
        for at in self.blocks.into_values() {
            let item = &self.items[at];
            if !matches!(item, Item::Tool { .. }) {
                emit(Update::ItemDone {
                    thread: self.thread.into(),
                    item: item.clone(),
                });
            }
        }
        for item in &mut self.items {
            if let Item::Tool { state, .. } = item
                && *state == ToolState::Running
            {
                *state = ToolState::Failed;
                emit(Update::ItemDone {
                    thread: self.thread.into(),
                    item: item.clone(),
                });
            }
        }
        self.items
    }
}

pub(super) fn valid_session(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn failure(kind: &Value) -> &'static str {
    match kind.as_str() {
        Some("authentication_failed" | "authentication_error") => {
            "Claude Code authentication failed. Check `claude auth status`."
        }
        Some("billing_error") => "Claude Code reported a billing error.",
        Some("rate_limit" | "rate_limit_error") => "Claude Code reached a rate limit.",
        Some("error_max_turns") => "Claude Code reached its turn limit.",
        Some("error_max_budget_usd") => "Claude Code reached its budget limit.",
        _ => "Claude Code could not complete the response.",
    }
}
