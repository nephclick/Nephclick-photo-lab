//! A project idea for the Home screen: from a prompt such as "poster for a summer jazz night", a name, a canvas
//! size, a background, a palette and a short brief the user can start from.

use serde_json::Value;

use crate::AiError;
use crate::palette::{Palette, Rgb};

/// Smallest and largest canvas side an idea may ask for, in pixels.
pub const SIDE_RANGE: (u32, u32) = (16, 8000);

/// A project to start from.
#[derive(Clone, Debug, PartialEq)]
pub struct Idea {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub background: Rgb,
    pub palette: Palette,
    /// A few sentences: what to make and how.
    pub brief: String,
}

/// The instructions sent with an idea prompt.
pub fn prompt(request: &str) -> String {
    format!(
        "You are a creative director helping someone start a new image in an image editor. Their idea: \"{request}\".\n\
         Reply with only a JSON object, no prose, of this shape:\n\
         {{\"name\": \"short document name\", \"width\": pixels, \"height\": pixels, \"background\": \"#rrggbb\",\n\
           \"colors\": [\"#rrggbb\", … exactly 5], \"brief\": \"2-4 sentences: composition, typography and mood\"}}\n\
         Pick a size that suits the medium (for example 1080x1350 for a social post, 2480x3508 for an A4 poster)."
    )
}

/// The idea in a model's `reply`. A missing or absurd size falls back to 1920 × 1080; no colours is an error.
pub fn parse(reply: &str) -> Result<Idea, AiError> {
    let v = crate::json::object(reply).ok_or_else(|| AiError::BadReply("no idea in the reply".into()))?;
    let name = v.get("name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).unwrap_or("Untitled Idea");
    let name: String = name.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':')).take(60).collect();
    let side = |key: &str, fallback: u32| {
        v.get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && *n >= SIDE_RANGE.0 as f64 && *n <= SIDE_RANGE.1 as f64)
            .map_or(fallback, |n| n.round() as u32)
    };
    let (width, height) = (side("width", 1920), side("height", 1080));
    let colors: Vec<Rgb> =
        v.get("colors").and_then(Value::as_array).map(|a| a.iter().filter_map(|c| c.as_str().and_then(Rgb::parse)).collect()).unwrap_or_default();
    if colors.is_empty() {
        return Err(AiError::BadReply("the idea had no colours".into()));
    }
    let background = v.get("background").and_then(Value::as_str).and_then(Rgb::parse).unwrap_or(Rgb([255, 255, 255]));
    let brief: String = v.get("brief").and_then(Value::as_str).unwrap_or("").trim().chars().take(800).collect();
    let name = if name.is_empty() { "Untitled Idea".to_string() } else { name };
    Ok(Idea { palette: Palette::from_colors(&name, &colors), name, width, height, background, brief })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idea_parses() {
        let i = parse(r##"{"name":"Jazz Night","width":2480,"height":3508,"background":"#101820","colors":["#f2aa4c","#101820","#e94e77","#fefefe","#3a6ea5"],"brief":"Bold type."}"##).unwrap();
        assert_eq!((i.name.as_str(), i.width, i.height), ("Jazz Night", 2480, 3508));
        assert_eq!(i.background, Rgb([0x10, 0x18, 0x20]));
        assert_eq!(i.palette.colors.len(), 5);
        assert_eq!(i.brief, "Bold type.");
    }

    #[test]
    fn absurd_sizes_and_names_are_tamed() {
        let i = parse(r##"{"name":"../../etc/passwd\u0000","width":1e12,"height":-4,"colors":["#000"]}"##).unwrap();
        assert_eq!((i.width, i.height), (1920, 1080));
        assert!(!i.name.contains('/') && !i.name.contains('\0'));
        assert!(parse(r#"{"name":"x"}"#).is_err());
        assert!(parse("sorry, I can't").is_err());
    }
}
