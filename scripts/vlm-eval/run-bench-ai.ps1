# Measure GPU memory of the existing AI correction (bench_ai) for comparison
# with the local VLM (docs/local-vlm-v0-procedure.md, V0-3).
# Runs bench_ai once per model and once with all models loaded together,
# sampling GPU memory while each run is alive. DirectML only.
param(
    [Parameter(Mandatory = $true)][string]$BenchExe,
    [Parameter(Mandatory = $true)][string]$Image,
    [Parameter(Mandatory = $true)][string]$OutDir,
    [string[]]$Models = @("realesrgan_x4plus", "realesrgan_anime6b", "realesr_general_v3",
        "nmkd_siax_4x", "realcugan_4x", "denoise_realplksr"),
    [int]$Runs = 5
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Invoke-One([string]$label, [string]$modelList) {
    $dir = Join-Path $OutDir $label
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $base = Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUAdapterMemory |
        Measure-Object -Property DedicatedUsage -Maximum
    "baseline_adapter_dedicated=$($base.Maximum)" | Out-File -FilePath (Join-Path $dir "baseline.txt") -Encoding ascii
    $benchArgs = @("--image", $Image, "--models", $modelList, "--runs", "$Runs", "--warmup", "1",
        "--backend", "directml", "--json", (Join-Path $dir "bench.json"))
    ($benchArgs -join " ") | Out-File -FilePath (Join-Path $dir "args.txt") -Encoding ascii
    $p = Start-Process -FilePath $BenchExe -ArgumentList $benchArgs -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $dir "bench.out.log") `
        -RedirectStandardError (Join-Path $dir "bench.err.log")
    $s = Start-Process -FilePath "powershell.exe" -PassThru -WindowStyle Hidden -ArgumentList @(
        "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $here "gpu-sample.ps1"),
        "-ProcessId", "$($p.Id)", "-OutCsv", (Join-Path $dir "gpu.csv"), "-IntervalMs", "200")
    $p.WaitForExit()
    $s.WaitForExit(10000) | Out-Null
    Write-Output ("{0}: exit={1}" -f $label, $p.ExitCode)
}

foreach ($m in $Models) { Invoke-One $m $m }
Invoke-One "all" ($Models -join ",")
