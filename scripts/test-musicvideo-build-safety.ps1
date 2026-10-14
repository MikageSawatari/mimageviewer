# Headless regression tests. Process objects are fake; no product is launched.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
. (Join-Path $PSScriptRoot 'musicvideo-build-safety.ps1')

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw "[musicvideo-build-safety-test] $Message" }
}

$script:processes = @()
$script:stopCalls = 0
function Get-Process {
    param([string] $Name, $ErrorAction)
    Assert-True ($Name -eq 'mimageviewer-musicvideo') 'unexpected process query'
    return $script:processes
}
function Stop-Process {
    param([int] $Id, [switch] $Force, $ErrorAction)
    $script:stopCalls++
}

# Evaluate the actual call sites with fake processes, including both developer
# policies and portable build/package paths. No surrounding build code runs.
$coreExe = 'unused'
$musicvideoExe = Join-Path $repoRoot 'target\dev-runtime\mimageviewer-musicvideo.exe'
$releaseMusicvideoExe = Join-Path $repoRoot 'target\release\mimageviewer-musicvideo.exe'
$workspaceTargetDir = Join-Path $repoRoot 'target'
$portableTargetDir = Join-Path $repoRoot 'target-portable'
$editorPackageDir = Join-Path $repoRoot 'dist\mImageViewer_portable_v4.6.0'
foreach ($scriptName in @('build-dev.ps1', 'build-release.ps1', 'build-dist.ps1', 'build-portable.ps1')) {
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile(
        (Join-Path $PSScriptRoot $scriptName), [ref] $tokens, [ref] $errors)
    Assert-True ($errors.Count -eq 0) "parse failed: $scriptName"
    $calls = @($ast.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.CommandAst] -and
            $node.GetCommandName() -eq 'Assert-MivMusicvideoBuildOutputsIdle'
    }, $true))
    Assert-True ($calls.Count -eq 1) "missing or ambiguous editor guard: $scriptName"
    $call = $calls[0].Extent.Text
    if ($scriptName -eq 'build-portable.ps1') {
        $musicvideoExe = Join-Path $portableTargetDir 'release\mimageviewer-musicvideo.exe'
    }
    $expectedPaths = switch ($scriptName) {
        'build-dev.ps1' { @($musicvideoExe) }
        'build-release.ps1' { @($releaseMusicvideoExe) }
        'build-dist.ps1' { @($releaseMusicvideoExe, (Join-Path $portableTargetDir 'release\mimageviewer-musicvideo.exe')) }
        'build-portable.ps1' { @($musicvideoExe, (Join-Path $editorPackageDir 'mimageviewer-musicvideo.exe')) }
    }
    foreach ($PreserveRuntime in @($false, $true)) {
        foreach ($path in $expectedPaths) {
            # Case-insensitive Windows path equality, not a prefix match.
            $script:processes = @([pscustomobject]@{ Id = 4242; Path = $path.ToUpperInvariant() })
            $message = $null
            try { Invoke-Expression $call } catch { $message = $_.Exception.Message }
            Assert-True ([bool] $message -and $message.Contains('Save your work and close the editor')) "running output was not rejected: $scriptName $path"
            Assert-True ($script:stopCalls -eq 0) "editor was stopped: $scriptName"
        }
    }
    $script:processes = @([pscustomobject]@{ Id = 4242; Path = (Join-Path $repoRoot 'unrelated\mimageviewer-musicvideo.exe') })
    Invoke-Expression $call
    $script:processes = @()
    Invoke-Expression $call
    $script:processes = @([pscustomobject]@{ Id = 4242; Path = $null })
    $message = $null
    try { Invoke-Expression $call } catch { $message = $_.Exception.Message }
    Assert-True ([bool] $message) "unknown process path was accepted: $scriptName"
    Assert-True ($script:stopCalls -eq 0) 'unknown process was stopped'
}

# Import only pure manifest functions; never execute the portable build script.
$portableAst = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'build-portable.ps1'), [ref] $tokens, [ref] $errors)
foreach ($name in @('Write-SmokeBuildManifest', 'Assert-SmokeBuildManifest')) {
    $definitions = @($portableAst.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $true))
    Assert-True ($definitions.Count -eq 1) "manifest function is missing: $name"
    Invoke-Expression $definitions[0].Extent.Text
}
$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = [System.IO.Path]::GetFullPath((Join-Path $tempRoot ("miv-editor-manifest-{0}" -f [guid]::NewGuid().ToString('N'))))
Assert-True ($testRoot.StartsWith(($tempRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase)) 'test root escaped temp'
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    $manifestPath = Join-Path $testRoot 'manifest.json'
    $files = @('core', 'remote', 'epub', 'editor') | ForEach-Object {
        $path = Join-Path $testRoot $_
        [System.IO.File]::WriteAllText($path, $_)
        $path
    }
    Write-SmokeBuildManifest $manifestPath 'fingerprint' $files[0] $files[1] $files[2] $files[3]
    $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    Assert-True ($manifest.musicvideo_sha256 -eq (Get-FileHash -LiteralPath $files[3] -Algorithm SHA256).Hash.ToLowerInvariant()) 'manifest omits editor hash'
    Assert-SmokeBuildManifest $manifestPath 'fingerprint' $files[0] $files[1] $files[2] $files[3]
    [System.IO.File]::WriteAllText($files[3], 'changed editor')
    $message = $null
    try {
        Assert-SmokeBuildManifest $manifestPath 'fingerprint' $files[0] $files[1] $files[2] $files[3]
    } catch { $message = $_.Exception.Message }
    Assert-True ([bool] $message -and $message.Contains('hash does not match')) 'changed editor was accepted by manifest'
    $support = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'ui-smoke\release-checks\Support.ps1') -Raw -Encoding UTF8
    Assert-True ($support.Contains("(Join-Path `$smoke 'mimageviewer-musicvideo.exe')")) 'ReleaseChecks omits editor manifest input'
} finally {
    Assert-True ($testRoot.StartsWith(($tempRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase)) 'cleanup escaped temp'
    Remove-Item -LiteralPath $testRoot -Recurse -Force
}
Write-Host '[musicvideo-build-safety-test] PASS'
