// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! A conversation's record, kept by the app for the backends whose program
//! does not give its history back.
//!
//! One file per conversation, one [`Item`] per line, appended as the
//! conversation goes: a run that ends in the middle of a write loses at most
//! the line it was writing, and a line that cannot be read is skipped, so
//! one bad line never hides the rest of a conversation.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::vocab::{Item, Update};

/// The records in one folder.
#[derive(Debug, Clone)]
pub struct Transcripts {
    folder: PathBuf,
}

impl Transcripts {
    /// Records kept in `folder`, which is made when the first is written.
    pub fn in_folder(folder: &Path) -> Transcripts {
        Transcripts {
            folder: folder.to_owned(),
        }
    }

    /// A conversation's id as a file name: what is not safe in one is
    /// replaced, so an id from outside cannot name another folder.
    fn file(&self, chat: &str) -> PathBuf {
        let name: String = chat
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.folder.join(format!("{name}.jsonl"))
    }

    /// Adds `items` to the end of a conversation's record.
    pub fn append(&self, chat: &str, items: &[Item]) -> Result<(), String> {
        if items.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.folder)
            .map_err(|error| format!("{}: {error}", self.folder.display()))?;
        let path = self.file(chat);
        let mut text = String::new();
        for item in items {
            text.push_str(&item.to_json().to_string());
            text.push('\n');
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        // One write, so that a line is not split between two.
        file.write_all(text.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Everything in a conversation's record, in order, and how many lines
    /// could not be read. A conversation with no record has none.
    pub fn read(&self, chat: &str) -> (Vec<Item>, usize) {
        let Ok(text) = std::fs::read_to_string(self.file(chat)) else {
            return (Vec::new(), 0);
        };
        let mut items = Vec::new();
        let mut skipped = 0;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|value| Item::from_json(&value))
            {
                Some(item) => items.push(item),
                None => skipped += 1,
            }
        }
        (items, skipped)
    }

    /// Whether a conversation has a record at all.
    pub fn exists(&self, chat: &str) -> bool {
        self.file(chat).is_file()
    }

    /// Deletes a conversation's record.
    pub fn remove(&self, chat: &str) {
        let _ = std::fs::remove_file(self.file(chat));
    }
}

/// Keeps the record of the conversations it is told to watch, from what
/// happens in them, whichever one is on screen: a turn that ends while the
/// person looks at another conversation is kept all the same.
///
/// What a person says is written at once. What the agent says is gathered
/// as it arrives and written when its turn ends, however it ends.
#[derive(Debug)]
pub struct Recorder {
    files: Transcripts,
    watched: HashSet<String>,
    /// What each watched conversation's turn in progress has said so far,
    /// in the order it began.
    turns: HashMap<String, Vec<Item>>,
}

impl Recorder {
    pub fn new(files: Transcripts) -> Recorder {
        Recorder {
            files,
            watched: HashSet::new(),
            turns: HashMap::new(),
        }
    }

    /// Starts keeping a conversation's record.
    pub fn watch(&mut self, chat: &str) {
        self.watched.insert(chat.to_owned());
    }

    /// Stops, and drops what a turn in progress had said.
    pub fn forget(&mut self, chat: &str) {
        self.watched.remove(chat);
        self.turns.remove(chat);
    }

    pub fn watching(&self, chat: &str) -> bool {
        self.watched.contains(chat)
    }

    /// The person said something: written now.
    pub fn said(&self, chat: &str, item: Item) {
        if !self.watching(chat) {
            return;
        }
        if let Err(error) = self.files.append(chat, &[item]) {
            log::warn!("agent: the record could not be written: {error}");
        }
    }

    /// Takes in what happened in a conversation.
    pub fn feed(&mut self, update: &Update) {
        let thread = match update {
            Update::AgentWords { thread, .. }
            | Update::ThoughtWords { thread, .. }
            | Update::ItemStarted { thread, item: Item::Tool { .. } }
            | Update::ItemDone { thread, item: Item::Tool { .. } }
            | Update::TurnEnded { thread, .. } => thread,
            _ => return,
        };
        if matches!(update, Update::TurnEnded { .. }) {
            if let Some(items) = self.turns.remove(thread.as_str())
                && let Err(error) = self.files.append(thread, &items)
            {
                log::warn!("agent: the record could not be written: {error}");
            }
            return;
        }
        if !self.watched.contains(thread.as_str()) {
            return;
        }
        let items = self.turns.entry(thread.clone()).or_default();
        match update {
            Update::AgentWords { item, words, .. } => match find(items, item) {
                Some(Item::Agent { text, .. }) => text.push_str(words),
                _ => items.push(Item::Agent { id: item.clone(), text: words.clone() }),
            },
            Update::ThoughtWords { item, words, .. } => match find(items, item) {
                Some(Item::Reasoning { text, .. }) => text.push_str(words),
                _ => items.push(Item::Reasoning { id: item.clone(), text: words.clone() }),
            },
            Update::ItemStarted { item, .. } | Update::ItemDone { item, .. } => {
                match find(items, item.id()) {
                    Some(slot) => *slot = item.clone(),
                    None => items.push(item.clone()),
                }
            }
            _ => {}
        }
    }
}

fn find<'a>(items: &'a mut [Item], id: &str) -> Option<&'a mut Item> {
    items.iter_mut().find(|item| item.id() == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocab::ToolState;

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("concat-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    fn said(id: &str, text: &str) -> Item {
        Item::Agent { id: id.into(), text: text.into() }
    }

    #[test]
    fn a_record_grows_and_reads_back_in_order() {
        let folder = folder("grows");
        let records = Transcripts::in_folder(&folder);
        assert_eq!(records.read("c1"), (Vec::new(), 0));
        assert!(!records.exists("c1"));
        records.append("c1", &[said("a", "one")]).unwrap();
        records
            .append(
                "c1",
                &[
                    Item::Tool {
                        id: "t".into(), name: "x".into(), summary: "s".into(),
                        input: "i".into(), output: "o".into(), state: ToolState::Done,
                    },
                    said("b", "two"),
                ],
            )
            .unwrap();
        let (items, skipped) = records.read("c1");
        assert_eq!(skipped, 0);
        assert_eq!(items.iter().map(Item::id).collect::<Vec<_>>(), ["a", "t", "b"]);
        // Another conversation's record is its own.
        assert!(records.read("c2").0.is_empty());
        records.remove("c1");
        assert!(!records.exists("c1"));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_line_that_cannot_be_read_hides_nothing_else() {
        let folder = folder("damaged");
        let records = Transcripts::in_folder(&folder);
        records.append("c", &[said("a", "one")]).unwrap();
        // A line cut short, as a run that ended mid-write leaves it, then
        // more written after it by a later run.
        let path = records.file("c");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"type\":\"agent\",\"id\":\"cut\",\"te");
        text.push('\n');
        std::fs::write(&path, text).unwrap();
        records.append("c", &[said("b", "two")]).unwrap();
        let (items, skipped) = records.read("c");
        assert_eq!(items.iter().map(Item::id).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(skipped, 1);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn an_id_cannot_name_a_file_elsewhere() {
        let folder = folder("names");
        let records = Transcripts::in_folder(&folder);
        records.append("../../escape", &[said("a", "x")]).unwrap();
        assert!(records.file("../../escape").starts_with(&folder));
        assert!(records.exists("../../escape"));
        assert!(!folder.join("..").join("..").join("escape.jsonl").exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    fn ended(thread: &str) -> Update {
        Update::TurnEnded { thread: thread.into(), turn: "u".into(), status: "completed".into(), error: None }
    }

    fn words(thread: &str, item: &str, words: &str) -> Update {
        Update::AgentWords { thread: thread.into(), item: item.into(), words: words.into() }
    }

    #[test]
    fn a_turn_is_written_when_it_ends_in_the_order_it_happened() {
        let folder = folder("recorder");
        let records = Transcripts::in_folder(&folder);
        let mut recorder = Recorder::new(records.clone());
        recorder.watch("c");
        recorder.said("c", Item::User { id: "u".into(), text: "cut".into(), images: vec![] });
        recorder.feed(&Update::ThoughtWords { thread: "c".into(), item: "r".into(), words: "hm".into() });
        recorder.feed(&words("c", "m1", "Hel"));
        recorder.feed(&Update::ItemStarted {
            thread: "c".into(),
            item: Item::Tool {
                id: "t".into(), name: "x".into(), summary: "s".into(),
                input: String::new(), output: String::new(), state: ToolState::Running,
            },
        });
        recorder.feed(&words("c", "m1", "lo"));
        recorder.feed(&Update::ItemDone {
            thread: "c".into(),
            item: Item::Tool {
                id: "t".into(), name: "x".into(), summary: "s".into(),
                input: String::new(), output: String::new(), state: ToolState::Done,
            },
        });
        // The person's words are down already; the agent's wait for the end.
        assert_eq!(records.read("c").0.iter().map(Item::id).collect::<Vec<_>>(), ["u"]);
        recorder.feed(&ended("c"));
        let (items, _) = records.read("c");
        assert_eq!(items.iter().map(Item::id).collect::<Vec<_>>(), ["u", "r", "m1", "t"]);
        assert_eq!(items[2], Item::Agent { id: "m1".into(), text: "Hello".into() });
        assert!(matches!(&items[3], Item::Tool { state: ToolState::Done, .. }));
        // The next turn does not write the last one again.
        recorder.feed(&words("c", "m2", "more"));
        recorder.feed(&ended("c"));
        assert_eq!(records.read("c").0.len(), 5);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn only_what_is_watched_is_kept_and_each_conversation_has_its_own_turn() {
        let folder = folder("watched");
        let records = Transcripts::in_folder(&folder);
        let mut recorder = Recorder::new(records.clone());
        recorder.watch("a");
        recorder.watch("b");
        recorder.feed(&words("a", "m", "one"));
        recorder.feed(&words("b", "m", "two"));
        recorder.feed(&words("other", "m", "not kept"));
        recorder.feed(&ended("b"));
        recorder.feed(&ended("other"));
        assert!(!records.exists("a"), "a's turn has not ended");
        assert_eq!(records.read("b").0, vec![Item::Agent { id: "m".into(), text: "two".into() }]);
        assert!(!records.exists("other"));
        recorder.forget("a");
        recorder.feed(&ended("a"));
        assert!(!records.exists("a"), "a forgotten turn is not written");
        let _ = std::fs::remove_dir_all(&folder);
    }
}
