#!/usr/bin/env python3
"""
Detector probe for mImageViewer auto-mask planning.

This is a development-only tool. It downloads candidate detector models, runs
all selected models against images in a folder, draws detections onto output
images, and writes JSONL/CSV reports.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable

import cv2
import numpy as np

try:
    import onnxruntime as ort
except Exception as exc:  # pragma: no cover - reported in main
    ort = None
    ORT_IMPORT_ERROR = exc
else:
    ORT_IMPORT_ERROR = None


IMAGE_EXTS = {".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff"}

NUDENET_LABELS = [
    "FEMALE_GENITALIA_COVERED",
    "FACE_FEMALE",
    "BUTTOCKS_EXPOSED",
    "FEMALE_BREAST_EXPOSED",
    "FEMALE_GENITALIA_EXPOSED",
    "MALE_BREAST_EXPOSED",
    "ANUS_EXPOSED",
    "FEET_EXPOSED",
    "BELLY_COVERED",
    "FEET_COVERED",
    "ARMPITS_COVERED",
    "ARMPITS_EXPOSED",
    "FACE_MALE",
    "BELLY_EXPOSED",
    "MALE_GENITALIA_EXPOSED",
    "ANUS_COVERED",
    "FEMALE_BREAST_COVERED",
    "BUTTOCKS_COVERED",
]

NUDENET_CORE_LABELS = {
    "FEMALE_GENITALIA_EXPOSED",
    "MALE_GENITALIA_EXPOSED",
    "ANUS_EXPOSED",
}


@dataclass(frozen=True)
class ModelSpec:
    id: str
    group: str
    display_name: str
    kind: str
    url: str
    filename: str
    input_size: int | None = None
    conf: float = 0.35
    iou: float = 0.45
    labels: list[str] = field(default_factory=list)
    include_labels: set[str] | None = None
    preproc: str = "letterbox"
    license_note: str = ""
    default: bool = False
    downloadable: bool = True
    min_bytes: int = 1024
    extra_files: dict[str, str] = field(default_factory=dict)


def opencv_media_url(name: str) -> str:
    base = "https://media.githubusercontent.com/media/opencv/opencv_zoo/main"
    return f"{base}/models/face_detection_yunet/{name}"


def hf_url(repo: str, path: str) -> str:
    return f"https://huggingface.co/{repo}/resolve/main/{path}"


CATALOG: list[ModelSpec] = [
    ModelSpec(
        id="face_yunet_2023mar",
        group="face",
        display_name="OpenCV YuNet 2023mar",
        kind="yunet",
        url=opencv_media_url("face_detection_yunet_2023mar.onnx"),
        filename="face_detection_yunet_2023mar.onnx",
        conf=0.60,
        iou=0.30,
        license_note="OpenCV Zoo face_detection_yunet: MIT",
    ),
    ModelSpec(
        id="face_yunet_2026may",
        group="face",
        display_name="OpenCV YuNet 2026may",
        kind="yunet",
        url=opencv_media_url("face_detection_yunet_2026may.onnx"),
        filename="face_detection_yunet_2026may.onnx",
        conf=0.60,
        iou=0.30,
        license_note="OpenCV Zoo face_detection_yunet: MIT",
        default=True,
    ),
    ModelSpec(
        id="face_yunet_2023mar_int8",
        group="face",
        display_name="OpenCV YuNet 2023mar int8",
        kind="yunet",
        url=opencv_media_url("face_detection_yunet_2023mar_int8.onnx"),
        filename="face_detection_yunet_2023mar_int8.onnx",
        conf=0.60,
        iou=0.30,
        license_note="OpenCV Zoo face_detection_yunet: MIT",
    ),
    ModelSpec(
        id="face_yunet_2023mar_int8bq",
        group="face",
        display_name="OpenCV YuNet 2023mar int8bq",
        kind="yunet",
        url=opencv_media_url("face_detection_yunet_2023mar_int8bq.onnx"),
        filename="face_detection_yunet_2023mar_int8bq.onnx",
        conf=0.60,
        iou=0.30,
        license_note="OpenCV Zoo face_detection_yunet: MIT",
    ),
    ModelSpec(
        id="deepghs_real_face_v0_n",
        group="face",
        display_name="DeepGHS real_face_detection v0_n",
        kind="yolo",
        url=hf_url("deepghs/real_face_detection", "face_detect_v0_n/model.onnx"),
        filename="deepghs_real_face_v0_n.onnx",
        input_size=640,
        labels=["face"],
        conf=0.35,
        iou=0.45,
        preproc="letterbox",
        license_note="Hugging Face model; verify model card/license before reuse",
        extra_files={
            "labels.json": hf_url("deepghs/real_face_detection", "face_detect_v0_n/labels.json"),
            "threshold.json": hf_url("deepghs/real_face_detection", "face_detect_v0_n/threshold.json"),
            "model_type.json": hf_url("deepghs/real_face_detection", "face_detect_v0_n/model_type.json"),
        },
    ),
    ModelSpec(
        id="deepghs_real_face_v0_n_yv11",
        group="face",
        display_name="DeepGHS real_face_detection v0_n_yv11",
        kind="yolo",
        url=hf_url("deepghs/real_face_detection", "face_detect_v0_n_yv11/model.onnx"),
        filename="deepghs_real_face_v0_n_yv11.onnx",
        input_size=640,
        labels=["face"],
        conf=0.35,
        iou=0.45,
        preproc="letterbox",
        license_note="Hugging Face model; verify model card/license before reuse",
    ),
    ModelSpec(
        id="deepghs_real_face_v0_n_yv12",
        group="face",
        display_name="DeepGHS real_face_detection v0_n_yv12",
        kind="yolo",
        url=hf_url("deepghs/real_face_detection", "face_detect_v0_n_yv12/model.onnx"),
        filename="deepghs_real_face_v0_n_yv12.onnx",
        input_size=640,
        labels=["face"],
        conf=0.35,
        iou=0.45,
        preproc="letterbox",
        license_note="Hugging Face model; verify model card/license before reuse",
    ),
    ModelSpec(
        id="deepghs_yolov8n_face",
        group="face",
        display_name="DeepGHS yolov8n-face",
        kind="yolo",
        url=hf_url("deepghs/yolo-face", "yolov8n-face/model.onnx"),
        filename="deepghs_yolov8n_face.onnx",
        input_size=640,
        labels=["face"],
        conf=0.35,
        iou=0.45,
        preproc="letterbox",
        license_note="Hugging Face model; verify model card/license before reuse",
    ),
    ModelSpec(
        id="nudenet_320n",
        group="sensitive",
        display_name="NudeNet 320n",
        kind="yolo",
        url="https://raw.githubusercontent.com/notAI-tech/NudeNet/v3/nudenet/320n.onnx",
        filename="nudenet_320n.onnx",
        input_size=320,
        labels=NUDENET_LABELS,
        include_labels=NUDENET_CORE_LABELS,
        conf=0.25,
        iou=0.45,
        preproc="square_pad",
        license_note="NudeNet README says model is based on Ultralytics YOLOv8n; verify license before reuse",
        default=True,
        min_bytes=1_000_000,
    ),
    ModelSpec(
        id="nudenet_640m",
        group="sensitive",
        display_name="NudeNet 640m",
        kind="yolo",
        url="https://github.com/notAI-tech/NudeNet/releases/download/v3.4-weights/640m.onnx",
        filename="nudenet_640m.onnx",
        input_size=640,
        labels=NUDENET_LABELS,
        include_labels=NUDENET_CORE_LABELS,
        conf=0.25,
        iou=0.45,
        preproc="square_pad",
        license_note="NudeNet README says model is based on Ultralytics YOLOv8m; GitHub release asset may require manual download",
        downloadable=False,
        min_bytes=20_000_000,
    ),
    ModelSpec(
        id="deepghs_nudenet_320n",
        group="sensitive",
        display_name="DeepGHS NudeNet ONNX 320n",
        kind="yolo",
        url=hf_url("deepghs/nudenet_onnx", "320n.onnx"),
        filename="deepghs_nudenet_320n.onnx",
        input_size=320,
        labels=NUDENET_LABELS,
        include_labels=NUDENET_CORE_LABELS,
        conf=0.25,
        iou=0.45,
        preproc="square_pad",
        license_note="Hugging Face conversion of NudeNet; verify upstream and model license before reuse",
        min_bytes=1_000_000,
    ),
]

CATALOG_BY_ID = {m.id: m for m in CATALOG}


@dataclass
class Detection:
    model_id: str
    label: str
    score: float
    box: tuple[float, float, float, float]  # x, y, w, h


class Detector:
    def __init__(self, spec: ModelSpec, model_path: Path, providers: list[str]):
        self.spec = spec
        self.model_path = model_path
        self.providers = providers

    def detect(self, image_bgr: np.ndarray) -> list[Detection]:
        raise NotImplementedError


class YuNetDetector(Detector):
    def __init__(self, spec: ModelSpec, model_path: Path, providers: list[str]):
        super().__init__(spec, model_path, providers)
        if not hasattr(cv2, "FaceDetectorYN_create") and not hasattr(cv2, "FaceDetectorYN"):
            raise RuntimeError("OpenCV FaceDetectorYN is not available in this cv2 build")
        self.detector = cv2.FaceDetectorYN.create(
            str(model_path),
            "",
            (320, 320),
            spec.conf,
            spec.iou,
            5000,
        )

    def detect(self, image_bgr: np.ndarray) -> list[Detection]:
        h, w = image_bgr.shape[:2]
        self.detector.setInputSize((w, h))
        _, faces = self.detector.detect(image_bgr)
        if faces is None:
            return []
        dets: list[Detection] = []
        for face in faces:
            x, y, bw, bh = [float(v) for v in face[:4]]
            score = float(face[-1])
            if score < self.spec.conf:
                continue
            dets.append(Detection(self.spec.id, "face", score, clamp_box(x, y, bw, bh, w, h)))
        return dets


class YoloDetector(Detector):
    def __init__(self, spec: ModelSpec, model_path: Path, providers: list[str]):
        super().__init__(spec, model_path, providers)
        if ort is None:
            raise RuntimeError(f"onnxruntime import failed: {ORT_IMPORT_ERROR!r}")
        sess_options = ort.SessionOptions()
        sess_options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        self.session = ort.InferenceSession(str(model_path), sess_options, providers=providers)
        self.input_name = self.session.get_inputs()[0].name
        self.input_size = spec.input_size or infer_input_size(self.session) or 640
        labels = spec.labels or load_sidecar_labels(model_path)
        self.labels = labels if labels else ["object"]

    def detect(self, image_bgr: np.ndarray) -> list[Detection]:
        h, w = image_bgr.shape[:2]
        input_tensor, transform = preprocess_yolo(image_bgr, self.input_size, self.spec.preproc)
        outputs = self.session.run(None, {self.input_name: input_tensor})
        candidates = decode_yolo_outputs(outputs, len(self.labels), self.spec.conf)
        dets = postprocess_yolo(
            candidates,
            self.labels,
            transform,
            (w, h),
            self.spec.iou,
            self.spec.include_labels,
            self.spec.id,
        )
        return dets


def infer_input_size(session) -> int | None:
    shape = session.get_inputs()[0].shape
    dims = [d for d in shape if isinstance(d, int)]
    if len(dims) >= 2:
        return int(max(dims[-2], dims[-1]))
    return None


def load_sidecar_labels(model_path: Path) -> list[str] | None:
    sidecar = model_path.with_name("labels.json")
    if not sidecar.exists():
        return None
    try:
        data = json.loads(sidecar.read_text(encoding="utf-8"))
    except Exception:
        return None
    if isinstance(data, list):
        return [str(x) for x in data]
    if isinstance(data, dict):
        if "labels" in data and isinstance(data["labels"], list):
            return [str(x) for x in data["labels"]]
        if all(str(k).isdigit() for k in data.keys()):
            return [str(data[str(i)]) for i in range(len(data))]
    return None


def preprocess_yolo(image_bgr: np.ndarray, size: int, mode: str):
    h, w = image_bgr.shape[:2]
    if mode == "square_pad":
        square = max(h, w)
        padded = np.zeros((square, square, 3), dtype=np.uint8)
        padded[:h, :w] = image_bgr
        resized = cv2.resize(padded, (size, size), interpolation=cv2.INTER_LINEAR)
        transform = {"mode": mode, "scale": size / square, "pad_x": 0.0, "pad_y": 0.0}
    else:
        scale = min(size / w, size / h)
        new_w = int(round(w * scale))
        new_h = int(round(h * scale))
        resized_inner = cv2.resize(image_bgr, (new_w, new_h), interpolation=cv2.INTER_LINEAR)
        resized = np.full((size, size, 3), 114, dtype=np.uint8)
        pad_x = (size - new_w) // 2
        pad_y = (size - new_h) // 2
        resized[pad_y : pad_y + new_h, pad_x : pad_x + new_w] = resized_inner
        transform = {"mode": mode, "scale": scale, "pad_x": float(pad_x), "pad_y": float(pad_y)}

    rgb = cv2.cvtColor(resized, cv2.COLOR_BGR2RGB)
    tensor = rgb.astype(np.float32) / 255.0
    tensor = np.transpose(tensor, (2, 0, 1))[None, :, :, :]
    return np.ascontiguousarray(tensor), transform


def decode_yolo_outputs(outputs: list[np.ndarray], class_count: int, conf_th: float):
    arr = outputs[0]
    arr = np.asarray(arr)
    arr = np.squeeze(arr)
    if arr.ndim != 2:
        raise RuntimeError(f"Unsupported YOLO output shape: {outputs[0].shape}")

    # Common Ultralytics export: [4 + C, anchors]. End-to-end exports often use [anchors, 6].
    if arr.shape[0] in (4 + class_count, 5 + class_count) and arr.shape[0] < arr.shape[1]:
        arr = arr.T

    candidates = []
    for row in arr:
        if row.shape[0] >= 6 and row.shape[0] not in (4 + class_count, 5 + class_count):
            # x1,y1,x2,y2,score,class style.
            x1, y1, x2, y2, score, cls = row[:6]
            score = float(score)
            if score < conf_th:
                continue
            candidates.append((float(x1), float(y1), float(x2), float(y2), int(cls), score, "xyxy"))
            continue

        if row.shape[0] == 4 + class_count:
            xywh = row[:4]
            scores = row[4:]
            cls = int(np.argmax(scores))
            score = float(scores[cls])
        elif row.shape[0] == 5 + class_count:
            xywh = row[:4]
            obj = float(row[4])
            scores = row[5:] * obj
            cls = int(np.argmax(scores))
            score = float(scores[cls])
        else:
            # Fallback: infer class scores after first 4 entries.
            xywh = row[:4]
            scores = row[4:]
            cls = int(np.argmax(scores))
            score = float(scores[cls])

        if score < conf_th:
            continue
        x, y, w, h = [float(v) for v in xywh]
        candidates.append((x, y, w, h, cls, score, "xywh"))
    return candidates


def postprocess_yolo(
    candidates,
    labels: list[str],
    transform: dict,
    image_size: tuple[int, int],
    iou_th: float,
    include_labels: set[str] | None,
    model_id: str,
) -> list[Detection]:
    img_w, img_h = image_size
    dets: list[Detection] = []
    boxes_xyxy = []
    scores = []
    decoded = []

    scale = transform["scale"]
    pad_x = transform["pad_x"]
    pad_y = transform["pad_y"]

    for item in candidates:
        a, b, c, d, cls, score, fmt = item
        label = labels[cls] if 0 <= cls < len(labels) else f"class_{cls}"
        if include_labels is not None and label not in include_labels:
            continue
        if fmt == "xyxy":
            x1, y1, x2, y2 = a, b, c, d
        else:
            x, y, bw, bh = a, b, c, d
            x1, y1 = x - bw / 2.0, y - bh / 2.0
            x2, y2 = x + bw / 2.0, y + bh / 2.0
        x1 = (x1 - pad_x) / scale
        x2 = (x2 - pad_x) / scale
        y1 = (y1 - pad_y) / scale
        y2 = (y2 - pad_y) / scale
        x1 = max(0.0, min(float(img_w), x1))
        x2 = max(0.0, min(float(img_w), x2))
        y1 = max(0.0, min(float(img_h), y1))
        y2 = max(0.0, min(float(img_h), y2))
        if x2 <= x1 or y2 <= y1:
            continue
        boxes_xyxy.append((x1, y1, x2, y2))
        scores.append(score)
        decoded.append((label, score, x1, y1, x2, y2))

    keep = nms(boxes_xyxy, scores, iou_th)
    for idx in keep:
        label, score, x1, y1, x2, y2 = decoded[idx]
        dets.append(Detection(model_id, label, score, (x1, y1, x2 - x1, y2 - y1)))
    return dets


def nms(boxes: list[tuple[float, float, float, float]], scores: list[float], iou_th: float) -> list[int]:
    if not boxes:
        return []
    order = np.argsort(np.asarray(scores))[::-1]
    keep: list[int] = []
    boxes_np = np.asarray(boxes, dtype=np.float32)
    areas = (boxes_np[:, 2] - boxes_np[:, 0]) * (boxes_np[:, 3] - boxes_np[:, 1])
    while order.size > 0:
        i = int(order[0])
        keep.append(i)
        if order.size == 1:
            break
        rest = order[1:]
        xx1 = np.maximum(boxes_np[i, 0], boxes_np[rest, 0])
        yy1 = np.maximum(boxes_np[i, 1], boxes_np[rest, 1])
        xx2 = np.minimum(boxes_np[i, 2], boxes_np[rest, 2])
        yy2 = np.minimum(boxes_np[i, 3], boxes_np[rest, 3])
        inter_w = np.maximum(0.0, xx2 - xx1)
        inter_h = np.maximum(0.0, yy2 - yy1)
        inter = inter_w * inter_h
        union = areas[i] + areas[rest] - inter
        iou = inter / np.maximum(union, 1e-6)
        order = rest[iou <= iou_th]
    return keep


def clamp_box(x: float, y: float, w: float, h: float, img_w: int, img_h: int):
    x1 = max(0.0, min(float(img_w), x))
    y1 = max(0.0, min(float(img_h), y))
    x2 = max(0.0, min(float(img_w), x + w))
    y2 = max(0.0, min(float(img_h), y + h))
    return x1, y1, max(0.0, x2 - x1), max(0.0, y2 - y1)


def default_model_dir() -> Path:
    return Path("dev") / "detector_probe" / "models"


def resolve_models(selector: str) -> list[ModelSpec]:
    if selector == "all":
        return [m for m in CATALOG if m.downloadable]
    if selector == "default":
        return [m for m in CATALOG if m.default]
    if selector in ("face", "sensitive"):
        return [m for m in CATALOG if m.group == selector]
    ids = [s.strip() for s in selector.split(",") if s.strip()]
    missing = [i for i in ids if i not in CATALOG_BY_ID]
    if missing:
        raise SystemExit(f"Unknown model id(s): {', '.join(missing)}")
    return [CATALOG_BY_ID[i] for i in ids]


def looks_like_html(path: Path) -> bool:
    try:
        head = path.read_bytes()[:256].lstrip().lower()
    except OSError:
        return False
    return head.startswith(b"<!doctype html") or head.startswith(b"<html")


def download_file(url: str, dst: Path, force: bool = False, min_bytes: int = 1024) -> None:
    if (
        dst.exists()
        and not force
        and dst.stat().st_size >= min_bytes
        and not looks_like_html(dst)
    ):
        return
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_suffix(dst.suffix + ".tmp")
    req = urllib.request.Request(url, headers={"User-Agent": "miv-detector-probe/1.0"})
    print(f"download: {url}")
    try:
        with urllib.request.urlopen(req, timeout=120) as resp, tmp.open("wb") as f:
            total = int(resp.headers.get("Content-Length") or 0)
            done = 0
            last_print = 0
            while True:
                chunk = resp.read(1024 * 1024)
                if not chunk:
                    break
                f.write(chunk)
                done += len(chunk)
                if total and done - last_print >= 16 * 1024 * 1024:
                    print(f"  {done / 1024 / 1024:.1f} / {total / 1024 / 1024:.1f} MiB")
                    last_print = done
    except urllib.error.URLError as exc:
        if tmp.exists():
            tmp.unlink()
        raise RuntimeError(f"download failed: {url}: {exc}") from exc
    if tmp.stat().st_size < min_bytes or looks_like_html(tmp):
        tmp.unlink(missing_ok=True)
        raise RuntimeError(
            f"downloaded file looks invalid: {dst.name} from {url} "
            f"(expected at least {min_bytes} bytes)"
        )
    tmp.replace(dst)


def download_models(models: list[ModelSpec], model_dir: Path, force: bool = False) -> None:
    for spec in models:
        model_path = model_dir / spec.filename
        download_file(spec.url, model_path, force, spec.min_bytes)
        for filename, url in spec.extra_files.items():
            download_file(url, model_path.with_name(filename), force, 1)


def load_detectors(models: list[ModelSpec], model_dir: Path, providers: list[str]) -> list[Detector]:
    detectors = []
    for spec in models:
        model_path = model_dir / spec.filename
        if not model_path.exists():
            raise SystemExit(
                f"Model file missing for {spec.id}: {model_path}\n"
                f"Run: python tools\\detector_probe\\detector_probe.py download --models {spec.id}"
            )
        print(f"load: {spec.id} ({spec.kind})")
        if spec.kind == "yunet":
            detectors.append(YuNetDetector(spec, model_path, providers))
        elif spec.kind == "yolo":
            detectors.append(YoloDetector(spec, model_path, providers))
        else:
            raise SystemExit(f"Unsupported model kind: {spec.kind}")
    return detectors


def iter_images(input_dir: Path, recursive: bool) -> Iterable[Path]:
    if recursive:
        paths = input_dir.rglob("*")
    else:
        paths = input_dir.iterdir()
    for p in paths:
        if p.is_file() and p.suffix.lower() in IMAGE_EXTS:
            yield p


def read_image(path: Path) -> np.ndarray | None:
    data = np.fromfile(str(path), dtype=np.uint8)
    if data.size == 0:
        return None
    img = cv2.imdecode(data, cv2.IMREAD_UNCHANGED)
    if img is None:
        return None
    if img.ndim == 2:
        img = cv2.cvtColor(img, cv2.COLOR_GRAY2BGR)
    elif img.shape[2] == 4:
        img = cv2.cvtColor(img, cv2.COLOR_BGRA2BGR)
    return img


def write_image(path: Path, image_bgr: np.ndarray) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    ext = path.suffix.lower()
    if ext == ".jpg":
        ext = ".jpeg"
    ok, encoded = cv2.imencode(ext, image_bgr)
    if not ok:
        raise RuntimeError(f"cv2.imencode failed for {path}")
    encoded.tofile(str(path))


def color_for_model(model_id: str) -> tuple[int, int, int]:
    # BGR color, deterministic and readable.
    seed = sum((i + 1) * ord(c) for i, c in enumerate(model_id))
    hue = seed % 180
    color = np.uint8([[[hue, 190, 245]]])
    bgr = cv2.cvtColor(color, cv2.COLOR_HSV2BGR)[0, 0]
    return int(bgr[0]), int(bgr[1]), int(bgr[2])


def draw_detections(image_bgr: np.ndarray, detections: list[Detection]) -> np.ndarray:
    out = image_bgr.copy()
    h, w = out.shape[:2]
    thickness = max(2, int(round(math.sqrt(w * h) / 500)))
    font_scale = max(0.45, min(1.2, math.sqrt(w * h) / 1600))
    for det in detections:
        x, y, bw, bh = det.box
        x1, y1, x2, y2 = int(round(x)), int(round(y)), int(round(x + bw)), int(round(y + bh))
        color = color_for_model(det.model_id)
        cv2.rectangle(out, (x1, y1), (x2, y2), color, thickness)
        text = f"{det.model_id}:{det.label} {det.score:.2f}"
        (tw, th), base = cv2.getTextSize(text, cv2.FONT_HERSHEY_SIMPLEX, font_scale, thickness)
        tx = max(0, min(x1, w - tw - 4))
        ty = max(th + 4, y1 - 4)
        cv2.rectangle(out, (tx, ty - th - base - 4), (tx + tw + 4, ty + base), (0, 0, 0), -1)
        cv2.putText(out, text, (tx + 2, ty - 2), cv2.FONT_HERSHEY_SIMPLEX, font_scale, color, thickness, cv2.LINE_AA)
    return out


def detection_to_json(det: Detection) -> dict:
    x, y, w, h = det.box
    return {
        "model": det.model_id,
        "label": det.label,
        "score": round(float(det.score), 6),
        "box": [round(float(x), 3), round(float(y), 3), round(float(w), 3), round(float(h), 3)],
    }


def run_probe(args) -> None:
    input_dir = Path(args.input)
    output_dir = Path(args.output)
    model_dir = Path(args.model_dir)
    models = resolve_models(args.models)
    if args.include_all_labels:
        models = [
            ModelSpec(
                **{
                    **m.__dict__,
                    "include_labels": None,
                }
            )
            for m in models
        ]
    if args.download:
        download_models(models, model_dir, args.force_download)

    providers = ["CPUExecutionProvider"]
    detectors = load_detectors(models, model_dir, providers)
    image_paths = list(iter_images(input_dir, args.recursive))
    if not image_paths:
        raise SystemExit(f"No images found: {input_dir}")

    output_dir.mkdir(parents=True, exist_ok=True)
    jsonl_path = output_dir / "detections.jsonl"
    csv_path = output_dir / "summary.csv"
    stats = {d.spec.id: {"images": 0, "detections": 0, "ms": 0.0, "errors": 0} for d in detectors}

    with jsonl_path.open("w", encoding="utf-8") as jf:
        for idx, path in enumerate(image_paths, start=1):
            rel = path.relative_to(input_dir)
            print(f"[{idx}/{len(image_paths)}] {rel}")
            image = read_image(path)
            if image is None:
                jf.write(json.dumps({"path": str(rel), "error": "decode failed"}, ensure_ascii=False) + "\n")
                continue

            all_dets: list[Detection] = []
            per_model = []
            for detector in detectors:
                t0 = time.perf_counter()
                try:
                    dets = detector.detect(image)
                    elapsed_ms = (time.perf_counter() - t0) * 1000.0
                    stats[detector.spec.id]["images"] += 1
                    stats[detector.spec.id]["detections"] += len(dets)
                    stats[detector.spec.id]["ms"] += elapsed_ms
                    all_dets.extend(dets)
                    per_model.append(
                        {
                            "model": detector.spec.id,
                            "elapsed_ms": round(elapsed_ms, 3),
                            "detections": [detection_to_json(d) for d in dets],
                        }
                    )
                    print(f"  {detector.spec.id}: {len(dets)} dets, {elapsed_ms:.1f} ms")
                except Exception as exc:
                    elapsed_ms = (time.perf_counter() - t0) * 1000.0
                    stats[detector.spec.id]["errors"] += 1
                    per_model.append(
                        {
                            "model": detector.spec.id,
                            "elapsed_ms": round(elapsed_ms, 3),
                            "error": repr(exc),
                            "detections": [],
                        }
                    )
                    print(f"  {detector.spec.id}: ERROR {exc!r}")

            annotated = draw_detections(image, all_dets)
            out_path = output_dir / rel
            write_image(out_path, annotated)
            jf.write(
                json.dumps(
                    {
                        "path": str(rel).replace("\\", "/"),
                        "width": int(image.shape[1]),
                        "height": int(image.shape[0]),
                        "output": str(out_path),
                        "models": per_model,
                    },
                    ensure_ascii=False,
                )
                + "\n"
            )

    with csv_path.open("w", newline="", encoding="utf-8-sig") as cf:
        writer = csv.writer(cf)
        writer.writerow(["model", "images", "detections", "errors", "avg_ms"])
        for model_id, s in stats.items():
            avg_ms = s["ms"] / s["images"] if s["images"] else 0.0
            writer.writerow([model_id, s["images"], s["detections"], s["errors"], f"{avg_ms:.3f}"])

    print(f"done: {output_dir}")
    print(f"report: {jsonl_path}")
    print(f"summary: {csv_path}")


def print_model_list() -> None:
    for spec in CATALOG:
        mark = "*" if spec.default else ("!" if not spec.downloadable else " ")
        labels = "all"
        if spec.include_labels:
            labels = ",".join(sorted(spec.include_labels))
        print(f"{mark} {spec.id:28} {spec.group:9} {spec.kind:5} {spec.display_name}")
        print(f"    file: {spec.filename}")
        print(f"    labels: {labels}")
        if spec.license_note:
            print(f"    note: {spec.license_note}")
    print("\n* = default, ! = manual download / explicit selection only")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run detector models and draw detections onto images.")
    parser.add_argument("--model-dir", default=str(default_model_dir()), help="Model cache directory.")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("list-models", help="List built-in model catalog.")

    dl = sub.add_parser("download", help="Download selected models.")
    dl.add_argument("--models", default="default", help="default, all, face, sensitive, or comma-separated ids.")
    dl.add_argument("--force", action="store_true", help="Re-download existing files.")

    run = sub.add_parser("run", help="Run selected models against an image folder.")
    run.add_argument("--input", required=True, help="Input image folder.")
    run.add_argument("--output", required=True, help="Output folder.")
    run.add_argument("--models", default="default", help="default, all, face, sensitive, or comma-separated ids.")
    run.add_argument("--recursive", action="store_true", help="Process subfolders.")
    run.add_argument("--download", action="store_true", help="Download missing models before running.")
    run.add_argument("--force-download", action="store_true", help="Re-download models even if present.")
    run.add_argument(
        "--include-all-labels",
        action="store_true",
        help="For NudeNet-like models, draw all labels instead of only core genital/anus labels.",
    )
    return parser


def main(argv: list[str]) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command == "list-models":
        print_model_list()
        return 0
    if args.command == "download":
        download_models(resolve_models(args.models), Path(args.model_dir), args.force)
        return 0
    if args.command == "run":
        run_probe(args)
        return 0
    parser.error("unknown command")
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
