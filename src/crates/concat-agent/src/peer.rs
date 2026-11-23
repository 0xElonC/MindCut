// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! A program that speaks JSON-RPC lines, and the plumbing to talk to it.
//!
//! One line of JSON each way. A line with an `id` and a `result` or `error`
//! answers a request this side made; one with a `method` and no `id` is a
//! notification; one with a `method` and an `id` is a request from the
//! program that wants an answer. Codex leaves out the `"jsonrpc"` member and
//! ACP agents expect it: [`Spec::jsonrpc`] says which.
//!
//! Nothing here knows which program it is talking to or what its methods
//! mean: that is the backend's (`backend/`). Nothing here blocks the caller
//! except [`Peer::call_blocking`], which is for a worker thread: the window
//! asks with [`Peer::call`] and is answered on the reader thread.
//!
//! A peer is a child process ([`Peer::spawn`]) or any pair of streams
//! ([`Peer::attach`]), which is how a test stands in for the program.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::{Value, json};

/// What a request came back as: the `result`, or the error's message.
pub type Reply = Result<Value, String>;

/// What the program says without being asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// An event: a thread started, words arrived, a tool finished.
    Notification { method: String, params: Value },
    /// A question that waits for [`Writer::respond`].
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// The process ended, with the last thing it said on its error stream.
    Closed(String),
}

/// One line, read.
#[derive(Debug, PartialEq)]
pub enum Line {
    Response { id: u64, reply: Reply },
    Incoming(Incoming),
}

/// Reads a line of the protocol. `None` for anything that is not one.
pub fn read_line(line: &str) -> Option<Line> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let object = value.as_object()?;
    let id = object.get("id").cloned();
    let method = object.get("method").and_then(Value::as_str);
    match (method, id) {
        (Some(method), Some(id)) => Some(Line::Incoming(Incoming::Request {
            id,
            method: method.to_owned(),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        })),
        (Some(method), None) => Some(Line::Incoming(Incoming::Notification {
            method: method.to_owned(),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        })),
        (None, Some(id)) => {
            let id = id.as_u64()?;
            let reply = match object.get("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("The agent refused the request")
                    .to_owned()),
                None => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
            };
            Some(Line::Response { id, reply })
        }
        (None, None) => None,
    }
}

type Waiting = Box<dyn FnOnce(Reply) + Send>;

/// How the process ended, once it has: waits a moment for a program that is
/// only just exiting, and gives up on one that is not.
fn exit_code(child: &Mutex<Child>) -> Option<String> {
    for _ in 0..20 {
        let status = child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_wait();
        match status {
            Ok(Some(status)) => {
                return Some(match status.code() {
                    Some(code) => format!("exit code {code}"),
                    None => "ended by a signal".to_owned(),
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
    None
}

/// What the window says when the program ends: which program, how, and the
/// last thing it said, so a failure on someone else's computer can be read
/// off the screen.
fn stopped_message(name: &str, program: &str, code: Option<String>, said: &str) -> String {
    let mut message = format!("{name} stopped");
    if let Some(code) = code {
        message.push_str(&format!(" ({code})"));
    }
    if !said.is_empty() {
        message.push_str(&format!(": {said}"));
    }
    message.push_str(&format!(" [{program}]"));
    message
}

/// Where an npm shim leads: the `node` to run and the script it runs. The
/// shim `codex.cmd` only starts `node codex.js` through the command
/// interpreter, a layer that has no console to work with when the editor
/// starts it without a window, so the editor starts node itself.
pub(crate) fn shim_target(
    program: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Option<(PathBuf, PathBuf)> {
    let extension = program.extension()?.to_str()?.to_ascii_lowercase();
    if extension != "cmd" && extension != "bat" {
        return None;
    }
    let folder = program.parent()?;
    let script = folder.join("node_modules/@openai/codex/bin/codex.js");
    if !exists(&script) {
        return None;
    }
    // Node is beside the shim when npm and node were installed together, and
    // on the PATH otherwise.
    let beside = folder.join("node.exe");
    let node = if exists(&beside) {
        beside
    } else {
        PathBuf::from("node")
    };
    Some((node, script))
}

/// The command that runs the program with `arguments`.
fn launcher(program: &Path, arguments: &[&str]) -> Command {
    let mut command = match shim_target(program, |path| path.is_file()) {
        Some((node, script)) => {
            let mut command = Command::new(node);
            command.arg(script);
            command
        }
        None => Command::new(program),
    };
    command.args(arguments);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window behind the editor.
        command.creation_flags(0x0800_0000);
    }
    command
}

/// What the program says when asked to check its configuration (`arguments`),
/// for the message shown when it ended without saying anything: the usual
/// reason a program that starts fine by hand stops at once is a file it reads
/// on the way up.
fn probe(program: &Path, arguments: &[&str]) -> String {
    let mut command = launcher(program, arguments);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(command.output().map_err(|error| error.to_string()));
    });
    match receive.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(output)) => {
            let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
            if text.trim().is_empty() {
                text = String::from_utf8_lossy(&output.stdout).into_owned();
            }
            let text = text.trim();
            let start = text.len().saturating_sub(500);
            let start = (start..=text.len())
                .find(|i| text.is_char_boundary(*i))
                .unwrap_or(0);
            format!("checking its configuration: {}", &text[start..])
        }
        Ok(Err(error)) => format!("checking its configuration failed: {error}"),
        Err(_) => "checking its configuration did not finish".to_owned(),
    }
}

/// How to run a program and how it talks.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// What the person calls it, for the messages shown when it stops.
    pub name: &'static str,
    /// What follows the program's name on the command line.
    pub arguments: &'static [&'static str],
    /// A command line that makes the program check its configuration and say
    /// what is wrong, for when it stops without a word. Empty for none.
    pub probe: &'static [&'static str],
    /// Whether each line carries `"jsonrpc": "2.0"`.
    pub jsonrpc: bool,
}

/// The two ends and the process behind them: what [`Peer::attach`] talks to.
pub struct Streams {
    pub input: Box<dyn Write + Send>,
    pub output: Box<dyn Read + Send>,
    /// What the program says about itself, if it has a stream for it.
    pub errors: Option<Box<dyn Read + Send>>,
    pub child: Option<Child>,
    /// The program, for the probe when it stops without a word.
    pub program: Option<PathBuf>,
}

/// The write side of a [`Peer`]. Cheap to clone, so that the reader thread
/// can answer what the program asks.
#[derive(Clone)]
pub struct Writer {
    out: Arc<Mutex<Box<dyn Write + Send>>>,
    jsonrpc: bool,
    name: &'static str,
}

impl Writer {
    fn send(&self, mut message: Value) -> Result<(), String> {
        if self.jsonrpc {
            message["jsonrpc"] = json!("2.0");
        }
        let mut out = self
            .out
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writeln!(out, "{message}")
            .and_then(|()| out.flush())
            .map_err(|error| format!("{} could not be written to: {error}", self.name))
    }

    /// Says something that wants no answer.
    pub fn notify(&self, method: &str, params: Value) {
        let message = if params.is_null() {
            json!({ "method": method })
        } else {
            json!({ "method": method, "params": params })
        };
        if let Err(error) = self.send(message) {
            log::warn!("agent: {error}");
        }
    }

    /// Answers an [`Incoming::Request`].
    pub fn respond(&self, id: Value, result: Value) {
        if let Err(error) = self.send(json!({ "id": id, "result": result })) {
            log::warn!("agent: {error}");
        }
    }
}

/// A running program that speaks JSON-RPC lines.
pub struct Peer {
    writer: Writer,
    next: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, Waiting>>>,
    child: Option<Arc<Mutex<Child>>>,
}

impl Peer {
    /// Starts `program` as described by `spec`. `on_incoming` is called on
    /// the reader thread for everything the program says unasked, and once
    /// with [`Incoming::Closed`] when the process ends for any reason.
    pub fn spawn(
        program: &Path,
        spec: Spec,
        on_incoming: impl Fn(Incoming, &Writer) + Send + 'static,
    ) -> Result<Peer, String> {
        let mut command = launcher(program, spec.arguments);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("{} could not be started: {error}", program.display()))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| format!("{} has no input", spec.name))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| format!("{} has no output", spec.name))?;
        let errors = child
            .stderr
            .take()
            .ok_or_else(|| format!("{} has no error stream", spec.name))?;
        Ok(Peer::attach(
            Streams {
                input: Box::new(input),
                output: Box::new(output),
                errors: Some(Box::new(errors)),
                child: Some(child),
                program: Some(program.to_owned()),
            },
            spec,
            on_incoming,
        ))
    }

    /// Talks over `streams`, which is a process's or anything else's.
    pub fn attach(
        streams: Streams,
        spec: Spec,
        on_incoming: impl Fn(Incoming, &Writer) + Send + 'static,
    ) -> Peer {
        let pending: Arc<Mutex<HashMap<u64, Waiting>>> = Arc::new(Mutex::new(HashMap::new()));
        let tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let writer = Writer {
            out: Arc::new(Mutex::new(streams.input)),
            jsonrpc: spec.jsonrpc,
            name: spec.name,
        };

        let keep = Arc::clone(&tail);
        let error_reader = streams.errors.map(|errors| {
            std::thread::spawn(move || {
                for line in BufReader::new(errors).lines().map_while(Result::ok) {
                    log::debug!("agent: it says: {line}");
                    let mut keep = keep.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if keep.len() == 12 {
                        keep.pop_front();
                    }
                    keep.push_back(line);
                }
            })
        });

        let waiting = Arc::clone(&pending);
        let lines = Arc::clone(&tail);
        let program_path = streams.program;
        let process = streams.child.map(|child| Arc::new(Mutex::new(child)));
        let watched = process.clone();
        let answering = writer.clone();
        let output = streams.output;
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                match read_line(&line) {
                    Some(Line::Response { id, reply }) => {
                        let waiter = waiting
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(&id);
                        if let Some(waiter) = waiter {
                            waiter(reply);
                        }
                    }
                    Some(Line::Incoming(incoming)) => on_incoming(incoming, &answering),
                    None => {
                        log::debug!("agent: a line that is not protocol: {line}");
                        let mut keep = lines
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if keep.len() == 12 {
                            keep.pop_front();
                        }
                        keep.push_back(line);
                    }
                }
            }
            // Ended: everything still waiting for an answer gets this one.
            // The error stream is read on its own thread and may not have
            // finished with the last of what it said: wait for it, or the
            // reason the program gave is lost and "stopped" is all there is.
            if let Some(reader) = error_reader {
                let _ = reader.join();
            }
            let code = watched.as_deref().and_then(exit_code);
            let said = tail
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .rev()
                .find(|line| !line.trim().is_empty())
                .cloned()
                .unwrap_or_default();
            let said = match (&program_path, said.is_empty(), spec.probe.is_empty()) {
                (Some(program), true, false) => probe(program, spec.probe),
                _ => said,
            };
            let program_name = program_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| spec.name.to_owned());
            let stopped = stopped_message(spec.name, &program_name, code, &said);
            let waiters: Vec<Waiting> = waiting
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .drain()
                .map(|(_, waiter)| waiter)
                .collect();
            for waiter in waiters {
                waiter(Err(stopped.clone()));
            }
            on_incoming(Incoming::Closed(stopped), &answering);
        });

        Peer {
            writer,
            next: AtomicU64::new(1),
            pending,
            child: process,
        }
    }

    /// The write side, to hand to whatever answers the program's questions.
    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }

    /// Asks `method`; `then` is called with the answer on the reader thread,
    /// or at once with the reason the question could not be sent.
    pub fn call(&self, method: &str, params: Value, then: impl FnOnce(Reply) + Send + 'static) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, Box::new(then));
        if let Err(error) = self
            .writer
            .send(json!({ "id": id, "method": method, "params": params }))
        {
            let waiter = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
            if let Some(waiter) = waiter {
                waiter(Err(error));
            }
        }
    }

    /// [`Peer::call`], waited for. For a worker thread, never the window's.
    pub fn call_blocking(&self, method: &str, params: Value, patience: Duration) -> Reply {
        let (send, receive) = mpsc::channel();
        self.call(method, params, move |reply| {
            let _ = send.send(reply);
        });
        receive
            .recv_timeout(patience)
            .unwrap_or_else(|_| Err(format!("The agent did not answer {method} in time")))
    }

    /// Says something that wants no answer.
    pub fn notify(&self, method: &str, params: Value) {
        self.writer.notify(method, params);
    }

    /// Answers an [`Incoming::Request`].
    pub fn respond(&self, id: Value, result: Value) {
        self.writer.respond(id, result);
    }

    /// Ends the process, when there is one.
    pub fn kill(&self) {
        let Some(child) = &self.child else { return };
        let mut child = child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_npm_shim_leads_to_node_and_the_script() {
        let shim = Path::new("/n/npm/codex.cmd");
        let script = Path::new("/n/npm/node_modules/@openai/codex/bin/codex.js");
        let found = shim_target(shim, |p| p == script);
        assert_eq!(found, Some((PathBuf::from("node"), script.to_owned())));

        let node = Path::new("/n/npm/node.exe");
        let found = shim_target(shim, |p| p == script || p == node);
        assert_eq!(found, Some((node.to_owned(), script.to_owned())));
    }

    #[test]
    fn a_program_that_is_not_a_shim_is_run_itself() {
        let anything = |_: &Path| true;
        assert_eq!(shim_target(Path::new("/n/codex.exe"), anything), None);
        assert_eq!(shim_target(Path::new("/n/codex"), anything), None);
        // A shim whose script is not where npm puts it is left to the shell.
        assert_eq!(shim_target(Path::new("/n/codex.cmd"), |_| false), None);
    }

    use super::*;

    #[test]
    fn the_three_kinds_of_line_are_told_apart() {
        assert_eq!(
            read_line(r#"{"id":3,"result":{"ok":true}}"#),
            Some(Line::Response {
                id: 3,
                reply: Ok(json!({ "ok": true }))
            })
        );
        assert_eq!(
            read_line(r#"{"id":4,"error":{"code":-32600,"message":"no such thread"}}"#),
            Some(Line::Response {
                id: 4,
                reply: Err("no such thread".into())
            })
        );
        assert_eq!(
            read_line(r#"{"method":"turn/started","params":{"threadId":"t"}}"#),
            Some(Line::Incoming(Incoming::Notification {
                method: "turn/started".into(),
                params: json!({ "threadId": "t" })
            }))
        );
        assert_eq!(
            read_line(r#"{"id":"a","method":"item/tool/requestUserInput","params":{}}"#),
            Some(Line::Incoming(Incoming::Request {
                id: json!("a"),
                method: "item/tool/requestUserInput".into(),
                params: json!({})
            }))
        );
    }

    #[test]
    fn what_is_not_protocol_is_skipped() {
        assert_eq!(read_line("warming up…"), None);
        assert_eq!(read_line("[1,2]"), None);
        assert_eq!(read_line("{}"), None);
    }

    const SPEC: Spec = Spec {
        name: "Test",
        arguments: &[],
        probe: &[],
        jsonrpc: false,
    };

    #[test]
    fn a_program_that_is_not_there_says_so() {
        let error = Peer::spawn(Path::new("/no/such/codex"), SPEC, |_, _| {})
            .err()
            .expect("cannot start");
        assert!(error.contains("could not be started"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_ends_answers_what_waits() {
        // `true` exits at once: the question in flight is answered with the
        // stop, and the close is reported.
        let (closed, heard) = mpsc::channel();
        let codex = Peer::spawn(Path::new("/usr/bin/true"), SPEC, move |incoming, _| {
            if let Incoming::Closed(_) = incoming {
                let _ = closed.send(());
            }
        })
        .expect("true starts");
        let reply = codex.call_blocking("ping", json!({}), Duration::from_secs(5));
        assert!(reply.is_err());
        heard.recv_timeout(Duration::from_secs(5)).expect("closed");
    }
}
