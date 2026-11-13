"""ASR計測toolのduplex送受信と失敗時の部分結果保持。実worker・モデルは使わない。"""
import importlib.util
from pathlib import Path
import queue
import io
from unittest import mock
import threading
import unittest
import wave

spec = importlib.util.spec_from_file_location("asr_transcribe", Path(__file__).with_name("asr_transcribe.py"))
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)


def make_wav():
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16000)
        wav.writeframes(bytes(32000))
    return buffer.getvalue()


class Pipe:
    def write(self, data):
        return len(data)

    def flush(self):
        pass

    def close(self):
        pass


class DuplexTests(unittest.TestCase):
    def test_worker_fatal_wins_over_broken_pipe_and_keeps_partial_result(self):
        events = queue.Queue()
        events.put(tool.WriterFailure(BrokenPipeError("write stopped"), "write_pcm"))
        events.put({"type": "progress", "request_id": 7, "processed_ms": 1000})
        memory = {"current": {"commit_bytes": 100}, "interval_peaks": {"commit_bytes": 200}}
        events.put({"type": "window", "request_id": 7, "start_ms": 0, "read_end_ms": 1000,
                    "join_ms": 900, "final": False, "memory": memory,
                    "segments": [{"start_ms": 100, "end_ms": 500, "text": "合成"}]})
        fatal = {"type": "error", "request_id": 7, "status": "fatal", "code": "inference", "error": "test fatal"}
        events.put(fatal)
        events.put(EOFError("worker ended"))

        class Broken(Pipe):
            def write(self, data):
                raise BrokenPipeError("write stopped")

        record = {"windows": [], "progress_ms": [], "segments": []}
        writers = []
        wav = wave.open(io.BytesIO(make_wav()), "rb")
        with mock.patch.object(tool.wave, "open", return_value=wav):
            with self.assertRaises(tool.WorkerFailure) as raised:
                tool.transcribe_file(Broken(), events, 7, Path("input.wav"), record, writers, timeout=2)
            for writer in writers:
                writer.join(2)
                self.assertFalse(writer.is_alive())
        self.assertEqual(raised.exception.frame, fatal)
        self.assertEqual(record["worker_failure"], fatal)
        self.assertEqual(record["progress_ms"], [1000])
        self.assertEqual(record["windows"][0]["memory"], memory)
        self.assertEqual(record["segments"], [{"start": .1, "end": .5, "text": "合成"}])
        self.assertEqual(record["source_samples"], 16000)
        self.assertEqual(record["writer_error"]["stage"], "write_pcm")

    def test_receive_window_while_pcm_writer_waits(self):
        window_seen = threading.Event()
        events = queue.Queue()

        class Windows(list):
            def append(self, item):
                super().append(item)
                window_seen.set()

        class Backpressure(Pipe):
            first = True

            def write(self, data):
                if self.first:
                    self.first = False
                    if not window_seen.wait(2):
                        raise RuntimeError("main receiver was blocked by PCM writing")
                if b'"type":"end"' in bytes(data):
                    events.put({"type": "end", "request_id": 1, "status": "ok", "duration_ms": 1000})
                return len(data)

        events.put({"type": "window", "request_id": 1, "start_ms": 0, "read_end_ms": 1000,
                    "join_ms": 1000, "final": True, "segments": []})
        record = {"windows": Windows(), "progress_ms": [], "segments": []}
        wav = wave.open(io.BytesIO(make_wav()), "rb")
        with mock.patch.object(tool.wave, "open", return_value=wav):
            tool.transcribe_file(Backpressure(), events, 1, Path("input.wav"), record, [], timeout=3)
        self.assertEqual(record["audio_seconds"], 1)
        self.assertTrue(record["writer_completed"])
        self.assertEqual(record["pcm_bytes_sent"], 32000)

    def test_local_input_error_does_not_wait_for_worker_eof(self):
        writers = []
        record = {"windows": [], "progress_ms": [], "segments": []}
        with self.assertRaises(FileNotFoundError):
            tool.transcribe_file(Pipe(), queue.Queue(), 1, Path("missing-asr-fixture.wav"), record, writers, timeout=2)
        for writer in writers:
            writer.join(2)
        self.assertEqual(record["writer_error"]["stage"], "open_wav")

    def test_failed_input_terminal_end_then_good_input_same_pipe(self):
        events = queue.Queue()
        pipe = Pipe()
        records = []
        for request, status in ((1, "item_failure"), (2, "ok")):
            end = {"type": "end", "request_id": request, "status": status, "duration_ms": 1000}
            if status == "item_failure":
                end.update(code="invalid_utf8", error="invalid token bytes")
            events.put(end)
            record = {"windows": [], "progress_ms": [], "segments": []}
            wav = wave.open(io.BytesIO(make_wav()), "rb")
            with mock.patch.object(tool.wave, "open", return_value=wav):
                tool.transcribe_file(pipe, events, request, Path("input.wav"), record, [], timeout=2)
            records.append(record)
        self.assertEqual([r["status"] for r in records], ["item_failure", "ok"])
        self.assertEqual(records[0]["item_failure"]["code"], "invalid_utf8")
        self.assertFalse(records[0]["segments_complete"])
        self.assertTrue(records[1]["segments_complete"])
        self.assertTrue(all(r["writer_completed"] and r["pcm_bytes_sent"] == 32000 for r in records))


if __name__ == "__main__":
    unittest.main()

