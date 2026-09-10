# Shared, noninteractive runner integration for the native button helper.
# This file only defines functions. Importing it does not construct a native
# input backend, open a pipe, launch an App, or call SendInput.

function Import-UiSmokeButtonHelperTypes {
    param([Parameter(Mandatory = $true)][string] $HelperRoot)

    if ('Miv.UiSmoke.ButtonHelperDraft.ButtonHelperRunnerApi' -as [type]) {
        return
    }

    $sourcePaths = @(
        (Join-Path $HelperRoot 'ButtonGestureReducer.cs'),
        (Join-Path $HelperRoot 'ButtonWireProtocol.cs'),
        (Join-Path $HelperRoot 'ButtonInputBackend.cs'),
        (Join-Path $HelperRoot 'ButtonHelperOwner.cs'),
        (Join-Path $HelperRoot 'ButtonHelperLoop.cs'),
        (Join-Path $HelperRoot 'ButtonOsObserver.cs'),
        (Join-Path $HelperRoot 'ButtonOsObserverNative.cs'),
        (Join-Path $HelperRoot 'ButtonInputBackendNative.cs'),
        (Join-Path $HelperRoot 'LocalButtonPipe.cs'),
        (Join-Path $HelperRoot 'ButtonHelperHost.cs'),
        (Join-Path $HelperRoot 'ButtonHelperRunnerApi.cs')
    )
    foreach ($sourcePath in $sourcePaths) {
        if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
            throw "native button helper source is missing: $sourcePath"
        }
    }

    $usingBlock = @'
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;
using Miv.UiSmoke.ButtonDraft;
using Miv.UiSmoke.ButtonHelperDraft;
'@
    $bodies = $sourcePaths | ForEach-Object {
        $text = [System.IO.File]::ReadAllText($_, [System.Text.Encoding]::UTF8)
        [System.Text.RegularExpressions.Regex]::Replace(
            $text,
            '(?m)^using [^;]+;\r?\n',
            '')
    }
    $source = $usingBlock + [Environment]::NewLine + ($bodies -join [Environment]::NewLine)
    Add-Type -TypeDefinition $source -Language CSharp
}

function Invoke-WithUiSmokeButtonHelperEnvironment {
    param(
        [Parameter(Mandatory = $true)][string] $PipeName,
        [Parameter(Mandatory = $true)][string] $SessionNonce,
        [Parameter(Mandatory = $true)][uint32] $ServerProcessId,
        [Parameter(Mandatory = $true)][scriptblock] $Action
    )
    if ([string]::IsNullOrWhiteSpace($PipeName) -or
        [string]::IsNullOrWhiteSpace($SessionNonce) -or
        $ServerProcessId -eq 0) {
        throw 'native button helper child environment is incomplete'
    }
    $values = [ordered]@{
        MIV_UI_SMOKE_BUTTON_PIPE = $PipeName
        MIV_UI_SMOKE_BUTTON_SESSION = $SessionNonce
        MIV_UI_SMOKE_BUTTON_SERVER_PID = $ServerProcessId.ToString(
            [System.Globalization.CultureInfo]::InvariantCulture)
    }
    $previous = @{}
    try {
        foreach ($entry in $values.GetEnumerator()) {
            $previous[$entry.Key] = [Environment]::GetEnvironmentVariable(
                $entry.Key,
                [EnvironmentVariableTarget]::Process)
            [Environment]::SetEnvironmentVariable(
                $entry.Key,
                [string]$entry.Value,
                [EnvironmentVariableTarget]::Process)
        }
        & $Action
    }
    finally {
        foreach ($entry in $values.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable(
                $entry.Key,
                $previous[$entry.Key],
                [EnvironmentVariableTarget]::Process)
        }
    }
}

function Resolve-UiSmokeButtonHelperFinalization {
    param(
        [Parameter(Mandatory = $true)][int] $PrimaryExitCode,
        [Parameter(Mandatory = $true)][string] $PrimaryPhase,
        [AllowNull()][string] $PrimaryFailure,
        [Parameter(Mandatory = $true)][bool] $HelperRequired,
        [Parameter(Mandatory = $true)][bool] $HelperStarted,
        [AllowNull()][object] $HelperStatus
    )

    $mayTerminateApp = -not $HelperStarted
    $helperSucceeded = $false
    $helperFailure = $null

    if (-not $HelperRequired) {
        $mayTerminateApp = $true
        $helperSucceeded = $true
    }
    elseif (-not $HelperStarted) {
        $helperFailure = 'native button helper did not start'
    }
    elseif ($null -eq $HelperStatus) {
        # A started owner without a terminal snapshot has unknown release state.
        # Keep the exact App alive rather than inventing a release observation.
        $helperFailure = 'native button helper finalization did not return a status'
    }
    else {
        $mayTerminateApp = [bool]$HelperStatus.SafeToTerminateApp
        $releaseState = [string]$HelperStatus.ReleaseState
        $helperSucceeded = [bool]$HelperStatus.Joined `
            -and [bool]$HelperStatus.SafeToTerminateApp `
            -and [bool]$HelperStatus.Succeeded `
            -and -not [bool]$HelperStatus.HasOutstandingRelease `
            -and $releaseState -ceq 'ConfirmedReleased'
        if (-not $helperSucceeded) {
            $helperFailure = 'native button helper did not complete safely' `
                + " (terminal=$($HelperStatus.TerminalKind), release=$releaseState)"
        }
    }

    # App/script/runner failure is primary. Cleanup evidence never changes its
    # exit code, phase, or reported reason. A helper failure becomes primary
    # only when the App otherwise reported success.
    $finalExitCode = $PrimaryExitCode
    $finalPhase = $PrimaryPhase
    $finalFailure = $PrimaryFailure
    if ($HelperRequired -and -not $helperSucceeded -and $PrimaryExitCode -eq 0) {
        $finalExitCode = 2
        $finalPhase = 'button-helper-failed'
        $finalFailure = $helperFailure
    }

    [pscustomobject]@{
        ExitCode = [int]$finalExitCode
        Phase = [string]$finalPhase
        Failure = $finalFailure
        HelperFailure = $helperFailure
        HelperSucceeded = [bool]$helperSucceeded
        MayTerminateApp = [bool]$mayTerminateApp
        ScenarioSucceeded = [bool]($finalExitCode -eq 0 -and $helperSucceeded)
    }
}
