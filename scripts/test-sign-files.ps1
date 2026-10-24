# Script-only signing-probe tests. A .cmd stub never signs anything and needs
# neither a certificate nor SimplySign. Only disposable copies of the host PE
# are created; no product build, signing, launch, or profile access occurs.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'sign-files.ps1')

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "[sign-files-test] $Message" }
}

function Get-ProbeCalls {
    $arguments = $null
    foreach ($line in Get-Content -LiteralPath $env:MIV_SIGN_PROBE_LOG -Encoding ASCII) {
        if ($line -eq 'BEGIN') {
            $arguments = New-Object 'System.Collections.Generic.List[string]'
        } elseif ($line.StartsWith('ARG:')) {
            $arguments.Add($line.Substring(4))
        } elseif ($line -eq 'END') {
            [pscustomobject]@{ Arguments = $arguments.ToArray() }
        } else {
            throw "[sign-files-test] Unexpected log line: $line"
        }
    }
}

function Invoke-ProbeCase([int] $SignExit, [int] $TimestampExit, [int] $VerifyExit, [string] $ErrorPattern) {
    $env:MIV_SIGN_PROBE_SIGN_EXIT = [string]$SignExit
    $env:MIV_SIGN_PROBE_TIMESTAMP_EXIT = [string]$TimestampExit
    $env:MIV_SIGN_PROBE_VERIFY_EXIT = [string]$VerifyExit
    if (Test-Path -LiteralPath $env:MIV_SIGN_PROBE_LOG) { Remove-Item -LiteralPath $env:MIV_SIGN_PROBE_LOG }
    $message = $null
    try { Invoke-MivSignProbe } catch { $message = $_.Exception.Message }
    if ($ErrorPattern) {
        Assert-True ($null -ne $message -and $message -match $ErrorPattern) "Unexpected failure: $message (expected $ErrorPattern)"
        Assert-True ($message.Contains('stub stderr')) 'Failure lost native stderr'
        Assert-True ($message.Contains('stub stdout')) 'Failure lost native stdout'
    } else {
        Assert-True ($null -eq $message) "Probe should succeed even with stderr: $message"
    }
    Assert-True ($ErrorActionPreference -eq 'Stop') 'Probe leaked its error preference'
    Assert-True ((Get-FileHash -LiteralPath $hostExe -Algorithm SHA256).Hash -eq $hostHash) 'Source host PE was changed'
    $calls = @(Get-ProbeCalls)
    Assert-True ($calls.Count -gt 0) 'Probe never called the stub'
    $probeDir = Split-Path -Parent $calls[0].Arguments[-1]
    $tempPrefix = $tempRoot + '\miv-sign-probe-'
    Assert-True ($probeDir.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) "Target escaped probe temp root: $probeDir"
    Assert-True ((Split-Path -Leaf $probeDir) -match '^miv-sign-probe-[0-9a-f]{32}$') 'Probe directory is not uniquely named'
    Assert-True (-not $probeDirs.Contains($probeDir)) 'Probe reused a temporary directory'
    $probeDirs.Add($probeDir)
    foreach ($call in $calls) {
        $target = $call.Arguments[-1]
        Assert-True ($target -eq (Join-Path $probeDir ([IO.Path]::GetFileName($hostExe)))) "Unexpected target: $target"
        Assert-True (-not $target.Equals($hostExe, [StringComparison]::OrdinalIgnoreCase)) 'Probe targeted the original host'
    }
    Assert-True (-not (Test-Path -LiteralPath $probeDir)) "Probe temp directory was retained: $probeDir"
    return $calls
}

$envNames = @('MIV_SIGNTOOL', 'MIV_SIGN_SUBJECT', 'MIV_SIGN_SHA1', 'MIV_SIGN_TS',
    'MIV_SIGN_PROBE_LOG', 'MIV_SIGN_PROBE_SIGN_EXIT', 'MIV_SIGN_PROBE_TIMESTAMP_EXIT', 'MIV_SIGN_PROBE_VERIFY_EXIT')
$savedEnv = @{}
foreach ($name in $envNames) { $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = Join-Path $tempRoot ('miv-sign-files-test-' + [guid]::NewGuid().ToString('N'))
Assert-True ([IO.Path]::GetFullPath($testRoot).StartsWith(($tempRoot + '\'), [StringComparison]::OrdinalIgnoreCase)) 'Test root escaped OS temp'
New-Item -ItemType Directory -Path $testRoot | Out-Null
$hostExe = (Get-Process -Id $PID).Path
$hostHash = (Get-FileHash -LiteralPath $hostExe -Algorithm SHA256).Hash
$probeDirs = New-Object 'System.Collections.Generic.List[string]'
try {
    $env:MIV_SIGNTOOL = Join-Path $testRoot 'signtool stub.cmd'
    $env:MIV_SIGN_SUBJECT = 'MIV probe test subject'
    $env:MIV_SIGN_SHA1 = $null
    $env:MIV_SIGN_TS = 'https://tsa.invalid/miv-sign-probe'
    $env:MIV_SIGN_PROBE_LOG = Join-Path $testRoot 'calls.log'
    @'
@echo off
setlocal
set "subcommand=%~1"
>>"%MIV_SIGN_PROBE_LOG%" echo BEGIN
:args
if "%~1"=="" goto done
>>"%MIV_SIGN_PROBE_LOG%" echo ARG:%~1
shift
goto args
:done
>>"%MIV_SIGN_PROBE_LOG%" echo END
echo stub stdout %subcommand%
echo stub stderr %subcommand% 1>&2
if /i "%subcommand%"=="sign" exit /b %MIV_SIGN_PROBE_SIGN_EXIT%
if /i "%subcommand%"=="timestamp" exit /b %MIV_SIGN_PROBE_TIMESTAMP_EXIT%
if /i "%subcommand%"=="verify" exit /b %MIV_SIGN_PROBE_VERIFY_EXIT%
exit /b 99
'@ | Set-Content -LiteralPath $env:MIV_SIGNTOOL -Encoding ASCII

    $calls = @(Invoke-ProbeCase 0 0 0 '')
    Assert-True ($calls.Count -eq 3) 'Success should call sign/timestamp/verify once each'
    $target = $calls[0].Arguments[-1]
    Assert-True (($calls[0].Arguments -join '|') -eq "sign|/n|$env:MIV_SIGN_SUBJECT|/fd|sha256|$target") 'Sign arguments changed or include timestamping'
    Assert-True (($calls[1].Arguments -join '|') -eq "timestamp|/tr|$env:MIV_SIGN_TS|/td|sha256|$target") 'Timestamp arguments changed'
    Assert-True (($calls[2].Arguments -join '|') -eq "verify|/pa|$target") 'Verify arguments changed'

    $env:MIV_SIGN_SHA1 = '0123456789ABCDEF'
    $calls = @(Invoke-ProbeCase 23 0 0 'signing key is not usable.*SimplySign.*session expired.*not logged in.*SCardSvr.*CSP not registered.*restricted.*non-elevated')
    Assert-True ($calls.Count -eq 1 -and $calls[0].Arguments[0] -eq 'sign') 'Key failure was retried or reached timestamp/verify'
    Assert-True ($calls[0].Arguments[1] -eq '/sha1' -and $calls[0].Arguments[2] -eq $env:MIV_SIGN_SHA1) 'Thumbprint selector did not override subject'

    $calls = @(Invoke-ProbeCase 0 24 0 ('timestamp server not reachable: ' + [regex]::Escape($env:MIV_SIGN_TS) + '.*MIV_SIGN_TS'))
    Assert-True ($calls.Count -eq 4) 'Timestamp failure should call sign once and timestamp three times'
    Assert-True (@($calls | Where-Object { $_.Arguments[0] -eq 'timestamp' }).Count -eq 3) 'Timestamp did not make exactly three attempts'
    Assert-True (@($calls | Where-Object { $_.Arguments[0] -eq 'verify' }).Count -eq 0) 'Verify ran after timestamp failure'

    $calls = @(Invoke-ProbeCase 0 0 25 'probe verification failed')
    Assert-True ($calls.Count -eq 3 -and $calls[2].Arguments[0] -eq 'verify') 'Verification failure took an unexpected path'

    # Check orchestration without reading the real certificate store or signing.
    & {
        $script:readySteps = New-Object 'System.Collections.Generic.List[string]'
        function Assert-MivSignCertificatePresent { $script:readySteps.Add('certificate') }
        function Invoke-MivSignProbe { $script:readySteps.Add('probe') }
        Assert-MivSignReady
        Assert-True (($script:readySteps -join '|') -eq 'certificate|probe') 'Readiness did not check certificate before probe'
        $script:readySteps.Clear()
        function Assert-MivSignCertificatePresent { throw 'missing certificate fixture' }
        $message = $null
        try { Assert-MivSignReady } catch { $message = $_.Exception.Message }
        Assert-True ($message -eq 'missing certificate fixture' -and $script:readySteps.Count -eq 0) 'Probe ran after certificate failure'
    }
    Write-Host '[sign-files-test] PASS (success, key failure, TSA retries, verify failure, cleanup, source hash, readiness order)'
} finally {
    foreach ($name in $envNames) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name], 'Process') }
    Remove-Item -LiteralPath $testRoot -Recurse -Force
}
