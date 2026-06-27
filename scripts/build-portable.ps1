# mImageViewer portable (loose-deps) build + package script.
#
# Produces a self-contained portable folder + zip that runs by extracting and
# double-clicking, with NO launcher, NO include_bytes extraction, and NO APPDATA
# usage (data goes to <exe_dir>\data). See docs/portable-build-plan.md.
#
# Output:
#   dist\mImageViewer_portable_v<VERSION>\        (loose folder)
#   dist\mImageViewer_portable_v<VERSION>.zip     (distributable)
#
# Usage:
#   PS> scripts\build-portable.ps1
#   PS> scripts\build-portable.ps1 -SkipBuild      (re-assemble only, reuse last core build)

[CmdletBinding()]
param(
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Get-Location).Path

# ---------------------------------------------------------------------------
# LIBCLANG_PATH (ffmpeg-sys-the-third bindgen). Mirror of build-release.ps1.
# ---------------------------------------------------------------------------
function Ensure-LibclangPath {
    if ($env:LIBCLANG_PATH -and (Test-Path (Join-Path $env:LIBCLANG_PATH 'libclang.dll'))) { return }
    $candidates = @(
        'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin',
        'C:\Program Files (x86)\Microsoft Visual Studio\17\BuildTools\VC\Tools\Llvm\x64\bin',
        'C:\Program Files\LLVM\bin'
    )
    foreach ($dir in $candidates) {
        if (Test-Path (Join-Path $dir 'libclang.dll')) {
            $env:LIBCLANG_PATH = $dir
            Write-Host "[portable] using LIBCLANG_PATH=$dir"
            return
        }
    }
}

# ---------------------------------------------------------------------------
# Read package version from Cargo.toml (first `version = "x"` under [package]).
# ---------------------------------------------------------------------------
$cargoToml = Get-Content (Join-Path $repoRoot 'Cargo.toml')
$version = $null
foreach ($line in $cargoToml) {
    if ($line -match '^version\s*=\s*"([^"]+)"') { $version = $Matches[1]; break }
}
if (-not $version) { throw "[portable] could not parse version from Cargo.toml" }
Write-Host "[portable] version = $version"

# ---------------------------------------------------------------------------
# Stop any running core/portable instances that may lock the exe.
# Only repo-built ones (path under repo root) are touched.
# ---------------------------------------------------------------------------
$repoPrefix = ($repoRoot.TrimEnd('\') + '\').ToLower()
Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like 'mimageviewer*' } |
    ForEach-Object {
        $p = $null
        try { $p = $_.Path } catch { $p = $null }
        if ($p -and $p.ToLower().StartsWith($repoPrefix)) {
            Write-Host "[portable] stopping $($_.Name) (PID=$($_.Id))"
            try { Stop-Process -Id $_.Id -Force -ErrorAction Stop } catch {}
        }
    }

# ---------------------------------------------------------------------------
# Build the portable core (no launcher; native deps NOT embedded).
#
# Build into a SEPARATE target dir (target-portable) so the portable core never
# overwrites the non-portable target\release\mimageviewer-core.exe. Sharing one
# output path let cargo hand back a stale core of the other feature flavor
# (0.5s "Finished", no Compiling line). target-* is already gitignored.
# ---------------------------------------------------------------------------
$portableTargetDir = Join-Path $repoRoot 'target-portable'
$coreExe = Join-Path $portableTargetDir 'release\mimageviewer-core.exe'
if (-not $SkipBuild) {
    Ensure-LibclangPath
    Write-Host "[portable] cargo build --release --bin mimageviewer-core --features portable --target-dir target-portable"
    & cargo build --release --bin mimageviewer-core --features portable --target-dir $portableTargetDir
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
if (-not (Test-Path $coreExe)) { throw "[portable] core exe not found: $coreExe" }

# ---------------------------------------------------------------------------
# Assemble the distribution folder.
# ---------------------------------------------------------------------------
$distRoot = Join-Path $repoRoot 'dist'
$pkgName = "mImageViewer_portable_v$version"
$pkgDir = Join-Path $distRoot $pkgName
if (Test-Path $pkgDir) { Remove-Item -LiteralPath $pkgDir -Recurse -Force }
New-Item -ItemType Directory -Path $pkgDir | Out-Null
New-Item -ItemType Directory -Path (Join-Path $pkgDir 'models') | Out-Null

# (source relative to repo root, destination relative to pkgDir)
$copies = @(
    @{ src = 'target-portable\release\mimageviewer-core.exe'; dst = 'mimageviewer.exe' }
    @{ src = 'vendor\ffmpeg\bin\avcodec-61.dll';     dst = 'avcodec-61.dll' }
    @{ src = 'vendor\ffmpeg\bin\avformat-61.dll';    dst = 'avformat-61.dll' }
    @{ src = 'vendor\ffmpeg\bin\avutil-59.dll';      dst = 'avutil-59.dll' }
    @{ src = 'vendor\ffmpeg\bin\avfilter-10.dll';    dst = 'avfilter-10.dll' }
    @{ src = 'vendor\ffmpeg\bin\swscale-8.dll';      dst = 'swscale-8.dll' }
    @{ src = 'vendor\ffmpeg\bin\swresample-5.dll';   dst = 'swresample-5.dll' }
    @{ src = 'vendor\pdfium\bin\pdfium.dll';         dst = 'pdfium.dll' }
    @{ src = 'vendor\ort\onnxruntime.dll';           dst = 'onnxruntime.dll' }
    @{ src = 'vendor\ort\onnxruntime_providers_shared.dll'; dst = 'onnxruntime_providers_shared.dll' }
    @{ src = 'vendor\susie-worker\mimageviewer-susie32.exe'; dst = 'mimageviewer-susie32.exe' }
    # NOTE: mimageviewer-vst3-host.exe is intentionally NOT bundled. The unsigned
    # bridge exe is false-flagged by some security software, which blocked the
    # portable zip download (v2.0.0). Without the host, src/video/dsp/vst3_supported()
    # returns false and the app auto-disables VST3 (it cannot be turned on in settings).
    # Permanent fix = code-sign the bridge exe, then restore this line to re-bundle it.
    # (Keep this file ASCII-only: PowerShell 5.1 reads BOM-less .ps1 as the system
    #  ANSI codepage, and a CP932-misdecoded Japanese comment once silently swallowed
    #  the LICENSE-ffmpeg.txt entry below via comment line-continuation. See the
    #  CLAUDE.md encoding policy: .ps1 = ASCII only.)
    @{ src = 'vendor\ffmpeg\LICENSE.txt';            dst = 'LICENSE-ffmpeg.txt' }
    @{ src = 'UNRAR-LICENSE.txt';                     dst = 'UNRAR-LICENSE.txt' }
    @{ src = 'installer\readme_portable.txt';        dst = 'readme.txt' }
)

# AI models loaded at runtime (must match EMBEDDED_MODELS in src/ai/model_manager.rs).
$models = @(
    'realesrgan_x4plus.onnx',
    'realesrgan_x4plus_anime_6b.onnx',
    'realesr_general_x4v3.onnx',
    'realcugan_4x_conservative.onnx',
    '4x_NMKD-Siax_200k.onnx',
    'dejpg_realplksr_otf.onnx',
    'migan.onnx'
)
foreach ($m in $models) {
    $copies += @{ src = "vendor\models\$m"; dst = "models\$m" }
}

$missing = @()
foreach ($c in $copies) {
    $src = Join-Path $repoRoot $c.src
    $dst = Join-Path $pkgDir $c.dst
    if (-not (Test-Path $src)) { $missing += $c.src; continue }
    Copy-Item -LiteralPath $src -Destination $dst -Force
}

if ($missing.Count -gt 0) {
    Write-Warning "[portable] MISSING source files (package incomplete):"
    foreach ($m in $missing) { Write-Warning "  - $m" }
    throw "[portable] aborting: $($missing.Count) required file(s) missing. Run scripts/bootstrap-vendor.sh and restore models."
}

# ---------------------------------------------------------------------------
# Verify every expected destination file exists (catch silent copy failures).
# ---------------------------------------------------------------------------
$expected = $copies | ForEach-Object { Join-Path $pkgDir $_.dst }
$absent = $expected | Where-Object { -not (Test-Path $_) }
if ($absent) {
    Write-Warning "[portable] destination files missing after copy:"
    foreach ($a in $absent) { Write-Warning "  - $a" }
    throw "[portable] packaging incomplete."
}

$exeSizeMb = [math]::Round((Get-Item $coreExe).Length / 1MB, 1)
Write-Host "[portable] portable core exe size: $exeSizeMb MB (embedded native deps removed)"

# ---------------------------------------------------------------------------
# Zip it.
# ---------------------------------------------------------------------------
$zipPath = Join-Path $distRoot "$pkgName.zip"
if (Test-Path $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
Compress-Archive -Path (Join-Path $pkgDir '*') -DestinationPath $zipPath -Force
$zipSizeMb = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)

Write-Host ""
Write-Host "[portable] DONE"
Write-Host "  folder: $pkgDir"
Write-Host "  zip:    $zipPath ($zipSizeMb MB)"
Write-Host ""
Write-Host "Next: extract the zip to a writable location (NOT Program Files) and run mimageviewer.exe."
Write-Host "Verify: data/ is created next to the exe; APPDATA\mimageviewer is untouched."
