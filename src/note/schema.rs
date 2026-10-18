//! The JSON a model must return for a note. Every object closes with `additionalProperties:
//! false` and lists every field as required, as structured outputs want; lengths aren't
//! constrained here (the APIs don't support it), `reply` caps them instead.

use serde_json::{Value, json};

/// The schema's name, for APIs that want one.
pub const NAME: &str = "note";

/// The note schema. Built the same way every time, so it caches with the prompt.
pub fn schema() -> Value {
    let strings = |description: &str| json!({"type": "array", "items": {"type": "string"}, "description": description});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "summary", "body", "key_points", "tasks", "tags", "links", "home"],
        "properties": {
            "title": {
                "type": "string",
                "description": "What the note is about, in the user's own terms. No date."
            },
            "summary": {
                "type": "string",
                "description": "One sentence a reader could stop after."
            },
            "body": {
                "type": "string",
                "description": "The user's text, tidied in their own voice, as Markdown. Empty when the user wrote nothing and the source speaks for itself."
            },
            "key_points": strings("Facts from the source: numbers, names, decisions, quotes with who said them. Empty is fine."),
            "tasks": strings("Things to do that the user or the source implies, each a short imperative. Empty is fine."),
            "tags": strings("Up to five tags, without '#'. Prefer the notebook's existing tags."),
            "links": {
                "type": "array",
                "description": "Notes this one relates to: only names from the candidate notes, or a clearly new note marked new.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["note", "new"],
                    "properties": {
                        "note": {"type": "string"},
                        "new": {"type": "boolean"}
                    }
                }
            },
            "home": {
                "description": "Where this note belongs: a candidate topic note or folder, or null.",
                "anyOf": [
                    {"type": "null"},
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "note", "reason"],
                        "properties": {
                            "kind": {"type": "string", "enum": ["topic", "folder"]},
                            "note": {"type": "string"},
                            "reason": {"type": "string"}
                        }
                    }
                ]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every object in `value` closes its properties and requires all of them.
    fn objects_are_closed(value: &Value, path: &str) {
        match value {
            Value::Object(map) => {
                if map.get("type") == Some(&json!("object")) {
                    assert_eq!(
                        map.get("additionalProperties"),
                        Some(&json!(false)),
                        "{path}"
                    );
                    let properties: Vec<&String> =
                        map["properties"].as_object().unwrap().keys().collect();
                    let mut required: Vec<String> = map["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|key| key.as_str().unwrap().to_owned())
                        .collect();
                    required.sort();
                    let mut properties: Vec<String> = properties.into_iter().cloned().collect();
                    properties.sort();
                    assert_eq!(required, properties, "{path}");
                }
                for (key, child) in map {
                    objects_are_closed(child, &format!("{path}/{key}"));
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    objects_are_closed(child, &format!("{path}/{index}"));
                }
            }
            _ => {}
        }
    }

    #[test]
    fn every_object_is_closed_and_fully_required() {
        objects_are_closed(&schema(), "");
    }

    #[test]
    fn the_schema_is_the_same_bytes_every_time() {
        assert_eq!(schema().to_string(), schema().to_string());
        assert!(!schema().to_string().contains("minLength"));
    }
}
