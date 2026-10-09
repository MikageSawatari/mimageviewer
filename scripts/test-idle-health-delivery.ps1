# Headless tests only: extract the control helpers without executing the harness.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0
$source = Join-Path $PSScriptRoot "check-idle-health.ps1"
$tokens = $null; $parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($source, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -ne 0) { throw ($parseErrors | Out-String) }
$typeBlock = $ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.IfStatementAst] -and
    $node.Extent.Text.StartsWith('if (-not ("MivLogDelivery1"')
}, $true) | Select-Object -First 1
Invoke-Expression $typeBlock.Extent.Text
foreach ($name in @("Get-DeliveryCheckpoint", "Save-DeliveryPrefix")) {
    $function = $ast.FindAll({ param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst]
    }, $true) | Where-Object { $_.Name -eq $name } | Select-Object -First 1
    Invoke-Expression $function.Extent.Text
}

Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.IO.Pipes;
using System.Text;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Threading.Tasks;
public static class FakeLogControl1 {
    static byte[] Read(Stream pipe, int count) {
        var bytes = new byte[count]; int offset = 0;
        while (offset < count) {
            int read = pipe.Read(bytes, offset, count - offset);
            if (read == 0) throw new EndOfStreamException(); offset += read;
        }
        return bytes;
    }
    public static Task<string> Serve(string name, string response) {
        var security = new PipeSecurity();
        security.AddAccessRule(new PipeAccessRule(new SecurityIdentifier(WellKnownSidType.WorldSid, null), PipeAccessRights.FullControl, AccessControlType.Allow));
        var pipe = new NamedPipeServerStream(name, PipeDirection.InOut, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous, 4096, 4096, security);
        return Task.Run(() => {
            using (pipe) {
                pipe.WaitForConnection();
                int length = BitConverter.ToInt32(Read(pipe, 4), 0);
                string request = Encoding.UTF8.GetString(Read(pipe, length));
                byte[] payload = Encoding.UTF8.GetBytes(response);
                pipe.Write(BitConverter.GetBytes(payload.Length), 0, 4);
                pipe.Write(payload, 0, payload.Length); pipe.Flush();
                return request;
            }
        });
    }
}
'@

$count = 0
function Assert-True { param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
    $script:count++
}
function Assert-Rejected { param([scriptblock]$Action, [string]$Message)
    $rejected = $false
    try { & $Action } catch { $rejected = $true }
    Assert-True $rejected $Message
}
$session = [pscustomobject]@{ log_delivery_schema = 1; session_id = "test-session"; pid = 4294000; process_start_unix_ms = 1234 }
$name = "mimageviewer-log-$($session.pid)-$($session.process_start_unix_ms)"
$ack = [ordered]@{ schema = 1; session_id = "test-session"; pid = 4294000; process_start_unix_ms = 1234; request_id = "test-nonce" }
$server = [FakeLogControl1]::Serve($name, ($ack | ConvertTo-Json -Compress))
$result = Get-DeliveryCheckpoint -Session $session -RequestId "test-nonce"
Assert-True ($server.Wait(3000)) "fake server did not complete"
$request = $server.Result | ConvertFrom-Json
Assert-True ($request.schema -eq 1 -and $request.request_id -eq "test-nonce" -and $result.session_id -eq "test-session") "framing or request/ACK mismatch"

$ack.request_id = "wrong-nonce"
$server = [FakeLogControl1]::Serve($name, ($ack | ConvertTo-Json -Compress))
Assert-Rejected { Get-DeliveryCheckpoint -Session $session -RequestId "test-nonce" } "wrong nonce accepted"
Assert-True ($server.Wait(3000)) "mismatch server did not complete"
$server = [FakeLogControl1]::Serve($name, '{"error":"Busy"}')
Assert-Rejected { Get-DeliveryCheckpoint -Session $session -RequestId "test-nonce" } "Busy accepted"
Assert-True ($server.Wait(3000)) "Busy server did not complete"
Assert-Rejected { Get-DeliveryCheckpoint -Session ([pscustomobject]@{ log_delivery_schema = 0 }) -RequestId "test-nonce" } "legacy session accepted"

$directory = Join-Path (Join-Path $PSScriptRoot "..\target") ("test-idle-delivery-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $directory | Out-Null
try {
    $path = Join-Path $directory "live.jsonl"
    [System.IO.File]::WriteAllText($path, "saved`n", [System.Text.UTF8Encoding]::new($false))
    $file = [System.IO.File]::Open($path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try { $identity = [MivLogDelivery1]::Identity($file.SafeFileHandle) } finally { $file.Dispose() }
    [System.IO.File]::AppendAllText($path, '{"partial":', [System.Text.UTF8Encoding]::new($false))
    $snapshot = Join-Path $directory "prefix.jsonl"
    Save-DeliveryPrefix -Ack ([pscustomobject]@{ perf_path = $path; perf_file_identity = $identity; perf_end_offset = 6 }) -Destination $snapshot
    Assert-True ([System.IO.File]::ReadAllText($snapshot) -eq "saved`n") "snapshot read appended partial tail"
    Assert-Rejected { [MivLogDelivery1]::Snapshot($path, "wrong-file", 6, (Join-Path $directory "wrong.jsonl")) } "wrong identity accepted"
    Assert-Rejected { [MivLogDelivery1]::Snapshot($path, $identity, 9999, (Join-Path $directory "long.jsonl")) } "unwritten prefix accepted"
    foreach ($drops in @(0, 1)) {
        $perf = Join-Path $directory "gate-$drops.jsonl"
        $checkpoint = [ordered]@{
            t = 25.01; cat = "log"; kind = "drain_checkpoint"; schema = 1
            session_id = $session.session_id; pid = $session.pid
            process_start_unix_ms = $session.process_start_unix_ms
            request_id = "test-nonce"; purpose = "measurement"; coverage_t = 25.0
            ticket_cutoff = 3; settled_watermark = 3; written_watermark = 3
            normal_status = "ready"; perf_status = "ready"
            normal_dropped_total = 0; perf_dropped_total = $drops; diagnostic_dropped_total = 0
        }
        $events = @(
            [ordered]@{ t = 0.0; cat = "session"; kind = "start"; log_delivery_schema = 1;
                session_id = $session.session_id; pid = $session.pid; process_start_unix_ms = $session.process_start_unix_ms },
            [ordered]@{ t = 1.0; cat = "frame"; kind = "begin"; n = 1 },
            [ordered]@{ t = 1.0; cat = "ui"; kind = "tail_repaint"; n = 1; action = "none"; reasons = @() },
            $checkpoint
        )
        $lines = ($events | ForEach-Object { $_ | ConvertTo-Json -Compress }) -join "`n"
        [System.IO.File]::WriteAllText($perf, $lines + "`n", [System.Text.UTF8Encoding]::new($false))
        $file = [System.IO.File]::OpenRead($perf)
        try { $fileIdentity = [MivLogDelivery1]::Identity($file.SafeFileHandle) } finally { $file.Dispose() }
        $checkpoint.perf_path = $perf; $checkpoint.perf_file_identity = $fileIdentity
        $checkpoint.perf_end_offset = (Get-Item -LiteralPath $perf).Length
        $server = [FakeLogControl1]::Serve($name, ($checkpoint | ConvertTo-Json -Compress))
        $delivered = Get-DeliveryCheckpoint -Session $session -RequestId "test-nonce"
        Assert-True ($server.Wait(3000)) "gate fake server did not complete"
        $prefix = Join-Path $directory "gate-$drops-prefix.jsonl"
        Save-DeliveryPrefix -Ack $delivered -Destination $prefix
        $ackFile = Join-Path $directory "gate-$drops-ack.json"
        $delivered | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ackFile -Encoding UTF8
        $report = Join-Path $directory "gate-$drops-report.json"
        & python (Join-Path $PSScriptRoot "analyze_perf.py") $prefix idle-health --start-t 10 --end-t 25 `
            --expected-pid $session.pid --allow-sleeping-window --checkpoint-id test-nonce --delivery-ack $ackFile --json-out $report | Out-Null
        $expected = 0; $expectedStatus = "pass"
        if ($drops -gt 0) { $expected = 2; $expectedStatus = "CannotJudge" }
        Assert-True ($LASTEXITCODE -eq $expected) "real analyzer delivery gate returned wrong exit code"
        $result = Get-Content -LiteralPath $report -Encoding UTF8 -Raw | ConvertFrom-Json
        Assert-True ($result.status -eq $expectedStatus) "real analyzer delivery gate returned wrong status"
    }
}
finally {
    $resolved = [System.IO.Path]::GetFullPath($directory)
    $target = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\target")) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($target, [System.StringComparison]::OrdinalIgnoreCase)) { throw "unsafe test cleanup" }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
Write-Host "$count idle logging-control tests passed; no application launched."
