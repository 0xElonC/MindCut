// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What this app asks an Agent Client Protocol agent, and what its events
//! mean.
//!
//! Requests are built here as plain JSON, and events are read into the
//! chat's vocabulary ([`Update`]), so that each is testable without a
//! process.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::AcpProfile;
use crate::backend::{Client, Models};
use crate::peer::Reply;
use crate::vocab::{Input, Item, Model, SERVER_NAME, ThreadSetup, ToolState, Update, shorten};

/// The version of the protocol this app speaks.
pub const PROTOCOL_VERSION: u64 = 1;

/// The biggest picture sent whole. A bigger one is left out, with a word to
/// the agent that it was.
const LARGEST_IMAGE: u64 = 10 * 1024 * 1024;

/// `initialize`.
pub fn initialize_params(client: &Client) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {},
        "clientInfo": { "name": client.name, "title": client.title, "version": client.version },
    })
}

/// `{"a": {"b": {"c": value}}}` for the path `["a", "b", "c"]`.
fn nested(path: &[&str], value: Value) -> Value {
    path.iter()
        .rev()
        .fold(value, |inner, key| json!({ *key: inner }))
}

fn setup_fields(setup: &ThreadSetup, profile: &AcpProfile) -> Map<String, Value> {
    let env: Vec<Value> = setup
        .mcp
        .env
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();
    let mut object = Map::new();
    object.insert("cwd".into(), json!(setup.cwd));
    object.insert(
        "mcpServers".into(),
        json!([{
            "name": SERVER_NAME,
            "command": setup.mcp.command,
            "args": setup.mcp.args,
            "env": env,
        }]),
    );
    if !profile.system_prompt_meta.is_empty() {
        object.insert(
            "_meta".into(),
            nested(profile.system_prompt_meta, json!(setup.instructions)),
        );
    }
    object
}

/// `session/new`.
pub fn new_params(setup: &ThreadSetup, profile: &AcpProfile) -> Value {
    Value::Object(setup_fields(setup, profile))
}

/// `session/load`: takes the conversation up again in `setup`'s folder, with
/// its instructions and MCP server.
pub fn load_params(setup: &ThreadSetup, profile: &AcpProfile, session: &str) -> Value {
    let mut object = setup_fields(setup, profile);
    object.insert("sessionId".into(), json!(session));
    Value::Object(object)
}

/// `session/set_config_option`.
pub fn config_params(session: &str, option: &str, value: &str) -> Value {
    json!({ "sessionId": session, "configId": option, "value": value })
}

/// `session/prompt`.
pub fn prompt_params(session: &str, blocks: Vec<Value>) -> Value {
    json!({ "sessionId": session, "prompt": blocks })
}

/// `session/cancel`.
pub fn cancel_params(session: &str) -> Value {
    json!({ "sessionId": session })
}

/// `session/new`'s answer: the id of the session.
pub fn read_session(result: &Value) -> Result<String, String> {
    result
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "The agent did not start a conversation".to_owned())
}

const ENCODING: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding: what the protocol carries a picture as.
pub fn base64(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        text.push(ENCODING[(n >> 18) as usize & 63] as char);
        text.push(ENCODING[(n >> 12) as usize & 63] as char);
        text.push(if chunk.len() > 1 { ENCODING[(n >> 6) as usize & 63] as char } else { '=' });
        text.push(if chunk.len() > 2 { ENCODING[n as usize & 63] as char } else { '=' });
    }
    text
}

/// The picture's type, from its name.
fn mime_of(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// A picture read from disk as the protocol wants it: its type and its
/// bytes in base64. The protocol takes no path.
pub fn read_image(path: &Path) -> Result<(String, String), String> {
    let mime = mime_of(path).ok_or("this kind of picture is not supported")?;
    let size = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    if size > LARGEST_IMAGE {
        return Err(format!("it is {} MB, over the {} MB limit", size >> 20, LARGEST_IMAGE >> 20));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    Ok((mime.to_owned(), base64(&bytes)))
}

/// A message's content as the protocol's blocks. `read` fetches a picture's
/// type and base64; one it cannot fetch is said in words instead, so that
/// the agent is not left thinking it saw it. A skill has no place here.
pub fn prompt_blocks(
    inputs: &[Input],
    read: impl Fn(&Path) -> Result<(String, String), String>,
) -> Vec<Value> {
    inputs
        .iter()
        .filter_map(|input| match input {
            Input::Text(text) => Some(json!({ "type": "text", "text": text })),
            Input::Image(path) => Some(match read(path) {
                Ok((mime, data)) => json!({ "type": "image", "data": data, "mimeType": mime }),
                Err(reason) => json!({
                    "type": "text",
                    "text": format!(
                        "[A picture was attached but could not be sent: {} ({reason})]",
                        path.display()
                    ),
                }),
            }),
            Input::Skill { .. } => None,
        })
        .collect()
}

/// The models and depths a new session offers: the options named `model`
/// and `thought_level`. Models are kept when `profile` says they are
/// wanted, and a default the person's settings could not mean is dropped.
pub fn read_models(result: &Value, profile: &AcpProfile) -> Models {
    let options = result
        .get("configOptions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let option = |id: &str| options.iter().find(|option| option.get("id").and_then(Value::as_str) == Some(id));
    let values = |option: Option<&Value>| -> Vec<(String, String)> {
        option
            .and_then(|option| option.get("options"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|choice| {
                let value = choice.get("value")?.as_str()?.to_owned();
                let name = choice
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&value)
                    .to_owned();
                Some((value, name))
            })
            .collect()
    };
    let current = |option: Option<&Value>| {
        option
            .and_then(|option| option.get("currentValue"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let model_option = option("model");
    let effort_option = option("thought_level");
    let efforts: Vec<String> = values(effort_option).into_iter().map(|(value, _)| value).collect();
    let default_effort = current(effort_option);
    let list: Vec<Model> = values(model_option)
        .into_iter()
        .filter(|(id, _)| (profile.model_filter)(id))
        .map(|(id, name)| Model {
            id,
            name,
            efforts: efforts.clone(),
            default_effort: default_effort.clone().unwrap_or_else(|| "medium".into()),
        })
        .collect();
    let default_model = current(model_option).filter(|id| list.iter().any(|model| &model.id == id));
    Models {
        list,
        default_model,
        default_effort,
    }
}

/// The answer to leave to run something: the first way to allow it, else the
/// first way there is. The person let the agent act on its own.
pub fn permission_answer(params: &Value) -> Value {
    let options = params
        .get("options")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let chosen = options
        .iter()
        .find(|option| {
            option
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("allow"))
        })
        .or_else(|| options.first())
        .and_then(|option| option.get("optionId"))
        .and_then(Value::as_str);
    match chosen {
        Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

/// How a turn ended, from the answer to `session/prompt`: the status the
/// chat knows ("completed", "interrupted", "failed") and what went wrong.
pub fn turn_end(reply: &Reply) -> (String, Option<String>) {
    match reply {
        Err(error) => ("failed".into(), Some(error.clone())),
        Ok(result) => match result.get("stopReason").and_then(Value::as_str) {
            Some("cancelled") => ("interrupted".into(), None),
            Some("end_turn") | None => ("completed".into(), None),
            Some(other) => ("failed".into(), Some(format!("The agent stopped: {other}"))),
        },
    }
}

/// Which kind of words an item is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Message,
    Thought,
}

/// Reads one session's events into the chat's vocabulary. The protocol sends
/// words in pieces with no id; this gives consecutive pieces of one kind an
/// item of their own, and anything else in between ends it.
#[derive(Debug, Default)]
pub struct Reader {
    open: Option<(Open, String)>,
}

impl Reader {
    /// What `update` (the `update` member of a `session/update`) says.
    pub fn read(
        &mut self,
        seq: &mut u64,
        thread: &str,
        update: &Value,
        profile: &AcpProfile,
    ) -> Vec<Update> {
        let words = || {
            update
                .get("content")
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let kind = update.get("sessionUpdate").and_then(Value::as_str);
        match kind {
            Some("agent_message_chunk") => {
                let Some(words) = words() else { return Vec::new() };
                let item = self.item(Open::Message, seq, "m");
                vec![Update::AgentWords { thread: thread.to_owned(), item, words }]
            }
            Some("agent_thought_chunk") => {
                let Some(words) = words() else { return Vec::new() };
                let item = self.item(Open::Thought, seq, "r");
                vec![Update::ThoughtWords { thread: thread.to_owned(), item, words }]
            }
            Some("tool_call") => {
                self.open = None;
                self.tool(thread, update, profile, false)
            }
            Some("tool_call_update") => self.tool(thread, update, profile, true),
            Some("session_info_update") => update
                .get("title")
                .and_then(Value::as_str)
                .map(|title| {
                    vec![Update::Named { thread: thread.to_owned(), name: title.to_owned() }]
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn item(&mut self, kind: Open, seq: &mut u64, mark: &str) -> String {
        if let Some((open, id)) = &self.open
            && *open == kind
        {
            return id.clone();
        }
        *seq += 1;
        let id = format!("acp-{mark}{seq}");
        self.open = Some((kind, id.clone()));
        id
    }

    fn tool(&mut self, thread: &str, update: &Value, profile: &AcpProfile, change: bool) -> Vec<Update> {
        let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
            return Vec::new();
        };
        let status = update.get("status").and_then(Value::as_str);
        let state = match status {
            Some("completed") => ToolState::Done,
            Some("failed") => ToolState::Failed,
            _ => ToolState::Running,
        };
        let name = update
            .get("name")
            .or_else(|| update.get("title"))
            .and_then(Value::as_str)
            .map(|name| (profile.tool_name)(name).to_owned())
            .unwrap_or_else(|| "tool".into());
        let mut given = update.get("rawInput").cloned().unwrap_or(Value::Null);
        // The agent's own line about the call: the best summary there is.
        let said = given
            .as_object_mut()
            .and_then(|object| object.remove("toolSummary"))
            .and_then(|text| text.as_str().map(str::to_owned));
        let input = match &given {
            Value::Object(object) if !object.is_empty() => given.to_string(),
            _ => String::new(),
        };
        let item = Item::Tool {
            id: id.to_owned(),
            summary: said.unwrap_or_else(|| shorten(&input, 80)),
            name,
            input,
            // The protocol says how a call went, not what it answered.
            output: String::new(),
            state,
        };
        let thread = thread.to_owned();
        if change && state != ToolState::Running {
            vec![Update::ItemDone { thread, item }]
        } else {
            vec![Update::ItemStarted { thread, item }]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocab::McpServer;

    fn profile() -> AcpProfile {
        AcpProfile {
            arguments: &[],
            system_prompt_meta: &["vendor", "systemPrompt", "append"],
            model_filter: |id| id.starts_with("gemini-"),
            tool_name: |name| name.strip_prefix("mcp_eloncut_").unwrap_or(name),
        }
    }

    fn setup() -> ThreadSetup {
        ThreadSetup {
            cwd: "/p".into(),
            model: None,
            instructions: "edit video".into(),
            mcp: McpServer {
                command: "/app/concat".into(),
                args: vec!["--mcp".into(), "window".into()],
                env: vec![("CONCAT_API_TOKEN".into(), "t".into())],
            },
        }
    }

    #[test]
    fn a_session_starts_with_the_mcp_server_and_the_instructions_where_the_profile_puts_them() {
        let params = new_params(&setup(), &profile());
        assert_eq!(params["cwd"], "/p");
        let server = &params["mcpServers"][0];
        assert_eq!(server["name"], SERVER_NAME);
        assert_eq!(server["command"], "/app/concat");
        assert_eq!(server["args"], json!(["--mcp", "window"]));
        assert_eq!(server["env"], json!([{ "name": "CONCAT_API_TOKEN", "value": "t" }]));
        assert_eq!(params["_meta"]["vendor"]["systemPrompt"]["append"], "edit video");
        let loaded = load_params(&setup(), &profile(), "s1");
        assert_eq!(loaded["sessionId"], "s1");
        assert_eq!(loaded["mcpServers"], params["mcpServers"]);
        let mut bare = profile();
        bare.system_prompt_meta = &[];
        assert!(new_params(&setup(), &bare).get("_meta").is_none());
    }

    #[test]
    fn base64_is_the_standard_one() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+");
    }

    #[test]
    fn pictures_go_as_data_and_what_cannot_go_is_said() {
        let inputs = [
            Input::Text("look".into()),
            Input::Image("/a.png".into()),
            Input::Image("/big.png".into()),
            Input::Skill { name: "s".into(), path: "/s".into() },
        ];
        let blocks = prompt_blocks(&inputs, |path| {
            if path == Path::new("/a.png") {
                Ok(("image/png".into(), "QUJD".into()))
            } else {
                Err("too big".into())
            }
        });
        assert_eq!(blocks.len(), 3, "the skill has no place");
        assert_eq!(blocks[0], json!({ "type": "text", "text": "look" }));
        assert_eq!(blocks[1], json!({ "type": "image", "data": "QUJD", "mimeType": "image/png" }));
        assert_eq!(blocks[2]["type"], "text");
        assert!(blocks[2]["text"].as_str().unwrap().contains("/big.png"));
        assert!(blocks[2]["text"].as_str().unwrap().contains("too big"));
        assert_eq!(mime_of(Path::new("x.JPG")), Some("image/jpeg"));
        assert_eq!(mime_of(Path::new("x.bmp")), None);
        assert!(read_image(Path::new("/no/such.png")).is_err());
        assert!(read_image(Path::new("/no/such.bmp")).is_err());
    }

    #[test]
    fn models_and_depths_are_read_from_the_options_and_filtered() {
        let result = json!({ "configOptions": [
            { "id": "model", "currentValue": "gemini-x", "options": [
                { "value": "gemini-x", "name": "Gemini X" },
                { "value": "claude-y", "name": "Claude Y" },
                { "value": "gemini-z" } ] },
            { "id": "thought_level", "currentValue": "high", "options": [
                { "value": "low", "name": "Low" }, { "value": "high", "name": "High" } ] },
            { "id": "mode", "currentValue": "default", "options": [] },
        ]});
        let models = read_models(&result, &profile());
        assert_eq!(
            models.list.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(),
            ["gemini-x", "gemini-z"]
        );
        assert_eq!(models.list[0].name, "Gemini X");
        assert_eq!(models.list[1].name, "gemini-z");
        assert_eq!(models.list[0].efforts, ["low", "high"]);
        assert_eq!(models.list[0].default_effort, "high");
        assert_eq!(models.default_model.as_deref(), Some("gemini-x"));
        assert_eq!(models.default_effort.as_deref(), Some("high"));

        // A current model that is filtered out is not a default to offer.
        let other = json!({ "configOptions": [
            { "id": "model", "currentValue": "claude-y", "options": [
                { "value": "claude-y" }, { "value": "gemini-x" } ] } ]});
        let models = read_models(&other, &profile());
        assert_eq!(models.default_model, None);
        assert!(models.list[0].efforts.is_empty());
        assert_eq!(read_models(&json!({}), &profile()), Models::default());
    }

    #[test]
    fn leave_to_run_something_is_given() {
        let asked = json!({ "options": [
            { "optionId": "deny", "kind": "reject_once" },
            { "optionId": "allow", "kind": "allow_once" } ] });
        assert_eq!(
            permission_answer(&asked),
            json!({ "outcome": { "outcome": "selected", "optionId": "allow" } })
        );
        let odd = json!({ "options": [{ "optionId": "only", "kind": "other" }] });
        assert_eq!(permission_answer(&odd)["outcome"]["optionId"], "only");
        assert_eq!(permission_answer(&json!({}))["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn a_turn_ends_as_the_agent_says() {
        let ended = |reason: &str| turn_end(&Ok(json!({ "stopReason": reason })));
        assert_eq!(ended("end_turn"), ("completed".into(), None));
        assert_eq!(ended("cancelled"), ("interrupted".into(), None));
        let (status, error) = ended("max_tokens");
        assert_eq!(status, "failed");
        assert!(error.unwrap().contains("max_tokens"));
        assert_eq!(turn_end(&Err("gone".into())), ("failed".into(), Some("gone".into())));
    }

    #[test]
    fn words_of_one_kind_run_on_and_anything_between_starts_a_new_item() {
        let mut reader = Reader::default();
        let mut seq = 0;
        let mut read = |reader: &mut Reader, update: Value| reader.read(&mut seq, "s", &update, &profile());
        let chunk = |text: &str| json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } });
        let first = read(&mut reader, chunk("Hel"));
        let second = read(&mut reader, chunk("lo"));
        let (Update::AgentWords { item: a, .. }, Update::AgentWords { item: b, .. }) = (&first[0], &second[0]) else {
            panic!("words");
        };
        assert_eq!(a, b, "one message");
        // A thought is another item, and a tool call ends the message.
        let thought = read(
            &mut reader,
            json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hm" } }),
        );
        let Update::ThoughtWords { item: t, .. } = &thought[0] else { panic!("thought") };
        assert_ne!(t, a);
        read(&mut reader, json!({ "sessionUpdate": "tool_call", "toolCallId": "c1", "status": "in_progress" }));
        let after = read(&mut reader, chunk("done"));
        let Update::AgentWords { item: c, .. } = &after[0] else { panic!("words") };
        assert_ne!(c, a, "a new message after the tool");
        // Empty words and what the chat does not draw say nothing.
        assert!(read(&mut reader, chunk("")).is_empty());
        assert!(read(&mut reader, json!({ "sessionUpdate": "usage_update" })).is_empty());
    }

    #[test]
    fn a_tool_call_reads_with_its_own_name_summary_and_how_it_went() {
        let mut reader = Reader::default();
        let mut seq = 0;
        let started = reader.read(
            &mut seq,
            "s",
            &json!({
                "sessionUpdate": "tool_call", "toolCallId": "c1", "status": "in_progress",
                "name": "mcp_eloncut_clips_split",
                "rawInput": { "clipIds": ["a"], "toolSummary": "Split the clip" },
            }),
            &profile(),
        );
        let [Update::ItemStarted { item: Item::Tool { id, name, summary, input, state, .. }, .. }] =
            started.as_slice()
        else {
            panic!("a tool starts: {started:?}");
        };
        assert_eq!((id.as_str(), name.as_str(), summary.as_str()), ("c1", "clips_split", "Split the clip"));
        assert_eq!(input, r#"{"clipIds":["a"]}"#);
        assert_eq!(*state, ToolState::Running);

        let failed = reader.read(
            &mut seq,
            "s",
            &json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "failed", "name": "mcp_eloncut_clips_split" }),
            &profile(),
        );
        assert!(matches!(
            failed.as_slice(),
            [Update::ItemDone { item: Item::Tool { state: ToolState::Failed, .. }, .. }]
        ));
        // An update that is not yet the end changes the line in place.
        let working = reader.read(
            &mut seq,
            "s",
            &json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress" }),
            &profile(),
        );
        assert!(matches!(working.as_slice(), [Update::ItemStarted { .. }]));
        // Without a summary the call's input stands in, and a shell command
        // keeps the name it has.
        let shell = reader.read(
            &mut seq,
            "s",
            &json!({ "sessionUpdate": "tool_call", "toolCallId": "c2", "name": "run_command",
                "rawInput": { "CommandLine": "ls" } }),
            &profile(),
        );
        let [Update::ItemStarted { item: Item::Tool { name, summary, .. }, .. }] = shell.as_slice() else {
            panic!("a tool");
        };
        assert_eq!((name.as_str(), summary.as_str()), ("run_command", r#"{"CommandLine":"ls"}"#));
    }

    #[test]
    fn the_title_is_the_conversations_name() {
        let mut reader = Reader::default();
        let named = reader.read(&mut 0, "s", &json!({ "sessionUpdate": "session_info_update", "title": "Trip" }), &profile());
        assert_eq!(named, [Update::Named { thread: "s".into(), name: "Trip".into() }]);
    }
}
