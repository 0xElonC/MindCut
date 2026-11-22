// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The app's own list of its conversations.
//!
//! Codex keeps the conversations themselves and can bring one back by its
//! id. What it cannot say is which of the many threads on a computer were
//! had in this app, which project each was about, or what the person has
//! named them, so the app keeps that: one small JSON file of entries.

pub mod transcript;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// One conversation in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Chat {
    /// The program's id for the conversation.
    pub id: String,
    pub title: String,
    /// The project folder it was about.
    pub project: String,
    /// When it last changed, in seconds since the epoch.
    pub updated: u64,
    pub messages: u32,
    /// Which backend it was had with ([`crate::backend::BackendId`]'s name).
    /// A list written before there were several says Codex.
    pub backend: String,
}

/// The list, newest first, and the file it is kept in.
#[derive(Debug, Default)]
pub struct Store {
    file: Option<PathBuf>,
    chats: Vec<Chat>,
}

impl Store {
    /// The list kept at `file`: empty when there is none yet or it cannot be
    /// read, which loses a list and never the conversations.
    pub fn open(file: &Path) -> Store {
        let chats = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| Self::read(&value))
            .unwrap_or_default();
        Store {
            file: Some(file.to_owned()),
            chats,
        }
    }

    /// A list that is not written anywhere, for a test.
    pub fn in_memory() -> Store {
        Store::default()
    }

    fn read(value: &Value) -> Vec<Chat> {
        value
            .get("chats")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|chat| {
                Some(Chat {
                    id: chat.get("id")?.as_str()?.to_owned(),
                    title: chat.get("title")?.as_str()?.to_owned(),
                    project: chat.get("project")?.as_str()?.to_owned(),
                    updated: chat.get("updated")?.as_u64()?,
                    messages: chat.get("messages")?.as_u64()? as u32,
                    backend: chat
                        .get("backend")
                        .and_then(Value::as_str)
                        .unwrap_or("codex")
                        .to_owned(),
                })
            })
            .collect()
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let chats: Vec<Value> = self
            .chats
            .iter()
            .map(|chat| {
                json!({
                    "id": chat.id, "title": chat.title, "project": chat.project,
                    "updated": chat.updated, "messages": chat.messages, "backend": chat.backend,
                })
            })
            .collect();
        let text = json!({ "chats": chats }).to_string();
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        // Written beside and renamed over, so a crash leaves the old list.
        let beside = file.with_extension("json.tmp");
        if std::fs::write(&beside, text).is_ok()
            && let Err(error) = std::fs::rename(&beside, file)
        {
            log::warn!("agent: the chat list could not be saved: {error}");
        }
    }

    /// Every chat, newest first; those whose title holds `query` (any case)
    /// when it is not empty.
    pub fn list(&self, query: &str) -> Vec<&Chat> {
        let query = query.trim().to_lowercase();
        let mut chats: Vec<&Chat> = self
            .chats
            .iter()
            .filter(|chat| query.is_empty() || chat.title.to_lowercase().contains(&query))
            .collect();
        chats.sort_by(|a, b| b.updated.cmp(&a.updated));
        chats
    }

    pub fn get(&self, id: &str) -> Option<&Chat> {
        self.chats.iter().find(|chat| chat.id == id)
    }

    /// Adds a chat, or replaces the one with its id.
    pub fn put(&mut self, chat: Chat) {
        match self.chats.iter_mut().find(|known| known.id == chat.id) {
            Some(known) => *known = chat,
            None => self.chats.push(chat),
        }
        self.save();
    }

    /// Changes one chat in place, if it is there.
    pub fn edit(&mut self, id: &str, change: impl FnOnce(&mut Chat)) {
        if let Some(chat) = self.chats.iter_mut().find(|chat| chat.id == id) {
            change(chat);
            self.save();
        }
    }

    /// Forgets a chat.
    pub fn remove(&mut self, id: &str) {
        self.chats.retain(|chat| chat.id != id);
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, title: &str, updated: u64) -> Chat {
        Chat {
            id: id.into(),
            title: title.into(),
            project: "/p".into(),
            updated,
            messages: 2,
            backend: "codex".into(),
        }
    }

    #[test]
    fn newest_first_and_searched_by_title() {
        let mut store = Store::in_memory();
        store.put(chat("a", "Trip vlog", 10));
        store.put(chat("b", "Interview", 20));
        let all: Vec<&str> = store.list("").iter().map(|chat| chat.id.as_str()).collect();
        assert_eq!(all, ["b", "a"]);
        assert_eq!(store.list("VLOG").len(), 1);
        assert!(store.list("nothing").is_empty());
    }

    #[test]
    fn putting_again_replaces_and_editing_changes_in_place() {
        let mut store = Store::in_memory();
        store.put(chat("a", "One", 1));
        store.put(chat("a", "Two", 2));
        assert_eq!(store.list("").len(), 1);
        store.edit("a", |chat| chat.title = "Three".into());
        assert_eq!(store.get("a").map(|chat| chat.title.as_str()), Some("Three"));
        store.remove("a");
        assert!(store.get("a").is_none());
    }

    #[test]
    fn the_list_survives_a_restart() {
        let folder = std::env::temp_dir().join(format!("concat-agent-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let file = folder.join("chats.json");
        let mut store = Store::open(&file);
        store.put(chat("a", "Kept", 5));
        let reopened = Store::open(&file);
        assert_eq!(reopened.get("a").map(|chat| chat.title.as_str()), Some("Kept"));
        // A file that is not the list is an empty list, not a failure.
        std::fs::write(&file, "not json").expect("writes");
        assert!(Store::open(&file).list("").is_empty());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_list_from_before_there_were_several_backends_says_codex() {
        let old = serde_json::json!({ "chats": [{
            "id": "x", "title": "Old", "project": "/p", "updated": 1, "messages": 3,
        }]});
        let chats = Store::read(&old);
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].backend, "codex");
    }
}
