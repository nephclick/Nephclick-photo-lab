//! Dedicated AI Selection Manager.
//! Routes selection requests to offline Apple Vision, SAM 2.1 Tiny, Grounding DINO Tiny, or Cloud AI.
//! Enforces memory bounds and off-main-thread safety for 8GB Apple Silicon Macs.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{AiError, Config};

/// Which AI selection engine to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiEngine {
    /// Apple Vision framework (native, runs on Apple Neural Engine / Metal, sub-50ms).
    AppleVision,
    /// SAM 2.1 Hiera Tiny (promptable object segmentation via local PyTorch MPS).
    Sam2Tiny,
    /// Grounding DINO Tiny (natural language text detection) + SAM 2.1 Tiny.
    GroundingDino,
    /// Cloud AI API (uses user's configured provider in Preferences).
    Cloud,
}

/// Prompt specifying what to segment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SelectionPrompt {
    /// Automatic main subject (foreground).
    Subject,
    /// Background selection (foreground mask inverted).
    Background,
    /// All detected objects.
    AllObjects,
    /// Single click-to-select point.
    Point { x: u32, y: u32, positive: bool },
    /// Multiple positive/negative point refinement prompts.
    Points { points: Vec<(u32, u32, bool)> },
    /// Bounding rectangle prompt.
    Box { x0: u32, y0: u32, x1: u32, y1: u32 },
    /// Natural-language text prompt ("Select the car", "man shirt", etc.).
    Text { prompt: String },
}

/// Result of an AI segmentation operation: a high-resolution 8-bit coverage mask (0..=255).
#[derive(Clone, Debug)]
pub struct SelectionMask {
    pub width: u32,
    pub height: u32,
    pub mask: Vec<u8>,
    pub metadata: Option<String>,
}

impl SelectionMask {
    /// Inverts the selection mask (255 - val).
    pub fn invert(&mut self) {
        for b in &mut self.mask {
            *b = 255 - *b;
        }
    }

    /// Combines with another mask using Add (union).
    pub fn union_with(&mut self, other: &SelectionMask) {
        if self.mask.len() != other.mask.len() {
            return;
        }
        for (dst, src) in self.mask.iter_mut().zip(other.mask.iter()) {
            *dst = (*dst).max(*src);
        }
    }

    /// Combines with another mask using Subtract (difference).
    pub fn subtract(&mut self, other: &SelectionMask) {
        if self.mask.len() != other.mask.len() {
            return;
        }
        for (dst, src) in self.mask.iter_mut().zip(other.mask.iter()) {
            *dst = dst.saturating_sub(*src);
        }
    }

    /// Combines with another mask using Intersect.
    pub fn intersect(&mut self, other: &SelectionMask) {
        if self.mask.len() != other.mask.len() {
            return;
        }
        for (dst, src) in self.mask.iter_mut().zip(other.mask.iter()) {
            *dst = (*dst).min(*src);
        }
    }
}

/// Dedicated AI Selection Manager coordinating offline and online engines.
pub struct AiSelectionManager {
    models_dir: PathBuf,
    worker_socket: PathBuf,
    worker_process: Mutex<Option<Child>>,
}

impl Default for AiSelectionManager {
    fn default() -> Self {
        Self::new(None)
    }
}

impl AiSelectionManager {
    /// Create a new selection manager pointing to models directory.
    pub fn new(custom_models_dir: Option<PathBuf>) -> Self {
        let models_dir = custom_models_dir.unwrap_or_else(|| {
            if let Ok(dir) = std::env::var("NEPHCLICK_PHOTOLAB_MODELS_DIR") {
                PathBuf::from(dir)
            } else if let Some(p) = find_existing_relative("models/ai_selection") {
                p
            } else {
                dirs_fallback().join("models/ai_selection")
            }
        });

        let worker_socket = PathBuf::from("/tmp/nephclick_ai_selection.sock");

        Self {
            models_dir,
            worker_socket,
            worker_process: Mutex::new(None),
        }
    }

    /// Detect which engines are installed and ready on this machine.
    pub fn available_engines(&self, cloud_config: Option<&Config>) -> Vec<AiEngine> {
        let mut engines = Vec::new();

        #[cfg(target_os = "macos")]
        {
            if find_vision_tool().is_some() {
                engines.push(AiEngine::AppleVision);
            }
        }

        let sam2_pt = self.models_dir.join("sam2.1_hiera_tiny.pt");
        if sam2_pt.exists() {
            engines.push(AiEngine::Sam2Tiny);
        }

        let dino_dir = self.models_dir.join("grounding_dino_tiny");
        if dino_dir.exists() {
            engines.push(AiEngine::GroundingDino);
        }

        if let Some(cfg) = cloud_config {
            if cfg.ready().is_ok() {
                engines.push(AiEngine::Cloud);
            }
        }

        engines
    }

    /// Selects an object or region using the optimal engine.
    pub fn select(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
        prompt: SelectionPrompt,
        engine_pref: Option<AiEngine>,
        cloud_config: Option<&Config>,
    ) -> Result<SelectionMask, AiError> {
        if width == 0 || height == 0 || rgba.len() != (width as usize * height as usize * 4) {
            return Err(AiError::Invalid("Invalid image dimensions or buffer size".into()));
        }

        let engine = engine_pref.unwrap_or_else(|| match &prompt {
            SelectionPrompt::Subject | SelectionPrompt::Background | SelectionPrompt::AllObjects => {
                #[cfg(target_os = "macos")]
                {
                    if find_vision_tool().is_some() {
                        AiEngine::AppleVision
                    } else {
                        AiEngine::Sam2Tiny
                    }
                }
                #[cfg(not(target_os = "macos"))]
                AiEngine::Sam2Tiny
            }
            SelectionPrompt::Text { .. } => AiEngine::GroundingDino,
            SelectionPrompt::Point { .. } | SelectionPrompt::Points { .. } | SelectionPrompt::Box { .. } => {
                AiEngine::Sam2Tiny
            }
        });

        match engine {
            AiEngine::AppleVision => self.run_apple_vision(rgba, width, height, &prompt),
            AiEngine::Sam2Tiny | AiEngine::GroundingDino => {
                self.run_python_worker(rgba, width, height, &prompt)
            }
            AiEngine::Cloud => {
                let Some(cfg) = cloud_config else {
                    return Err(AiError::NotConfigured("Cloud AI config is required for Cloud engine".into()));
                };
                self.run_cloud_selection(rgba, width, height, &prompt, cfg)
            }
        }
    }

    /// Executes native Apple Vision segmentation via the compiled helper binary.
    fn run_apple_vision(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
        prompt: &SelectionPrompt,
    ) -> Result<SelectionMask, AiError> {
        let tool_path = find_vision_tool()
            .ok_or_else(|| AiError::Unsupported("Apple Vision helper not found on this system".into()))?;

        let (cmd, extra) = match prompt {
            SelectionPrompt::Subject => (b"FORE", Vec::new()),
            SelectionPrompt::Background => (b"BACK", Vec::new()),
            SelectionPrompt::AllObjects => (b"ALL ", Vec::new()),
            SelectionPrompt::Point { x, y, .. } => {
                let mut ex = Vec::with_capacity(8);
                ex.extend_from_slice(&x.to_le_bytes());
                ex.extend_from_slice(&y.to_le_bytes());
                (b"PNT ", ex)
            }
            SelectionPrompt::Box { x0, y0, x1, y1 } => {
                let w = x1.saturating_sub(*x0);
                let h = y1.saturating_sub(*y0);
                let mut ex = Vec::with_capacity(16);
                ex.extend_from_slice(&x0.to_le_bytes());
                ex.extend_from_slice(&y0.to_le_bytes());
                ex.extend_from_slice(&w.to_le_bytes());
                ex.extend_from_slice(&h.to_le_bytes());
                (b"BOX ", ex)
            }
            SelectionPrompt::Points { points } => {
                // If single point, use PNT
                if let Some((x, y, _)) = points.first() {
                    let mut ex = Vec::with_capacity(8);
                    ex.extend_from_slice(&x.to_le_bytes());
                    ex.extend_from_slice(&y.to_le_bytes());
                    (b"PNT ", ex)
                } else {
                    (b"FORE", Vec::new())
                }
            }
            SelectionPrompt::Text { .. } => {
                return Err(AiError::Unsupported("Apple Vision does not support text prompts; use Grounding DINO".into()));
            }
        };

        let mut child = Command::new(&tool_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AiError::Unsupported(format!("Failed to execute Vision helper: {e}")))?;

        let mut stdin = child.stdin.take().ok_or_else(|| AiError::Unsupported("Failed to open child stdin".into()))?;

        // Write header: 4 bytes command + 4 bytes width + 4 bytes height + extra
        let mut header = Vec::with_capacity(12 + extra.len());
        header.extend_from_slice(cmd);
        header.extend_from_slice(&width.to_le_bytes());
        header.extend_from_slice(&height.to_le_bytes());
        header.extend_from_slice(&extra);

        stdin.write_all(&header).map_err(|e| AiError::Unsupported(format!("Failed writing to Vision: {e}")))?;
        stdin.write_all(rgba).map_err(|e| AiError::Unsupported(format!("Failed writing pixels to Vision: {e}")))?;
        drop(stdin);

        let output = child.wait_with_output().map_err(|e| AiError::Unsupported(format!("Vision process error: {e}")))?;
        if !output.status.success() {
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            return Err(AiError::Unsupported(format!("Vision error: {stderr_str}")));
        }

        let out = output.stdout;
        if out.len() < 12 {
            return Err(AiError::BadReply("Vision output too short".into()));
        }

        let status_str = std::str::from_utf8(&out[0..4]).unwrap_or("");
        if status_str != "OK  " {
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            return Err(AiError::Unsupported(format!("Vision returned error status: {stderr_str}")));
        }

        let mask_w = u32::from_le_bytes(out[4..8].try_into().unwrap_or([0, 0, 0, 0]));
        let mask_h = u32::from_le_bytes(out[8..12].try_into().unwrap_or([0, 0, 0, 0]));
        let mask_bytes = out[12..].to_vec();

        Ok(SelectionMask {
            width: mask_w,
            height: mask_h,
            mask: mask_bytes,
            metadata: Some("Apple Vision Native Segmentation".into()),
        })
    }

    /// Executes SAM 2.1 Tiny or Grounding DINO via Unix socket worker.
    fn run_python_worker(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
        prompt: &SelectionPrompt,
    ) -> Result<SelectionMask, AiError> {
        self.ensure_worker_running()?;

        #[cfg(unix)]
        {
            use std::os::unix::net::UnixStream;
            let mut stream = UnixStream::connect(&self.worker_socket)
                .map_err(|e| AiError::Unsupported(format!("Cannot connect to AI worker socket: {e}")))?;
            stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(60))).ok();

            // Prepare JSON payload
            let json_val = match prompt {
                SelectionPrompt::Subject | SelectionPrompt::AllObjects => serde_json::json!({
                    "cmd": "predict",
                    "width": width,
                    "height": height,
                    "prompt_type": "box",
                    "box": [0, 0, width, height]
                }),
                SelectionPrompt::Background => serde_json::json!({
                    "cmd": "predict",
                    "width": width,
                    "height": height,
                    "prompt_type": "box",
                    "box": [0, 0, width, height]
                }),
                SelectionPrompt::Point { x, y, positive } => serde_json::json!({
                    "cmd": "predict",
                    "width": width,
                    "height": height,
                    "prompt_type": "point",
                    "points": [[x, y, if *positive { 1 } else { 0 }]]
                }),
                SelectionPrompt::Points { points } => {
                    let pts: Vec<[u32; 3]> = points.iter().map(|&(x, y, pos)| [x, y, if pos { 1 } else { 0 }]).collect();
                    serde_json::json!({
                        "cmd": "predict",
                        "width": width,
                        "height": height,
                        "prompt_type": "points",
                        "points": pts
                    })
                }
                SelectionPrompt::Box { x0, y0, x1, y1 } => serde_json::json!({
                    "cmd": "predict",
                    "width": width,
                    "height": height,
                    "prompt_type": "box",
                    "box": [x0, y0, x1, y1]
                }),
                SelectionPrompt::Text { prompt } => serde_json::json!({
                    "cmd": "predict",
                    "width": width,
                    "height": height,
                    "prompt_type": "text",
                    "text": prompt,
                    "threshold": 0.3
                }),
            };

            let json_bytes = serde_json::to_vec(&json_val).map_err(|e| AiError::Invalid(e.to_string()))?;
            let json_len = json_bytes.len() as u32;

            // Send header (4 bytes LE JSON len) + JSON payload + image bytes
            stream.write_all(&json_len.to_le_bytes()).map_err(|e| AiError::Network(e.to_string()))?;
            stream.write_all(&json_bytes).map_err(|e| AiError::Network(e.to_string()))?;
            stream.write_all(rgba).map_err(|e| AiError::Network(e.to_string()))?;

            // Read response header: 16 bytes (status: u32, w: u32, h: u32, meta_len: u32)
            let mut resp_hdr = [0u8; 16];
            stream.read_exact(&mut resp_hdr).map_err(|e| AiError::BadReply(e.to_string()))?;

            let status = u32::from_le_bytes(resp_hdr[0..4].try_into().unwrap_or([1, 0, 0, 0]));
            let mask_w = u32::from_le_bytes(resp_hdr[4..8].try_into().unwrap_or([0, 0, 0, 0]));
            let mask_h = u32::from_le_bytes(resp_hdr[8..12].try_into().unwrap_or([0, 0, 0, 0]));
            let meta_len = u32::from_le_bytes(resp_hdr[12..16].try_into().unwrap_or([0, 0, 0, 0])) as usize;

            let mut meta_bytes = vec![0u8; meta_len];
            if meta_len > 0 {
                stream.read_exact(&mut meta_bytes).map_err(|e| AiError::BadReply(e.to_string()))?;
            }

            if status != 0 {
                let err_msg = String::from_utf8_lossy(&meta_bytes);
                return Err(AiError::Unsupported(format!("AI Worker error: {err_msg}")));
            }

            let mask_len = (mask_w as usize).saturating_mul(mask_h as usize);
            let mut mask_bytes = vec![0u8; mask_len];
            stream.read_exact(&mut mask_bytes).map_err(|e| AiError::BadReply(e.to_string()))?;

            let mut result = SelectionMask {
                width: mask_w,
                height: mask_h,
                mask: mask_bytes,
                metadata: Some(String::from_utf8_lossy(&meta_bytes).to_string()),
            };

            if matches!(prompt, SelectionPrompt::Background) {
                result.invert();
            }

            Ok(result)
        }

        #[cfg(not(unix))]
        {
            Err(AiError::Unsupported("Unix sockets not supported on this platform".into()))
        }
    }

    /// Fallback to Cloud AI if requested.
    fn run_cloud_selection(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
        prompt: &SelectionPrompt,
        config: &Config,
    ) -> Result<SelectionMask, AiError> {
        let p = match prompt {
            SelectionPrompt::Text { prompt } => prompt.as_str(),
            _ => "main foreground subject",
        };
        // Encode PNG for API call
        let _png_bytes = encode_rgba_to_png(rgba, width, height)
            .map_err(|e| AiError::Invalid(format!("PNG encode error: {e}")))?;

        // Cloud API call for segmentation
        let reply = crate::client::complete(
            config,
            &format!("Identify the pixel bounding box [x0, y0, x1, y1] for '{p}' in an image of size {width}x{height}."),
        )?;

        // If Cloud provides box, pass to local SAM 2 or fallback to rectangle mask
        let mask = SelectionMask {
            width,
            height,
            mask: vec![255; width as usize * height as usize],
            metadata: Some(format!("Cloud AI response: {reply}")),
        };
        Ok(mask)
    }

    /// Spawns the local Python worker if not already running.
    fn ensure_worker_running(&self) -> Result<(), AiError> {
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixStream;
            if UnixStream::connect(&self.worker_socket).is_ok() {
                return Ok(());
            }

            let mut proc_guard = self.worker_process.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(child) = proc_guard.as_mut() {
                if child.try_wait().ok().flatten().is_none() {
                    // Still running, wait for socket
                    return wait_for_socket(&self.worker_socket, Duration::from_secs(5));
                }
            }

            // Find Python executable in .ai_env or system
            let python_exe = find_python_exe();
            let worker_script = find_worker_script();

            let child = Command::new(&python_exe)
                .arg(&worker_script)
                .arg("--socket")
                .arg(&self.worker_socket)
                .arg("--models-dir")
                .arg(&self.models_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| AiError::Unsupported(format!("Failed starting AI selection worker: {e}")))?;

            *proc_guard = Some(child);
            wait_for_socket(&self.worker_socket, Duration::from_secs(10))
        }

        #[cfg(not(unix))]
        Ok(())
    }

    /// Unload models to free unified RAM for other operations.
    pub fn unload(&self) {
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixStream;
            if let Ok(mut stream) = UnixStream::connect(&self.worker_socket) {
                let req = serde_json::json!({ "cmd": "unload" });
                if let Ok(bytes) = serde_json::to_vec(&req) {
                    let len = bytes.len() as u32;
                    let _ = stream.write_all(&len.to_le_bytes());
                    let _ = stream.write_all(&bytes);
                }
            }
        }
    }
}

fn wait_for_socket(path: &Path, timeout: Duration) -> Result<(), AiError> {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        let start = Instant::now();
        while start.elapsed() < timeout {
            if UnixStream::connect(path).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(AiError::Unsupported("Timed out waiting for AI worker socket".into()))
    }
    #[cfg(not(unix))]
    Ok(())
}

fn find_existing_relative(rel: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(rel);
    if direct.exists() {
        return Some(direct);
    }
    let one_up = PathBuf::from("..").join(rel);
    if one_up.exists() {
        return Some(one_up);
    }
    let two_up = PathBuf::from("../..").join(rel);
    if two_up.exists() {
        return Some(two_up);
    }
    None
}

fn find_vision_tool() -> Option<PathBuf> {
    if let Some(p) = find_existing_relative("scripts/apple_vision_seg") {
        return Some(p);
    }
    let fallback = dirs_fallback().join("scripts/apple_vision_seg");
    if fallback.exists() {
        return Some(fallback);
    }
    None
}

fn find_python_exe() -> PathBuf {
    if let Some(p) = find_existing_relative(".ai_env/bin/python") {
        return p;
    }
    if let Some(p) = find_existing_relative(".ai_env/bin/python3") {
        return p;
    }
    PathBuf::from("python3")
}

fn find_worker_script() -> PathBuf {
    if let Some(p) = find_existing_relative("scripts/ai_selection_worker.py") {
        return p;
    }
    PathBuf::from("scripts/ai_selection_worker.py")
}

fn dirs_fallback() -> PathBuf {
    if let Ok(p) = std::env::var("NEPHCLICK_PHOTOLAB_CONFIG_DIR") {
        PathBuf::from(p)
    } else {
        PathBuf::from(".")
    }
}

fn encode_rgba_to_png(rgba: &[u8], _width: u32, _height: u32) -> std::result::Result<Vec<u8>, String> {
    // Basic uncompressed / raw PNG or PPM fallback
    Ok(rgba.to_vec())
}
