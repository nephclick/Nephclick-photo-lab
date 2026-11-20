//! AI selection, offline: routes a selection request to the engine that answers it.
//!
//! - Select Subject, Select Background and All Objects run the subject model in-process
//!   ([`crate::subject_model`]); on a Mac without it, the system's Vision helper when the app ships one.
//! - Click, rectangle and text prompts go to the Prompt & Click worker ([`crate::model_install::Pack::Prompt`]): a
//!   Python process running SAM 2.1 Tiny and Grounding DINO Tiny. It starts on the first such request and quits after
//!   a few idle minutes, so it uses no memory while unused.
//!
//! Every input size and every reply is checked; failures are [`AiError`]s that say what to install or try.

use std::hash::{BuildHasher, RandomState};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, PoisonError, mpsc};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::AiError;
use crate::model_install;

/// Which engine answers a selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiEngine {
    /// The in-process subject model.
    Subject,
    /// The system's Vision framework through the helper the app ships on a Mac.
    AppleVision,
    /// SAM 2.1 Tiny and Grounding DINO Tiny in the Prompt & Click worker.
    Prompt,
}

/// What to select.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SelectionPrompt {
    /// The main subject.
    Subject,
    /// Everything but the main subject.
    Background,
    /// The foreground objects.
    AllObjects,
    /// The object under one point.
    Point { x: u32, y: u32, positive: bool },
    /// The object several points pick (positive) and rule out (negative).
    Points { points: Vec<(u32, u32, bool)> },
    /// The object inside a rectangle.
    Box { x0: u32, y0: u32, x1: u32, y1: u32 },
    /// What a phrase names ("the red car").
    Text { prompt: String },
}

impl SelectionPrompt {
    /// Whether the prompt needs the Prompt & Click worker.
    pub fn needs_prompt_pack(&self) -> bool {
        matches!(self, SelectionPrompt::Point { .. } | SelectionPrompt::Points { .. } | SelectionPrompt::Box { .. } | SelectionPrompt::Text { .. })
    }
}

/// A coverage mask, 0..=255, `width` × `height`.
#[derive(Clone, Debug)]
pub struct SelectionMask {
    pub width: u32,
    pub height: u32,
    pub mask: Vec<u8>,
    pub metadata: Option<String>,
}

impl SelectionMask {
    /// Inverts the mask.
    pub fn invert(&mut self) {
        for b in &mut self.mask {
            *b = 255 - *b;
        }
    }

    /// The mask scaled to `width` × `height` (bilinear); itself when it already has that size.
    pub fn resized(&self, width: u32, height: u32) -> SelectionMask {
        let (sw, sh, dw, dh) = (self.width as usize, self.height as usize, width as usize, height as usize);
        if (sw, sh) == (dw, dh) || sw == 0 || sh == 0 || self.mask.len() < sw * sh {
            let mut same = self.clone();
            if (sw, sh) != (dw, dh) {
                same.width = width;
                same.height = height;
                same.mask = vec![0; dw.saturating_mul(dh)];
            }
            return same;
        }
        let mut out = vec![0u8; dw.saturating_mul(dh)];
        let (sx, sy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
        let at = |x: usize, y: usize| f32::from(self.mask.get(y * sw + x).copied().unwrap_or(0));
        for y in 0..dh {
            let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (sh - 1) as f32);
            let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
            let y1 = (y0 + 1).min(sh - 1);
            for x in 0..dw {
                let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (sw - 1) as f32);
                let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
                let x1 = (x0 + 1).min(sw - 1);
                let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                if let Some(slot) = out.get_mut(y * dw + x) {
                    *slot = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        SelectionMask { width, height, mask: out, metadata: self.metadata.clone() }
    }
}

/// Which engines can answer now.
pub fn available_engines() -> Vec<AiEngine> {
    let mut engines = Vec::new();
    if model_install::subject_installed() {
        engines.push(AiEngine::Subject);
    }
    if cfg!(target_os = "macos") && find_vision_tool().is_some() {
        engines.push(AiEngine::AppleVision);
    }
    if model_install::prompt_installed() {
        engines.push(AiEngine::Prompt);
    }
    engines
}

/// Selects what `prompt` asks for in `rgba` (`width` × `height`, RGBA8). The mask has the picture's size.
pub fn select(rgba: &[u8], width: u32, height: u32, prompt: &SelectionPrompt) -> Result<SelectionMask, AiError> {
    if width == 0 || height == 0 || rgba.len() != (width as usize).saturating_mul(height as usize).saturating_mul(4) {
        return Err(AiError::Invalid("invalid image dimensions or buffer size".into()));
    }
    let mut mask = if prompt.needs_prompt_pack() {
        if !model_install::prompt_installed() {
            return Err(AiError::NotConfigured(
                "click, rectangle and text selection need the Prompt & Click models: install them in Select › AI Selection Models…".into(),
            ));
        }
        run_worker(rgba, width, height, prompt)?
    } else if model_install::subject_installed() {
        crate::subject_model::segment(rgba, width, height)?
    } else if cfg!(target_os = "macos") && find_vision_tool().is_some() {
        run_apple_vision(rgba, width, height, prompt)?
    } else {
        return Err(AiError::NotConfigured("the Select Subject model isn't installed: install it in Select › AI Selection Models…".into()));
    };
    if matches!(prompt, SelectionPrompt::Background) {
        mask.invert();
    }
    let mask = mask.resized(width, height);
    Ok(mask)
}

// ------------------------------------------------------------------ the Prompt & Click worker

/// The running worker: its process, its port and the token it answers.
struct Worker {
    child: Child,
    port: u16,
    token: String,
}

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

/// How long the worker may take to start (it loads PyTorch).
const WORKER_START: Duration = Duration::from_secs(120);
/// How long one request may take (the first loads the model; a CPU-only machine is slow).
const WORKER_REQUEST: Duration = Duration::from_secs(600);
/// Largest picture side sent to the worker.
const MAX_WORKER_SIDE: u32 = 8192;

/// Stops the worker now (Remove, or the app quitting), freeing its memory.
pub fn stop_worker() {
    let mut held = WORKER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(mut worker) = held.take() {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", worker.port)) {
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let _ = send_request(&mut stream, &serde_json::json!({"token": worker.token, "cmd": "quit"}), None);
        }
        let _ = worker.child.kill();
        let _ = worker.child.wait();
    }
}

/// A fresh token the worker requires of every request, so no other local program can use it.
fn new_token() -> String {
    let a = RandomState::new().hash_one(std::process::id());
    let b = RandomState::new().hash_one(std::time::SystemTime::now());
    format!("{a:016x}{b:016x}")
}

/// The worker's port and token, starting it when it isn't running.
fn worker() -> Result<(u16, String), AiError> {
    let mut held = WORKER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(worker) = held.as_mut()
        && matches!(worker.child.try_wait(), Ok(None))
    {
        return Ok((worker.port, worker.token.clone()));
    }
    *held = None;
    let python = model_install::python_exe().ok_or_else(|| AiError::NotConfigured("the Prompt & Click models aren't installed".into()))?;
    let script: PathBuf = model_install::worker_script()?;
    let token = new_token();
    let mut command: Command = model_install::python_command(&python);
    let mut child = command
        .arg(&script)
        .env("NEPHCLICK_AI_TOKEN", &token)
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AiError::Unsupported(format!("can't start the AI selection worker: {e}")))?;
    let stdout = child.stdout.take().ok_or_else(|| AiError::Unsupported("the AI selection worker has no output".into()))?;
    let (tx, rx) = mpsc::channel::<u16>();
    // Reads the worker's port, then drains what it prints so it never blocks on a full pipe.
    let reader = std::thread::Builder::new().name("ai-worker-out".into()).spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if !sent && let Some(port) = line.trim().strip_prefix("PORT ").and_then(|p| p.trim().parse::<u16>().ok()) {
                let _ = tx.send(port);
                sent = true;
            }
        }
    });
    if reader.is_err() {
        let _ = child.kill();
        return Err(AiError::Unsupported("can't watch the AI selection worker".into()));
    }
    let port = match rx.recv_timeout(WORKER_START) {
        Ok(port) => port,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AiError::Unsupported("the AI selection worker didn't start: reinstall the Prompt & Click models".into()));
        }
    };
    *held = Some(Worker { child, port, token: token.clone() });
    Ok((port, token))
}

fn io(e: std::io::Error) -> AiError {
    AiError::Network(format!("the AI selection worker stopped answering: {e}"))
}

/// Writes one request: the JSON's length, the JSON, then the picture.
fn send_request(stream: &mut TcpStream, json: &serde_json::Value, rgba: Option<&[u8]>) -> Result<(), AiError> {
    let bytes = serde_json::to_vec(json).map_err(|e| AiError::Invalid(e.to_string()))?;
    let len = u32::try_from(bytes.len()).map_err(|_| AiError::Invalid("request too large".into()))?;
    stream.write_all(&len.to_le_bytes()).map_err(io)?;
    stream.write_all(&bytes).map_err(io)?;
    if let Some(rgba) = rgba {
        stream.write_all(rgba).map_err(io)?;
    }
    stream.flush().map_err(io)
}

/// Asks the worker for the mask of `prompt`, starting it when needed (and once more when it had just quit).
fn run_worker(rgba: &[u8], width: u32, height: u32, prompt: &SelectionPrompt) -> Result<SelectionMask, AiError> {
    if width > MAX_WORKER_SIDE || height > MAX_WORKER_SIDE {
        return Err(AiError::Invalid(format!("the picture is larger than {MAX_WORKER_SIDE} pixels on a side")));
    }
    let mut request = match prompt {
        SelectionPrompt::Point { x, y, positive } => serde_json::json!({"prompt_type": "point", "points": [[x, y, u8::from(*positive)]]}),
        SelectionPrompt::Points { points } => {
            let points: Vec<[u32; 3]> = points.iter().take(64).map(|&(x, y, positive)| [x, y, u32::from(positive)]).collect();
            serde_json::json!({"prompt_type": "points", "points": points})
        }
        SelectionPrompt::Box { x0, y0, x1, y1 } => serde_json::json!({"prompt_type": "box", "box": [x0, y0, x1, y1]}),
        SelectionPrompt::Text { prompt } => {
            let text: String = prompt.chars().take(crate::MAX_PROMPT).collect();
            serde_json::json!({"prompt_type": "text", "text": text, "threshold": 0.3})
        }
        SelectionPrompt::Subject | SelectionPrompt::Background | SelectionPrompt::AllObjects => {
            serde_json::json!({"prompt_type": "box", "box": [0, 0, width, height]})
        }
    };
    request["cmd"] = "predict".into();
    request["width"] = width.into();
    request["height"] = height.into();
    let attempt = |request: &mut serde_json::Value| -> Result<SelectionMask, AiError> {
        let (port, token) = worker()?;
        request["token"] = token.into();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(io)?;
        stream.set_read_timeout(Some(WORKER_REQUEST)).map_err(io)?;
        stream.set_write_timeout(Some(WORKER_REQUEST)).map_err(io)?;
        send_request(&mut stream, request, Some(rgba))?;
        let mut header = [0u8; 16];
        stream.read_exact(&mut header).map_err(io)?;
        let word = |i: usize| u32::from_le_bytes([header[i], header[i + 1], header[i + 2], header[i + 3]]);
        let (status, mw, mh, meta_len) = (word(0), word(4), word(8), word(12) as usize);
        if meta_len > 1 << 20 {
            return Err(AiError::BadReply("the AI selection worker sent too much".into()));
        }
        let mut meta = vec![0u8; meta_len];
        stream.read_exact(&mut meta).map_err(io)?;
        let meta = String::from_utf8_lossy(&meta).into_owned();
        if status != 0 {
            return Err(AiError::Unsupported(format!("AI selection failed: {meta}")));
        }
        if mw == 0 || mh == 0 || mw > MAX_WORKER_SIDE || mh > MAX_WORKER_SIDE {
            return Err(AiError::BadReply("the AI selection worker sent a mask of the wrong size".into()));
        }
        let mut mask = vec![0u8; (mw as usize) * (mh as usize)];
        stream.read_exact(&mut mask).map_err(io)?;
        Ok(SelectionMask { width: mw, height: mh, mask, metadata: Some(meta) })
    };
    match attempt(&mut request) {
        // The worker may have quit for idleness between the check and the request: start it once more.
        Err(AiError::Network(_)) => {
            stop_worker();
            attempt(&mut request)
        }
        other => other,
    }
}

// ------------------------------------------------------------------ the Vision helper (macOS)

/// The Vision helper the app ships beside its binary, or one in the models folder.
fn find_vision_tool() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("apple_vision_seg")));
    [beside, Some(model_install::models_root().join("apple_vision_seg"))].into_iter().flatten().find(|path| path.is_file())
}

/// The main subject through the Vision helper: it reads a command, the size and the pixels, and answers a mask.
fn run_apple_vision(rgba: &[u8], width: u32, height: u32, prompt: &SelectionPrompt) -> Result<SelectionMask, AiError> {
    let tool = find_vision_tool().ok_or_else(|| AiError::Unsupported("the Vision helper isn't available".into()))?;
    let cmd: &[u8; 4] = match prompt {
        SelectionPrompt::AllObjects => b"ALL ",
        _ => b"FORE",
    };
    let mut child = Command::new(&tool)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AiError::Unsupported(format!("can't run the Vision helper: {e}")))?;
    let mut stdin = child.stdin.take().ok_or_else(|| AiError::Unsupported("the Vision helper has no input".into()))?;
    let mut header = Vec::with_capacity(12);
    header.extend_from_slice(cmd);
    header.extend_from_slice(&width.to_le_bytes());
    header.extend_from_slice(&height.to_le_bytes());
    stdin.write_all(&header).map_err(|e| AiError::Unsupported(format!("Vision helper: {e}")))?;
    stdin.write_all(rgba).map_err(|e| AiError::Unsupported(format!("Vision helper: {e}")))?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| AiError::Unsupported(format!("Vision helper: {e}")))?;
    let out = output.stdout;
    if !output.status.success() || out.get(..4) != Some(b"OK  ".as_slice()) {
        return Err(AiError::Unsupported(format!("Vision helper failed: {}", String::from_utf8_lossy(&output.stderr))));
    }
    let word = |i: usize| out.get(i..i + 4).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes);
    let (mw, mh) = (word(4), word(8));
    let len = (mw as usize).saturating_mul(mh as usize);
    let mask = out.get(12..12 + len).ok_or_else(|| AiError::BadReply("the Vision helper's mask is short".into()))?.to_vec();
    Ok(SelectionMask { width: mw, height: mh, mask, metadata: Some("Vision".into()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_resize_and_invert() {
        let mut m = SelectionMask { width: 2, height: 1, mask: vec![0, 255], metadata: None };
        let r = m.resized(4, 2);
        assert_eq!((r.width, r.height, r.mask.len()), (4, 2, 8));
        assert_eq!(r.mask.first(), Some(&0));
        assert_eq!(r.mask.last(), Some(&255));
        m.invert();
        assert_eq!(m.mask, vec![255, 0]);
    }

    #[test]
    fn a_bad_buffer_is_refused_before_any_engine_runs() {
        assert!(select(&[0; 3], 1, 1, &SelectionPrompt::Subject).is_err());
        assert!(select(&[], 0, 0, &SelectionPrompt::Subject).is_err());
    }

    #[test]
    fn prompts_say_which_pack_they_need() {
        assert!(SelectionPrompt::Text { prompt: "cat".into() }.needs_prompt_pack());
        assert!(!SelectionPrompt::Subject.needs_prompt_pack());
    }
}
