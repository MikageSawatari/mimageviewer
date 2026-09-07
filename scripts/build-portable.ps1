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
#   PS> scripts\build-portable.ps1 -SmokeTestScript (diagnostic package; no dist/zip/sign)

[CmdletBinding()]
param(
    [switch] $SkipBuild,
    [switch] $Sign,
    [switch] $SmokeTestScript
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if ($SmokeTestScript -and $Sign) {
    throw '[portable-smoke-build] -Sign is not supported for diagnostic smoke artifacts'
}

function Get-NormalizedPath {
    param([string] $Path)
    return [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
}

function Assert-ExactPath {
    param(
        [string] $Path,
        [string] $Expected,
        [string] $Label
    )
    $actualFull = Get-NormalizedPath $Path
    $expectedFull = Get-NormalizedPath $Expected
    if (-not $actualFull.Equals($expectedFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "[$Label] refusing unexpected path: $actualFull (expected $expectedFull)"
    }
    return $actualFull
}

function Assert-NoReparseTree {
    param(
        [string] $Path,
        [string] $Label
    )
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $pending = New-Object System.Collections.Stack
    $pending.Push((Get-Item -LiteralPath $Path -Force))
    while ($pending.Count -gt 0) {
        $item = $pending.Pop()
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "[$Label] refusing reparse point: $($item.FullName)"
        }
        if ($item.PSIsContainer) {
            foreach ($child in (Get-ChildItem -LiteralPath $item.FullName -Force)) {
                $pending.Push($child)
            }
        }
    }
}

function Assert-NoReparsePath {
    param(
        [string] $Path,
        [string] $StopAt,
        [string] $Label
    )
    $current = Get-NormalizedPath $Path
    $stop = Get-NormalizedPath $StopAt
    if (-not ($current.Equals($stop, [System.StringComparison]::OrdinalIgnoreCase) -or
        $current.StartsWith(($stop + '\'), [System.StringComparison]::OrdinalIgnoreCase))) {
        throw "[$Label] refusing path outside repository: $current"
    }
    while ($true) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "[$Label] refusing reparse point: $($item.FullName)"
            }
        }
        if ($current.Equals($stop, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = Split-Path -Parent $current
        if (-not $parent -or $parent -eq $current) {
            throw "[$Label] could not reach repository root from $Path"
        }
        $current = Get-NormalizedPath $parent
    }
}

function Get-MivOrdinalUniquePaths {
    param([object[]] $Paths)

    [string[]] $sorted = @($Paths | ForEach-Object { [string] $_ })
    [System.Array]::Sort($sorted, [System.StringComparer]::Ordinal)

    $unique = New-Object 'System.Collections.Generic.List[string]'
    $previous = $null
    $hasPrevious = $false
    foreach ($path in $sorted) {
        if (-not $hasPrevious -or -not [System.StringComparer]::Ordinal.Equals($previous, $path)) {
            $unique.Add($path)
            $previous = $path
            $hasPrevious = $true
        }
    }
    return $unique.ToArray()
}

function Get-MivSourceFingerprintRecords {
    param([string] $Root)

    $sourcePaths = @(
        'Cargo.toml',
        'Cargo.lock',
        'build.rs',
        '.cargo',
        'src',
        'crates',
        'assets',
        'vendor/eframe',
        'vendor/egui-wgpu',
        'vendor/twemoji'
    )
    $relativeFiles = @(& git -C $Root ls-files --cached --others --exclude-standard -- $sourcePaths)
    if ($LASTEXITCODE -ne 0) {
        throw '[portable-smoke-build] failed to enumerate build source files'
    }
    # These build.rs inputs live below ignored vendor roots, so git enumeration
    # cannot see them. Include them explicitly in the source fingerprint.
    $explicitInputs = @(
        (Join-Path $Root 'vendor\ffmpeg\VERSION')
    )
    $explicitInputs += @(Get-ChildItem -LiteralPath (Join-Path $Root 'vendor\twemoji\svg') -Filter '*.svg' -File -ErrorAction SilentlyContinue |
        ForEach-Object { $_.FullName })
    $explicitInputs += @(Get-ChildItem -LiteralPath (Join-Path $Root 'assets\annotation-stamps') -Filter '*.svg' -File -ErrorAction SilentlyContinue |
        ForEach-Object { $_.FullName })
    foreach ($absoluteInput in $explicitInputs) {
        if (Test-Path -LiteralPath $absoluteInput -PathType Leaf) {
            $relativeFiles += (Get-NormalizedPath $absoluteInput).Substring((Get-NormalizedPath $Root).Length + 1).Replace('\', '/')
        }
    }
    $relativeFiles = @(Get-MivOrdinalUniquePaths $relativeFiles)
    if ($relativeFiles.Count -eq 0) {
        throw '[portable-smoke-build] build source file set is empty'
    }

    return @($relativeFiles | ForEach-Object {
        $relative = $_
        $absolute = Join-Path $Root ($relative -replace '/', '\')
        if (-not (Test-Path -LiteralPath $absolute -PathType Leaf)) {
            throw "[portable-smoke-build] source disappeared while fingerprinting: $relative"
        }
        $hash = (Get-FileHash -LiteralPath $absolute -Algorithm SHA256).Hash.ToLowerInvariant()
        "$relative`t$hash"
    })
}

function Get-MivSourceFingerprint {
    param([string] $Root)

    $records = @(Get-MivSourceFingerprintRecords $Root)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes(($records -join "`n"))
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return (($sha.ComputeHash($bytes) | ForEach-Object { $_.ToString('x2') }) -join '')
    }
    finally {
        $sha.Dispose()
    }
}

function Write-SmokeBuildManifest {
    param(
        [string] $Path,
        [string] $SourceFingerprint,
        [string] $CorePath,
        [string] $RemotePath
    )
    $head = (& git -C $repoRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $head) {
        throw '[portable-smoke-build] failed to read git HEAD'
    }
    $manifest = [ordered]@{
        schema_version = 1
        artifact_flavor = 'portable-test-script'
        cargo_profile = 'release'
        features = @('portable', 'test-script')
        source_head = $head
        source_fingerprint_sha256 = $SourceFingerprint
        core_sha256 = (Get-FileHash -LiteralPath $CorePath -Algorithm SHA256).Hash.ToLowerInvariant()
        remote_sha256 = (Get-FileHash -LiteralPath $RemotePath -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $json = $manifest | ConvertTo-Json -Depth 3
    [System.IO.File]::WriteAllText($Path, $json, (New-Object System.Text.UTF8Encoding($false)))
}

function Assert-SmokeBuildManifest {
    param(
        [string] $Path,
        [string] $ExpectedSourceFingerprint,
        [string] $CorePath,
        [string] $RemotePath
    )
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "[portable-smoke-build] diagnostic build manifest not found: $Path"
    }
    try {
        $manifest = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
    }
    catch {
        throw "[portable-smoke-build] invalid diagnostic build manifest: $Path"
    }
    $features = @($manifest.features)
    if ($manifest.schema_version -ne 1 -or
        $manifest.artifact_flavor -ne 'portable-test-script' -or
        $manifest.cargo_profile -ne 'release' -or
        $features.Count -ne 2 -or
        $features[0] -ne 'portable' -or
        $features[1] -ne 'test-script') {
        throw '[portable-smoke-build] manifest does not describe portable,test-script release artifacts'
    }
    if ($manifest.source_fingerprint_sha256 -ne $ExpectedSourceFingerprint) {
        throw '[portable-smoke-build] source changed since the diagnostic artifact was built; rerun without -SkipBuild'
    }
    $coreHash = (Get-FileHash -LiteralPath $CorePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $remoteHash = (Get-FileHash -LiteralPath $RemotePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($manifest.core_sha256 -ne $coreHash -or $manifest.remote_sha256 -ne $remoteHash) {
        throw '[portable-smoke-build] diagnostic artifact hash does not match its build manifest'
    }
}

# Optional code signing (Certum / SimplySign). Assert the certificate up front
# when -Sign is set (SimplySign Desktop must be logged in). See sign-files.ps1.
if ($Sign) {
    . (Join-Path $PSScriptRoot 'sign-files.ps1')
    Assert-MivSignReady
}

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
# Exact names only. "mimageviewer*" also matches Cargo's test harnesses
# (target\debug\deps\mimageviewer-<hash>.exe), which live under the repo root and so pass
# the path check - stopping those killed another session's `cargo test` (backlog 5.3).
$stoppableProcessNames = @(
    'mimageviewer',
    'mimageviewer-core',
    'mimageviewer-remote',
    'mimageviewer-vst3-host',
    'mimageviewer-susie32'
)
if (-not $SmokeTestScript) {
    Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $stoppableProcessNames -contains $_.Name } |
        ForEach-Object {
            $p = $null
            try { $p = $_.Path } catch { $p = $null }
            if ($p -and $p.ToLower().StartsWith($repoPrefix)) {
                Write-Host "[portable] stopping $($_.Name) (PID=$($_.Id))"
                try { Stop-Process -Id $_.Id -Force -ErrorAction Stop } catch {}
            }
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
$portableTargetName = if ($SmokeTestScript) { 'target-portable-test-script' } else { 'target-portable' }
$portableTargetDir = Join-Path $repoRoot $portableTargetName
$coreExe = Join-Path $portableTargetDir 'release\mimageviewer-core.exe'
$remoteExe = Join-Path $portableTargetDir 'release\mimageviewer-remote.exe'
$smokeBuildManifest = Join-Path $portableTargetDir 'release\mimageviewer-core.build-manifest.json'
Assert-NoReparsePath $portableTargetDir $repoRoot 'portable-target'
$sourceFingerprint = $null
if ($SmokeTestScript) {
    $sourceFingerprint = Get-MivSourceFingerprint $repoRoot
}
if (-not $SkipBuild) {
    if ($SmokeTestScript -and (Test-Path -LiteralPath $smokeBuildManifest -PathType Leaf)) {
        Assert-NoReparsePath $smokeBuildManifest $repoRoot 'portable-smoke-build-manifest'
        Remove-Item -LiteralPath $smokeBuildManifest -Force
    }
    Ensure-LibclangPath
    $coreFeatures = if ($SmokeTestScript) { 'portable,test-script' } else { 'portable' }
    $cargoExit = 0
    Push-Location $repoRoot
    try {
        Write-Host "[portable] cargo build --release --bin mimageviewer-core --features $coreFeatures --target-dir $portableTargetName"
        & cargo build --release --bin mimageviewer-core --features $coreFeatures --target-dir $portableTargetDir
        $cargoExit = $LASTEXITCODE
        if ($cargoExit -eq 0) {
            Write-Host "[portable] cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets --target-dir $portableTargetName"
            & cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets --target-dir $portableTargetDir
            $cargoExit = $LASTEXITCODE
        }
    }
    finally {
        Pop-Location
    }
    if ($cargoExit -ne 0) { exit $cargoExit }
    if ($SmokeTestScript) {
        $postBuildFingerprint = Get-MivSourceFingerprint $repoRoot
        if ($postBuildFingerprint -ne $sourceFingerprint) {
            throw '[portable-smoke-build] build sources changed during the diagnostic build; rebuild from a stable tree'
        }
    }
}
if (-not (Test-Path $coreExe)) { throw "[portable] core exe not found: $coreExe" }
if (-not (Test-Path $remoteExe)) { throw "[portable] remote service exe not found: $remoteExe" }
Assert-NoReparsePath $coreExe $repoRoot 'portable-core'
Assert-NoReparsePath $remoteExe $repoRoot 'portable-remote'
if ($SmokeTestScript) {
    if ($SkipBuild) {
        Assert-SmokeBuildManifest $smokeBuildManifest $sourceFingerprint $coreExe $remoteExe
    } else {
        Write-SmokeBuildManifest $smokeBuildManifest $sourceFingerprint $coreExe $remoteExe
    }
}

# ---------------------------------------------------------------------------
# Assemble the distribution folder.
# ---------------------------------------------------------------------------
$pkgName = "mImageViewer_portable_v$version"
$distRoot = Join-Path $repoRoot 'dist'
$pkgDir = if ($SmokeTestScript) {
    Join-Path $repoRoot 'target\portable-smoke-package'
} else {
    Join-Path $distRoot $pkgName
}
$expectedPackagePath = if ($SmokeTestScript) {
    Join-Path $repoRoot 'target\portable-smoke-package'
} else {
    Join-Path $repoRoot "dist\$pkgName"
}
$pkgDir = Assert-ExactPath $pkgDir $expectedPackagePath 'portable-package'
Assert-NoReparsePath (Split-Path -Parent $pkgDir) $repoRoot 'portable-package'
if (Test-Path -LiteralPath $pkgDir) {
    Assert-NoReparseTree $pkgDir 'portable-package'
    Remove-Item -LiteralPath $pkgDir -Recurse -Force
}
New-Item -ItemType Directory -Path $pkgDir | Out-Null
New-Item -ItemType Directory -Path (Join-Path $pkgDir 'models') | Out-Null

# (source relative to repo root, destination relative to pkgDir)
$copies = @(
    @{ src = $coreExe; dst = 'mimageviewer.exe' }
    @{ src = $remoteExe; dst = 'mimageviewer-remote.exe' }
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
    @{ src = 'vendor\egui-wgpu\LICENSE-MIT';         dst = 'egui-LICENSE-MIT.txt' }
    @{ src = 'vendor\egui-wgpu\LICENSE-APACHE';      dst = 'egui-LICENSE-APACHE.txt' }
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
    $src = if ([System.IO.Path]::IsPathRooted($c.src)) { $c.src } else { Join-Path $repoRoot $c.src }
    $dst = Join-Path $pkgDir $c.dst
    if (-not (Test-Path $src)) { $missing += $c.src; continue }
    Assert-NoReparsePath $src $repoRoot 'portable-package-source'
    Copy-Item -LiteralPath $src -Destination $dst -Force
}

if ($SmokeTestScript) {
    Copy-Item -LiteralPath $smokeBuildManifest -Destination (Join-Path $pkgDir '.test-script-build.json') -Force
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
# Sign the loose PE files (portable ships them uncompressed, no embedding), so
# sign the actual package copies here. onnxruntime*.dll are Microsoft-signed and
# NOT re-signed; models/*.onnx and the text files are not PE. vst3-host is not
# bundled in the portable package.
# ---------------------------------------------------------------------------
if ($Sign) {
    $portablePe = @(
        'mimageviewer.exe',
        'mimageviewer-remote.exe',
        'pdfium.dll',
        'mimageviewer-susie32.exe',
        'avcodec-61.dll', 'avformat-61.dll', 'avutil-59.dll',
        'avfilter-10.dll', 'swscale-8.dll', 'swresample-5.dll'
    ) | ForEach-Object { Join-Path $pkgDir $_ }
    Write-Host "[portable] signing portable PE files"
    Invoke-MivSign -Files $portablePe -Verify
}

# ---------------------------------------------------------------------------
# Zip it.
# ---------------------------------------------------------------------------
if ($SmokeTestScript) {
    Assert-NoReparseTree $pkgDir 'portable-smoke-build'
    Write-Host ""
    Write-Host '[portable-smoke-build] DONE'
    Write-Host "  folder: $pkgDir"
    Write-Host "  manifest: $(Join-Path $pkgDir '.test-script-build.json')"
    Write-Output $pkgDir
    exit 0
}

$zipPath = Join-Path $distRoot "$pkgName.zip"
if (Test-Path $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
Assert-NoReparseTree $pkgDir 'portable-package'
Compress-Archive -Path (Join-Path $pkgDir '*') -DestinationPath $zipPath -Force
$zipSizeMb = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)

Write-Host ""
Write-Host "[portable] DONE"
Write-Host "  folder: $pkgDir"
Write-Host "  zip:    $zipPath ($zipSizeMb MB)"
Write-Host ""
Write-Host "Next: extract the zip to a writable location (NOT Program Files) and run mimageviewer.exe."
Write-Host "Verify: data/ is created next to the exe; APPDATA\mimageviewer is untouched."
