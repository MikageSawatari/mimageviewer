# Run the remaining V0 conditions in sequence (docs/local-vlm-v0-procedure.md).
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = "G:\home\vlm-eval"
$vk = "$root\bin\llama-b12032-vulkan\app\llama-server.exe"
$cu = "$root\bin\llama-b12032-cuda\app\llama-server.exe"
$q4 = "$root\models\unsloth__Qwen3.5-4B-GGUF"
$q9 = "$root\models\unsloth__Qwen3.5-9B-GGUF"
$runs = @(
    @{ L = "C5"; S = $vk; M = "$q4\Qwen3.5-4B-Q4_K_M.gguf"; P = "$q4\mmproj-F16.gguf"; I = "$root\inputs\768"; D = "Vulkan0" },
    @{ L = "C1r"; S = $vk; M = "$q4\Qwen3.5-4B-Q4_K_M.gguf"; P = "$q4\mmproj-F16.gguf"; I = "$root\inputs\1024"; D = "Vulkan0" },
    @{ L = "C6a"; S = $cu; M = "$q4\Qwen3.5-4B-Q4_K_M.gguf"; P = "$q4\mmproj-F16.gguf"; I = "$root\inputs\1024"; D = "CUDA0" },
    @{ L = "C6b"; S = $cu; M = "$q9\Qwen3.5-9B-Q4_K_M.gguf"; P = "$q9\mmproj-F16.gguf"; I = "$root\inputs\1024"; D = "CUDA0" }
)
foreach ($r in $runs) {
    Write-Output "=== $($r.L)"
    & (Join-Path $here "run-condition.ps1") -ServerExe $r.S -Model $r.M -Mmproj $r.P -Label $r.L `
        -Inputs $r.I -OutDir "$root\runs\20261102-$($r.L)" -Device $r.D |
        Select-Object -Last 1
}
