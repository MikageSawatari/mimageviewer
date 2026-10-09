#!/usr/bin/env python3
"""Windows headless T1 release matrix. No app, video, or settings are opened.

python scripts/musicvideo/t1-measure.py
python scripts/musicvideo/t1-measure.py --smoke --no-build
python -m unittest discover -s scripts/musicvideo -p 'test_*.py' -v

Requires Python 3.10+, psutil and Windows PowerShell/WMI process trace access.
Trace failure is fail-closed: no performance result is accepted. The smoke mode
still renders three diagnostic frames, even if the environment is contested.
"""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import itertools
import json
import math
import os
from pathlib import Path
import platform
import queue
import statistics
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
MIB = 1024 * 1024
LIMITS = {"stable": 24*MIB, "transient_rgba": 8*MIB, "prepared": 32*MIB, "raster": 8*MIB}


def watched(name):
    stem = Path(name).stem.lower()
    # The privileged sandbox broker is infrastructure, not a Codex worker.
    codex = stem == "codex" or (stem.startswith("codex-") and "sandbox" not in stem)
    return stem in {"rustc", "cargo", "link", "lld-link", "cl"} or codex


class Competition:
    """Sticky findings and CPU ledger keyed by (PID, creation FILETIME).

    A new process's first CPU reading is consumption, not its baseline. The
    caller retains process handles and supplies a final reading after exit.
    Start-trace events cover processes born AND gone between polling samples.
    """
    def __init__(self, baseline=()):
        self.last = {r["key"]: r["cpu"] for r in baseline}
        self.cpu = {}
        self.reasons = []

    def reject(self, reason):
        if reason not in self.reasons:
            self.reasons.append(reason)

    def sample(self, records, phase):
        for r in records:
            key = r["key"]
            if key not in self.last:
                self.reject(f'{phase}: new PID {r["pid"]} {r["name"]}')
            delta = r["cpu"] - self.last.get(key, 0.0)
            if delta < 0:
                self.reject(f'{phase}: CPU counter regressed for {key}')
            if delta > 0:
                self.cpu[str(key)] = self.cpu.get(str(key), 0.0) + delta
                self.reject(f'{phase}: PID {r["pid"]} {r["name"]} consumed CPU')
            self.last[key] = r["cpu"]


class ProcessHandle:
    """GetProcessTimes remains available after termination while this is open."""
    def __init__(self, pid):
        self.pid = pid
        self.api = ctypes.WinDLL("kernel32", use_last_error=True)
        self.api.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        self.api.OpenProcess.restype = wintypes.HANDLE
        self.api.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)]*4
        self.api.GetProcessTimes.restype = wintypes.BOOL
        self.api.CloseHandle.argtypes = [wintypes.HANDLE]
        self.api.CloseHandle.restype = wintypes.BOOL
        self.handle = self.api.OpenProcess(0x1000, False, pid)  # QUERY_LIMITED_INFORMATION
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())

    def times(self):
        values = [wintypes.FILETIME() for _ in range(4)]
        if not self.api.GetProcessTimes(self.handle, *(ctypes.byref(v) for v in values)):
            raise ctypes.WinError(ctypes.get_last_error())
        ticks = [(v.dwHighDateTime << 32) | v.dwLowDateTime for v in values]
        return (self.pid, ticks[0]), (ticks[2] + ticks[3]) / 10_000_000

    def close(self):
        if self.handle:
            self.api.CloseHandle(self.handle)
            self.handle = None

    def memory(self):
        class Counters(ctypes.Structure):
            _fields_ = [("cb",wintypes.DWORD),("faults",wintypes.DWORD)] + [
                (name,ctypes.c_size_t) for name in ("peak_ws","ws","peak_paged","paged","peak_nonpaged","nonpaged","commit","peak_commit","private")]
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        api = ctypes.WinDLL("psapi", use_last_error=True).GetProcessMemoryInfo
        api.argtypes = [wintypes.HANDLE,ctypes.POINTER(Counters),wintypes.DWORD]
        api.restype = wintypes.BOOL
        if not api(self.handle, ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return {"working_set":counters.ws,"private_commit":counters.private,
                "peak_working_set":counters.peak_ws,"peak_private_commit":counters.peak_commit}


# Extrinsic start events include the executable name even when OpenProcess is
# already too late. Such a new watched PID rejects the run regardless of CPU.
# Unlike endpoint/snapshot-only polling, a short-lived compiler cannot disappear
# unnoticed. Failure to subscribe/receive READY rejects rather than falling back.
TRACE = r"""
$ErrorActionPreference = 'Stop'
try {
  Add-Type -AssemblyName System.Management
  $source = @'
using System;
using System.Management;
using System.Threading;
public static class T1ProcessTrace {
  public static void Run() {
    using (var watcher = new ManagementEventWatcher("SELECT * FROM Win32_ProcessStartTrace")) {
      watcher.EventArrived += (sender, args) => {
        Console.WriteLine(args.NewEvent["ProcessID"] + "|" + args.NewEvent["ProcessName"]);
        Console.Out.Flush();
      };
      watcher.Stopped += (sender, args) => {
        Console.WriteLine("ERROR|process start trace stopped: " + args.Status);
        Console.Out.Flush();
      };
      // Install the asynchronous handler BEFORE Start. Do not switch to a
      // separate WaitForNextEvent subscription after READY (startup gap).
      watcher.Start();
      Console.WriteLine("READY"); Console.Out.Flush();
      Thread.Sleep(Timeout.Infinite);
    }
  }
}
'@
  Add-Type -TypeDefinition $source -ReferencedAssemblies ([System.Management.ManagementEventWatcher].Assembly.Location)
  [T1ProcessTrace]::Run()
} catch { [Console]::WriteLine('ERROR|' + $_.Exception.Message); exit 1 }
"""


class Monitor:
    def __init__(self, psutil, own_codex=()):
        self.psutil = psutil
        self.owned = set(own_codex)
        self.handles = {}
        self.events = queue.Queue()
        self.findings = Competition()
        self.started_filetime = time.time_ns()//100 + 116444736000000000
        self.start_events = []
        self.trace = subprocess.Popen(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", TRACE],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            creationflags=subprocess.CREATE_NO_WINDOW)
        self.reader = threading.Thread(target=self.read_trace, daemon=True)
        self.reader.start()
        deadline = time.monotonic()+10
        pending = []
        try:
            while True:
                ready = self.events.get(timeout=max(.001,deadline-time.monotonic()))
                if ready == "READY":
                    break
                if ready.startswith("ERROR|"):
                    self.findings.reject(f'process start trace unavailable: {ready}')
                    break
                pending.append(ready)  # notifications can precede READY
                if time.monotonic() >= deadline:
                    raise queue.Empty
        except queue.Empty:
            self.findings.reject('process start trace READY timeout')
        for event in pending:
            self.events.put(event)
        initial = self.snapshot()
        self.findings.last = {r["key"]: r["cpu"] for r in initial if r["key"][1] < self.started_filetime}
        self.step("before")

    def read_trace(self):
        try:
            for line in self.trace.stdout:
                self.events.put(line.strip())
        except Exception as e:
            self.events.put(f'ERROR|process start trace reader failed: {e}')
        finally:
            self.events.put("ERROR|process start trace terminated")

    def snapshot(self):
        for proc in self.psutil.process_iter(["pid", "name"]):
            name = proc.info["name"] or ""
            if not watched(name):
                continue
            # Existing handles also disambiguate PID reuse.
            try:
                handle = ProcessHandle(proc.pid)
                key, _ = handle.times()
                if key in self.owned or key in self.handles:
                    handle.close()
                else:
                    self.handles[key] = (name, handle)
            except OSError as e:
                self.findings.reject(f'cannot retain watched PID {proc.pid} {name}: {e}')
        records = []
        for key, (name, handle) in self.handles.items():
            try:
                identity, cpu = handle.times()  # includes final CPU of exited PID
                assert identity == key
                records.append({"key":key, "pid":key[0], "name":name, "cpu":cpu})
            except OSError as e:
                self.findings.reject(f'cannot read final CPU of {key} {name}: {e}')
        return records

    def step(self, phase):
        while not self.events.empty():
            event = self.events.get_nowait()
            if event.startswith("ERROR|"):
                self.findings.reject(event)
            elif "|" in event:
                pid, name = event.split("|", 1)
                if watched(name):
                    # A start after subscribing is new, including reuse of an
                    # excluded ancestor's PID. Own ancestors were already alive.
                    self.findings.reject(f'{phase}: process start PID {pid} {name}')
                    self.start_events.append({"pid":int(pid),"name":name,"phase":phase})
        self.findings.sample(self.snapshot(), phase)
        if self.trace.poll() is not None:
            self.findings.reject('process start trace exited')

    def close(self):
        if self.trace.poll() is None:
            self.trace.terminate()
        self.trace.wait(timeout=5)
        self.reader.join(timeout=5)
        self.trace.stdout.close()
        for _, handle in self.handles.values():
            handle.close()


def own_codex(psutil, explicit_pid):
    """Exclude only actual Codex ancestors (or an explicitly identified owner)."""
    processes = [p for p in psutil.Process().parents() if watched(p.name()) and Path(p.name()).stem.lower().startswith("codex")]
    if explicit_pid:
        p = psutil.Process(explicit_pid)
        if not Path(p.name()).stem.lower().startswith("codex"):
            raise ValueError('--own-codex-pid is not a Codex process')
        processes.append(p)
    result = []
    for p in processes:
        handle = ProcessHandle(p.pid)
        try:
            result.append(handle.times()[0])
        finally:
            handle.close()
    return result


def memory_trend(samples):
    # Report head/tail plateaus and a trend, not an unprovable leak-free verdict.
    if len(samples) < 8:
        return {"assessment":"insufficient samples (smoke or short run)"}
    tail = samples[len(samples)//2:]
    n = max(1, len(tail)//4)
    result = {}
    for field in ("working_set", "private_commit"):
        early = statistics.median(s[field] for s in tail[:n])
        late = statistics.median(s[field] for s in tail[-n:])
        delta = late - early
        tolerance = max(8*MIB, early*0.05)
        result[field] = {"tail_start_median":early, "tail_end_median":late,
                         "tail_growth_bytes":delta, "plateau_heuristic":delta <= tolerance}
    return result


def run_case(args, exe, variant, case, repeat, long, psutil, owned):
    ident = f'{variant}-{case}-{"long" if long else "short"}-{repeat}'
    row = {"id":ident, "variant":variant, "case":case, "repeat":repeat, "long":long}
    monitor = Monitor(psutil, owned)
    try:
        until = time.monotonic() + (0.2 if args.smoke else args.idle_seconds)
        while time.monotonic() < until:
            monitor.step("before")
            time.sleep(0.02)
        frames = 3 if args.smoke else (9001 if long else 300)
        path = args.output / (ident + ".jsonl")
        memory = []
        if not monitor.findings.reasons or args.smoke:
            env = os.environ.copy()
            env.update(T1_FONT=str(args.font), RAYON_NUM_THREADS=str(args.rayon_threads),
                       T1_FRAMES=str(frames), T1_LONG="1" if long else "0")
            start = time.monotonic()
            with path.open("w", encoding="utf-8") as out:
                p = subprocess.Popen([str(exe), case, "none" if variant in ("none","before-none") else "current"],
                                     cwd=ROOT, stdout=out, stderr=subprocess.STDOUT, env=env)
                handle = ProcessHandle(p.pid)
                try:
                    while p.poll() is None:
                        monitor.step("during")
                        try:
                            memory.append({"elapsed_secs":time.monotonic()-start, **handle.memory()})
                        except OSError as e:
                            monitor.findings.reject(f'cannot read measurement memory: {e}')
                        time.sleep(0.02)
                    # Query the retained handle after exit for the final OS
                    # high-water values, not just the last polling sample.
                    try:
                        final_memory = handle.memory()
                    except OSError as e:
                        final_memory = None
                        monitor.findings.reject(f'cannot read final memory peaks: {e}')
                finally:
                    handle.close()
                    if p.poll() is None:
                        p.terminate()
                        p.wait()
            row["exit_code"] = p.returncode
            row["wall_secs"] = time.monotonic()-start
            telemetry = []
            for line in path.read_text(encoding="utf-8").splitlines():
                try:
                    telemetry.append(json.loads(line))
                except json.JSONDecodeError:
                    pass  # panic/build text is retained in the raw log
            summaries = [r for r in telemetry if r.get("type") == "result"]
            if p.returncode or len(summaries) != 1:
                monitor.findings.reject('harness failed or missing result')
            else:
                row["result"] = summaries[0]
                expected_variant = "none" if variant in ("none","before-none") else "current"
                if (row["result"].get("case"),row["result"].get("variant"),row["result"].get("frames"),row["result"].get("long")) != (case,expected_variant,frames,long):
                    monitor.findings.reject('harness scenario/variant/frame count does not match request')
                row["cache_series"] = [r for r in telemetry if r.get("type") == "sample"]
                if not memory:
                    monitor.findings.reject('no OS memory samples')
            row["memory_series"] = memory
            row["memory_trend"] = memory_trend(memory) if long and not args.smoke else {"assessment":"long-span retention verdict not assessed for short/smoke run"}
            row["peak_working_set"] = max((s["peak_working_set"] for s in memory), default=0)
            row["peak_private_commit"] = max((s["peak_private_commit"] for s in memory), default=0)
            if final_memory:
                row["final_os_memory"] = final_memory
                for field in ("peak_working_set","peak_private_commit"):
                    row[field] = max(row[field], final_memory[field])
        # Drain asynchronous start notifications; retained handles still supply
        # final CPU after processes exit. The extra window is conservative.
        until = time.monotonic()+0.5
        while time.monotonic() < until:
            monitor.step("during/final")
            time.sleep(0.02)
        row["watched_cpu_seconds"] = monitor.findings.cpu
        row["process_start_events"] = monitor.start_events
        for event in row["process_start_events"]:
            event["cpu_available"] = any(key[0] == event["pid"] and key[1] >= monitor.started_filetime for key in monitor.handles)
            if not event["cpu_available"]:
                monitor.findings.reject(f'PID {event["pid"]} {event["name"]} started/exited before a handle could be retained; CPU unavailable, run rejected')
        row["reasons"] = monitor.findings.reasons
        row["competition_free"] = not row["reasons"]
        row["accepted"] = not row["reasons"] and "result" in row and not args.smoke
        row["diagnostic_only"] = args.smoke
        return row
    finally:
        monitor.close()


def assessment(rows, smoke=False):
    valid = [r for r in rows if r.get("accepted")]
    result = {"undecorated_regression":"NOT ASSESSED: requires compatible before executable or reference JSON",
              "normal_p95":"NOT ASSESSED", "retention":"NOT ASSESSED", "per_bake_rgba":"NOT ASSESSED",
              "tile_over_8_3ms":[], "noise":{}, "note":"heuristics only; tile design decision belongs to owner"}
    if smoke or not valid:
        return result
    for variant, case in sorted({(r["variant"], r["case"]) for r in valid if not r["long"]}):
        group = [r["result"]["mean_ms"] for r in valid if (r["variant"], r["case"]) == (variant, case) and not r["long"]]
        result["noise"][f'{variant}/{case}'] = {"runs":len(group), "mean_ms":statistics.mean(group), "range_ms":max(group)-min(group)}
    target = [r for r in valid if r["variant"] in ("none","current")]
    normal = [r["result"]["p95_ms"] for r in target if r["case"].startswith("normal")]
    # 80% of the 60fps frame budget provides an explicit headroom heuristic.
    result["normal_p95"] = {"limit_ms":16.7, "headroom_target_ms":13.36, "worst_ms":max(normal, default=0),
                            "comfortably_below":bool(normal) and max(normal) < 13.36}
    result["retention"] = bool(target) and all(all(r["result"][f'max_{lane}_bytes'] <= LIMITS[lane] for lane in ("stable", "transient_rgba", "prepared")) for r in target)
    result["per_bake_rgba"] = bool(target) and all(r["result"]["max_raster_bytes"] <= LIMITS["raster"] for r in target)
    result["tile_over_8_3ms"] = [{"id":r["id"], "mean_ms":r["result"]["mean_ms"], "p95_ms":r["result"]["p95_ms"]} for r in valid if r["case"].startswith("tile") and r["result"]["p95_ms"] > 8.3]
    return result


def compare_reference(report, reference):
    fields = ("machine", "platform", "cpu", "logical_cpus", "profile", "font_sha256", "rayon_threads", "resolution", "matrix_frames", "long_frames", "long_outline_range_px")
    if any(report["metadata"].get(k) is None or reference["metadata"].get(k) is None or report["metadata"][k] != reference["metadata"][k] for k in fields):
        return {"assessed":False, "reason":"reference machine/font/Rayon/scenario settings differ"}
    comparisons = []
    for case in sorted({r["case"] for r in report["runs"] if r["variant"] == "none" and not r["long"]}):
        def group(data):
            return [r for r in data["runs"] if r.get("accepted") and r["variant"] == "none" and r["case"] == case and not r["long"]]
        current, old = group(report), group(reference)
        if len(current) != 3 or len(old) != 3:
            comparisons.append({"case":case, "assessed":False, "reason":"three accepted runs required on each side"})
            continue
        if {r["result"]["hash"] for r in current} != {r["result"]["hash"] for r in old}:
            comparisons.append({"case":case, "assessed":False, "reason":"undecorated RGBA hash changed"})
            continue
        def values(rows): return [r["result"]["mean_ms"] for r in rows]
        a, b = values(current), values(old)
        # Noise floor includes sample range and 2 standard errors of each mean.
        noise = max(max(a)-min(a), max(b)-min(b), 2*math.sqrt(statistics.variance(a)/3+statistics.variance(b)/3))
        delta = statistics.mean(a)-statistics.mean(b)
        comparisons.append({"case":case, "assessed":True, "delta_ms":delta, "noise_ms":noise, "within_noise":delta <= noise})
    return comparisons


def write_report(report, output):
    report["assessment"] = assessment(report["runs"], report["smoke"])
    report["assessment"]["coverage"] = f'{len(report["runs"])} / {report["planned_runs"]} runs; partial output is not a final acceptance'
    if report.get("reference"):
        report["assessment"]["undecorated_regression"] = compare_reference(report, report["reference"])
    elif any(r["variant"] == "before-none" for r in report["runs"]) and not report["smoke"]:
        reference = {"metadata":report["metadata"], "runs":[{**r,"variant":"none"} for r in report["runs"] if r["variant"] == "before-none"]}
        report["assessment"]["undecorated_regression"] = compare_reference(report, reference)
    (output / "results.json").write_text(json.dumps({k:v for k,v in report.items() if k != "reference"}, indent=2, ensure_ascii=False), encoding="utf-8")
    lines = ["T1 headless release measurement" + (" — SMOKE ONLY, NOT PERFORMANCE EVIDENCE" if report["smoke"] else ""),
             report["runtime_estimate"], report["before_status"],
             "id | accepted / uncontended | mean ms | p95 ms | cold prep / first frame ms | peak WS / commit MiB | stable / interp / prepared / bake MiB"]
    for row in report["runs"]:
        r = row.get("result", {})
        f = lambda k: f'{r[k]:.3f}' if k in r else '-'
        lanes = ' / '.join(f'{r.get("max_"+k+"_bytes",0)/MIB:.2f}' for k in ("stable", "transient_rgba", "prepared", "raster"))
        lines.append(f'{row["id"]} | {row["accepted"]} / {row["competition_free"]} | {f("mean_ms")} | {f("p95_ms")} | {f("initial_preparation_ms")} / {f("first_frame_ms")} | {row.get("peak_working_set",0)/MIB:.2f} / {row.get("peak_private_commit",0)/MIB:.2f} | {lanes}')
        lines.extend('REJECT: '+reason for reason in row["reasons"])
        if row.get("memory_trend"):
            lines.append('memory trend: '+json.dumps(row["memory_trend"]))
    lines += ["Criteria: none regression within three-run noise (matching reference required); normal p95 <13.36ms headroom target (<16.7ms 60fps); retention <=24+8+32MiB; each bake RGBA <=8MiB; tile p95 >8.3ms reported for owner decision.",
              json.dumps(report["assessment"], indent=2, ensure_ascii=False)]
    table = '\n'.join(lines)
    (output / "results.txt").write_text(table+'\n', encoding="utf-8")
    print(table, flush=True)


def main():
    # Windows redirected stdout commonly defaults to cp932; JSON/text files are
    # UTF-8 and the console must also print every rejection without encoding loss.
    sys.stdout.reconfigure(encoding='utf-8')
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--smoke', action='store_true', help='one case, three diagnostic frames; never accepted as performance evidence')
    parser.add_argument('--no-build', action='store_true')
    parser.add_argument('--font', type=Path, default=Path(r'C:\Windows\Fonts\YuGothM.ttc'))
    parser.add_argument('--rayon-threads', type=int, default=4)
    parser.add_argument('--idle-seconds', type=float, default=5)
    parser.add_argument('--own-codex-pid', type=int)
    parser.add_argument('--before-exe', type=Path, help='optional release harness with identical JSON/scenario protocol and old renderer')
    parser.add_argument('--reference-json', type=Path, help='matching old undecorated measurement for regression/noise comparison')
    parser.add_argument('--output', type=Path, default=ROOT/'target'/'t1-measure')
    args = parser.parse_args()
    if os.name != 'nt':
        parser.error('Windows required for process trace, exit CPU and private commit')
    if args.rayon_threads < 1 or args.idle_seconds <= 0:
        parser.error('positive Rayon count and pre-monitor interval required')
    try:
        import psutil
    except ImportError:
        parser.error('psutil required: python -m pip install psutil')
    args.font = args.font.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=True)
    exe = ROOT/'target'/'release'/'examples'/'t1_measure.exe'
    if not args.no_build:
        env = os.environ.copy()
        env['MSBUILDDISABLENODEREUSE'] = '1'
        subprocess.run(['cargo','build','--locked','--release','-p','musicvideo-core','--features','text-measure','--example','t1_measure'], cwd=ROOT, env=env, check=True)
    exe = exe.resolve(strict=True)
    owned = own_codex(psutil, args.own_codex_pid)
    variants = ['none','current']
    if args.before_exe:
        args.before_exe = args.before_exe.resolve(strict=True)
        variants.extend(['before-none','before'])
    cases = [(f'{kind}-{lane}-{effect}', False) for kind,lane,effect in itertools.product(('normal','tile'),('static','interp'),('plain','glow','rotate','both'))]
    cases += [(f'{kind}-{lane}-both', True) for kind,lane in itertools.product(('normal','tile'),('interp','mixed'))]
    jobs = list(itertools.product(cases, variants, range(1,4)))
    if args.smoke:
        jobs = [(('normal-static-plain',False), 'current',1)]
    frame_count = sum(9001 if item[0][1] else 300 for item in jobs) if not args.smoke else 3
    estimate = (f'{len(jobs)} processes, {frame_count:,} measured frames +31 cold/warm frames each. '
                f'At mean 5/20/100ms: ~{frame_count*.005/60:.0f}/{frame_count*.020/60:.0f}/{frame_count*.100/60:.0f} minutes '
                f'+ ~{len(jobs)*(args.idle_seconds+.5)/60:.0f} minutes monitoring and release build. '
                'Tile/128px outline may be much slower; no fixed completion guarantee.')
    metadata = {'machine':platform.node(), 'platform':platform.platform(), 'cpu':platform.processor(),
                'logical_cpus':os.cpu_count(), 'font':str(args.font), 'font_sha256':hashlib.sha256(args.font.read_bytes()).hexdigest(),
                'rayon_threads':args.rayon_threads, 'resolution':[1920,1080], 'matrix_frames':300, 'long_frames':9001,
                'profile':'release', 'fps':30, 'long_outline_range_px':[3,128], 'warm_frames':30, 'poll_seconds':.02,
                'own_codex_identities':owned, 'exe_sha256':hashlib.sha256(exe.read_bytes()).hexdigest(),
                'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                'git_diff_sha256':hashlib.sha256(subprocess.check_output(['git','-c','core.safecrlf=false','diff','HEAD'],cwd=ROOT,stderr=subprocess.DEVNULL)).hexdigest(),
                'driver_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    report = {'metadata':metadata,'smoke':args.smoke,'runtime_estimate':estimate,'runs':[], 'planned_runs':len(jobs),
              'before_status':'before fce65bb15: NOT MEASURED (old full-canvas tile path can allocate >1GiB; compatible old JSON harness not supplied). --before-exe can supply a separately prepared equivalent; never silently substitute current.',
              'limits':LIMITS,
              'plan':[{'case':case,'variant':variant,'repeat':repeat,'long':long,'frames':3 if args.smoke else (9001 if long else 300)} for (case,long),variant,repeat in jobs]}
    if args.before_exe:
        report['before_status'] = f'before renderer supplied: {args.before_exe}; equivalence/source revision must be recorded by supplier'
        metadata['before_exe_sha256'] = hashlib.sha256(args.before_exe.read_bytes()).hexdigest()
    if args.reference_json:
        report['reference'] = json.loads(args.reference_json.read_text(encoding='utf-8'))
    print(estimate, flush=True)
    failed = False
    for (case,long), variant, repeat in jobs:
        row = run_case(args, args.before_exe if variant.startswith('before') else exe, variant, case, repeat, long, psutil, owned)
        report['runs'].append(row)
        write_report(report, args.output)
        if row['reasons'] and not args.smoke:
            print('Stopped: rejected run; resume by rerunning in an idle environment.', flush=True)
            failed = True
            break
    return 2 if failed else 0


if __name__ == '__main__':
    raise SystemExit(main())
