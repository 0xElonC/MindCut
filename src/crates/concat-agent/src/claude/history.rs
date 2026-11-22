use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::wire::{Item, ToolState};

use super::unique_id;

#[derive(Default)]
pub(super) struct History {
    pub session_id: Option<String>,
    pub items: Vec<Item>,
}

impl History {
    pub fn path(dir: &Path, id: &str) -> Result<PathBuf, String> {
        if id.is_empty()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("Invalid Claude conversation ID".into());
        }
        Ok(dir.join(format!("{id}.json")))
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("Could not read Claude history: {error}")),
        };
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "Claude history is not valid JSON".to_owned())?;
        let items = value["items"]
            .as_array()
            .ok_or("Claude history has no item list")?;
        Ok(Self {
            session_id: value["session_id"].as_str().map(str::to_owned),
            items: items.iter().filter_map(read_item).collect(),
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let save = || -> io::Result<()> {
            std::fs::create_dir_all(path.parent().expect("a history file has a directory"))?;
            let temp = path.with_extension(format!("{}.tmp", unique_id()));
            let all = self.items.iter().map(write_item).collect::<Vec<_>>();
            let body = json!({ "session_id": self.session_id, "items": all });
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            let result = write!(file, "{body}");
            drop(file);
            let result = result.and_then(|()| std::fs::rename(&temp, path));
            if result.is_err() {
                let _ = std::fs::remove_file(&temp);
            }
            result
        };
        save().map_err(|error| format!("Could not save Claude history: {error}"))
    }
}

fn read_item(value: &Value) -> Option<Item> {
    let id = value["id"].as_str()?.to_owned();
    let text = |name: &str| value[name].as_str().unwrap_or_default().to_owned();
    match value["type"].as_str()? {
        "user" => Some(Item::User {
            id,
            text: text("text"),
            images: value["images"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|image| image.as_str().map(str::to_owned))
                .collect(),
        }),
        "assistant" => Some(Item::Agent {
            id,
            text: text("text"),
        }),
        "reasoning" => Some(Item::Reasoning {
            id,
            text: text("text"),
        }),
        "tool" => Some(Item::Tool {
            id,
            name: text("name"),
            summary: text("summary"),
            input: text("input"),
            output: text("output"),
            state: if value["failed"].as_bool() == Some(true) {
                ToolState::Failed
            } else {
                ToolState::Done
            },
        }),
        _ => None,
    }
}

fn write_item(item: &Item) -> Value {
    match item {
        Item::User { id, text, images } => {
            json!({ "type": "user", "id": id, "text": text, "images": images })
        }
        Item::Agent { id, text } => json!({ "type": "assistant", "id": id, "text": text }),
        Item::Reasoning { id, text } => json!({ "type": "reasoning", "id": id, "text": text }),
        Item::Tool {
            id,
            name,
            summary,
            input,
            output,
            state,
        } => json!({
            "type": "tool", "id": id, "name": name, "summary": summary,
            "input": input, "output": output, "failed": *state != ToolState::Done,
        }),
    }
}
