//! `nephclick-photolab-ai`: the AI service Nephclick PhotoLab calls itself.
//!
//! - [`Config`]: where requests go (any OpenAI-compatible API root; OpenRouter by default, so one key reaches many
//!   models), the text model and the image model.
//! - [`key`]: the API key, kept in its own owner-only file in the settings folder, never in the preferences, so
//!   `prefs.get`, exported settings and agents never see it.
//! - [`palette`]: colour palettes (five colours, five gradients, five patterns) from a prompt or from the built-in
//!   themed sets, and the parser that turns a model's reply into one.
//! - [`idea`]: a project idea for the Home screen (name, size, palette, brief) from a prompt.
//! - [`client`] (native only): the requests themselves. Text goes through the suite's `craft-ai` provider; image
//!   requests (AI Fill) use the same endpoint with an image in and an image out, which `craft-ai`'s text-only
//!   provider does not cover yet.
//!
//! Never panics: every reply is treated as hostile, every count and size is bounded, and every failure is an
//! [`AiError`] the user can act on. The key never appears in an error or a log line.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
pub mod client;
pub mod idea;
mod json;
pub mod key;
pub mod palette;
#[cfg(not(target_arch = "wasm32"))]
pub mod selection_manager;

use std::fmt;

/// OpenRouter's API root: the default service.
pub const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1";

/// Where requests go and which models answer them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The API root, up to `/v1`.
    pub endpoint: String,
    /// The API key; `None` or empty for a local service that needs none.
    pub key: Option<String>,
    /// The model that writes text (palettes, ideas).
    pub text_model: String,
    /// The model that paints pixels (AI Fill).
    pub image_model: String,
}

impl Config {
    /// Whether a request could be sent: an endpoint, and a key unless the endpoint is on this machine.
    pub fn ready(&self) -> Result<(), AiError> {
        if self.endpoint.trim().is_empty() {
            return Err(AiError::NotConfigured("no AI service address is set (Edit › Preferences › AI Models)".into()));
        }
        let has_key = self.key.as_deref().is_some_and(|k| !k.trim().is_empty());
        if !has_key && !is_loopback(&self.endpoint) {
            return Err(AiError::NotConfigured("no API key is set (Edit › Preferences › AI Models)".into()));
        }
        Ok(())
    }
}

/// Whether `endpoint` names this machine (a local model server needs no key).
pub fn is_loopback(endpoint: &str) -> bool {
    let e = endpoint.trim();
    let rest = e.split_once("://").map_or(e, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host_port.split(':').next().unwrap_or(""),
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// What went wrong, in words the user can act on. Never carries the API key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiError {
    /// Something the user must set first.
    NotConfigured(String),
    /// The service refused the request (authentication, unknown model, quota).
    Rejected(String),
    /// The service could not be reached.
    Network(String),
    /// The service answered with something that is not what was asked for.
    BadReply(String),
    /// The request itself is not valid (empty prompt, image too large).
    Invalid(String),
    /// Not available in this build (the browser).
    Unsupported(String),
    /// Reading or writing the stored key failed.
    Storage(String),
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiError::NotConfigured(m) => write!(f, "AI is not set up: {m}"),
            AiError::Rejected(m) => write!(f, "The AI service refused the request: {m}"),
            AiError::Network(m) => write!(f, "Could not reach the AI service: {m}"),
            AiError::BadReply(m) => write!(f, "The AI service sent an unexpected reply: {m}"),
            AiError::Invalid(m) => write!(f, "{m}"),
            AiError::Unsupported(m) => write!(f, "{m}"),
            AiError::Storage(m) => write!(f, "Could not use the stored API key: {m}"),
        }
    }
}

impl std::error::Error for AiError {}

/// Longest prompt accepted, in bytes: a prompt is a sentence or two, not a document.
pub const MAX_PROMPT: usize = 2000;

/// The prompt trimmed, or an error when it is empty or too long.
pub fn check_prompt(prompt: &str) -> Result<&str, AiError> {
    let p = prompt.trim();
    if p.is_empty() {
        return Err(AiError::Invalid("Type what you want first.".into()));
    }
    if p.len() > MAX_PROMPT {
        return Err(AiError::Invalid(format!("The prompt is too long (at most {MAX_PROMPT} characters).")));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(endpoint: &str, key: Option<&str>) -> Config {
        Config { endpoint: endpoint.into(), key: key.map(str::to_string), text_model: "t".into(), image_model: "i".into() }
    }

    #[test]
    fn ready_needs_a_key_except_on_this_machine() {
        assert!(config(OPENROUTER_ENDPOINT, Some("sk-or-1")).ready().is_ok());
        assert!(matches!(config(OPENROUTER_ENDPOINT, None).ready(), Err(AiError::NotConfigured(_))));
        assert!(matches!(config(OPENROUTER_ENDPOINT, Some("  ")).ready(), Err(AiError::NotConfigured(_))));
        assert!(config("http://localhost:11434/v1", None).ready().is_ok());
        assert!(config("http://127.0.0.1:1234/v1", None).ready().is_ok());
        assert!(matches!(config("", Some("k")).ready(), Err(AiError::NotConfigured(_))));
    }

    #[test]
    fn lookalike_hosts_are_not_local() {
        assert!(!is_loopback("https://localhost.example.com/v1"));
        assert!(!is_loopback("https://127.0.0.1.example.com/v1"));
        assert!(is_loopback("http://[::1]:8080/v1"));
    }

    #[test]
    fn prompts_are_trimmed_and_bounded() {
        assert_eq!(check_prompt("  sunset  ").ok(), Some("sunset"));
        assert!(check_prompt("   ").is_err());
        assert!(check_prompt(&"a".repeat(MAX_PROMPT + 1)).is_err());
    }
}
