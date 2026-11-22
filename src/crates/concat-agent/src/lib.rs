// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The agent behind the window's chat.
//!
//! The agent is a program already installed on this computer (today the
//! Codex), run as a child process: the same models, sign-in and settings
//! the person already has, with nothing copied into the app. This crate
//! starts it and speaks its protocol, and says what comes back in a small
//! vocabulary the window can draw without knowing the program's.
//!
//! - [`vocab`] is that vocabulary: [`vocab::Update`], [`vocab::Item`] and
//!   the types a request is made of.
//! - [`peer`] is a child process that speaks JSON-RPC lines, and the
//!   request/response plumbing.
//! - [`backend`] is one module per program: how to find it, how to talk to
//!   it, and how to read what it says into the vocabulary.
//! - [`store`] is the app's own list of its conversations.
//!
//! Nothing here knows about the window, or about video: the agent reaches
//! the project through the Concat MCP server it is started with
//! (see [`vocab::ThreadSetup`]), the same tools any agent has.

// The vocabulary and store types are plain records named for what they
// are; their fields say what they are, and each type says what it is for.
#![allow(missing_docs)]

pub mod backend;
pub mod peer;
pub mod store;
pub mod vocab;

#[cfg(test)]
mod testing;
