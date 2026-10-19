"""CPU-load acceptance gate for the similar-index store owner (headless fixtures only).

Copy to target/similar-startup-investigation/stress-load.py to retain the investigation command.
Compile the focused tests first so all stress rounds run under the bounded CPU load.
"""
import argparse
import multiprocessing as mp
import os
from pathlib import Path
import subprocess
import time


def burn(until):
    value = 1
    while time.monotonic() < until:
        for _ in range(10000):
            value = (value * 1664525 + 1013904223) & 0xFFFFFFFF


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stress-count", type=int, default=10)
    args = parser.parse_args()
    if args.stress_count < 10:
        parser.error("acceptance requires at least 10 rounds")
    os.environ["MSBUILDDISABLENODEREUSE"] = "1"
    output = Path("target/similar-startup-investigation")
    output.mkdir(parents=True, exist_ok=True)
    log = output / f"owner-cpu-load-{args.stress_count}.log"
    command = ["cargo", "nextest", "run", "-p", "mimageviewer", "--lib", "-E",
               "test(/^similar_index::/)", "--stress-count", str(args.stress_count),
               "-j", "48", "--no-fail-fast"]
    started = time.monotonic()
    until = started + 180
    workers = [mp.Process(target=burn, args=(until,)) for _ in range(24)]
    try:
        for worker in workers:
            worker.start()
        with log.open("w", encoding="utf-8") as out:
            out.write(f"cpu_count={os.cpu_count()} cpu_processes=24 cpu_limit_seconds=180 command={command!r}\n")
            out.flush()
            result = subprocess.run(command, stdout=out, stderr=subprocess.STDOUT, timeout=1800)
        print(f"exit={result.returncode} wall={time.monotonic()-started:.3f}s log={log}", flush=True)
        print("\n".join(log.read_text(encoding="utf-8", errors="replace").splitlines()[-8:]))
        return result.returncode
    finally:
        for worker in workers:
            if worker.pid is not None:
                if worker.is_alive():
                    worker.terminate()
                worker.join()


if __name__ == "__main__":
    raise SystemExit(main())
