# Stage FFmpeg beside Cargo's base profile output directory (Cargo 1.100+).
# Cargo and nextest add this directory to the test process DLL search path.
[CmdletBinding()]
param(
    [ValidateNotNullOrEmpty()] [ValidatePattern('^[A-Za-z0-9_-]+$')] [string[]] $CargoProfile = @('test'),
    [string] $RepoRoot = '',
    [string] $TargetDir = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'test-tools-common.ps1')
if (-not $RepoRoot) { $RepoRoot = Join-Path $PSScriptRoot '..' }
$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path
$dllNames = @('avcodec-61.dll', 'avformat-61.dll', 'avutil-59.dll',
    'avfilter-10.dll', 'swscale-8.dll', 'swresample-5.dll')
$sourceDir = Join-Path $RepoRoot 'vendor/ffmpeg/bin'
# Validate all inputs before copying any output; missing inputs are fatal.
foreach ($name in $dllNames) {
    $source = Join-Path $sourceDir $name
    if (-not [IO.File]::Exists($source)) {
        throw "[setup-test-dlls] Missing FFmpeg DLL: $source. Run bash scripts/setup-ffmpeg.sh in $RepoRoot."
    }
}
if (-not $TargetDir) {
    $meta = Invoke-MivTool 'cargo' @('metadata', '--no-deps', '--format-version', '1') $RepoRoot
    if ($meta.ExitCode -ne 0) { throw "[setup-test-dlls] cargo metadata failed: $($meta.StdErr)" }
    $TargetDir = (ConvertFrom-Json $meta.StdOut).target_directory
    if (-not $TargetDir -or -not [IO.Path]::IsPathRooted($TargetDir)) {
        throw '[setup-test-dlls] cargo metadata did not provide an absolute target_directory.'
    }
}
if (-not [IO.Path]::IsPathRooted($TargetDir)) { $TargetDir = Join-Path $RepoRoot $TargetDir }
$TargetDir = [IO.Path]::GetFullPath($TargetDir)
$profileDirs = @($CargoProfile | ForEach-Object { Get-MivCargoProfileDirectory $_ } | Sort-Object -Unique)
foreach ($profileDir in $profileDirs) {
    $destinationDir = Join-Path $TargetDir $profileDir
    [void] [IO.Directory]::CreateDirectory($destinationDir)
    foreach ($name in $dllNames) {
        $source = Join-Path $sourceDir $name
        $destination = Join-Path $destinationDir $name
        if ([IO.File]::Exists($destination) -and
            (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -eq
            (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash) { continue }
        [IO.File]::Copy($source, $destination, $true)
        Write-Host "[setup-test-dlls] copied $name -> $destinationDir"
    }
}
