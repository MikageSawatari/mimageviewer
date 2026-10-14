# Offline regression tests. Load production helpers without running queries,
# writing the report, or executing the script's exit statement.
# Usage: .\scripts\test-check-dependency-updates.ps1
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'check-dependency-updates.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw 'Production script has parse errors' }
$functions = $ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)
. ([scriptblock]::Create(($functions | ForEach-Object { $_.Extent.Text }) -join "`n"))

function Assert-Equal {
    param($Actual, $Expected, [string]$Label)
    if ($Actual -cne $Expected) { throw "$Label expected [$Expected], got [$Actual]" }
}

# Mock only process/network/filesystem inputs. Keep JSON decoding, enumeration,
# version comparison, bash discovery, setup output parsing and table cells real.
$useGh = $true
$tagJson = '[{"name":"vstsdk3612_03_12_2018_build_67"},{"name":"v3.8.1_build_84"},{"name":"v3.8.0_build_66"},{"name":"v9.0.0"},{"name":"3.99.0_build_1"}]'
$existingPaths = @{}
$gitExecPath = ''
$wherePaths = ''
$commandCalls = New-Object 'System.Collections.Generic.List[string]'
$setupCode = 0
$setupText = ''

function Get-Command {
    param([string]$Name, $CommandType, $ErrorAction)
    if ($Name -eq 'gh' -and $useGh) { return [pscustomobject]@{ Source = 'fixture-gh.exe' } }
    return $null
}

function Get-Json {
    param([string]$Url)
    Assert-Equal $Url 'https://api.github.com/repos/steinbergmedia/vst3sdk/tags?per_page=100' 'REST URL'
    $parsed = $tagJson | ConvertFrom-Json
    Write-Output -NoEnumerate $parsed
}

function Test-Path {
    param([string]$LiteralPath, $PathType)
    return $existingPaths.ContainsKey([IO.Path]::GetFullPath($LiteralPath))
}

function Invoke-QueryCommand {
    param([string]$Name, [string[]]$Arguments)
    $commandCalls.Add("$Name $($Arguments -join ' ')")
    switch ($Name) {
        'gh' {
            Assert-Equal $Arguments[0] 'api' 'gh query mode'
            return [pscustomobject]@{ Code = 0; Text = $tagJson }
        }
        'git' {
            Assert-Equal ($Arguments -join ' ') '--exec-path' 'git read-only discovery'
            return [pscustomobject]@{ Code = $(if ($gitExecPath) { 0 } else { 1 }); Text = $gitExecPath }
        }
        'where.exe' {
            Assert-Equal ($Arguments -join ' ') 'bash' 'where discovery'
            return [pscustomobject]@{ Code = $(if ($wherePaths) { 0 } else { 1 }); Text = $wherePaths }
        }
    }
    Assert-Equal $Name 'C:\Program Files\Git\bin\bash.exe' 'resolved executable invocation'
    Assert-Equal $Arguments.Count 2 'setup argument count'
    Assert-Equal $Arguments[1] 'check' 'query-only setup mode'
    return [pscustomobject]@{ Code = $setupCode; Text = $setupText }
}

# Reproduce the real PS5.1 ConvertFrom-Json array shape via gh stdout, rather
# than supplying a pre-enumerated array from a mocked Get-GitHubJson function.
$tags = @(Get-GitHubJson 'repos/steinbergmedia/vst3sdk/tags?per_page=100')
Assert-Equal $tags.Count 5 'gh JSON array enumeration'
$latest = @(Get-LatestVst3Tag $tags)
Assert-Equal $latest.Count 1 'one selected tag'
Assert-Equal $latest[0] 'v3.8.1_build_84' 'old and non-build tags excluded'
$rows = New-Object 'System.Collections.Generic.List[object]'
Add-Row 'VST3 SDK' 'fixture current' $latest[0] 'fixture'
Assert-Equal (ConvertTo-TableCell $rows[0].Latest) 'v3.8.1_build_84' 'single Latest table cell'

$useGh = $false
$tags = @(Get-GitHubJson 'repos/steinbergmedia/vst3sdk/tags?per_page=100')
Assert-Equal $tags.Count 5 'REST array enumeration'
Assert-Equal (Get-LatestVst3Tag $tags) 'v3.8.1_build_84' 'REST latest tag'

$tagJson = '[{"name":"v3.9.0_build_999"},{"name":"v3.10.0_build_9"},{"name":"v3.10.0_build_100"},{"name":"v3.10.0_build_10"}]'
Assert-Equal (Get-LatestVst3Tag @(Get-GitHubJson 'repos/steinbergmedia/vst3sdk/tags?per_page=100')) 'v3.10.0_build_100' 'numeric version and build ordering'
$tagJson = '[{"name":"vstsdk3612_legacy"},{"name":"v3.8.1"}]'
$rejected = $false
try { $null = Get-LatestVst3Tag @(Get-GitHubJson 'repos/steinbergmedia/vst3sdk/tags?per_page=100') }
catch { $rejected = $true }
Assert-Equal $rejected $true 'no recognized tags rejected'
$useGh = $true
$tagJson = '{"tag_name":"v0.14.0"}'
Assert-Equal (Get-GitHubJson 'repos/Frieve-A/effetune-mixwright/releases/latest').tag_name 'v0.14.0' 'scalar release preserved'
Write-Output 'GitHub JSON and VST3 tag tests passed'

$oldProgramFiles = $env:ProgramFiles
$oldWindowsDir = $env:WINDIR
try {
    $env:ProgramFiles = 'C:\Program Files'
    $env:WINDIR = 'C:\Windows'
    $knownBash = 'C:\Program Files\Git\bin\bash.exe'
    $existingPaths = @{ $knownBash = $true }
    $commandCalls.Clear()
    Assert-Equal (Find-GitBash) $knownBash 'Git bash without PATH entry'
    Assert-Equal $commandCalls.Count 0 'known path takes priority'

    $gitExecPath = 'C:/Tools/PortableGit/mingw64/libexec/git-core'
    foreach ($candidate in @('C:\Tools\PortableGit\bin\bash.exe', 'C:\Tools\PortableGit\usr\bin\bash.exe')) {
        $existingPaths = @{ $candidate = $true }
        $commandCalls.Clear()
        Assert-Equal (Find-GitBash) $candidate 'git exec-path discovery'
        Assert-Equal $commandCalls.Count 1 'git discovery before where'
    }

    $gitExecPath = ''
    $wherePaths = "C:\WINDOWS\System32\bash.exe`r`nC:\CustomGit\usr\bin\bash.exe`r`n"
    $existingPaths = @{ 'C:\WINDOWS\System32\bash.exe' = $true; 'C:\CustomGit\usr\bin\bash.exe' = $true }
    Assert-Equal (Find-GitBash) 'C:\CustomGit\usr\bin\bash.exe' 'where skips WSL and continues'
    Assert-Equal (Test-GitBashPath 'C:/Windows/System32/bash.exe') $false 'WSL slash normalization'
    $wherePaths = 'C:\WINDOWS\System32\bash.exe'
    Assert-Equal (Find-GitBash) '' 'WSL alone means no usable bash'
    $wherePaths = ''
    $existingPaths = @{}
    Assert-Equal (Find-GitBash) '' 'bash absent'

    $rows = New-Object 'System.Collections.Generic.List[object]'
    $commandCalls.Clear()
    Add-SetupCheck 'PDFium' 'scripts/setup-pdfium.sh' 'vendor/pdfium/VERSION' ''
    Assert-Equal $rows[0].Latest 'skipped (bash not found)' 'missing bash row'
    Assert-Equal $commandCalls.Count 0 'missing bash does not execute'

    $fixtures = @(
        @{ Name = 'PDFium'; Script = 'scripts/setup-pdfium.sh'; Text = "Latest release: chromium/8100`nNew version available: chromium/8100"; Latest = 'chromium/8100' },
        @{ Name = 'FFmpeg'; Script = 'scripts/setup-ffmpeg.sh'; Text = "Matched asset: ffmpeg-fixture.zip`nUp to date."; Latest = 'ffmpeg-fixture.zip' },
        @{ Name = 'LibRaw'; Script = 'scripts/setup-libraw.sh'; Text = "Current LibRaw: 0.22.2; latest release: 0.23.0`nNew version available."; Latest = '0.23.0' }
    )
    foreach ($fixture in $fixtures) {
        $setupText = $fixture.Text
        Add-SetupCheck $fixture.Name $fixture.Script 'fixture-missing-version' $knownBash
        Assert-Equal $rows[$rows.Count - 1].Latest $fixture.Latest 'successful setup check parsing'
    }
    $setupCode = 1
    Add-SetupCheck 'PDFium' 'scripts/setup-pdfium.sh' 'vendor/pdfium/VERSION' $knownBash
    Assert-Equal $rows[$rows.Count - 1].Latest 'unavailable' 'query failure after discovery'
    Write-Output 'Git bash discovery and setup row tests passed'
} finally {
    $env:ProgramFiles = $oldProgramFiles
    $env:WINDIR = $oldWindowsDir
}

Write-Output 'All dependency update regression tests passed (no network or report writes).'
