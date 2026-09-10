param()

$ErrorActionPreference = 'Stop'
$helperRoot = Join-Path $PSScriptRoot 'ui-smoke\button-helper'
$sourcePaths = @(
    (Join-Path $helperRoot 'ButtonGestureReducer.cs'),
    (Join-Path $helperRoot 'ButtonWireProtocol.cs'),
    (Join-Path $helperRoot 'ButtonInputBackend.cs'),
    (Join-Path $helperRoot 'ButtonHelperOwner.cs'),
    (Join-Path $helperRoot 'ButtonHelperLoop.cs'),
    (Join-Path $helperRoot 'ButtonOsObserver.cs'),
    (Join-Path $helperRoot 'ButtonOsObserverNative.cs'),
    (Join-Path $helperRoot 'ButtonInputBackendNative.cs'),
    (Join-Path $helperRoot 'LocalButtonPipe.cs'),
    (Join-Path $helperRoot 'ButtonHelperHost.cs'),
    (Join-Path $helperRoot 'ButtonHelperRunnerApi.cs'),
    (Join-Path $helperRoot 'tests\ButtonGestureReducerTests.cs'),
    (Join-Path $helperRoot 'tests\ButtonInputBackendDraftTests.cs'),
    (Join-Path $helperRoot 'tests\ButtonHelperHostDraftTests.cs'),
    (Join-Path $helperRoot 'tests\ButtonHelperDraftTests.cs')
)
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

[ButtonGestureReducerTests]::RunAll()
[ButtonHelperDraftTests]::RunAll()
