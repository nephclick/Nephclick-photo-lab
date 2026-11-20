#!/usr/bin/env python3
"""
Nephclick PhotoLab AI Selection Worker
Provides offline inference for SAM 2.1 Tiny and Grounding DINO Tiny
over a Unix domain socket using binary image and mask buffers.
Optimized for 8GB Apple Silicon (MPS acceleration, lazy loading, memory safety).
"""

import os
import sys
import gc
import json
import socket
import struct
import argparse
from typing import Optional, Tuple, Dict, Any

# Ensure local dependencies and sam2_repo are on path
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
SAM2_REPO = os.path.join(SCRIPT_DIR, "sam2_repo")
if os.path.exists(SAM2_REPO) and SAM2_REPO not in sys.path:
    sys.path.insert(0, SAM2_REPO)

import numpy as np
import torch
from PIL import Image

DEVICE = "mps" if torch.backends.mps.is_available() else "cpu"
print(f"[AI Worker] Starting AI Selection Worker. Inference device: {DEVICE}")

class ModelManager:
    def __init__(self, models_dir: str):
        self.models_dir = models_dir
        self.sam2_predictor = None
        self.dino_processor = None
        self.dino_model = None
        self.last_image_hash = None

    def get_sam2_predictor(self):
        if self.sam2_predictor is None:
            print("[AI Worker] Loading SAM 2.1 Tiny...")
            from sam2.build_sam import build_sam2
            from sam2.sam2_image_predictor import SAM2ImagePredictor

            checkpoint = os.path.join(self.models_dir, "sam2.1_hiera_tiny.pt")
            model_cfg = "configs/sam2.1/sam2.1_hiera_t.yaml"
            if not os.path.exists(checkpoint):
                raise FileNotFoundError(f"SAM 2.1 checkpoint not found at {checkpoint}")

            model = build_sam2(model_cfg, checkpoint, device=DEVICE)
            self.sam2_predictor = SAM2ImagePredictor(model)
            print("[AI Worker] SAM 2.1 Tiny loaded.")
        return self.sam2_predictor

    def get_grounding_dino(self):
        if self.dino_model is None or self.dino_processor is None:
            print("[AI Worker] Loading Grounding DINO Tiny...")
            from transformers import AutoProcessor, AutoModelForZeroShotObjectDetection

            dino_dir = os.path.join(self.models_dir, "grounding_dino_tiny")
            if not os.path.exists(dino_dir):
                dino_dir = "IDEA-Research/grounding-dino-tiny"

            self.dino_processor = AutoProcessor.from_pretrained(dino_dir)
            self.dino_model = AutoModelForZeroShotObjectDetection.from_pretrained(dino_dir).to(DEVICE)
            self.dino_model.eval()
            print("[AI Worker] Grounding DINO Tiny loaded.")
        return self.dino_processor, self.dino_model

    def unload(self, target: Optional[str] = None):
        """Unload models to free memory for 8GB Mac constraint"""
        if target in (None, "dino"):
            self.dino_processor = None
            self.dino_model = None
            print("[AI Worker] Grounding DINO unloaded.")
        if target in (None, "sam2"):
            self.sam2_predictor = None
            self.last_image_hash = None
            print("[AI Worker] SAM 2.1 Tiny unloaded.")
        gc.collect()
        if DEVICE == "mps":
            torch.mps.empty_cache()

def read_exact(sock: socket.socket, n: int) -> bytes:
    data = bytearray()
    while len(data) < n:
        chunk = sock.recv(n - len(data))
        if not chunk:
            raise ConnectionResetError("Socket connection closed prematurely")
        data.extend(chunk)
    return bytes(data)

def handle_request(manager: ModelManager, header_bytes: bytes, sock: socket.socket) -> Tuple[bytes, bytes]:
    """
    Protocol:
    Client sends:
    - 4 bytes uint32: JSON payload length (LE)
    - N bytes JSON:
        {
          "cmd": "predict" | "unload" | "ping",
          "width": int,
          "height": int,
          "prompt_type": "point" | "points" | "box" | "text",
          "points": [[x, y, label], ...],
          "box": [x0, y0, x1, y1],
          "text": "...",
          "threshold": 0.3
        }
    - (If cmd == "predict") width * height * 4 bytes RGBA image buffer.

    Worker replies:
    - 4 bytes uint32: status (0 = success, 1 = error)
    - 4 bytes uint32: mask width (LE)
    - 4 bytes uint32: mask height (LE)
    - 4 bytes uint32: JSON metadata length (LE)
    - M bytes JSON string
    - mask_width * mask_height bytes GRAY8 mask (0-255)
    """
    json_len = struct.unpack("<I", header_bytes)[0]
    json_data = read_exact(sock, json_len).decode("utf-8")
    req = json.loads(json_data)
    cmd = req.get("cmd", "predict")

    if cmd == "ping":
        meta = json.dumps({"status": "ok", "device": DEVICE}).encode("utf-8")
        resp_hdr = struct.pack("<IIII", 0, 0, 0, len(meta))
        return resp_hdr + meta, b""

    if cmd == "unload":
        manager.unload(req.get("target"))
        meta = json.dumps({"status": "unloaded"}).encode("utf-8")
        resp_hdr = struct.pack("<IIII", 0, 0, 0, len(meta))
        return resp_hdr + meta, b""

    width = req["width"]
    height = req["height"]
    img_bytes = read_exact(sock, width * height * 4)

    # Convert RGBA to RGB for inference
    img_np = np.frombuffer(img_bytes, dtype=np.uint8).reshape((height, width, 4)).copy()
    rgb_img = img_np[:, :, :3]

    prompt_type = req.get("prompt_type", "box")
    predictor = manager.get_sam2_predictor()

    # Set image into SAM 2 predictor
    # Cache image embedding across queries if same image
    img_hash = hash((width, height, img_bytes[0:100], img_bytes[-100:]))
    if manager.last_image_hash != img_hash:
        predictor.set_image(rgb_img)
        manager.last_image_hash = img_hash

    boxes = None
    point_coords = None
    point_labels = None
    meta_info = {}

    if prompt_type == "text":
        prompt_text = req.get("text", "")
        print(f"[AI Worker] Text selection requested: '{prompt_text}'")
        processor, dino_model = manager.get_grounding_dino()

        # Format input for Grounding DINO
        pil_img = Image.fromarray(rgb_img)
        text_prompt = prompt_text.strip().lower()
        if not text_prompt.endswith("."):
            text_prompt += "."

        inputs = processor(images=pil_img, text=text_prompt, return_tensors="pt").to(DEVICE)
        with torch.no_grad():
            outputs = dino_model(**inputs)

        threshold = float(req.get("threshold", 0.3))
        results = processor.post_process_grounded_object_detection(
            outputs,
            inputs.input_ids,
            threshold=threshold,
            text_threshold=threshold,
            target_sizes=[(height, width)]
        )

        detected_boxes = results[0]["boxes"].cpu().numpy()
        scores = results[0]["scores"].cpu().numpy()
        labels = results[0]["labels"]

        if len(detected_boxes) > 0:
            # Pick highest confidence box
            best_idx = np.argmax(scores)
            best_box = detected_boxes[best_idx]
            boxes = np.array([best_box])
            meta_info["detected_box"] = [float(v) for v in best_box]
            meta_info["confidence"] = float(scores[best_idx])
            meta_info["label"] = str(labels[best_idx])
            print(f"[AI Worker] Grounding DINO detected box: {best_box} (score: {scores[best_idx]:.2f})")
        else:
            print("[AI Worker] Grounding DINO found no matching object.")
            # Return empty mask
            empty_mask = np.zeros((height, width), dtype=np.uint8)
            meta = json.dumps({"found": False}).encode("utf-8")
            resp_hdr = struct.pack("<IIII", 0, width, height, len(meta))
            return resp_hdr + meta, empty_mask.tobytes()

    elif prompt_type in ("point", "points"):
        pts = req.get("points", [])
        if pts:
            coords = [[p[0], p[1]] for p in pts]
            labels = [p[2] if len(p) > 2 else 1 for p in pts]
            point_coords = np.array(coords, dtype=np.float32)
            point_labels = np.array(labels, dtype=np.int32)
    elif prompt_type == "box":
        box = req.get("box")
        if box:
            boxes = np.array([box], dtype=np.float32)

    # Run SAM 2 prediction
    masks, scores, _ = predictor.predict(
        point_coords=point_coords,
        point_labels=point_labels,
        box=boxes,
        multimask_output=False
    )

    mask = masks[0] if len(masks.shape) == 3 else masks
    # Convert bool/float mask to uint8 (0 or 255)
    binary_mask = (mask > 0.0).astype(np.uint8) * 255

    meta_info["score"] = float(scores[0]) if len(scores) > 0 else 1.0
    meta_bytes = json.dumps(meta_info).encode("utf-8")
    resp_hdr = struct.pack("<IIII", 0, width, height, len(meta_bytes))
    return resp_hdr + meta_bytes, binary_mask.tobytes()

def run_server(socket_path: str, models_dir: str):
    if os.path.exists(socket_path):
        try:
            os.unlink(socket_path)
        except OSError:
            pass

    manager = ModelManager(models_dir)
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(socket_path)
    server.listen(5)
    print(f"[AI Worker] Listening on Unix socket: {socket_path}")

    try:
        while True:
            conn, _ = server.accept()
            with conn:
                while True:
                    hdr = conn.recv(4)
                    if not hdr or len(hdr) < 4:
                        break
                    try:
                        resp_hdr_meta, mask_bytes = handle_request(manager, hdr, conn)
                        conn.sendall(resp_hdr_meta)
                        if mask_bytes:
                            conn.sendall(mask_bytes)
                    except Exception as e:
                        print(f"[AI Worker] Error processing request: {e}")
                        err_meta = json.dumps({"error": str(e)}).encode("utf-8")
                        err_hdr = struct.pack("<IIII", 1, 0, 0, len(err_meta))
                        conn.sendall(err_hdr + err_meta)
                        break
    finally:
        if os.path.exists(socket_path):
            try:
                os.unlink(socket_path)
            except OSError:
                pass

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Nephclick PhotoLab AI Selection Worker")
    parser.add_argument("--socket", default="/tmp/nephclick_ai_selection.sock", help="Path to Unix socket")
    parser.add_argument("--models-dir", default=os.path.join(SCRIPT_DIR, "../models/ai_selection"), help="Models folder")
    args = parser.parse_args()

    os.makedirs(args.models_dir, exist_ok=True)
    run_server(args.socket, args.models_dir)
