"""Generate synthetic and public-evaluation fixtures using unmodified Python.

Run at repo root: python src/ai/transcript_postprocess/generate_fixtures.py
No private transcripts are read or copied.
"""
import json
import hashlib
from pathlib import Path
import subprocess
import sys
import types

EVAL = Path(r"G:\home\mimageviewer-asr-eval")

def source(name):
    return subprocess.check_output(
        ["git", "-C", str(EVAL), "show", f"exp-asr-eval:scripts/asr-eval/{name}.py"]
    ).decode("utf-8")

common = types.ModuleType("asr_common")
exec(source("asr_common"), common.__dict__)
sys.modules["asr_common"] = common
post = types.ModuleType("postprocess")
exec(source("postprocess"), post.__dict__)

cases = [
    ("stock-boundaries", [
        (0, 999, "ご視聴ありがとうございました"),
        (2000, 3000, "ご視聴ありがとうございました"),
        (64000, 68999, "最後までご清聴いただき本当にありがとうございました。"),
        (70000, 75000, "最後までご清聴いただき本当にありがとうございました。"),
        (80000, 86000, "チャンネル登録をよろしくお願いいたします"),
        (87000, 93000, "チャンネル登録をお願いしますと伝えた"),
    ]),
    ("history-and-normalization", [
        (i * 2000, i * 2000 + 1500, text) for i, text in enumerate([
            "ＡＢＣ！？", "abc", "別の文", "ＡＢＣ", "別の文。", "abc", "abc", "abc",
            " \n!🎵\u200b", "ΟΔΟΣ", "οδος", "かなかなかなかなかなかな", "かなかなかな",
        ])
    ]),
    ("seconds-float-boundaries", [
        (1, 1001, "ご視聴ありがとうございました"),
        (4001, 5001, "境界の文"),
        (64001, 65001, "境界の文"),
    ]),
    ("history-window", [
        (0, 1000, "one"), (60000, 61000, "one"), (120001, 121001, "one"),
        (122000, 123000, "a"), (124000, 125000, "b"), (126000, 127000, "c"),
        (128000, 129000, "d"), (130000, 131000, "e"), (132000, 133000, "one"),
    ]),
    ("loops", [(i * 3000, i * 3000 + 2000, text) for i, text in enumerate([
        "前ああああああ後", "12345678" * 6, "123456789" * 6,
        "ab" * 4, "ab" * 5, "x\n" * 6, "あ" * 12 + "い" * 8,
        "🎵" * 6 + "本文", "短い文", "\u001c\u001d\u001e\u001f",
    ])]),
    ("stock-does-not-enter-history", [
        (0, 6000, "ご視聴ありがとうございました"),
        (7000, 9500, "ご視聴ありがとうございました"),
        (10000, 12500, "ご視聴ありがとうございました"),
    ]),
]
fixture = {"origin": "exp-asr-eval:scripts/asr-eval/postprocess.py clean(segments, None)", "cases": []}
for name, segments in cases:
    source_segments = [{"start": s / 1000, "end": e / 1000, "text": t} for s, e, t in segments]
    result = post.clean(source_segments, None)
    fixture["cases"].append({
        "name": name, "input": segments,
        "kept": [(round(s["start"] * 1000), round(s["end"] * 1000), s["text"]) for s in result["segments"]],
        "removed_counts": {rule: sum(s["rule"] == rule for s in result["removed"]) for rule in ("stock", "repeat")},
    })
fixture["loops"] = [(text, post.LOOP.sub(lambda m: m.group(1) * 3, text)) for _, segs in cases for _, _, text in segs]
Path(__file__).with_name("fixtures.json").write_text(json.dumps(fixture, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

# Public evaluation inputs only. Never enumerate the private user-videos tree.
evaluation = {"origin": fixture["origin"], "cases": [], "loops": [], "sources": []}
run = EVAL / "scripts/asr-eval/work/runs/turbo-q5_0-vulkan-mc0"
for material in ("m1_explainer", "m3_fleurs_clean", "m5_video"):
    path = run / material / "transcript.json"
    data = path.read_bytes()
    segments = json.loads(data)["segments"]
    # The transport uses integer ms. These saved outputs already have ms precision.
    def ms_rows(rows):
        result = []
        for seg in rows:
            start, end = round(seg["start"] * 1000), round(seg["end"] * 1000)
            assert abs(start / 1000 - seg["start"]) < 1e-9
            assert abs(end / 1000 - seg["end"]) < 1e-9
            result.append((start, end, seg["text"]))
        return result
    result = post.clean(segments, None)
    evaluation["cases"].append({
        "name": material,
        "input": ms_rows(segments),
        "kept": ms_rows(result["segments"]),
        "removed_counts": {rule: sum(seg["rule"] == rule for seg in result["removed"]) for rule in ("stock", "repeat")},
    })
    evaluation["sources"].append({
        "material": material,
        "path": str(path.relative_to(EVAL)).replace("\\", "/"),
        "sha256": hashlib.sha256(data).hexdigest(),
        "license": "CC BY 4.0 (Google FLEURS ja_jp test; Wikipedia-derived sentences)" if material == "m3_fleurs_clean" else "Test script written for this evaluation; no third-party rights in the text",
    })
Path(__file__).with_name("evaluation_fixtures.json").write_text(json.dumps(evaluation, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
