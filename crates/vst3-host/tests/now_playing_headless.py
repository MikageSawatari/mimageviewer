"""Real pipes and inert SDK receivers; never load a plugin or product UI."""
import json
import queue
import re
import struct
import subprocess
import sys
import threading
import time


def spawn(exe, mode):
    proc = subprocess.Popen([exe, mode], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    lines = queue.Queue()
    history = []

    def read():
        for raw in proc.stderr:
            line = raw.decode("utf-8", errors="replace").rstrip()
            history.append(line)
            lines.put(line)

    threading.Thread(target=read, daemon=True).start()
    return proc, lines, history


def send(proc, command):
    data = json.dumps(command, ensure_ascii=False, separators=(",", ":")).encode()
    proc.stdin.write(struct.pack("<I", len(data)))
    proc.stdin.write(data)
    proc.stdin.flush()


def wait_line(proc, lines, needle, timeout=5):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            line = lines.get(timeout=max(0.01, end - time.monotonic()))
        except queue.Empty:
            break
        if needle in line:
            return line
    raise AssertionError(f"missing {needle!r}, host status={proc.poll()}")


def snapshot(revision, artwork="", title=""):
    return dict(cmd="set_now_playing", slot_id=0, revision=revision,
                title=title, album="", artist="", artwork=artwork,
                artworkMimeType="image/png" if artwork else "")


def slow(exe):
    proc, lines, history = spawn(exe, "--slow")
    try:
        artwork = "A" * 11184811 + "="
        send(proc, snapshot(1, artwork, "first"))
        wait_line(proc, lines, "FIXTURE notify_entered")
        # GUI is executing while stdin continues to admit full 8 MiB images.
        for revision in range(2, 14):
            send(proc, snapshot(revision, artwork, "latest"))
        send(proc, snapshot(14))  # Clear owns the ordering watermark.
        send(proc, snapshot(13, artwork, "must not resurrect"))
        send(proc, snapshot(15, artwork, "new media"))
        send(proc, dict(cmd="release"))
        wait_line(proc, lines, "FIXTURE applied revision=15")
        send(proc, dict(cmd="close"))
        send(proc, dict(cmd="open"))
        send(proc, snapshot(1, title="new incarnation"))
        wait_line(proc, lines, "FIXTURE applied revision=1")
        proc.stdin.close()
        assert proc.wait(timeout=5) == 0, "headless host failure"
        line = wait_line(proc, lines, "FIXTURE high")
        values = {k: int(v) for k, v in re.findall(r"(\w+)=(\d+)", line)}
        assert values["running"] == 1 and values["pending"] == 1
        assert values["wakes"] <= 1 and values["mailbox_bytes"] == 16777216
        assert values["frame"] <= 33554432 and values["generic_image_bytes"] == 0
        assert values["released"] == 1
        assert not any("applied revision=13" in x for x in history)
        assert proc.stdout.read() == b"", "no metadata ack/error event"
        print("PASS slow GUI: " + line)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


def hang(exe, close, watchdog=False):
    proc, lines, _ = spawn(exe, "--hang" if watchdog else "--hang-sync")
    try:
        send(proc, snapshot(1))
        wait_line(proc, lines, "FIXTURE notify_entered")
        start = time.monotonic()
        send(proc, dict(cmd="query_state_concurrent" if close else "query_state"))
        if close:
            send(proc, dict(cmd="close"))
        status = proc.wait(timeout=6.5)
        elapsed = time.monotonic() - start
        assert status & 0xffffffff == 0xEFFEC002, status
        assert 4.8 <= elapsed <= 6.5, elapsed
        line = wait_line(proc, lines, "FIXTURE watchdog expired")
        assert "get_state_queued=1 done=0" in line, line
        if not watchdog:
            assert f"operation={'unload' if close else 'query'}" in line, line
        assert proc.stdout.read() == b"", "timeout must not send generic Error/ack"
        print(f"PASS hang close={close} watchdog={watchdog}: exited after {elapsed:.3f}s; {line}")
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


def slow_sync(exe, close):
    proc, lines, _ = spawn(exe, "--slow")
    try:
        send(proc, snapshot(1, title="slow notification"))
        wait_line(proc, lines, "FIXTURE notify_entered")
        send(proc, dict(cmd="close" if close else "query_state"))
        # Greater than the fake query budget: queue waiting must not kill host.
        time.sleep(0.35)
        assert proc.poll() is None, "sync budget incorrectly killed executing notify"
        send(proc, dict(cmd="release"))
        operation = "unload" if close else "query"
        wait_line(proc, lines, f"FIXTURE sync_complete operation={operation}")
        proc.stdin.close()
        assert proc.wait(timeout=5) == 0
        assert proc.stdout.read() == b"", "metadata queue wait emitted generic Error"
        print(f"PASS slow notify then same {operation} future completed")
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


if __name__ == "__main__":
    slow(sys.argv[1])
    slow_sync(sys.argv[1], False)
    slow_sync(sys.argv[1], True)
    hang(sys.argv[1], False)
    hang(sys.argv[1], True)
    hang(sys.argv[1], True, watchdog=True)
