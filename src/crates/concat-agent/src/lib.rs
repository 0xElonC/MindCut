// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The agent behind the window's chat.
//!
//! [`backend`] defines the window's interface to the installed CLI backends.
//! Codex uses its app-server; [`claude`] uses Claude Code's stream-json mode.
//! Both emit the same [`wire::Update`] vocabulary without exposing sign-in.
//!
//! - [`locate`] finds the `codex` program.
//! - [`rpc`] is the process and the request/response plumbing.
//! - [`wire`] builds the requests this app makes and reads what Codex says
//!   back ([`wire::Update`]).
//! - [`store`] is the app's own list of its conversations.
//!
//! Nothing here knows about the window, or about video: the agent reaches
//! the project through the Concat MCP server it is started with
//! (see [`wire::ThreadSetup`]), the same tools any agent has.

// The wire and store types are plain records named for what Codex calls
// them; their fields say what they are, and each type says what it is for.
#![allow(missing_docs)]

pub mod backend;
pub mod claude;
pub mod locate;
pub mod rpc;
pub mod store;
pub mod wire;
