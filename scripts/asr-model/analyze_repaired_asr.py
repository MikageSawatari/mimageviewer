"""最終ASR計測とT0を比較する。本文を表示せず、捨てたbyte位置・窓・メモリを記録する。"""
import argparse
import json
from pathlib import Path
import wave


def repair_retained(segments):
    """独立した参照decoder。originは元CLI区間番号と本文中のbyte offset。"""
    pending, origins, traces, repaired = b"", [], [], []

    def discarded(data, locations, reason):
        traces.append({"hex": data.hex(), "bytes": len(data), "reason": reason,
                       "origins": [{"raw_segment": segment, "byte_offset": offset}
                                   for segment, offset in locations]})

    for index, segment in segments:
        raw = segment["text"].encode("utf-8", errors="surrogateescape")
        data = pending + raw
        locations = origins + [(index, i) for i in range(len(raw))]
        pending, origins, valid = b"", [], bytearray()
        i = 0
        while i < len(data):
            lead = data[i]
            n = (1 if lead < 0x80 else 2 if 0xc2 <= lead <= 0xdf else
                 3 if 0xe0 <= lead <= 0xef else 4 if 0xf0 <= lead <= 0xf4 else 0)
            if not n:
                discarded(data[i:i+1], locations[i:i+1], "invalid_lead")
                i += 1
                continue
            j = 1
            while j < n and i + j < len(data):
                b = data[i+j]
                if not 0x80 <= b <= 0xbf or (j == 1 and (
                    (lead == 0xe0 and b < 0xa0) or (lead == 0xed and b >= 0xa0) or
                    (lead == 0xf0 and b < 0x90) or (lead == 0xf4 and b >= 0x90))):
                    break
                j += 1
            if j < n and i + j < len(data):
                discarded(data[i:i+j], locations[i:i+j], "mismatch")
                i += j  # 不一致を起こした後続byteは必ず再評価する。
            elif i + n > len(data):
                pending, origins = data[i:], locations[i:]
                break
            else:
                valid.extend(data[i:i+n])
                i += n
        text = valid.decode("utf-8", errors="strict").strip()
        if text:
            repaired.append({**segment, "text": text})
    if pending:
        discarded(pending, origins, "window_tail")
    return repaired, traces


def normalized(segments):
    return [(round(s["start"] * 1000), round(s["end"] * 1000), s["text"]) for s in segments]


def reference_windows(work, name, duration_ms=None):
    root = work / "runs/t0" / ("uv-G" if name == "G" else "m6") / "follow"
    report = json.loads((root / "transcript.json").read_text(encoding="utf-8"))
    windows, joined = [], []
    for index, run in enumerate(report["runs"]):
        # replacementを使わず原byteを回収する。JSON構文は壊れておらず本文だけが不正。
        raw = json.loads((root / f"w{index}/whisper.json").read_bytes().decode("utf-8", errors="surrogateescape"))
        offset = round(run["read"][0] * 1000)
        read_end = round(run["read"][1] * 1000)
        seam = read_end if run["window"][1] is None else round(run["window"][1] * 1000)
        t0_read_end = read_end
        # T0の秒表記は小数点以下2桁へ丸められている。最終境界だけは
        # workerと同じ実WAVのsample数由来のfloor値で比較し、丸め差は残す。
        if run["window"][1] is None and duration_ms is not None:
            read_end = seam = duration_ms
        retained = []
        for raw_index, s in enumerate(raw["transcription"]):
            start, end = offset + s["offsets"]["from"], offset + s["offsets"]["to"]
            if run["window"][1] is None or start < seam:
                retained.append((raw_index, {"start":start/1000,"end":end/1000,"text":s["text"]}))
        repaired, traces = repair_retained(retained)
        for trace in traces:
            for origin in trace["origins"]:
                s = raw["transcription"][origin["raw_segment"]]
                origin["start_ms"] = offset + s["offsets"]["from"]
                origin["end_ms"] = offset + s["offsets"]["to"]
        windows.append({"start_ms":offset,"read_end_ms":read_end,"join_ms":seam,
                        "segment_count":len(repaired),"utf8_discarded_bytes":sum(t["bytes"] for t in traces),
                        "utf8_discarded_fragments":len(traces),"discard_trace":traces})
        if t0_read_end != read_end:
            windows[-1]["t0_rounded_read_end_ms"] = t0_read_end
            windows[-1]["t0_rounding_delta_ms"] = t0_read_end - read_end
        joined.extend(repaired)
    return report, windows, joined


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", required=True, type=Path)
    parser.add_argument("--evaluation-work", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    run = json.loads(args.run.read_text(encoding="utf-8"))
    results = []
    for f in run["files"]:
        parent = Path(f["input"]).parent.name
        name = "G" if Path(f["input"]).stem == "G" else "m6" if parent in ("m6", "m6_long") else parent
        if name in ("m6", "G"):
            with wave.open(f["input"], "rb") as wav:
                if wav.getframerate() != 16000:
                    raise ValueError("Expected evaluation WAV at 16kHz")
                duration_ms = wav.getnframes() // 16
            old, expected_windows, repaired = reference_windows(args.evaluation_work, name, duration_ms)
        else:
            old = json.loads((args.evaluation_work / "runs/turbo-q5_0-vulkan-mc0" / parent / "transcript.json").read_text(encoding="utf-8"))
            repaired, expected_windows = old["segments"], None
        actual, previous, expected = normalized(f["segments"]), normalized(old["segments"]), normalized(repaired)
        result = {"material":name,"segments":len(actual),"old_segments":len(previous),
                  "exact_old_match":actual == previous,"exact_repaired_reference_match":actual == expected,
                  "different_positions_old":sum(a!=b for a,b in zip(actual,previous))+abs(len(actual)-len(previous)),
                  "wall_s":f["wall_s"],"audio_seconds":f["audio_seconds"],
                  "realtime_ratio":f["wall_s"]/f["audio_seconds"],"windows":f["windows"],
                  "end":f["end"]}
        if expected_windows is not None:
            keys = ("start_ms","read_end_ms","join_ms","segment_count","utf8_discarded_bytes","utf8_discarded_fragments")
            result["window_reference_match"] = len(f["windows"]) == len(expected_windows) and all(
                all(actual_w[key] == expected_w[key] for key in keys)
                for actual_w, expected_w in zip(f["windows"],expected_windows))
            result["reference_windows"] = expected_windows
        results.append(result)
        print(f"{name}: segments={len(actual)}/{len(previous)} old={actual==previous} repaired={actual==expected} "
              f"windows={len(f['windows'])} discard={f['end']['utf8_discarded_bytes']}B/{f['end']['utf8_discarded_fragments']}")
    with args.out.open("x", encoding="utf-8") as output:
        json.dump({"results":results,"hello":run["hello"],"load":run["load"],
                   "process_peaks":run["process_peaks"],"memory_samples":len(run["memory_samples"])},
                  output,ensure_ascii=False,indent=2)
        output.write("\n")
    return 0 if all(r["exact_repaired_reference_match"] and r.get("window_reference_match",True) for r in results) else 2


if __name__ == "__main__":
    raise SystemExit(main())
