# Pure script regression tests; fake PE only, never executes a VST3 host.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'vst3-host-identity.ps1')
function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "[vst3-identity-test] $Message" }
}
function Assert-Throws([scriptblock] $Action, [string] $Pattern) {
    try { & $Action } catch {
        Assert-True ($_.Exception.Message -match $Pattern) "Unexpected error: $_"
        return
    }
    throw "[vst3-identity-test] Missing expected failure: $Pattern"
}
function Write-FakeHost([string] $Path, [string] $Marker) {
    $bytes = New-Object byte[] 256
    $bytes[0] = 0x4d; $bytes[1] = 0x5a; $bytes[0x3c] = 0x40
    $bytes[64] = 0x50; $bytes[65] = 0x45
    $text = [System.Text.Encoding]::ASCII.GetBytes($Marker)
    $out = New-Object byte[] ($bytes.Length + $text.Length)
    [Array]::Copy($bytes, $out, $bytes.Length)
    [Array]::Copy($text, 0, $out, $bytes.Length, $text.Length)
    [System.IO.File]::WriteAllBytes($Path, $out)
}
$temp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = Join-Path $temp ('miv-vst3-identity-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    $source = Join-Path $testRoot 'crates\vst3-host'
    $hostPath = Join-Path $testRoot 'vendor\vst3-host\mimageviewer-vst3-host.exe'
    New-Item -ItemType Directory -Path (Split-Path -Parent $hostPath) -Force | Out-Null
    foreach ($name in @('CMakeLists.txt', 'include/z.h', 'include/nested/a.h', 'src/main.cpp', 'src/sdk/module_win32.cpp', 'tests/pure.cpp')) {
        $path = Join-Path $source $name
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, "raw fixture $name`r`n")
    }
    $hash = Get-MivVst3HostSourceHash -SourceRoot $source
    $lfSource = Join-Path $testRoot 'lf-source'
    foreach ($file in @(Get-MivTreeFiles -Path $source)) {
        $relative = $file.FullName.Substring($source.Length + 1)
        $lfPath = Join-Path $lfSource $relative
        New-Item -ItemType Directory -Path (Split-Path -Parent $lfPath) -Force | Out-Null
        [System.IO.File]::WriteAllBytes($lfPath, [System.Text.Encoding]::UTF8.GetBytes("raw fixture $($relative.Replace('\', '/'))`n"))
    }
    Assert-True ((Get-MivVst3HostSourceHash -SourceRoot $lfSource) -eq $hash) 'LF and CRLF source trees differ'
    $cliHash = & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'vst3-host-identity.ps1') -HashSourceRoot $lfSource
    Assert-True ($LASTEXITCODE -eq 0 -and $cliHash -eq $hash) 'CMake hash CLI differs from validator'
    # Only CRLF changes: BOM, invalid UTF-8, NUL and standalone CR are preserved.
    $byteFixture = Join-Path $testRoot 'bytes.bin'
    [System.IO.File]::WriteAllBytes($byteFixture, [byte[]]@(239, 187, 191, 255, 0, 13, 65, 13, 10, 13))
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $expected = ([BitConverter]::ToString($sha.ComputeHash([byte[]]@(239, 187, 191, 255, 0, 13, 65, 10, 13)))).Replace('-', '').ToLowerInvariant()
        Assert-True ((Get-MivVst3NormalizedFileHash -Path $byteFixture) -eq $expected) 'Normalized bytes other than CRLF'
    } finally { $sha.Dispose() }
    Assert-Throws { Assert-MivVst3HostIdentity -RepoRoot $testRoot } 'Vendor host is missing'
    Write-FakeHost $hostPath ''
    Assert-Throws { Assert-MivVst3HostIdentity -RepoRoot $testRoot } 'Missing/stale source identity'
    Write-FakeHost $hostPath ('MIV_VST3_HOST_SOURCE_SHA256:' + ('0' * 64))
    Assert-Throws { Assert-MivVst3HostIdentity -RepoRoot $testRoot } 'Missing/stale source identity'
    Write-FakeHost $hostPath ('MIV_VST3_HOST_SOURCE_SHA256:' + $hash)
    Assert-MivVst3HostIdentity -RepoRoot $testRoot
    & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'vst3-host-identity.ps1') -ValidateRepo $testRoot
    Assert-True ($LASTEXITCODE -eq 0) 'CLI rejected the current-source fake PE'
    $ignored = Join-Path $source 'build\generated\build_identity.h'
    New-Item -ItemType Directory -Path (Split-Path -Parent $ignored) -Force | Out-Null
    [System.IO.File]::WriteAllText($ignored, 'ignored generated header')
    Assert-True ((Get-MivVst3HostSourceHash -SourceRoot $source) -eq $hash) 'Included generated build files'
    $modified = Join-Path $source 'src\sdk\module_win32.cpp'
    [System.IO.File]::AppendAllText($modified, 'changed')
    Assert-True ((Get-MivVst3HostSourceHash -SourceRoot $source) -ne $hash) 'Missed nested source change'
    Assert-Throws { Assert-MivVst3HostIdentity -RepoRoot $testRoot } 'Missing/stale source identity'
    $newHash = Get-MivVst3HostSourceHash -SourceRoot $source
    Write-FakeHost $hostPath ('MIV_VST3_HOST_SOURCE_SHA256:' + $newHash)
    Assert-MivVst3HostIdentity -RepoRoot $testRoot
    Write-FakeHost $hostPath (('MIV_VST3_HOST_SOURCE_SHA256:' + $newHash + "`n") * 2)
    Assert-Throws { Assert-MivVst3HostIdentity -RepoRoot $testRoot } 'Missing/stale source identity'
    $release = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'build-release.ps1') -Raw -Encoding UTF8
    Assert-True (-not $release.Contains('Ensure-VendorVst3BridgeFromCache')) 'Old cache importer remains'
    Assert-True (-not $release.Contains('Copy-Item -LiteralPath $appDataVst3Bridge')) 'APPDATA cache import remains'
    $identityIndex = $release.IndexOf('# A successful rebuild and every reuse route')
    Assert-True ($identityIndex -ge 0 -and $identityIndex -lt $release.IndexOf('Invoke-MivSign -Files $vendorEmbedTargets')) 'Identity check happens after signing'
    $coreIndex = $release.IndexOf('$coreExit = Invoke-ReleaseCargo')
    Assert-True ($release.LastIndexOf('Assert-MivVst3HostIdentity -RepoRoot $repoRoot', $coreIndex) -gt $identityIndex) 'No identity recheck before core embedding'
    foreach ($scriptName in @('build-release.ps1', 'build-dist.ps1', 'vst3-host-identity.ps1')) {
        $tokens = $null; $errors = $null
        $null = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $scriptName), [ref]$tokens, [ref]$errors)
        Assert-True ($errors.Count -eq 0) "Script parse failed: $scriptName"
    }
    $tokens = $null; $errors = $null
    $releaseAst = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'build-release.ps1'), [ref]$tokens, [ref]$errors)
    $sdkBranches = @($releaseAst.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.IfStatementAst] -and
            $node.Clauses[0].Item1.Extent.Text -eq '-not (Test-Path $vst3SdkLicense)'
    }, $true))
    Assert-True ($sdkBranches.Count -eq 1 -and $sdkBranches[0].Clauses[0].Item2.Extent.Text.Contains('Assert-MivVst3HostIdentity -RepoRoot $repoRoot')) 'SDK-missing reuse branch lacks identity gate'
    $rebuildBranches = @($releaseAst.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.IfStatementAst] -and
            $node.Clauses[0].Item1.Extent.Text -eq '-not $SkipVst3Bridge' -and $node.ElseClause
    }, $true))
    Assert-True ($rebuildBranches.Count -eq 1 -and $rebuildBranches[0].ElseClause.Extent.Text.Contains('Assert-MivVst3HostIdentity -RepoRoot $repoRoot')) 'SkipVst3Bridge reuse branch lacks identity gate'
    Write-Host '[vst3-identity-test] PASS'
} finally {
    $resolved = [System.IO.Path]::GetFullPath($testRoot).TrimEnd('\')
    Assert-True ($resolved.StartsWith($temp + '\', [System.StringComparison]::OrdinalIgnoreCase)) 'Cleanup escaped temp'
    $null = @(Get-MivTreeFiles -Path $resolved)
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
