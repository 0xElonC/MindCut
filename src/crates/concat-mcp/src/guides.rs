// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Concat contributors

//! The operation manual, compiled into the server, independent of any project.
//! Add a Markdown file in `guides/` with a topic-id filename, an H1 and a
//! one-line summary after the H1; the build includes it without registration.

use concat_api::ApiError;
use serde_json::{Value, json};

struct Guide {
    id: &'static str,
    summary: &'static str,
    body: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/guides.rs"));

/// Returns the directory or one topic, entirely from embedded strings.
pub(crate) fn read(arguments: &Value) -> Result<String, ApiError> {
    let fields = arguments
        .as_object()
        .ok_or_else(|| ApiError::invalid("guide_read arguments must be an object"))?;
    if let Some(unknown) = fields.keys().find(|field| field.as_str() != "topic") {
        return Err(ApiError::invalid(format!(
            "unknown guide_read argument: {unknown}"
        )));
    }
    let topic = fields
        .get("topic")
        .map(|topic| {
            topic
                .as_str()
                .ok_or_else(|| ApiError::invalid("topic must be a string"))
        })
        .transpose()?;
    match topic {
        None => {
            let mut directory = String::from(
                "MindCut operation manual\nCall guide_read with {\"topic\": \"<id>\"} to read a topic.\n\n",
            );
            for guide in GUIDES {
                directory.push_str(&format!("- {}: {}\n", guide.id, guide.summary));
            }
            Ok(directory)
        }
        Some(id) => GUIDES
            .iter()
            .find(|guide| guide.id == id)
            .map(|guide| guide.body.to_owned())
            .ok_or_else(|| {
                let ids: Vec<_> = GUIDES.iter().map(|guide| guide.id).collect();
                ApiError::invalid(format!(
                    "Unknown guide topic {id:?}. Available topic ids: {}",
                    ids.join(", ")
                ))
            }),
    }
}

pub(crate) fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "topic": {
                "type": "string",
                "description": "Topic id from the directory; omit to list all topics and summaries.",
            },
        },
        "additionalProperties": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tool names are snake_case identifiers in inline backticks. Parameter
    // names and edit command ops are camelCase; JSON examples and fenced code
    // are not tool references. Keep this check independent of the prose so
    // a misspelling, including a wholly invented tool, fails the same check.
    fn check_tool_names(body: &str) -> Result<(), String> {
        let tools = crate::tools::catalogue();
        let mut fence = None;
        for line in body.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                let marker = &trimmed[..3];
                if fence == Some(marker) {
                    fence = None;
                } else if fence.is_none() {
                    fence = Some(marker);
                }
                continue;
            }
            if fence.is_some() {
                continue;
            }
            for name in line.split('`').skip(1).step_by(2) {
                if name.contains('_')
                    && name.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
                    && !tools.iter().any(|tool| tool.name == name)
                {
                    return Err(format!("unknown tool `{name}`"));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn every_embedded_guide_references_existing_tools() {
        assert!(GUIDES.iter().any(|guide| guide.id == "index"));
        for guide in GUIDES {
            check_tool_names(guide.body).unwrap_or_else(|why| panic!("{}: {why}", guide.id));
        }
    }

    #[test]
    fn tool_name_check_rejects_an_invented_tool() {
        assert_eq!(
            check_tool_names("Use `timeline_read`, then `nonexistent_tool`."),
            Err("unknown tool `nonexistent_tool`".to_owned())
        );
        assert!(check_tool_names("`guide_read`, `clipId`, `addShapeClip`, `topic`").is_ok());
    }

    #[test]
    fn invalid_arguments_do_not_silently_return_the_directory() {
        for arguments in [
            json!([]),
            json!({"topic": null}),
            json!({"topic": 1}),
            json!({"path": "/tmp"}),
        ] {
            assert!(read(&arguments).is_err(), "{arguments}");
        }
    }
}
