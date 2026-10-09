"""Deterministic monitor checks; no formal benchmark or application launch."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import unittest

spec = importlib.util.spec_from_file_location("t1_measure", Path(__file__).with_name("t1-measure.py"))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def record(pid=1, birth=100, cpu=0.0, name="cargo.exe"):
    return {"key":(pid,birth), "pid":pid, "name":name, "cpu":cpu}


class CompetitionTests(unittest.TestCase):
    def test_idle_existing_process_is_accepted(self):
        c = m.Competition([record(cpu=100)])
        c.sample([record(cpu=100)], "before")
        c.sample([record(cpu=100)], "during")
        self.assertEqual(c.reasons, [])
        self.assertEqual(c.cpu, {})

    def test_pre_run_cpu_rejects(self):
        c = m.Competition([record(cpu=1)])
        c.sample([record(cpu=1.01)], "before")
        self.assertTrue(any("before:" in r for r in c.reasons))

    def test_new_pid_counts_initial_cpu_and_rejects(self):
        c = m.Competition()
        c.sample([record(cpu=.25)], "during")
        self.assertAlmostEqual(sum(c.cpu.values()), .25)
        self.assertTrue(any("new PID" in r for r in c.reasons))

    def test_new_idle_pid_also_rejects(self):
        c = m.Competition()
        c.sample([record()], "before")
        self.assertTrue(c.reasons)

    def test_exited_process_final_cpu_is_included_and_sticky(self):
        c = m.Competition([record(cpu=1)])
        c.sample([record(cpu=1.1)], "during")
        # A retained handle supplies this final value AFTER PID disappears.
        c.sample([record(cpu=1.3)], "during/final")
        c.sample([], "during/final")
        self.assertAlmostEqual(sum(c.cpu.values()), .3)
        self.assertTrue(c.reasons)

    def test_pid_reuse_is_a_new_identity(self):
        c = m.Competition([record(cpu=10)])
        c.sample([record(birth=101, cpu=.1)], "during")
        self.assertTrue(any("new PID" in r for r in c.reasons))
        self.assertAlmostEqual(sum(c.cpu.values()), .1)

    def test_all_requested_executables_are_watched(self):
        for name in ("rustc.exe","cargo.exe","link.exe","lld-link.exe","cl.exe","codex.exe","codex-x86_64-pc-windows-msvc.exe"):
            self.assertTrue(m.watched(name), name)
        self.assertFalse(m.watched("t1_measure.exe"))
        self.assertFalse(m.watched("python.exe"))
        self.assertFalse(m.watched("codex-windows-sandbox-service.exe"))

    def test_between_samples_start_event_rejects(self):
        monitor = m.Monitor.__new__(m.Monitor)
        monitor.events = m.queue.Queue()
        monitor.events.put("123|lld-link.exe")
        monitor.findings = m.Competition()
        monitor.start_events = []
        monitor.snapshot = lambda: []  # process already terminated
        monitor.trace = type("Trace", (), {"poll":lambda self: None})()
        monitor.step("during")
        self.assertTrue(any("process start PID 123" in r for r in monitor.findings.reasons))
        self.assertEqual(monitor.start_events[0]["pid"], 123)

    def test_trace_failure_is_fail_closed(self):
        monitor = m.Monitor.__new__(m.Monitor)
        monitor.events = m.queue.Queue()
        monitor.events.put("ERROR|access denied")
        monitor.findings = m.Competition()
        monitor.snapshot = lambda: []
        monitor.trace = type("Trace", (), {"poll":lambda self: 1})()
        monitor.step("before")
        self.assertTrue(monitor.findings.reasons)
        def unreadable_stream():
            yield "READY"
            raise UnicodeError("invalid trace encoding")
        monitor.trace = type("Trace", (), {"poll":lambda self: None, "stdout":unreadable_stream()})()
        monitor.read_trace()
        monitor.step("during")
        self.assertTrue(any("reader failed" in r for r in monitor.findings.reasons))

    @unittest.skipUnless(os.name == 'nt', 'Windows retained process handle')
    def test_real_handle_reads_final_cpu_after_exit(self):
        child = subprocess.Popen([sys.executable, '-c', 'import time; end=time.perf_counter()+.5\nwhile time.perf_counter()<end: pass'])
        handle = m.ProcessHandle(child.pid)
        try:
            key, initial = handle.times()
            child.wait(timeout=10)
            same_key, final = handle.times()
            memory = handle.memory()
            self.assertGreater(memory["peak_working_set"], 0)
            self.assertGreater(memory["peak_private_commit"], 0)
            self.assertEqual(key, same_key)
            self.assertGreater(final, initial)
            c = m.Competition([record(pid=key[0],birth=key[1],cpu=initial)])
            c.sample([record(pid=key[0],birth=key[1],cpu=final)], "during/final")
            self.assertAlmostEqual(sum(c.cpu.values()), final-initial)
            self.assertTrue(c.reasons)
        finally:
            handle.close()
            if child.poll() is None:
                child.kill()
                child.wait()


class ReportTests(unittest.TestCase):
    def test_memory_plateau_and_growth_are_distinguished(self):
        stable = [{"working_set":100*m.MIB,"private_commit":100*m.MIB} for _ in range(16)]
        self.assertTrue(m.memory_trend(stable)["private_commit"]["plateau_heuristic"])
        growing = [{"working_set":i*10*m.MIB,"private_commit":i*10*m.MIB} for i in range(16)]
        self.assertFalse(m.memory_trend(growing)["private_commit"]["plateau_heuristic"])

    def test_smoke_and_missing_baseline_do_not_claim_pass(self):
        report = m.assessment([], True)
        self.assertIn("NOT ASSESSED", report["undecorated_regression"])
        self.assertEqual(report["retention"], "NOT ASSESSED")

    def test_matching_reference_compares_noise_and_hash(self):
        meta = {"machine":"test", "font_sha256":"font", "profile":"release", "platform":"Windows",
                "cpu":"test CPU","logical_cpus":4,"rayon_threads":4,"resolution":[1920,1080],
                "matrix_frames":300,"long_frames":9001,"long_outline_range_px":[3,128]}
        def report(means, hash_value="glyph"):
            return {"metadata":meta, "runs":[{"accepted":True,"variant":"none","case":"normal-static-plain","long":False,
                    "result":{"mean_ms":v,"hash":hash_value}} for v in means]}
        comparison = m.compare_reference(report([1.0,1.01,.99]), report([1.0,1.01,.99]))
        self.assertTrue(comparison[0]["within_noise"])
        self.assertFalse(m.compare_reference(report([2.0,2.01,1.99]), report([1.0,1.01,.99]))[0]["within_noise"])
        self.assertFalse(m.compare_reference(report([1,1,1],"changed"), report([1,1,1]))[0]["assessed"])

    def test_mismatching_machine_is_not_compared(self):
        self.assertFalse(m.compare_reference({"metadata":{"machine":"a"}}, {"metadata":{"machine":"b"}})["assessed"])

    def test_reference_survives_incremental_report_writes(self):
        import contextlib
        import io
        from unittest.mock import patch
        report = {"runs":[],"smoke":True,"metadata":{},"reference":{"metadata":{},"runs":[]},
                  "planned_runs":1,"runtime_estimate":"smoke","before_status":"unavailable"}
        with patch.object(Path, 'write_text'), contextlib.redirect_stdout(io.StringIO()):
            m.write_report(report, m.ROOT/'target')
            m.write_report(report, m.ROOT/'target')
        self.assertIn("reference", report)


if __name__ == '__main__':
    unittest.main()
