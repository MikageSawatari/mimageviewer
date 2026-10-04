# Fixture-only CRT-preflight tests. Never loads a DLL or launches a real host.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'vst3-host-vcrt.ps1')
. (Join-Path $PSScriptRoot 'sign-files.ps1')
function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "[vst3-vcrt-test] $Message" }
}
function Assert-Throws([scriptblock] $Action, [string] $Pattern) {
    try { & $Action } catch {
        Assert-True ($_.Exception.Message -match $Pattern) "Unexpected failure: $_"
        Assert-True ($_.Exception.Message.Contains('cmake --build')) 'Missing actionable recovery'
        return
    }
    throw "[vst3-vcrt-test] Expected rejection: $Pattern"
}
function Move-FixtureDirectory([string] $From, [string] $To, [string] $FixtureRoot) {
    $prefix = [System.IO.Path]::GetFullPath($FixtureRoot).TrimEnd('\') + '\'
    foreach ($path in @($From, $To)) {
        Assert-True ([System.IO.Path]::GetFullPath($path).StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) 'Move escaped fixture root'
    }
    $null = @(Get-MivTreeFiles -Path $From)
    Move-Item -LiteralPath $From -Destination $To
}
$temp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
$root = Join-Path $temp ('miv-vst3-vcrt-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $source = Join-Path $root 'vendor\vcrt'
    $stage = Join-Path $root 'vendor\vst3-host\vcrt'
    New-Item -ItemType Directory -Path $source, $stage -Force | Out-Null
    foreach ($name in @('msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
        [System.IO.File]::WriteAllText((Join-Path $source $name), "fixture $name")
        Copy-Item -LiteralPath (Join-Path $source $name) -Destination (Join-Path $stage $name)
    }
    Assert-MivVst3HostVcrt -RepoRoot $root
    $file = Join-Path $stage 'msvcp140.dll'
    [System.IO.File]::WriteAllText($file, 'corrupted')
    Assert-Throws { Assert-MivVst3HostVcrt -RepoRoot $root } 'differs from canonical'
    Remove-Item -LiteralPath $file
    Assert-Throws { Assert-MivVst3HostVcrt -RepoRoot $root } 'missing/unreadable'
    New-Item -ItemType Directory -Path $file | Out-Null
    Assert-Throws { Assert-MivVst3HostVcrt -RepoRoot $root } 'regular and have no reparse'
    Remove-Item -LiteralPath $file
    Copy-Item -LiteralPath (Join-Path $source 'msvcp140.dll') -Destination $file
    $renamed = Join-Path $root 'retained-stage'
    Move-FixtureDirectory $stage $renamed $root
    New-Item -ItemType Junction -Path $stage -Value $renamed | Out-Null
    try { Assert-Throws { Assert-MivVst3HostVcrt -RepoRoot $root } 'regular and have no reparse' }
    finally { [System.IO.Directory]::Delete($stage) }
    Move-FixtureDirectory $renamed $stage $root
    Assert-MivVst3HostVcrt -RepoRoot $root
    foreach ($name in @('test-full.ps1', 'build-dist.ps1')) {
        $script = Get-Content -LiteralPath (Join-Path $PSScriptRoot $name) -Raw -Encoding UTF8
        $gate = $script.IndexOf('Assert-MivVst3HostVcrt -RepoRoot $repoRoot')
        $test = if ($name -eq 'test-full.ps1') { $script.IndexOf('& cargo test --workspace') } else { $script.IndexOf("-File (Join-Path `$scripts 'test-full.ps1')") }
        Assert-True ($gate -ge 0 -and $gate -lt $test) "$name runs tests before direct-host CRT preflight"
    }
    Write-Host '[vst3-vcrt-test] PASS'
} finally {
    $resolved = [System.IO.Path]::GetFullPath($root).TrimEnd('\')
    Assert-True ($resolved.StartsWith($temp + '\', [System.StringComparison]::OrdinalIgnoreCase)) 'Cleanup escaped temp'
    $null = @(Get-MivTreeFiles -Path $resolved)
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
