# Noninteractive regression checks. Cargo is a PowerShell function stub in a
# child PowerShell process: no Rust test executable or real window is launched.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$runner = Join-Path $PSScriptRoot 'test-live-windows.ps1'
$scratch = Join-Path $repoRoot ('target/live-runner-fake-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null
$harness = Join-Path $scratch 'harness.ps1'
$calls = Join-Path $scratch 'calls.jsonl'

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "[live-runner-test] $Message" }
}

$tokens = $null
$parseErrors = $null
[void][System.Management.Automation.Language.Parser]::ParseFile($runner, [ref]$tokens, [ref]$parseErrors)
Assert-True ($parseErrors.Count -eq 0) 'runner must parse on Windows PowerShell'
Assert-True (@([IO.File]::ReadAllBytes($runner) | Where-Object { $_ -gt 127 }).Count -eq 0) 'runner must be ASCII'

@'
param([string] $Runner, [string] $Calls, [int] $FakeExit, [switch] $Approved)
$ErrorActionPreference = 'Stop'
function global:cargo {
    ConvertTo-Json -InputObject @($args) -Compress | Add-Content -LiteralPath $Calls -Encoding ASCII
    $global:LASTEXITCODE = $FakeExit
}
& $Runner -InteractiveApproved:$Approved
exit $LASTEXITCODE
'@ | Set-Content -LiteralPath $harness -Encoding ASCII

function Invoke-Case([bool] $Approved, [int] $FakeExit) {
    if (Test-Path -LiteralPath $calls) { Remove-Item -LiteralPath $calls }
    $arguments = @('-NoProfile', '-File', $harness, '-Runner', $runner,
        '-Calls', $calls, '-FakeExit', $FakeExit)
    if ($Approved) { $arguments += '-Approved' }
    $beforePreference = $ErrorActionPreference
    try {
        # PS 5.1 turns redirected native stderr into ErrorRecords. Expected
        # rejection is test evidence, not a terminating harness error.
        $ErrorActionPreference = 'Continue'
        & powershell @arguments > (Join-Path $scratch 'stdout.txt') 2> (Join-Path $scratch 'stderr.txt')
        $caseExit = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $beforePreference
    }
    $recorded = @()
    if (Test-Path -LiteralPath $calls) {
        $recorded = @(Get-Content -LiteralPath $calls -Encoding ASCII | ForEach-Object { ,(ConvertFrom-Json $_) })
    }
    return [pscustomobject]@{ Exit = $caseExit; Calls = $recorded }
}

$denied = Invoke-Case $false 0
Assert-True ($denied.Exit -ne 0 -and $denied.Calls.Count -eq 0) 'missing approval must stop before Cargo'
$success = Invoke-Case $true 0
Assert-True ($success.Exit -eq 0 -and $success.Calls.Count -eq 1) 'runner must run only the eframe graph'
foreach ($call in $success.Calls) {
    $joined = $call -join ' '
    Assert-True ($joined.Contains('--run-ignored only')) 'only ignored tests may run'
    Assert-True ($joined.Contains('--ignore-default-filter')) 'default filters must not drop live tests'
    Assert-True ($joined.Contains('--test-threads 1')) 'desktop tests must be serial'
    Assert-True ($joined.Contains('--retries 0')) 'no retries'
    Assert-True ($joined.Contains('--no-tests fail')) 'zero selection must fail'
    $filterIndex = [array]::IndexOf($call, '-E')
    Assert-True ($filterIndex -ge 0) 'an exact allowlist is required'
    $filter = $call[$filterIndex + 1]
    Assert-True ($filter -eq 'test(=native::run::tests::windows_process_test::synchronous_damage_returns_before_outer_paint_gate)') 'runner must select only the live eframe test'
}
Assert-True (($success.Calls[0] -join ' ').Contains('--manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib')) 'eframe graph changed'
$failed = Invoke-Case $true 7
Assert-True ($failed.Exit -eq 7 -and $failed.Calls.Count -eq 1) 'failure exit must propagate'
$empty = Invoke-Case $true 4
Assert-True ($empty.Exit -eq 4 -and $empty.Calls.Count -eq 1) 'zero-match exit must propagate'
Write-Host "[live-runner-test] PASS: 4 fake cases; no live tests executed. Artifacts: $scratch"
