#!/usr/bin/env python3
"""Nephclick PhotoLab's Prompt & Click selection worker.

Runs SAM 2.1 Tiny (click and rectangle prompts) and Grounding DINO Tiny (text prompts) offline, from the weights the
app's installer fetched into HF_HOME. The app starts it when a prompt or click selection runs, talks to it over a
socket on 127.0.0.1, and it quits on its own after NEPHCLICK_AI_IDLE seconds without a request, so it holds no
memory while unused.

Protocol (little-endian):
  request:  u32 JSON length, JSON {"token", "cmd": "predict"|"ping"|"quit", "width", "height", "prompt_type":
            "point"|"points"|"box"|"text", "points": [[x, y, label], ...], "box": [x0, y0, x1, y1], "text",
            "threshold"}, then width*height*4 RGBA bytes for "predict".
  reply:    u32 status (0 ok, 1 error), u32 mask width, u32 mask height, u32 metadata length, metadata (JSON or
            the error text), then width*height mask bytes (0..255) when the status is 0.
"""

import gc
import json
import os
import socket
import struct
import sys

SAM_MODEL = "facebook/sam2.1-hiera-tiny"
DINO_MODEL = "IDEA-Research/grounding-dino-tiny"
IDLE = max(30, int(os.environ.get("NEPHCLICK_AI_IDLE", "180")))
TOKEN = os.environ.get("NEPHCLICK_AI_TOKEN", "")
MAX_SIDE = 8192
MAX_JSON = 1 << 20
MAX_BOXES = 8


def log(text):
    sys.stderr.write("[AI worker] " + text + "\n")
    sys.stderr.flush()


import numpy as np  # noqa: E402
import torch  # noqa: E402
from PIL import Image  # noqa: E402

if torch.backends.mps.is_available():
    DEVICE = "mps"
elif torch.cuda.is_available():
    DEVICE = "cuda"
else:
    DEVICE = "cpu"
# Leave a core for the app.
try:
    torch.set_num_threads(max(1, (os.cpu_count() or 2) - 1))
except Exception:  # pragma: no cover - older torch
    pass


class Models:
    def __init__(self):
        self.sam = None
        self.sam_processor = None
        self.dino = None
        self.dino_processor = None

    def get_sam(self):
        if self.sam is None:
            from transformers import Sam2Model, Sam2Processor

            log("loading SAM 2.1 Tiny on " + DEVICE)
            self.sam_processor = Sam2Processor.from_pretrained(SAM_MODEL)
            self.sam = Sam2Model.from_pretrained(SAM_MODEL).to(DEVICE).eval()
        return self.sam_processor, self.sam

    def get_dino(self):
        if self.dino is None:
            from transformers import AutoModelForZeroShotObjectDetection, AutoProcessor

            log("loading Grounding DINO Tiny on " + DEVICE)
            self.dino_processor = AutoProcessor.from_pretrained(DINO_MODEL)
            self.dino = AutoModelForZeroShotObjectDetection.from_pretrained(DINO_MODEL).to(DEVICE).eval()
        return self.dino_processor, self.dino

    def drop_dino(self):
        self.dino = None
        self.dino_processor = None
        gc.collect()
        if DEVICE == "mps":
            torch.mps.empty_cache()


def read_exact(conn, n):
    data = bytearray()
    while len(data) < n:
        chunk = conn.recv(min(n - len(data), 1 << 20))
        if not chunk:
            raise ConnectionError("the connection closed early")
        data.extend(chunk)
    return bytes(data)


def reply(conn, status, width=0, height=0, meta=b"", mask=b""):
    conn.sendall(struct.pack("<IIII", status, width, height, len(meta)) + meta + mask)


def sam_mask(models, image, points=None, labels=None, box=None):
    """The best of SAM's three masks for one object, as uint8 0/255 at the picture's size."""
    processor, model = models.get_sam()
    kwargs = {"images": image, "return_tensors": "pt"}
    if points:
        kwargs["input_points"] = [[[[float(x), float(y)] for (x, y) in points]]]
        kwargs["input_labels"] = [[[int(label) for label in labels]]]
    if box is not None:
        kwargs["input_boxes"] = [[[float(v) for v in box]]]
    inputs = processor(**kwargs).to(DEVICE)
    with torch.no_grad():
        outputs = model(**inputs, multimask_output=True)
    masks = processor.post_process_masks(outputs.pred_masks.cpu(), inputs["original_sizes"])[0]
    scores = outputs.iou_scores.detach().float().cpu()
    best = int(scores.reshape(-1, scores.shape[-1])[0].argmax())
    chosen = masks.reshape(-1, masks.shape[-3], masks.shape[-2], masks.shape[-1])[0, best]
    return chosen.numpy().astype(np.uint8) * 255


def text_boxes(models, image, text, threshold):
    processor, model = models.get_dino()
    query = text.strip().lower()
    if not query.endswith("."):
        query += "."
    inputs = processor(images=image, text=query, return_tensors="pt").to(DEVICE)
    with torch.no_grad():
        outputs = model(**inputs)
    sizes = [(image.height, image.width)]
    try:
        found = processor.post_process_grounded_object_detection(
            outputs, inputs.input_ids, threshold=threshold, text_threshold=0.25, target_sizes=sizes
        )[0]
    except TypeError:
        found = processor.post_process_grounded_object_detection(
            outputs, inputs.input_ids, box_threshold=threshold, text_threshold=0.25, target_sizes=sizes
        )[0]
    boxes = found["boxes"].detach().float().cpu().numpy().tolist()
    scores = found["scores"].detach().float().cpu().numpy().tolist()
    order = sorted(range(len(boxes)), key=lambda i: -scores[i])[:MAX_BOXES]
    return [boxes[i] for i in order]


def predict(models, req, rgba, width, height):
    image = Image.frombuffer("RGBA", (width, height), rgba, "raw", "RGBA", 0, 1).convert("RGB")
    kind = req.get("prompt_type", "box")
    meta = {"device": DEVICE, "type": kind}
    if kind in ("point", "points"):
        triples = req.get("points") or []
        if not triples:
            raise ValueError("a click selection needs a point")
        points = [(p[0], p[1]) for p in triples]
        labels = [1 if (len(p) < 3 or p[2]) else 0 for p in triples]
        mask = sam_mask(models, image, points=points, labels=labels)
    elif kind == "box":
        box = req.get("box") or [0, 0, width, height]
        mask = sam_mask(models, image, box=box)
    elif kind == "text":
        text = str(req.get("text") or "").strip()
        if not text:
            raise ValueError("a text selection needs words")
        boxes = text_boxes(models, image, text, float(req.get("threshold", 0.3)))
        meta["boxes"] = boxes
        mask = np.zeros((height, width), dtype=np.uint8)
        for box in boxes:
            mask = np.maximum(mask, sam_mask(models, image, box=box))
        # Grounding DINO is the larger model: let it go between text prompts on small machines.
        models.drop_dino()
    else:
        raise ValueError("unknown prompt type " + kind)
    mask = np.ascontiguousarray(mask, dtype=np.uint8)
    return json.dumps(meta).encode("utf-8"), mask.shape[1], mask.shape[0], mask.tobytes()


def handle(models, conn):
    (length,) = struct.unpack("<I", read_exact(conn, 4))
    if length > MAX_JSON:
        raise ValueError("request too large")
    req = json.loads(read_exact(conn, length).decode("utf-8"))
    if TOKEN and req.get("token") != TOKEN:
        reply(conn, 1, meta=b"not authorised")
        return True
    cmd = req.get("cmd", "predict")
    if cmd == "ping":
        reply(conn, 0, meta=json.dumps({"device": DEVICE}).encode("utf-8"))
        return True
    if cmd == "quit":
        reply(conn, 0)
        return False
    width, height = int(req.get("width", 0)), int(req.get("height", 0))
    if not (0 < width <= MAX_SIDE and 0 < height <= MAX_SIDE):
        reply(conn, 1, meta=b"image size out of range")
        return True
    rgba = read_exact(conn, width * height * 4)
    try:
        meta, mw, mh, mask = predict(models, req, rgba, width, height)
        reply(conn, 0, mw, mh, meta, mask)
    except Exception as error:  # report, keep serving
        log("request failed: %r" % (error,))
        reply(conn, 1, meta=str(error).encode("utf-8", "replace")[:2000])
    return True


def main():
    models = Models()
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.bind(("127.0.0.1", 0))
    server.listen(4)
    server.settimeout(IDLE)
    print("PORT %d" % server.getsockname()[1], flush=True)
    running = True
    while running:
        try:
            conn, _ = server.accept()
        except socket.timeout:
            log("idle, quitting")
            break
        with conn:
            conn.settimeout(600)
            try:
                running = handle(models, conn)
            except Exception as error:
                log("bad request: %r" % (error,))
    server.close()


if __name__ == "__main__":
    main()
