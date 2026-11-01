"""Direct, bounded stdio MCP check; never reads AI client configuration.

Run with Python isolated mode (-I). Supply exactly one server argv after --:
  python -I scripts/mcp_stdio_check.py --timeout 15 --env APPDATA=ABS_TEMP -- \
      ABS_LAUNCHER --mcp --data-dir ABS_DISPOSABLE_DATA
For a portable adapter, use ABS_ADAPTER --data-dir ABS_DISPOSABLE_DATA instead.
Optional: --call TOOL --arguments '{"key":"value"}' before --.
The caller owns isolation and any already-running disposable core. This script
does not start a core, find executables, register servers, or read client configs.
stdout is one JSON report; exit 0 means protocol/EOF/exit checks passed. A tool
result with isError=true is retained as a valid response, not a transport failure.
JSON-RPC error responses stop further requests and are reported as errors, but
still close stdin and verify the server's natural exit before returning exit 1.
"""

import argparse
import json
import math
import os
import queue
import subprocess
import sys
import threading
import time


PROTOCOL_VERSION = "2025-11-25"
MAX_LINE_BYTES = 8 * 1024 * 1024
MAX_STDOUT_BYTES = 16 * 1024 * 1024
MAX_STDERR_BYTES = 1024 * 1024
MAX_MESSAGES = 10000


class CheckFailure(Exception):
    pass


class RpcResponseError(Exception):
    """A completed request failed; the stream remains valid for EOF shutdown."""


def strict_json(text):
    def invalid_constant(value):
        raise ValueError("non-JSON constant: " + value)

    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate JSON key: " + key)
            result[key] = value
        return result

    def finite_float(value):
        number = float(value)
        if not math.isfinite(number):
            raise ValueError("JSON number outside finite range")
        return number

    try:
        return json.loads(text, parse_constant=invalid_constant,
                          parse_float=finite_float, object_pairs_hook=unique_object)
    except RecursionError as error:
        raise ValueError("JSON nesting limit exceeded") from error


def run_check(command, *, timeout=15.0, env_overrides=None, tool_name=None,
              tool_arguments=None, popen_factory=subprocess.Popen):
    """Start only command, returning a JSON-serializable diagnostic report.

    popen_factory is injectable for tests; it receives the exact argv list and
    binary PIPE streams, shell=False, and a copy of env plus explicit overrides.
    All writes (including stdin close) run off the deadline-owning thread, so a
    peer that stops reading cannot block timeout handling. Reader threads drain
    both output streams concurrently; captures and message count are bounded.
    """
    if not command or not all(isinstance(arg, str) and arg for arg in command):
        raise ValueError("command must be a nonempty argv of nonempty strings")
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("timeout must be finite and positive")
    if tool_arguments is not None and not isinstance(tool_arguments, dict):
        raise ValueError("tool arguments must be a JSON object")
    started = time.monotonic()
    deadline = started + timeout
    report = {"command": list(command), "responses": [], "notifications": [],
              "stdout": "", "stderr": "", "exit_code": None,
              "duration_seconds": 0, "stdin_closed": False,
              "stdout_eof": False, "stderr_eof": False, "errors": []}
    events = queue.Queue()
    writes = queue.Queue()
    captures = {"stdout": [], "stderr": []}
    threads = []
    process = None

    def reader(stream, name, limit):
        total = count = 0
        overflow = False
        try:
            while True:
                data = (stream.readline(MAX_LINE_BYTES + 1) if name == "stdout"
                        else stream.read(4096))
                if not data:
                    break
                count += 1
                available = max(0, limit - total)
                kept = data[:available]
                total += len(data)
                if kept and not overflow:
                    captures[name].append(kept)
                exceeded = (total > limit or (name == "stdout" and
                            (len(data) > MAX_LINE_BYTES or count > MAX_MESSAGES)))
                if exceeded and not overflow:
                    overflow = True
                    events.put(("failure", name + " output limit exceeded"))
                if not overflow and name == "stdout":
                    if not data.endswith(b"\n"):
                        events.put(("failure", "unterminated stdout JSON line at EOF"))
                        overflow = True
                    else:
                        events.put(("line", data))
        except Exception as error:
            events.put(("failure", name + " read failed: " + str(error)))
        finally:
            events.put((name + "_eof", None))

    def writer():
        try:
            while True:
                item = writes.get()
                if item is None:
                    process.stdin.close()
                    events.put(("closed", None))
                    return
                token, data = item
                offset = 0
                while offset < len(data):
                    written = process.stdin.write(data[offset:])
                    if written is None or written <= 0:
                        raise OSError("stdin write made no progress")
                    offset += written
                process.stdin.flush()
                events.put(("written", token))
        except Exception as error:
            events.put(("failure", "stdin write/close failed: " + str(error)))

    def receive():
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise CheckFailure("timeout")
        try:
            return events.get(timeout=min(remaining, 0.05))
        except queue.Empty:
            return "tick", None

    def output_event(kind, value):
        if kind == "failure":
            raise CheckFailure(value)
        if kind.endswith("_eof"):
            report[kind] = True
        if kind != "line":
            return None
        try:
            message = strict_json(value.decode("utf-8"))
        except (UnicodeError, ValueError) as error:
            raise CheckFailure("malformed stdout JSON: " + str(error)) from error
        if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
            raise CheckFailure("invalid JSON-RPC message")
        if "id" not in message and isinstance(message.get("method"), str):
            report["notifications"].append(message)
            return None
        if ("method" in message or "id" not in message or
                ("result" in message) == ("error" in message)):
            raise CheckFailure("invalid or unsupported JSON-RPC response")
        if "error" in message:
            error = message["error"]
            if (not isinstance(error, dict) or type(error.get("code")) is not int or
                    not isinstance(error.get("message"), str)):
                raise CheckFailure("invalid JSON-RPC error response")
        report["responses"].append(message)
        return message

    def send(message, token):
        data = (json.dumps(message, ensure_ascii=False, allow_nan=False,
                           separators=(",", ":")) + "\n").encode("utf-8")
        if len(data) > MAX_LINE_BYTES:
            raise CheckFailure("request output limit exceeded")
        writes.put((token, data))

    def request(request_id, method, params):
        send({"jsonrpc": "2.0", "id": request_id,
              "method": method, "params": params}, request_id)
        while True:
            kind, value = receive()
            message = output_event(kind, value)
            if message is not None:
                if type(message["id"]) is not int or message["id"] != request_id:
                    raise CheckFailure("unexpected response id (requests are FIFO)")
                if "error" in message:
                    raise RpcResponseError("JSON-RPC error for " + method)
                return message["result"]
            if report["stdout_eof"]:
                raise CheckFailure("stdout EOF before response to " + method)

    try:
        environment = os.environ.copy()
        environment.update(env_overrides or {})
        process = popen_factory(list(command), stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                env=environment, shell=False, bufsize=0)
        for stream, name, limit in ((process.stdout, "stdout", MAX_STDOUT_BYTES),
                                    (process.stderr, "stderr", MAX_STDERR_BYTES)):
            thread = threading.Thread(target=reader, args=(stream, name, limit), daemon=True)
            threads.append(thread)
            thread.start()
        thread = threading.Thread(target=writer, daemon=True)
        threads.append(thread)
        thread.start()
        try:
            initialized = request(1, "initialize", {
                "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                "clientInfo": {"name": "miv-direct-stdio-check", "version": "1"}})
            if (not isinstance(initialized, dict) or
                    initialized.get("protocolVersion") != PROTOCOL_VERSION):
                raise CheckFailure("initialize negotiated an unexpected protocol version")
            send({"jsonrpc": "2.0", "method": "notifications/initialized"}, "initialized")
            listed = request(2, "tools/list", {})
            if not isinstance(listed, dict) or not isinstance(listed.get("tools"), list):
                raise CheckFailure("tools/list result has no tools array")
            if tool_name is not None:
                request(3, "tools/call", {"name": tool_name, "arguments": tool_arguments or {}})
        except RpcResponseError as error:
            # The response completed normally. Do not turn a server-side request
            # error into a forced-exit result and lose the EOF/exit evidence.
            report["errors"].append(str(error))
        writes.put(None)
        while not (report["stdin_closed"] and report["stdout_eof"] and
                   report["stderr_eof"] and process.poll() is not None):
            kind, value = receive()
            if kind == "closed":
                report["stdin_closed"] = True
            elif output_event(kind, value) is not None:
                raise CheckFailure("unexpected response after final request")
        report["exit_code"] = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        if report["exit_code"] != 0:
            raise CheckFailure("server exited with code " + str(report["exit_code"]))
    except (CheckFailure, OSError, ValueError, subprocess.SubprocessError) as error:
        report["errors"].append(str(error))
    finally:
        if process is not None:
            # Do not wait on stdin.close here: a writer may be blocked in write.
            writes.put(None)
            if process.poll() is None:
                try:
                    process.kill()
                    process.wait(timeout=2)
                except (OSError, subprocess.SubprocessError) as error:
                    report["errors"].append("server cleanup failed: " + str(error))
            report["exit_code"] = process.poll()
            for thread in threads:
                thread.join(timeout=0.1)
            # Close handles only after their owning thread has stopped. Closing
            # underneath a blocked pipe operation could itself block cleanup.
            for thread, stream in zip(threads, (process.stdout, process.stderr, process.stdin)):
                if not thread.is_alive():
                    try:
                        stream.close()
                        if stream is process.stdin:
                            report["stdin_closed"] = True
                    except OSError as error:
                        report["errors"].append("pipe close failed: " + str(error))
            # Readers own their capture lists; snapshots also work if a peer kept
            # an inherited pipe open after the selected process was terminated.
            for name in captures:
                report[name] = b"".join(list(captures[name])).decode("utf-8", errors="replace")
            while True:
                try:
                    kind, value = events.get_nowait()
                except queue.Empty:
                    break
                if kind.endswith("_eof"):
                    report[kind] = True
                elif kind == "closed":
                    report["stdin_closed"] = True
        report["duration_seconds"] = round(time.monotonic() - started, 6)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=15.0)
    parser.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    parser.add_argument("--call", metavar="TOOL")
    parser.add_argument("--arguments", default="{}", metavar="JSON_OBJECT")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    overrides = {}
    for setting in args.env:
        name, separator, value = setting.partition("=")
        if not separator or not name or "\0" in setting:
            parser.error("--env requires NAME=VALUE")
        overrides[name] = value
    try:
        arguments = strict_json(args.arguments)
        if not isinstance(arguments, dict):
            raise ValueError("--arguments must be a JSON object")
        if not args.call and arguments:
            raise ValueError("--arguments requires --call")
        report = run_check(command, timeout=args.timeout, env_overrides=overrides,
                           tool_name=args.call, tool_arguments=arguments)
    except ValueError as error:
        parser.error(str(error))
    # Escaped Unicode also makes redirection safe on non-UTF-8 Windows consoles.
    print(json.dumps(report, ensure_ascii=True, allow_nan=False, indent=2))
    return int(bool(report["errors"]))


if __name__ == "__main__":
    sys.exit(main())
