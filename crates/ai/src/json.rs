//! Finding the JSON object in a model's reply: models wrap it in prose or code fences.

use serde_json::Value;

/// Largest reply parsed, in bytes.
const MAX_REPLY: usize = 256 * 1024;

/// The first JSON object in `text`: the whole text when it is one, else the outermost `{…}` span that parses.
pub(crate) fn object(text: &str) -> Option<Value> {
    if text.len() > MAX_REPLY {
        return None;
    }
    let trimmed = text.trim();
    if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }
    // Try each `{` as a start against the last `}`; a reply holds one object, so this finds it quickly.
    let end = trimmed.rfind('}')?;
    // At most 64 starts: a hostile reply full of braces must not cost a parse per byte.
    for (start, _) in trimmed.match_indices('{').take(64) {
        if start >= end {
            break;
        }
        let Some(span) = trimmed.get(start..=end) else { continue };
        if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(span) {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_object_in_fences_and_prose() {
        assert!(object(r#"{"a":1}"#).is_some());
        assert!(object("Here you go:\n```json\n{\"a\": 1}\n```\nEnjoy!").is_some());
        assert!(object("no json here").is_none());
        assert!(object("} {").is_none());
        assert!(object("[1,2]").is_none());
    }

    #[test]
    fn huge_replies_are_refused() {
        assert!(object(&format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_REPLY))).is_none());
    }
}
