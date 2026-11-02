# Run one V0 condition: start llama-server, sample GPU memory, caption every
# input image, then stop the server.
# Used by the local VLM V0 evaluation (docs/local-vlm-v0-procedure.md).
# The server listens on 127.0.0.1 only and only while this script runs.
param(
    [Parameter(Mandatory = $true)][string]$ServerExe,
    [Parameter(Mandatory = $true)][string]$Model,
    [Parameter(Mandatory = $true)][string]$Mmproj,
    [Parameter(Mandatory = $true)][string]$Label,
    [Parameter(Mandatory = $true)][string]$Inputs,
    [Parameter(Mandatory = $true)][string]$OutDir,
    [string]$Device = "",
    [int]$ImageMaxTokens = 0,
    [int]$CtxSize = 8192,
    [string[]]$Extra = @()
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$port = Get-Random -Minimum 20000 -Maximum 40000

$srvArgs = @("-m", $Model, "--mmproj", $Mmproj, "-ngl", "999", "-c", "$CtxSize", "-np", "1",
    "--host", "127.0.0.1", "--port", "$port", "--no-webui", "--reasoning", "off")
if ($Device -ne "") { $srvArgs += @("--device", $Device) }
if ($Extra.Count -gt 0) { $srvArgs += $Extra }
if ($ImageMaxTokens -gt 0) { $srvArgs += @("--image-max-tokens", "$ImageMaxTokens") }

# Baseline adapter usage before the model is loaded.
$base = Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUAdapterMemory |
    Measure-Object -Property DedicatedUsage -Maximum
"baseline_adapter_dedicated=$($base.Maximum)" | Out-File -FilePath (Join-Path $OutDir "baseline.txt") -Encoding ascii
($srvArgs -join " ") | Out-File -FilePath (Join-Path $OutDir "server-args.txt") -Encoding ascii

$t0 = Get-Date
$srv = Start-Process -FilePath $ServerExe -ArgumentList $srvArgs -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $OutDir "server.out.log") `
    -RedirectStandardError (Join-Path $OutDir "server.err.log")
$sampler = Start-Process -FilePath "powershell.exe" -PassThru -WindowStyle Hidden -ArgumentList @(
    "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $here "gpu-sample.ps1"),
    "-ProcessId", "$($srv.Id)", "-OutCsv", (Join-Path $OutDir "gpu.csv"))

try {
    $ready = $false
    for ($i = 0; $i -lt 600; $i++) {
        if ($srv.HasExited) { throw "llama-server exited early (code $($srv.ExitCode))" }
        try {
            $h = Invoke-RestMethod -Uri "http://127.0.0.1:$port/health" -TimeoutSec 2
            if ($h.status -eq "ok") { $ready = $true; break }
        } catch { }
        Start-Sleep -Milliseconds 500
    }
    if (-not $ready) { throw "llama-server did not become ready" }
    $loadSec = ((Get-Date) - $t0).TotalSeconds
    "load_seconds=$([Math]::Round($loadSec, 1))" | Out-File -FilePath (Join-Path $OutDir "load.txt") -Encoding ascii

    & python -I (Join-Path $here "run_eval.py") --server "http://127.0.0.1:$port" --inputs $Inputs `
        --out (Join-Path $OutDir "results.jsonl") --label $Label 2>&1 |
        Tee-Object -FilePath (Join-Path $OutDir "eval.log")
} finally {
    if (-not $srv.HasExited) { Stop-Process -Id $srv.Id -Force }
    $sampler.WaitForExit(10000) | Out-Null
}
