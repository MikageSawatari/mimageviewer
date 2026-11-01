"""Headless direct-client tests. Fake Popen only: no process is ever spawned.

Run: python -I scripts/test_mcp_stdio_check.py
"""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import queue
import threading
import time
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "mcp_stdio_check", Path(__file__).with_name("mcp_stdio_check.py"))
client = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(client)


class FakeOutput:
    def __init__(self):
        self.queue = queue.Queue()
        self.pending = b""
        self.eof = False

    def feed(self, data):
        self.queue.put(data)

    def finish(self):
        self.queue.put(None)

    def close(self):
        pass

    def read(self, size):
        while not self.pending and not self.eof:
            value = self.queue.get()
            if value is None:
                self.eof = True
            else:
                self.pending = value
        result, self.pending = self.pending[:size], self.pending[size:]
        return result

    def readline(self, size):
        result = b""
        while len(result) < size:
            byte = self.read(1)
            if not byte:
                break
            result += byte
            if byte == b"\n":
                break
        return result


class FakeInput:
    def __init__(self, process):
        self.process = process
        self.closed = False
        self.pending = b""

    def write(self, data):
        if self.process.block_writes:
            self.process.killed.wait()
            raise BrokenPipeError("killed during write")
        written = min(len(data), 7) if self.process.partial_writes else len(data)
        self.pending += data[:written]
        if self.pending.endswith(b"\n"):
            request = json.loads(self.pending)
            self.pending = b""
            self.process.requests.append(request)
            self.process.handle(request)
        return written

    def flush(self):
        pass

    def close(self):
        self.closed = True
        if not self.process.ignore_stdin_eof:
            self.process.finish(self.process.normal_exit)


class FakeProcess:
    def __init__(self, *, handler=None, normal_exit=0, ignore_stdin_eof=False,
                 block_writes=False, partial_writes=False):
        self.stdout = FakeOutput()
        self.stderr = FakeOutput()
        self.stdin = FakeInput(self)
        self.requests = []
        self.handler = handler
        self.normal_exit = normal_exit
        self.ignore_stdin_eof = ignore_stdin_eof
        self.block_writes = block_writes
        self.partial_writes = partial_writes
        self.returncode = None
        self.killed = threading.Event()

    def handle(self, request):
        if self.handler is not None and self.handler(self, request):
            return
        if "id" not in request:
            return
        result = ({"protocolVersion": client.PROTOCOL_VERSION}
                  if request["method"] == "initialize" else
                  {"tools": []} if request["method"] == "tools/list" else
                  {"content": [{"type": "text", "text": "隔離した応答"}]})
        self.reply(request["id"], result)

    def reply(self, request_id, result):
        self.stdout.feed((json.dumps({"jsonrpc": "2.0", "id": request_id,
                                     "result": result}, ensure_ascii=False) + "\n").encode("utf-8"))

    def finish(self, code):
        if self.returncode is None:
            self.returncode = code
            self.stdout.finish()
            self.stderr.finish()

    def poll(self):
        return self.returncode

    def wait(self, timeout):
        if self.returncode is None:
            raise client.subprocess.TimeoutExpired("fake", timeout)
        return self.returncode

    def kill(self):
        self.killed.set()
        self.finish(-9)


class DirectClientTests(unittest.TestCase):
    def check_process(self, process, **kwargs):
        factory = mock.Mock(return_value=process)
        report = client.run_check(["FAKE_SERVER", "--data-dir", "FAKE_DATA"],
                                  popen_factory=factory, timeout=5, **kwargs)
        factory.assert_called_once()
        return report, factory

    def test_fifo_handshake_call_exact_argv_env_and_eof(self):
        process = FakeProcess()
        arguments = {"path": "日本語\\使い捨て"}
        report, factory = self.check_process(process, env_overrides={"APPDATA": "DISPOSABLE"},
                                             tool_name="miv_status", tool_arguments=arguments)
        self.assertEqual(report["errors"], [])
        self.assertEqual(report["exit_code"], 0)
        self.assertTrue(report["stdin_closed"] and report["stdout_eof"] and report["stderr_eof"])
        self.assertEqual([r["method"] for r in process.requests],
                         ["initialize", "notifications/initialized", "tools/list", "tools/call"])
        self.assertEqual([r["id"] for r in process.requests if "id" in r], [1, 2, 3])
        self.assertEqual(process.requests[-1]["params"],
                         {"name": "miv_status", "arguments": arguments})
        argv, = factory.call_args.args
        self.assertEqual(argv, ["FAKE_SERVER", "--data-dir", "FAKE_DATA"])
        options = factory.call_args.kwargs
        self.assertIs(options["shell"], False)
        self.assertEqual(options["env"]["APPDATA"], "DISPOSABLE")
        self.assertEqual(options["stdin"], client.subprocess.PIPE)
        self.assertIn("隔離した応答", report["stdout"])
        self.assertGreaterEqual(report["duration_seconds"], 0)
        self.assertFalse(process.killed.is_set())

    def test_list_only_with_notification_and_parallel_stderr(self):
        def handler(process, request):
            if request["method"] == "initialize":
                # This wait can finish only if the stderr reader runs while the
                # protocol thread is awaiting its stdout response.
                process.stderr.feed(b"diagnostic\n" * 1000)
                deadline = time.monotonic() + 2
                while not process.stderr.queue.empty() and time.monotonic() < deadline:
                    time.sleep(0.001)
                self.assertTrue(process.stderr.queue.empty())
                process.stdout.feed(b'{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\n')
            return False
        report, _ = self.check_process(FakeProcess(handler=handler))
        self.assertEqual(report["errors"], [])
        self.assertEqual(len(report["responses"]), 2)
        self.assertEqual(len(report["notifications"]), 1)
        self.assertEqual(report["stderr"], "diagnostic\n" * 1000)

    def test_partial_pipe_writes_deliver_complete_requests(self):
        process = FakeProcess(partial_writes=True)
        report, _ = self.check_process(process, tool_name="miv_status")
        self.assertEqual(report["errors"], [])
        self.assertEqual(len(process.requests), 4)

    def test_tool_is_error_remains_reported_as_valid_transport(self):
        def handler(process, request):
            if request["method"] == "tools/list":
                process.reply(request["id"], {"tools": [{"name": "miv_status"}]})
                return True
            if request["method"] == "tools/call":
                process.reply(request["id"], {"isError": True, "content": [
                    {"type": "text", "text": "mIV が起動していません"}]})
                return True
            return False
        process = FakeProcess(handler=handler)
        report, _ = self.check_process(process, tool_name="miv_status")
        self.assertEqual(report["errors"], [])
        self.assertEqual(report["responses"][1]["result"]["tools"], [{"name": "miv_status"}])
        self.assertTrue(report["responses"][-1]["result"]["isError"])
        self.assertEqual(report["exit_code"], 0)
        self.assertTrue(report["stdin_closed"] and report["stdout_eof"] and report["stderr_eof"])
        self.assertFalse(process.killed.is_set())

    def test_rpc_error_keeps_response_and_checks_natural_exit_after_eof(self):
        for method, request_id, exit_code in (("tools/list", 2, 0), ("tools/list", 2, 9),
                                               ("initialize", 1, 0), ("tools/call", 3, 0)):
            with self.subTest(method=method, exit_code=exit_code):
                response = {"jsonrpc": "2.0", "id": request_id,
                            "error": {"code": -32603, "message": "expected fixture error"}}
                def handler(process, request):
                    if request["method"] == method:
                        process.stdout.feed((json.dumps(response) + "\n").encode())
                        process.stderr.feed(b"natural shutdown diagnostic\n")
                        return True
                    return False
                process = FakeProcess(handler=handler, normal_exit=exit_code)
                report, _ = self.check_process(process, tool_name="miv_status")
                # Assert cardinality before indexing, retaining the entire
                # report (especially timeout/raw streams) if scheduling stalls.
                self.assertEqual(len(report["responses"]), request_id, report)
                self.assertEqual(report["responses"][-1], response, report)
                self.assertTrue(report["errors"], report)
                self.assertEqual(report["errors"][0], "JSON-RPC error for " + method, report)
                self.assertEqual(report["exit_code"], exit_code, report)
                self.assertTrue(report["stdin_closed"] and report["stdout_eof"] and report["stderr_eof"], report)
                self.assertEqual(process.requests[-1]["method"], method, report)
                self.assertIn("natural shutdown diagnostic", report["stderr"], report)
                self.assertFalse(process.killed.is_set(), report)
                if exit_code:
                    self.assertIn("server exited with code 9", report["errors"], report)

    def test_early_eof_reports_exit_and_raw_diagnostics(self):
        def handler(process, request):
            process.stderr.feed(b"server stopped\n")
            process.finish(17)
            return True
        report, _ = self.check_process(FakeProcess(handler=handler))
        self.assertIn("EOF before response", report["errors"][0])
        self.assertEqual(report["exit_code"], 17)
        self.assertIn("server stopped", report["stderr"])

    def test_invalid_stdout_is_reported_and_killed(self):
        for data in (b"not JSON\n", b"\xff\n", b"[]\n",
                     b'{"jsonrpc":"2.0","id":1,"result":NaN}\n',
                     b'{"jsonrpc":"2.0","id":1,"result":1e999}\n',
                     b'{"jsonrpc":"2.0","id":1,"id":2,"result":{}}\n',
                     b'{"jsonrpc":"2.0","id":1,"result":{}}'):
            with self.subTest(data=data):
                def handler(process, request):
                    process.stdout.feed(data)
                    if not data.endswith(b"\n"):
                        process.stdout.finish()
                    return True
                process = FakeProcess(handler=handler)
                report, _ = self.check_process(process)
                self.assertTrue(report["errors"])
                self.assertTrue(process.killed.is_set())
                self.assertTrue(report["stdout"])

    def test_wrong_id_rpc_error_version_and_schema(self):
        variants = [
            ({"jsonrpc": "2.0", "id": 9, "result": {}}, "unexpected response id"),
            ({"jsonrpc": "2.0", "id": True, "result": {}}, "unexpected response id"),
            ({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": "other"}}, "protocol version"),
            ({"jsonrpc": "2.0", "id": 1, "method": "ping"}, "unsupported"),
        ]
        for response, expected in variants:
            with self.subTest(expected=expected):
                def handler(process, request):
                    process.stdout.feed((json.dumps(response) + "\n").encode())
                    return True
                report, _ = self.check_process(FakeProcess(handler=handler))
                self.assertIn(expected, report["errors"][0])
        def missing_tools(process, request):
            if request["method"] == "tools/list":
                process.reply(request["id"], {})
                return True
            return False
        report, _ = self.check_process(FakeProcess(handler=missing_tools))
        self.assertIn("tools array", report["errors"][0])

    def test_invalid_rpc_error_fields_are_protocol_violations(self):
        for error in (None, [], {}, {"code": -1}, {"message": "missing code"},
                      {"code": True, "message": "bool code"},
                      {"code": -1.0, "message": "float code"},
                      {"code": "-1", "message": "string code"},
                      {"code": -1, "message": None}, {"code": -1, "message": 7}):
            with self.subTest(error=error):
                def handler(process, request):
                    process.stdout.feed((json.dumps({"jsonrpc": "2.0", "id": 1,
                                                     "error": error}) + "\n").encode())
                    return True
                process = FakeProcess(handler=handler)
                report, _ = self.check_process(process)
                self.assertTrue(report["errors"], report)
                self.assertEqual(report["errors"][0], "invalid JSON-RPC error response", report)
                self.assertFalse(report["responses"], report)
                self.assertTrue(process.killed.is_set(), report)

    def test_nonzero_exit_after_eof(self):
        report, _ = self.check_process(FakeProcess(normal_exit=3))
        self.assertEqual(report["exit_code"], 3)
        self.assertIn("code 3", report["errors"][0])

    def test_timeout_read_write_and_post_eof_exit_are_bounded(self):
        for process in (FakeProcess(handler=lambda _p, _r: True),
                        FakeProcess(block_writes=True),
                        FakeProcess(ignore_stdin_eof=True)):
            with self.subTest(block_writes=process.block_writes):
                report = client.run_check(["FAKE_SERVER"], timeout=0.03,
                                          popen_factory=mock.Mock(return_value=process))
                self.assertEqual(report["errors"], ["timeout"])
                self.assertTrue(process.killed.is_set())
                self.assertLess(report["duration_seconds"], 1)

    def test_output_limits_bound_capture_and_kill(self):
        notifications = b'{"jsonrpc":"2.0","method":"notice"}\n' * 30
        for stream, patch_name, limit, data, capture_limit in (
                ("stdout", "MAX_STDOUT_BYTES", 32, notifications, 32),
                ("stderr", "MAX_STDERR_BYTES", 32, b"x" * 200, 32),
                ("stdout", "MAX_MESSAGES", 2, notifications, 3 * len(notifications) // 30),
                ("stdout", "MAX_LINE_BYTES", 256, b"x" * 300 + b"\n", 257)):
            with self.subTest(limit=patch_name):
                def handler(process, request):
                    getattr(process, stream).feed(data)
                    return True
                with mock.patch.object(client, patch_name, limit):
                    report, _ = self.check_process(FakeProcess(handler=handler))
                self.assertIn("output limit exceeded", report["errors"][0])
                # One whole read may cross a line/message limit; no later data
                # is captured, even when a malicious peer keeps producing bytes.
                self.assertLessEqual(len(report[stream]), capture_limit)

    def test_spawn_failure_and_invalid_inputs_do_not_start_anything_else(self):
        factory = mock.Mock(side_effect=OSError("fake unavailable"))
        report = client.run_check(["FAKE_SERVER"], popen_factory=factory)
        factory.assert_called_once()
        self.assertEqual(report["errors"], ["fake unavailable"])
        self.assertIsNone(report["exit_code"])
        for command, timeout, arguments in (([], 1, {}), (["fake"], 0, {}),
                                             (["fake"], float("nan"), {}),
                                             (["fake"], 1, [])):
            with self.assertRaises(ValueError):
                client.run_check(command, timeout=timeout, tool_arguments=arguments,
                                 popen_factory=factory)
        factory.assert_called_once()

    def test_cli_passes_exact_one_command_and_environment(self):
        argv = ["--env", "APPDATA=C:\\disposable", "--timeout", "12", "--call", "miv_status",
                "--arguments", '{"path":"日本語"}', "--", "C:\\fake.exe", "--mcp",
                "--data-dir", "C:\\disposable\\data"]
        with mock.patch.object(client, "run_check", return_value={"errors": []}) as run:
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(client.main(argv), 0)
        self.assertEqual(json.loads(output.getvalue()), {"errors": []})
        run.assert_called_once_with(["C:\\fake.exe", "--mcp", "--data-dir", "C:\\disposable\\data"],
                                    timeout=12, env_overrides={"APPDATA": "C:\\disposable"},
                                    tool_name="miv_status", tool_arguments={"path": "日本語"})
        self.assertNotEqual(os.environ.get("APPDATA"), "C:\\disposable")

    def test_cli_invalid_json_and_environment_never_spawn(self):
        for options in (["--env", "NO_EQUALS"], ["--arguments", "NaN"],
                        ["--arguments", "[]"], ["--arguments", '{"x":1}']):
            with self.subTest(options=options):
                with mock.patch.object(client, "run_check") as run:
                    with contextlib.redirect_stderr(io.StringIO()):
                        with self.assertRaises(SystemExit) as error:
                            client.main(options + ["--", "FAKE_SERVER"])
                    self.assertEqual(error.exception.code, 2)
                    run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
