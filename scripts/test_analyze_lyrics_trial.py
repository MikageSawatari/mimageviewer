import importlib.util
from pathlib import Path
import unittest
import json
import subprocess
import sys
import uuid

spec = importlib.util.spec_from_file_location("analysis", Path(__file__).with_name("analyze-lyrics-trial.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class LyricsTrialAnalysisTests(unittest.TestCase):
    def row(self, **changes):
        result = dict(cat="effetune", kind="lyrics_position", t=1, media_generation=1, available=True,
                      playing=True, lead_coherent=True, lead_seq=2, lead_ms=5,
                      lead_pcm_serial=3, lead_clock_serial_before=3, lead_clock_serial_after=3, seek_serial=3)
        result.update(changes)
        return result

    def test_exclusions_including_seek_after_pcm_admission(self):
        rows = [self.row(), self.row(), self.row(lead_seq=4, playing=False),
                self.row(lead_seq=6, lead_pcm_serial=2),  # seek after pump serial check
                self.row(lead_seq=8, lead_clock_serial_after=4),
                self.row(lead_seq=10, seek_serial=4), self.row(lead_coherent=False),
                self.row(available=False), self.row(lead_status="not_sampled", lead_coherent=False)]
        report = module.analyze(rows)["未分類"]
        self.assertEqual(report["accepted"], 1)
        self.assertEqual(report["excluded"], {"連番が進んでいない": 1, "一時停止・停止": 1,
                         "採取中のserial不一致": 2, "旧いseek serial": 1,
                         "読み取り中の更新": 1, "未採取": 1, "位置なし": 1})

    def test_generation_scene_and_percentiles(self):
        rows = [self.row(lead_ms=-5), self.row(lead_seq=4, lead_ms=15),
                self.row(t=2, media_generation=2)]
        report = module.analyze(rows, [{"name":"通常再生", "start":0, "end":2}])
        self.assertEqual(report["通常再生"]["lead_ms"]["median"], 5)
        self.assertEqual(report["通常再生"]["accepted"], 2)
        self.assertEqual(report["未分類"]["accepted"], 1)

    def test_empty_is_not_a_measurement(self):
        self.assertEqual(module.analyze([]), {})

    def test_cli_jsonl_and_scenes_and_empty_exit(self):
        root = Path(__file__).resolve().parents[1] / "target" / "lyrics-trial"
        root.mkdir(parents=True, exist_ok=True)
        directory = root / ("analysis-cli-" + uuid.uuid4().hex)
        directory.mkdir()
        perf = directory / "perf.jsonl"
        scenes = directory / "scenes.json"
        perf.write_text(json.dumps(self.row()) + "\n", encoding="utf-8")
        scenes.write_text(json.dumps([{"name":"seek", "start":0, "end":2}]), encoding="utf-8")
        command = [sys.executable, str(Path(module.__file__)), str(perf), "--scenes", str(scenes)]
        result = subprocess.run(command, capture_output=True, check=True)
        self.assertEqual(json.loads(result.stdout)["seek"]["accepted"], 1)
        perf.write_text("", encoding="utf-8")
        result = subprocess.run(command, capture_output=True)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(json.loads(result.stdout), {})


if __name__ == "__main__":
    unittest.main()
