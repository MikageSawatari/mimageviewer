"""Validate Cargo's executable report before signing or embedding release files."""

import argparse
import json
import os
import sys


def check(messages, expected):
    expected = os.path.normcase(os.path.abspath(expected))
    found = False
    for line in messages:
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and message.get("executable"):
            actual = os.path.normcase(os.path.abspath(message["executable"]))
            if actual != expected:
                raise ValueError(f"Cargo executable output mismatch: {actual} (expected {expected})")
            found = True
    if not found:
        raise ValueError(f"Cargo did not report the expected executable: {expected}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--expected", required=True)
    args = parser.parse_args()
    try:
        check((line.decode("utf-8") for line in sys.stdin.buffer), args.expected)
    except (ValueError, OSError) as error:
        print(f"[build-release] {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
