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
                    "utf8_discarded_bytes":0,"utf8_discarded_fragments":0,"segments": [{"start_ms": 100, "end_ms": 500, "text": "合成"}]})
        fatal = {"type": "error", "request_id": 7, "status": "fatal", "code": "inference", "error": "test fatal"}
        events.put(fatal)
        events.put(EOFError("worker ended"))

        class Broken(Pipe):
            def write(self, data):
                raise BrokenPipeError("write stopped")

        record = {"windows": [], "progress_ms": [], "utf8_discarded_bytes":0,"utf8_discarded_fragments":0,"segments": []}
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
                    events.put({"type": "end", "request_id": 1, "status": "ok", "duration_ms": 1000,"utf8_discarded_bytes":0,"utf8_discarded_fragments":0})
                return len(data)

        events.put({"type": "window", "request_id": 1, "start_ms": 0, "read_end_ms": 1000,
                    "join_ms": 1000, "final": True, "utf8_discarded_bytes":0,"utf8_discarded_fragments":0,"segments": []})
        record = {"windows": Windows(), "progress_ms": [], "utf8_discarded_bytes":0,"utf8_discarded_fragments":0,"segments": []}
        wav = wave.open(io.BytesIO(make_wav()), "rb")
        with mock.patch.object(tool.wave, "open", return_value=wav):
            tool.transcribe_file(Backpressure(), events, 1, Path("input.wav"), record, [], timeout=3)
        self.assertEqual(record["audio_seconds"], 1)
        self.assertTrue(record["writer_completed"])
        self.assertEqual(record["pcm_bytes_sent"], 32000)

    def test_local_input_error_does_not_wait_for_worker_eof(self):
        writers = []
        record = {"windows": [], "progress_ms": [], "utf8_discarded_bytes":0,"utf8_discarded_fragments":0,"segments": []}
        with self.assertRaises(FileNotFoundError):
            tool.transcribe_file(Pipe(), queue.Queue(), 1, Path("missing-asr-fixture.wav"), record, writers, timeout=2)
        for writer in writers:
            writer.join(2)
        self.assertEqual(record["writer_error"]["stage"], "open_wav")

    def test_repair_counts_and_next_input_same_pipe(self):
        events = queue.Queue()
        pipe = Pipe()
        records = []
        for request, discarded in ((1, 2), (2, 0)):
            counts = {"utf8_discarded_bytes":discarded,"utf8_discarded_fragments":int(discarded>0)}
            events.put({"type":"window","request_id":request,"start_ms":0,"read_end_ms":1000,
                        "join_ms":1000,"final":True,"segments":[],**counts})
            events.put({"type":"end","request_id":request,"status":"ok","duration_ms":1000,**counts})
            record = {"windows":[],"progress_ms":[],"segments":[]}
            wav = wave.open(io.BytesIO(make_wav()),"rb")
            with mock.patch.object(tool.wave,"open",return_value=wav):
                tool.transcribe_file(pipe,events,request,Path("input.wav"),record,[],timeout=2)
            records.append(record)
        self.assertEqual([r["utf8_discarded_bytes"] for r in records],[2,0])
        self.assertTrue(all(r["segments_complete"] and r["writer_completed"] for r in records))

    def test_unknown_status_or_mismatched_totals_are_rejected(self):
        for status, count in (("item_failure",0),("ok",1)):
            events=queue.Queue()
            events.put({"type":"end","request_id":1,"status":status,"duration_ms":1000,
                        "utf8_discarded_bytes":count,"utf8_discarded_fragments":count})
            wav=wave.open(io.BytesIO(make_wav()),"rb")
            writers=[]
            with mock.patch.object(tool.wave,"open",return_value=wav):
                with self.assertRaises(ValueError):
                    tool.transcribe_file(Pipe(),events,1,Path("input.wav"),
                                         {"windows":[],"progress_ms":[],"segments":[]},writers,timeout=2)
            for writer in writers:writer.join(2)


if __name__ == "__main__":
    unittest.main()

