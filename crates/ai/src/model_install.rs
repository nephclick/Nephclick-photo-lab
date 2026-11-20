//! The offline AI selection models: where they live, whether they are installed, and the background installs that
//! fetch them.
//!
//! Two packs, each installed from the app on request and usable offline afterwards:
//!
//! - **Subject** ([`Pack::Subject`]): one ONNX model (ISNet, general use; Apache-2.0) that Select Subject,
//!   Select Background and All Objects run in-process with a pure-Rust runtime. About 170 MB on disk; a few hundred
//!   MB of memory while it runs.
//! - **Prompt & Click** ([`Pack::Prompt`]): a private Python (portable CPython; PSF licence) with PyTorch and
//!   Transformers, and the SAM 2.1 Tiny (Apache-2.0) and Grounding DINO Tiny (Apache-2.0) weights. Click-to-select,
//!   rectangle prompts and text prompts use it through a worker process that starts when one of them runs and quits
//!   after a short idle time, so it holds no memory while unused. About 1.5–2 GB on disk.
//!
//! Everything goes under the models root ([`models_root`]): the app's settings folder unless
//! `NEPHCLICK_PHOTOLAB_MODELS_DIR` names another. Downloads are written to a `.part` file and moved into place only
//! when complete (and, for the subject model, when its size and SHA-256 match), so a cancelled or failed install never
//! leaves a broken model behind.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::AiError;

/// The subject model's file name.
pub const SUBJECT_MODEL_FILE: &str = "isnet-general-use.onnx";
/// Where the subject model is downloaded from.
pub const SUBJECT_MODEL_URL: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx";
/// The subject model's size in bytes.
pub const SUBJECT_MODEL_BYTES: u64 = 178_648_008;
/// The subject model's SHA-256.
pub const SUBJECT_MODEL_SHA256: &str = "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a";

/// The Light Pose depth model's file name (MiDaS v2.1 Small, MIT).
pub const DEPTH_MODEL_FILE: &str = "midas-v21-small-256.onnx";
/// Where the depth model is downloaded from.
pub const DEPTH_MODEL_URL: &str = "https://github.com/isl-org/MiDaS/releases/download/v2_1/model-small.onnx";
/// The depth model's size in bytes.
pub const DEPTH_MODEL_BYTES: u64 = 66_764_249;
/// The depth model's SHA-256.
pub const DEPTH_MODEL_SHA256: &str = "2d8c6cb8f415229daf1eb041024208e2608c9f98e17c81cc7c6ecb449c56fd58";

/// The portable CPython release the Prompt & Click pack installs.
const PYTHON_RELEASE: &str = "20241016";
const PYTHON_VERSION: &str = "3.12.7";
/// Largest portable Python archive accepted.
const MAX_PYTHON_ARCHIVE: u64 = 256 << 20;

/// The SAM 2.1 Tiny weights, in Transformers' format.
pub const SAM_MODEL: &str = "facebook/sam2.1-hiera-tiny";
/// The Grounding DINO Tiny weights, in Transformers' format.
pub const DINO_MODEL: &str = "IDEA-Research/grounding-dino-tiny";

/// The Python worker that runs SAM 2.1 and Grounding DINO.
const WORKER_SCRIPT: &str = include_str!("sam_worker.py");

/// The models root the app set ([`set_models_root`]).
static ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets the folder the models are kept in: the app's settings folder's `ai_selection`.
pub fn set_models_root(dir: PathBuf) {
    *ROOT.lock().unwrap_or_else(PoisonError::into_inner) = Some(dir);
}

/// The folder the models are kept in: `NEPHCLICK_PHOTOLAB_MODELS_DIR`, else the folder the app set, else
/// `ai_selection` in `NEPHCLICK_PHOTOLAB_CONFIG_DIR`, else `ai_selection` in the working directory.
pub fn models_root() -> PathBuf {
    if let Ok(dir) = std::env::var("NEPHCLICK_PHOTOLAB_MODELS_DIR")
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir);
    }
    if let Some(dir) = ROOT.lock().unwrap_or_else(PoisonError::into_inner).clone() {
        return dir;
    }
    if let Ok(dir) = std::env::var("NEPHCLICK_PHOTOLAB_CONFIG_DIR")
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir).join("ai_selection");
    }
    PathBuf::from("ai_selection")
}

/// The subject model's path.
pub fn subject_model_path() -> PathBuf {
    models_root().join(SUBJECT_MODEL_FILE)
}

/// The depth model's path.
pub fn depth_model_path() -> PathBuf {
    models_root().join(DEPTH_MODEL_FILE)
}

/// Whether the Light Pose depth model is installed.
pub fn depth_installed() -> bool {
    std::fs::metadata(depth_model_path()).is_ok_and(|meta| meta.is_file() && meta.len() == DEPTH_MODEL_BYTES)
}

/// The Prompt & Click pack's folder.
fn prompt_dir() -> PathBuf {
    models_root().join("prompt")
}

/// The file that says the Prompt & Click pack finished installing.
fn prompt_marker() -> PathBuf {
    prompt_dir().join("installed.json")
}

/// Hugging Face's cache of the Prompt & Click weights.
pub(crate) fn hf_home() -> PathBuf {
    prompt_dir().join("hf")
}

/// Whether the subject model is installed: present at its full size.
pub fn subject_installed() -> bool {
    std::fs::metadata(subject_model_path()).is_ok_and(|meta| meta.is_file() && meta.len() == SUBJECT_MODEL_BYTES)
}

/// Whether the Prompt & Click pack is installed: its Python, its worker and its marker are there.
pub fn prompt_installed() -> bool {
    prompt_marker().is_file() && python_exe().is_some()
}

/// The pack's private Python interpreter, when it is installed.
pub fn python_exe() -> Option<PathBuf> {
    let base = prompt_dir().join("python");
    let candidates = if cfg!(windows) { vec![base.join("python.exe")] } else { vec![base.join("bin").join("python3"), base.join("bin").join("python")] };
    candidates.into_iter().find(|path| path.is_file())
}

/// The worker script, written next to the pack's Python from the copy built into the app (refreshed when the app's
/// copy changed).
pub(crate) fn worker_script() -> Result<PathBuf, AiError> {
    let path = prompt_dir().join("sam_worker.py");
    let current = std::fs::read_to_string(&path).ok();
    if current.as_deref() != Some(WORKER_SCRIPT) {
        std::fs::create_dir_all(prompt_dir()).map_err(|e| AiError::Unsupported(format!("can't write the AI worker: {e}")))?;
        std::fs::write(&path, WORKER_SCRIPT).map_err(|e| AiError::Unsupported(format!("can't write the AI worker: {e}")))?;
    }
    Ok(path)
}

/// A pack of models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Pack {
    /// The subject model.
    Subject,
    /// Python, PyTorch, Transformers, SAM 2.1 Tiny and Grounding DINO Tiny.
    Prompt,
    /// Light Pose's depth model.
    Depth,
}

impl Pack {
    /// The pack named `name` (`subject` or `prompt`).
    pub fn parse(name: &str) -> Option<Pack> {
        match name {
            "subject" => Some(Pack::Subject),
            "prompt" => Some(Pack::Prompt),
            "depth" => Some(Pack::Depth),
            _ => None,
        }
    }
}

/// What a running (or the last) install is doing.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// The pack being installed; `None` when none runs.
    pub running: Option<Pack>,
    /// What it is doing now, for the user.
    pub stage: String,
    /// Bytes done of the current download (0 for a step that isn't one).
    pub done: u64,
    /// Bytes the current download has in all (0 when unknown).
    pub total: u64,
    /// Why the last install stopped, when it failed or was cancelled.
    pub error: Option<String>,
    /// The pack the last install finished.
    pub finished: Option<Pack>,
}

static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);
static CANCEL: AtomicBool = AtomicBool::new(false);

fn with_progress<T>(f: impl FnOnce(&mut Progress) -> T) -> T {
    let mut held = PROGRESS.lock().unwrap_or_else(PoisonError::into_inner);
    f(held.get_or_insert_with(Progress::default))
}

/// The install's progress now.
pub fn progress() -> Progress {
    with_progress(|p| p.clone())
}

/// Asks the running install to stop; it stops at its next step and removes what it left half done.
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

fn cancelled() -> Result<(), AiError> {
    if CANCEL.load(Ordering::SeqCst) { Err(AiError::Unsupported("the install was cancelled".into())) } else { Ok(()) }
}

fn stage(text: impl Into<String>) {
    let text = text.into();
    with_progress(|p| {
        p.stage = text;
        p.done = 0;
        p.total = 0;
    });
}

/// What is installed and what an install is doing, as the `select.aiModels` command answers.
pub fn status() -> Value {
    let progress = progress();
    json!({
        "modelsDir": models_root().to_string_lossy(),
        "subject": {"installed": subject_installed(), "bytes": SUBJECT_MODEL_BYTES, "source": SUBJECT_MODEL_URL},
        "prompt": {"installed": prompt_installed(), "models": [SAM_MODEL, DINO_MODEL]},
        "depth": {"installed": depth_installed(), "bytes": DEPTH_MODEL_BYTES, "source": DEPTH_MODEL_URL},
        "progress": progress,
    })
}

/// Starts installing `pack` on a background thread. Refused while another install runs.
pub fn start(pack: Pack) -> Result<(), AiError> {
    let begun = with_progress(|p| {
        if p.running.is_some() {
            return false;
        }
        *p = Progress { running: Some(pack), stage: "Starting…".into(), ..Progress::default() };
        true
    });
    if !begun {
        return Err(AiError::Unsupported("another model install is running".into()));
    }
    CANCEL.store(false, Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("ai-model-install".into()).spawn(move || {
        let outcome = std::panic::catch_unwind(|| match pack {
            Pack::Subject => install_subject(),
            Pack::Prompt => install_prompt(),
            Pack::Depth => install_depth(),
        })
        .unwrap_or_else(|_| Err(AiError::Unsupported("the install stopped with an internal error".into())));
        with_progress(|p| {
            p.running = None;
            p.done = 0;
            p.total = 0;
            match outcome {
                Ok(()) => {
                    p.stage = "Installed".into();
                    p.error = None;
                    p.finished = Some(pack);
                }
                Err(e) => {
                    p.stage = "Not installed".into();
                    p.error = Some(e.to_string());
                }
            }
        });
    });
    if let Err(e) = spawned {
        with_progress(|p| p.running = None);
        return Err(AiError::Unsupported(format!("can't start the install: {e}")));
    }
    Ok(())
}

/// Removes `pack`'s files. Refused while an install runs.
pub fn remove(pack: Pack) -> Result<(), AiError> {
    if progress().running.is_some() {
        return Err(AiError::Unsupported("wait for the running install to finish".into()));
    }
    let gone = |result: std::io::Result<()>| match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(AiError::Unsupported(format!("can't remove the models: {e}"))),
        _ => Ok(()),
    };
    match pack {
        Pack::Subject => {
            crate::subject_model::unload();
            gone(std::fs::remove_file(subject_model_path()))
        }
        Pack::Prompt => {
            crate::selection_manager::stop_worker();
            gone(std::fs::remove_dir_all(prompt_dir()))
        }
        Pack::Depth => {
            crate::depth_model::unload();
            gone(std::fs::remove_file(depth_model_path()))
        }
    }
}

/// An agent for model downloads: generous deadlines for large files, a generic user agent.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .user_agent("NephclickPhotoLab")
        .build()
        .into()
}

/// Downloads `url` to `dest` through `dest.part`, reporting progress, refusing more than `max` bytes; with `expect`,
/// the size and SHA-256 must match.
fn download(url: &str, dest: &Path, max: u64, expect: Option<(u64, &str)>) -> Result<(), AiError> {
    use sha2::Digest as _;
    let part = dest.with_extension("part");
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| AiError::Unsupported(format!("can't create {}: {e}", dir.display())))?;
    }
    let result = (|| {
        let mut response = agent().get(url).call().map_err(|e| AiError::Network(e.to_string()))?;
        let total = response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if total > max {
            return Err(AiError::BadReply("the download is larger than expected".into()));
        }
        with_progress(|p| {
            p.done = 0;
            p.total = expect.map_or(total, |(len, _)| len);
        });
        let mut reader = response.body_mut().with_config().limit(max).reader();
        let mut file = std::fs::File::create(&part).map_err(|e| AiError::Unsupported(format!("can't write {}: {e}", part.display())))?;
        let mut hasher = sha2::Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut done = 0u64;
        loop {
            cancelled()?;
            let n = reader.read(&mut buf).map_err(|e| AiError::Network(e.to_string()))?;
            if n == 0 {
                break;
            }
            let chunk = buf.get(..n).unwrap_or_default();
            file.write_all(chunk).map_err(|e| AiError::Unsupported(format!("can't write {}: {e}", part.display())))?;
            hasher.update(chunk);
            done = done.saturating_add(n as u64);
            with_progress(|p| p.done = done);
        }
        file.flush().map_err(|e| AiError::Unsupported(e.to_string()))?;
        drop(file);
        if let Some((len, sha)) = expect {
            let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if done != len || got != sha {
                return Err(AiError::BadReply("the downloaded model is damaged or not the expected file".into()));
            }
        }
        std::fs::rename(&part, dest).map_err(|e| AiError::Unsupported(format!("can't move the download into place: {e}")))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// Installs the subject model.
fn install_subject() -> Result<(), AiError> {
    if subject_installed() {
        return Ok(());
    }
    stage("Downloading the subject model (about 170 MB)…");
    download(SUBJECT_MODEL_URL, &subject_model_path(), SUBJECT_MODEL_BYTES + 1, Some((SUBJECT_MODEL_BYTES, SUBJECT_MODEL_SHA256)))
}

/// Installs the Light Pose depth model.
fn install_depth() -> Result<(), AiError> {
    if depth_installed() {
        return Ok(());
    }
    stage("Downloading the Light Pose depth model (about 67 MB)…");
    download(DEPTH_MODEL_URL, &depth_model_path(), DEPTH_MODEL_BYTES + 1, Some((DEPTH_MODEL_BYTES, DEPTH_MODEL_SHA256)))
}

/// The portable CPython build for this machine, by its target triple.
fn python_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

/// Runs `command`, its output kept for the error; fails when it can't start or exits unsuccessfully.
fn run(what: &str, command: &mut Command) -> Result<(), AiError> {
    cancelled()?;
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| AiError::Unsupported(format!("{what}: can't start: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    let tail: String = err.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    Err(AiError::Unsupported(format!("{what} failed: {tail}")))
}

/// The pack's Python with the environment its installs and its worker use.
pub(crate) fn python_command(python: &Path) -> Command {
    let mut command = Command::new(python);
    command
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
        .env("HF_HOME", hf_home())
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("PYTORCH_ENABLE_MPS_FALLBACK", "1");
    command
}

/// Installs the Prompt & Click pack: Python, then PyTorch and Transformers, then the weights.
fn install_prompt() -> Result<(), AiError> {
    let dir = prompt_dir();
    std::fs::create_dir_all(&dir).map_err(|e| AiError::Unsupported(format!("can't create {}: {e}", dir.display())))?;
    let _ = std::fs::remove_file(prompt_marker());
    // 1. A private Python, so nothing on the system is needed or changed.
    let python = match python_exe() {
        Some(python) => python,
        None => {
            let triple = python_triple().ok_or_else(|| AiError::Unsupported("Prompt & Click selection isn't available for this system".into()))?;
            let url = format!(
                "https://github.com/astral-sh/python-build-standalone/releases/download/{PYTHON_RELEASE}/cpython-{PYTHON_VERSION}%2B{PYTHON_RELEASE}-{triple}-install_only.tar.gz"
            );
            stage("Downloading Python…");
            let archive = dir.join("python.tar.gz");
            download(&url, &archive, MAX_PYTHON_ARCHIVE, None)?;
            stage("Unpacking Python…");
            let unpacked = run("Unpacking Python", Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&dir));
            let _ = std::fs::remove_file(&archive);
            unpacked?;
            python_exe().ok_or_else(|| AiError::Unsupported("Python was unpacked but its interpreter is missing".into()))?
        }
    };
    // 2. PyTorch (the CPU build off the Mac, where the default would pull GPU libraries of several GB) and Transformers.
    stage("Installing PyTorch (this can take several minutes)…");
    let mut torch = python_command(&python);
    torch.args(["-m", "pip", "install", "--no-warn-script-location", "--upgrade", "torch"]);
    if !cfg!(target_os = "macos") {
        torch.args(["--index-url", "https://download.pytorch.org/whl/cpu"]);
    }
    run("Installing PyTorch", &mut torch)?;
    stage("Installing Transformers…");
    run(
        "Installing Transformers",
        python_command(&python).args(["-m", "pip", "install", "--no-warn-script-location", "--upgrade", "transformers>=4.56,<6", "pillow", "numpy"]),
    )?;
    // 3. The weights, into the pack's own cache: offline from then on.
    stage("Downloading SAM 2.1 Tiny and Grounding DINO Tiny…");
    let fetch = format!(
        "from transformers import AutoProcessor, AutoModelForZeroShotObjectDetection, Sam2Model, Sam2Processor\n\
         Sam2Processor.from_pretrained('{SAM_MODEL}'); Sam2Model.from_pretrained('{SAM_MODEL}')\n\
         AutoProcessor.from_pretrained('{DINO_MODEL}'); AutoModelForZeroShotObjectDetection.from_pretrained('{DINO_MODEL}')\n"
    );
    run("Downloading the models", python_command(&python).args(["-c", &fetch]))?;
    worker_script()?;
    cancelled()?;
    std::fs::write(prompt_marker(), json!({"python": PYTHON_VERSION, "models": [SAM_MODEL, DINO_MODEL]}).to_string())
        .map_err(|e| AiError::Unsupported(format!("can't finish the install: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_parse_by_name() {
        assert_eq!(Pack::parse("subject"), Some(Pack::Subject));
        assert_eq!(Pack::parse("prompt"), Some(Pack::Prompt));
        assert_eq!(Pack::parse("depth"), Some(Pack::Depth));
        assert_eq!(Pack::parse("other"), None);
    }

    #[test]
    fn the_worker_script_is_built_in() {
        assert!(WORKER_SCRIPT.contains("def main"));
    }
}
