"""ASR protocol 1 の実モデル検証。WAV を逐次送信し、結果とメモリのピークを新規保存する。

python -I crates/vlm-worker/tools/asr_transcribe.py --worker vendor/vlm-worker/mimageviewer-vlm.exe \
    --model MODEL --device 0 --out target/asr-evidence/run.json WAV16K [WAV16K ...]

同じプロセス・モデルで複数入力を処理する。製品アプリや設定には触れない。
"""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import json
from pathlib import Path
import queue
import struct
import subprocess
import threading
import time
import wave

MAX_FRAME = 16 * 1024 * 1024


def exact(pipe, count):
    result = bytearray()
    while len(result) < count:
        part = pipe.read(count - len(result))
        if not part:
            raise EOFError("Worker pipe ended inside a response")
        result.extend(part)
    return result


def read_frame(pipe):
    size, = struct.unpack("<I", exact(pipe, 4))
    if not 4 <= size <= MAX_FRAME:
        raise ValueError("Invalid response frame size")
    body = exact(pipe, size)
    header_size, = struct.unpack_from("<I", body)
    if header_size != size - 4:
        raise ValueError("Unexpected response binary payload")
    return json.loads(body[4:].decode("utf-8", errors="strict"))


def send(pipe, header, payload=b""):
    data = json.dumps(header, separators=(",", ":")).encode("utf-8")
    for buffer in (struct.pack("<II", 4 + len(data) + len(payload), len(data)), data, payload):
        remaining = memoryview(buffer)
        while remaining:
            written = pipe.write(remaining)
            if not written:
                raise BrokenPipeError("Worker pipe stopped accepting PCM")
            remaining = remaining[written:]
    pipe.flush()


class Memory(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD),
                ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t), ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t),
                ("PrivateUsage", ctypes.c_size_t)]


class WorkerFailure(RuntimeError):
    def __init__(self, frame):
        self.frame = frame
        super().__init__(f"Worker {frame.get('code')}: {frame.get('error')}")


class WriterFailure:
    def __init__(self, error, stage):
        self.error, self.stage = error, stage


def transcribe_file(pipe, events, request, path, record, writers, timeout=1800):
    """PCM送信は別threadへ置き、呼出し側はfatalを含む応答を到着順に採用する。"""
    def write_pcm():
        stage = "open_wav"
        try:
            with wave.open(str(path), "rb") as wav:
                if (wav.getframerate(), wav.getnchannels(), wav.getsampwidth()) != (16000, 1, 2):
                    raise ValueError("Expected 16kHz mono s16 WAV")
                record["source_samples"] = wav.getnframes()
                record["source_duration_ms"] = wav.getnframes() // 16
                record["pcm_bytes_sent"] = 0
                while True:
                    stage = "read_wav"
                    chunk = wav.readframes(512 * 1024)
                    if not chunk:
                        break
                    stage = "write_pcm"
                    send(pipe, {"type": "pcm", "request_id": request}, chunk)
                    record["pcm_bytes_sent"] += len(chunk)
            stage = "write_end"
            send(pipe, {"type": "end", "request_id": request})
            record["writer_completed"] = True
        except BaseException as error:
            record["writer_error"] = {"stage": stage, "type": type(error).__name__, "error": str(error)}
            events.put(WriterFailure(error, stage))
            # pipeの書込み失敗ではEOFまで読ませ、先に送られたtyped fatalを優先する。
            # 再試行はしない。ローカルWAV失敗なら呼出し側が直ちに中止する。
            if stage.startswith("write_"):
                try:
                    pipe.close()
                except OSError:
                    pass

    writer = threading.Thread(target=write_pcm, daemon=True)
    writers.append(writer)
    writer.start()
    write_error = None
    while True:
        event = events.get(timeout=timeout)
        if isinstance(event, WriterFailure):
            write_error = event.error
            if not event.stage.startswith("write_"):
                raise event.error
            continue
        if isinstance(event, BaseException):
            if write_error is not None:
                raise write_error from event
            raise event
        if event.get("status") == "fatal":
            record["worker_failure"] = event
            raise WorkerFailure(event)
        if event.get("request_id") != request:
            raise ValueError("Response request mismatch")
        if event["type"] == "end":
            if event.get("status") not in ("ok", "item_failure"):
                raise ValueError("Unknown ASR end status")
            if event["status"] == "item_failure":
                if event.get("code") != "invalid_utf8" or not isinstance(event.get("error"), str):
                    raise ValueError("Invalid per-input failure response")
                record["item_failure"] = event
                record["error"] = event["error"]
            record["audio_seconds"] = event["duration_ms"] / 1000
            record["end"] = event
            record["status"] = event["status"]
            record["segments_complete"] = event["status"] == "ok"
            writer.join()
            if record.get("writer_error"):
                raise RuntimeError(f"PCM writer failed: {record['writer_error']}")
            if event["duration_ms"] != record["pcm_bytes_sent"] // 32:
                raise ValueError("Worker end duration differs from transmitted PCM")
            return
        if event["type"] == "progress":
            record["progress_ms"].append(event["processed_ms"])
        elif event["type"] == "window":
            record["windows"].append({key: value for key, value in event.items() if key != "segments"})
            record["segments"].extend({"start": s["start_ms"] / 1000, "end": s["end_ms"] / 1000,
                                      "text": s["text"]} for s in event["segments"])
        else:
            raise ValueError("Unexpected response type")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--device", type=int, required=True, help="Vulkan physical_index")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("wav", type=Path, nargs="+")
    args = parser.parse_args()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    log = args.out.with_suffix(".stderr.log")
    # 起動前に既存出力を保護する。失敗した出力も診断用に残す。
    with args.out.open("x", encoding="utf-8") as output, log.open("xb") as stderr:
        started = time.perf_counter()
        process = subprocess.Popen([str(args.worker.resolve()), "--asr-serve"], stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=stderr, bufsize=0)
        events = queue.Queue()
        stop = threading.Event()
        peaks = {"working_set_bytes": 0, "commit_bytes": 0, "private_bytes": 0}
        interval = {"working_set_bytes": 0, "commit_bytes": 0, "private_bytes": 0}
        memory_lock = threading.Lock()
        get_memory = ctypes.WinDLL("psapi").GetProcessMemoryInfo
        get_memory.argtypes = [wintypes.HANDLE, ctypes.POINTER(Memory), wintypes.DWORD]
        get_memory.restype = wintypes.BOOL

        def memory_snapshot(reset=False):
            counters = Memory()
            counters.cb = ctypes.sizeof(counters)
            if not get_memory(wintypes.HANDLE(int(process._handle)), ctypes.byref(counters), counters.cb):
                return None
            current = {"working_set_bytes": counters.WorkingSetSize, "commit_bytes": counters.PagefileUsage,
                       "private_bytes": counters.PrivateUsage}
            with memory_lock:
                for key, value in current.items():
                    interval[key] = max(interval[key], value)
                # OS が保持する累積ピークと、窓ごとにリセットする現在量のピークを分ける。
                peaks["working_set_bytes"] = max(peaks["working_set_bytes"], counters.PeakWorkingSetSize)
                peaks["commit_bytes"] = max(peaks["commit_bytes"], counters.PeakPagefileUsage)
                peaks["private_bytes"] = max(peaks["private_bytes"], counters.PrivateUsage)
                result = {"current": current, "interval_peaks": dict(interval), "process_peaks": dict(peaks)}
                if reset:
                    interval.update(current)
                return result

        def sample():
            while not stop.is_set():
                memory_snapshot()
                stop.wait(0.1)

        def reader():
            try:
                while True:
                    event = read_frame(process.stdout)
                    if event["type"] in ("window", "begin", "load", "error") or event.get("status") in ("fatal", "item_failure"):
                        # 書込み側の背圧と無関係に、到着した窓の時点で採取する。
                        event["memory"] = memory_snapshot(reset=True)
                        event["received_wall_s"] = time.perf_counter() - started
                    events.put(event)
            except BaseException as error:
                events.put(error)

        monitor = threading.Thread(target=sample, daemon=True)
        receiver = threading.Thread(target=reader, daemon=True)
        monitor.start()
        receiver.start()

        def receive(expected):
            event = events.get(timeout=1800)
            if isinstance(event, BaseException):
                raise event
            if event.get("status") == "fatal":
                raise WorkerFailure(event)
            if event["type"] != expected:
                raise ValueError(f"Expected {expected}, got {event['type']}")
            return event

        report = {"worker": str(args.worker.resolve()), "model": str(args.model.resolve()),
                  "device_index": args.device, "stderr_log": str(log.resolve()), "files": []}
        writers = []
        current = None
        file_start = None
        try:
            send(process.stdin, {"type": "hello"})
            report["hello"] = receive("hello")
            if report["hello"].get("role") != "asr" or report["hello"]["protocol_version"] != 1:
                raise ValueError("Worker role/protocol mismatch")
            send(process.stdin, {"type": "load", "model_path": str(args.model.resolve()), "device_index": args.device})
            report["load"] = receive("load")
            for request, path in enumerate(args.wav, 1):
                file_start = time.perf_counter()
                current = {"input": str(path.resolve()), "request_id": request, "status": "running",
                           "windows": [], "progress_ms": [], "segments": []}
                report["current_input"] = current
                send(process.stdin, {"type": "begin", "request_id": request})
                current["begin"] = receive("begin")
                transcribe_file(process.stdin, events, request, path, current, writers)
                current.update(wall_s=time.perf_counter() - file_start, process_peaks=dict(peaks))
                report["files"].append(current)
                print(f"input={request} status={current['status']} windows={len(current['windows'])} segments={len(current['segments'])}", flush=True)
                current = None
                del report["current_input"]
            send(process.stdin, {"type": "shutdown"})
            process.stdin.close()
            if process.wait(timeout=60) != 0:
                raise RuntimeError(f"Worker exit {process.returncode}")
        except BaseException as error:
            report["error"] = str(error)
            report["error_type"] = type(error).__name__
            if isinstance(error, WorkerFailure):
                report["worker_failure"] = error.frame
            if current is not None:
                current.update(status="failed", error=str(error), wall_s=time.perf_counter() - file_start,
                               process_peaks=dict(peaks), failure_memory=memory_snapshot())
            raise
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            for writer in writers:
                writer.join(timeout=5)
            stop.set()
            monitor.join()
            receiver.join(timeout=5)
            report["process_peaks"] = peaks
            report["wall_s"] = time.perf_counter() - started
            json.dump(report, output, ensure_ascii=False, indent=1)
            output.write("\n")
            process.stdout.close()


if __name__ == "__main__":
    main()
