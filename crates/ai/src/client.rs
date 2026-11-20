//! The requests (native only). Each call blocks until the service answers, so the app runs them on a worker
//! thread. Every request has finite timeouts and every reply a size cap.

use std::time::Duration;

use base64::Engine as _;
use craft_ai::{GenOpts, OpenAiCompat, Provider as _};
use serde_json::{Value, json};

use crate::{AiError, Config, check_prompt, idea, palette};

/// Largest image sent or received, in bytes (encoded).
pub const MAX_IMAGE_BYTES: usize = 24 * 1024 * 1024;

/// The app's name sent to services that show it in their usage pages (OpenRouter). Nothing about the user.
const APP_TITLE: &str = "Nephclick PhotoLab";

/// One text completion of `prompt` with the text model.
pub fn complete(config: &Config, prompt: &str) -> Result<String, AiError> {
    config.ready()?;
    let mut provider = OpenAiCompat::new(config.endpoint.trim(), config.key.clone(), config.text_model.trim());
    provider.complete(prompt, &GenOpts { max_tokens: 1500, temperature: 0.8 }).map_err(|e| from_craft(&e, config))
}

/// A palette for `request` ("sunset over a lavender field").
pub fn palette(config: &Config, request: &str) -> Result<palette::Palette, AiError> {
    let request = check_prompt(request)?;
    palette::parse(&complete(config, &palette::prompt(request))?)
}

/// A project idea for `request`.
pub fn idea(config: &Config, request: &str) -> Result<idea::Idea, AiError> {
    let request = check_prompt(request)?;
    idea::parse(&complete(config, &idea::prompt(request))?)
}

/// Checks that the service answers and the key is accepted, without sending any user content: lists its models.
pub fn test(config: &Config) -> Result<String, AiError> {
    config.ready()?;
    let mut provider = OpenAiCompat::new(config.endpoint.trim(), config.key.clone(), config.text_model.trim());
    let info = provider.test().map_err(|e| from_craft(&e, config))?;
    Ok(if info.models.is_empty() { "Connected.".to_string() } else { format!("Connected: {} models available.", info.models.len()) })
}

/// The instructions sent with AI Fill: the image is the selection's area with some surroundings; the model paints
/// the request into it and keeps the edges continuous with what is there.
pub fn fill_prompt(request: &str) -> String {
    format!(
        "Edit this image. In the middle of it, paint: {request}. Keep the same size, framing, perspective and lighting, \
         and blend the edges seamlessly into the existing surroundings. Return only the edited image."
    )
}

/// AI Fill: sends `png` (the selection's area) with `request` to the image model and returns the PNG (or other
/// image) bytes it painted. An empty request asks the model to fill the area from its surroundings.
pub fn fill(config: &Config, png: &[u8], request: &str) -> Result<Vec<u8>, AiError> {
    config.ready()?;
    if png.is_empty() || png.len() > MAX_IMAGE_BYTES {
        return Err(AiError::Invalid("The selected area is too large to send; select a smaller area.".into()));
    }
    let request = request.trim();
    let request = if request.is_empty() { "a natural continuation of the surroundings, removing anything in the middle" } else { request };
    if request.len() > crate::MAX_PROMPT {
        return Err(AiError::Invalid(format!("The prompt is too long (at most {} characters).", crate::MAX_PROMPT)));
    }
    edit_image(config, png, &fill_prompt(request))
}

/// The instructions sent with Light Pose's AI Generate: the whole photo, relit as `request` describes.
pub fn relight_prompt(request: &str) -> String {
    format!(
        "Relight this photograph: {request}. Rebuild the three-dimensional lighting: the direction, colour and softness \
         of the light, the highlights and the cast shadows, following the shapes in the scene. Keep the people, objects, \
         composition, framing, size and identity exactly the same; change only the lighting. Return only the relit image."
    )
}

/// Light Pose's AI Generate: sends the photo (`png`) with the lighting `request` to the image model and returns the
/// relit image's bytes.
pub fn relight(config: &Config, png: &[u8], request: &str) -> Result<Vec<u8>, AiError> {
    config.ready()?;
    if png.is_empty() || png.len() > MAX_IMAGE_BYTES {
        return Err(AiError::Invalid("The image is too large to send.".into()));
    }
    let request = request.trim();
    let request = if request.is_empty() { "soft natural light from the upper left" } else { request };
    if request.len() > crate::MAX_PROMPT {
        return Err(AiError::Invalid(format!("The prompt is too long (at most {} characters).", crate::MAX_PROMPT)));
    }
    edit_image(config, png, &relight_prompt(request))
}

/// Sends `png` with the instructions `text` to the image model and returns the image it answered.
fn edit_image(config: &Config, png: &[u8], text: &str) -> Result<Vec<u8>, AiError> {
    let data_url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
    let body = json!({
        "model": config.image_model.trim(),
        "modalities": ["image", "text"],
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": text},
                {"type": "image_url", "image_url": {"url": data_url}},
            ],
        }],
    });
    let url = format!("{}/chat/completions", config.endpoint.trim().trim_end_matches('/'));
    let mut req = agent().post(&url).header("Content-Type", "application/json").header("X-Title", APP_TITLE);
    if let Some(key) = config.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let mut resp = req.send_json(body).map_err(|e| from_ureq(e, config))?;
    let status = resp.status().as_u16();
    let bytes = resp
        .body_mut()
        .with_config()
        .limit((MAX_IMAGE_BYTES as u64) * 2)
        .read_to_vec()
        .map_err(|e| AiError::Network(redact(&e.to_string(), config)))?;
    let v: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) if status == 401 || status == 403 => return Err(AiError::Rejected("authentication failed; check the API key".into())),
        Err(_) if !(200..300).contains(&status) => return Err(AiError::Rejected(format!("HTTP {status}"))),
        Err(e) => return Err(AiError::BadReply(e.to_string())),
    };
    image_in_reply(&v)
}

/// The first image in a chat completion: `message.images[].image_url.url` (OpenRouter) or an `image_url` part of
/// `message.content`, as a base64 data URL.
pub fn image_in_reply(v: &Value) -> Result<Vec<u8>, AiError> {
    if let Some(msg) = v.pointer("/error/message").and_then(Value::as_str) {
        return Err(AiError::Rejected(msg.chars().take(300).collect()));
    }
    let message = v.pointer("/choices/0/message").ok_or_else(|| AiError::BadReply("no message in the reply".into()))?;
    let from_images = message.get("images").and_then(Value::as_array).into_iter().flatten();
    let from_content = message.get("content").and_then(Value::as_array).into_iter().flatten();
    let url = from_images
        .chain(from_content)
        .find_map(|part| part.pointer("/image_url/url").and_then(Value::as_str))
        .ok_or_else(|| {
            let said = message.get("content").and_then(Value::as_str).map(|s| s.chars().take(200).collect::<String>()).unwrap_or_default();
            AiError::BadReply(if said.is_empty() {
                "the model returned no image; choose an image model in Preferences › AI Models".into()
            } else {
                format!("the model returned no image (it said: {said})")
            })
        })?;
    let b64 = url.split_once(";base64,").map(|(_, d)| d).ok_or_else(|| AiError::BadReply("the image was not inline data".into()))?;
    if b64.len() > MAX_IMAGE_BYTES * 2 {
        return Err(AiError::BadReply("the image is too large".into()));
    }
    base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| AiError::BadReply(format!("the image data is damaged ({e})")))
}

/// An agent with finite deadlines: an image takes a while, but never forever.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_send_request(Some(Duration::from_secs(30)))
        .timeout_send_body(Some(Duration::from_secs(120)))
        .timeout_recv_response(Some(Duration::from_secs(300)))
        .timeout_recv_body(Some(Duration::from_secs(300)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn from_ureq(e: ureq::Error, config: &Config) -> AiError {
    AiError::Network(redact(&e.to_string(), config))
}

fn from_craft(e: &craft_ai::AiError, config: &Config) -> AiError {
    let text = redact(&e.to_string(), config);
    match e {
        craft_ai::AiError::NotConfigured => AiError::NotConfigured(text),
        craft_ai::AiError::Rejected(_) => AiError::Rejected(text),
        craft_ai::AiError::BadResponse(_) => AiError::BadReply(text),
        _ => AiError::Network(text),
    }
}

/// `text` with the key removed, in case a library echoed it.
fn redact(text: &str, config: &Config) -> String {
    match config.key.as_deref().map(str::trim).filter(|k| k.len() >= 4) {
        Some(key) => text.replace(key, "[key]"),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(key: Option<&str>) -> Config {
        Config { endpoint: crate::OPENROUTER_ENDPOINT.into(), key: key.map(str::to_string), text_model: "t".into(), image_model: "i".into() }
    }

    #[test]
    fn images_are_found_in_either_place() {
        let data = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        let a = json!({"choices":[{"message":{"content":"here","images":[{"type":"image_url","image_url":{"url":format!("data:image/png;base64,{data}")}}]}}]});
        assert_eq!(image_in_reply(&a).unwrap(), vec![1, 2, 3]);
        let b = json!({"choices":[{"message":{"content":[{"type":"text","text":"x"},{"type":"image_url","image_url":{"url":format!("data:image/webp;base64,{data}")}}]}}]});
        assert_eq!(image_in_reply(&b).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn replies_without_an_image_are_errors() {
        assert!(matches!(image_in_reply(&json!({})), Err(AiError::BadReply(_))));
        assert!(matches!(image_in_reply(&json!({"error":{"message":"No credits"}})), Err(AiError::Rejected(m)) if m == "No credits"));
        let said = image_in_reply(&json!({"choices":[{"message":{"content":"I can only write text."}}]})).unwrap_err();
        assert!(said.to_string().contains("I can only write text"));
        let bad = json!({"choices":[{"message":{"images":[{"image_url":{"url":"data:image/png;base64,@@@"}}]}}]});
        assert!(matches!(image_in_reply(&bad), Err(AiError::BadReply(_))));
        let remote = json!({"choices":[{"message":{"images":[{"image_url":{"url":"https://example.com/a.png"}}]}}]});
        assert!(matches!(image_in_reply(&remote), Err(AiError::BadReply(_))));
    }

    #[test]
    fn nothing_is_sent_without_a_key_or_with_a_bad_request() {
        assert!(matches!(complete(&config(None), "x"), Err(AiError::NotConfigured(_))));
        assert!(matches!(fill(&config(Some("k")), &[], "x"), Err(AiError::Invalid(_))));
        assert!(matches!(palette(&config(Some("k")), "  "), Err(AiError::Invalid(_))));
    }

    #[test]
    fn errors_never_carry_the_key() {
        assert_eq!(redact("bad token sk-or-secret here", &config(Some("sk-or-secret"))), "bad token [key] here");
    }
}
