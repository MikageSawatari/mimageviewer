#!/usr/bin/env python3
"""
mimageviewer-vst3-host.exe の制御プロトコル疎通テスト (POC スモーク)。

目的:
  - bridge 子プロセスが起動できること
  - "hello" コマンドに "ready" イベントを返せること
  - "shutdown" で graceful 終了できること

依存:
  - 標準ライブラリのみ (subprocess / struct)

実行:
  python tests/smoke_hello.py
"""
import os
import struct
import subprocess
import sys
import time


def write_msg(proc: subprocess.Popen, payload: str) -> None:
    data = payload.encode("utf-8")
    header = struct.pack("<I", len(data))
    assert proc.stdin is not None
    proc.stdin.write(header + data)
    proc.stdin.flush()


def read_msg(proc: subprocess.Popen, timeout_sec: float = 5.0) -> str:
    assert proc.stdout is not None
    deadline = time.monotonic() + timeout_sec
    while time.monotonic() < deadline:
        header = proc.stdout.read(4)
        if not header:
            time.sleep(0.05)
            continue
        if len(header) < 4:
            raise RuntimeError(f"short header read: {header!r}")
        (length,) = struct.unpack("<I", header)
        body = proc.stdout.read(length)
        if len(body) < length:
            raise RuntimeError(f"short body read: got {len(body)}/{length}")
        return body.decode("utf-8")
    raise TimeoutError("no response within deadline")


def main() -> int:
    here = os.path.dirname(os.path.abspath(__file__))
    bridge_exe = os.path.normpath(
        os.path.join(here, "..", "..", "..", "vendor", "vst3-host",
                      "mimageviewer-vst3-host.exe")
    )
    if not os.path.exists(bridge_exe):
        print(f"bridge exe not found: {bridge_exe}", file=sys.stderr)
        print("Run cmake build first.", file=sys.stderr)
        return 1

    print(f"Spawning {bridge_exe}")
    proc = subprocess.Popen(
        [bridge_exe],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        # hello → ready  (T09 v0.9.0: PROTOCOL_VERSION は 2 に bump 済み)
        write_msg(proc, '{"cmd":"hello","version":2}')
        reply = read_msg(proc)
        print(f"<- {reply}")
        if '"event":"ready"' not in reply:
            print(f"unexpected reply: {reply}", file=sys.stderr)
            return 2
        if '"version":2' not in reply:
            print(f"version mismatch: {reply}", file=sys.stderr)
            return 3

        # shutdown
        write_msg(proc, '{"cmd":"shutdown"}')
        # bridge は応答せずに終了する
        try:
            proc.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            print("bridge did not exit on shutdown", file=sys.stderr)
            proc.kill()
            return 4

        rc = proc.returncode
        print(f"bridge exited with code {rc}")
        if rc != 0:
            return 5

        print("PASS")
        return 0
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


if __name__ == "__main__":
    sys.exit(main())
