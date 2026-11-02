# Sample dedicated GPU memory for one process and for the whole adapter.
# Used by the local VLM V0 evaluation (docs/local-vlm-v0-procedure.md).
# Reads the GPU performance counters through CIM (class names are English on
# localized Windows). Stops when the target process exits or StopFile exists.
param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [Parameter(Mandatory = $true)][string]$OutCsv,
    [int]$IntervalMs = 500,
    [string]$StopFile = ""
)

$ErrorActionPreference = "Stop"
$prefix = "pid_" + $ProcessId + "_*"
"time,proc_dedicated,proc_shared,adapter_dedicated,adapter_shared" | Out-File -FilePath $OutCsv -Encoding ascii

while ($true) {
    if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) { break }
    if ($StopFile -ne "" -and (Test-Path $StopFile)) { break }

    $pd = [int64]0
    $ps = [int64]0
    Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory |
        Where-Object { $_.Name -like $prefix } |
        ForEach-Object { $pd += [int64]$_.DedicatedUsage; $ps += [int64]$_.SharedUsage }

    $ad = [int64]0
    $as = [int64]0
    Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUAdapterMemory |
        ForEach-Object {
            if ([int64]$_.DedicatedUsage -gt $ad) { $ad = [int64]$_.DedicatedUsage; $as = [int64]$_.SharedUsage }
        }

    $t = (Get-Date).ToString("o")
    "$t,$pd,$ps,$ad,$as" | Out-File -FilePath $OutCsv -Append -Encoding ascii
    Start-Sleep -Milliseconds $IntervalMs
}
