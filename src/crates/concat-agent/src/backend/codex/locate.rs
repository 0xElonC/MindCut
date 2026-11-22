// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Finding the `codex` program.
//!
//! A window started from the Dock or the Start menu does not have the PATH a
//! terminal has, so a search of PATH alone would miss the Codex a person
//! installed with npm or Homebrew. The usual homes are named as well.

use std::path::{Path, PathBuf};

/// The environment variable that names the program outright, for a person
/// whose Codex lives somewhere unusual.
pub const OVERRIDE: &str = "CONCAT_CODEX";

/// The names the program goes by: on Windows an npm install leaves a
/// `codex.cmd` shim rather than an executable, and Codex's own build is an
/// `.exe`.
pub fn names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["codex.exe", "codex.cmd"]
    } else {
        &["codex"]
    }
}

/// The Codex to run, if there is one on this computer.
pub fn codex() -> Option<PathBuf> {
    let var = |key: &str| {
        std::env::var_os(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = var("HOME").or_else(|| var("USERPROFILE"));
    // Where npm puts a global install on Windows, and where Codex's own
    // installer might: neither is on the PATH of a program started from the
    // Start menu until the person has logged in again.
    let mut extra: Vec<PathBuf> = Vec::new();
    for base in [var("APPDATA").map(|dir| dir.join("npm")), var("LOCALAPPDATA")]
        .into_iter()
        .flatten()
    {
        for name in names() {
            extra.push(base.join(name));
            extra.push(base.join("Programs").join("codex").join(name));
        }
    }
    let path = std::env::var_os("PATH");
    find(
        std::env::var_os(OVERRIDE).map(PathBuf::from),
        path.as_deref(),
        home.as_deref(),
        names(),
        &extra,
        |candidate| candidate.is_file(),
    )
}

/// [`codex`] over what it reads from the machine, so a test can give it
/// another.
pub fn find(
    forced: Option<PathBuf>,
    path: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
    names: &[&str],
    extra: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(forced) = forced.filter(|forced| !forced.as_os_str().is_empty()) {
        return exists(&forced).then_some(forced);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = path {
        for dir in std::env::split_paths(path) {
            for name in names {
                candidates.push(dir.join(name));
            }
        }
    }
    candidates.extend(
        [
            "/Applications/Codex.app/Contents/Resources/codex",
            "/opt/homebrew/bin/codex",
            "/usr/local/bin/codex",
            "/usr/bin/codex",
        ]
        .map(PathBuf::from),
    );
    candidates.extend(extra.iter().cloned());
    if let Some(home) = home {
        for relative in [
            ".codex/plugins/.plugin-appserver/codex",
            ".local/bin/codex",
            ".npm-global/bin/codex",
            ".bun/bin/codex",
            ".volta/bin/codex",
        ] {
            candidates.push(home.join(relative));
        }
    }
    candidates.into_iter().find(|candidate| exists(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIX: &[&str] = &["codex"];
    const WINDOWS: &[&str] = &["codex.exe", "codex.cmd"];

    #[test]
    fn a_named_program_is_the_only_one_tried() {
        let forced = PathBuf::from("/elsewhere/codex");
        assert_eq!(
            find(Some(forced.clone()), None, None, UNIX, &[], |path| path == forced),
            Some(forced)
        );
        // Named and missing: not a fall back to another, which would run a
        // program the person did not ask for.
        assert_eq!(
            find(Some("/elsewhere/codex".into()), None, None, UNIX, &[], |_| false),
            None
        );
    }

    #[test]
    fn path_comes_before_the_usual_homes() {
        let on_path = PathBuf::from("/tools/bin/codex");
        let found = find(
            None,
            Some(std::ffi::OsStr::new("/tools/bin")),
            Some(Path::new("/home/me")),
            UNIX,
            &[],
            |path| path == on_path || path == Path::new("/home/me/.local/bin/codex"),
        );
        assert_eq!(found, Some(on_path));
    }

    #[test]
    fn the_homes_are_tried_when_path_is_short() {
        let found = find(None, None, Some(Path::new("/home/me")), UNIX, &[], |path| {
            path == Path::new("/home/me/.codex/plugins/.plugin-appserver/codex")
        });
        assert!(found.is_some());
        assert_eq!(find(None, None, None, UNIX, &[], |_| false), None);
    }

    #[test]
    fn an_npm_shim_on_the_path_is_found_by_its_other_name() {
        // npm leaves `codex.cmd`, not `codex.exe`, in its folder.
        let dir = PathBuf::from("npm");
        let shim = dir.join("codex.cmd");
        let found = find(
            None,
            Some(dir.as_os_str()),
            None,
            WINDOWS,
            &[],
            |path| path == shim,
        );
        assert_eq!(found, Some(shim));
    }

    #[test]
    fn the_places_a_start_menu_program_cannot_see_on_its_path_are_tried() {
        let shim = PathBuf::from("AppData").join("npm").join("codex.cmd");
        let found = find(None, None, None, WINDOWS, std::slice::from_ref(&shim), |path| {
            path == shim
        });
        assert_eq!(found, Some(shim));
    }
}
