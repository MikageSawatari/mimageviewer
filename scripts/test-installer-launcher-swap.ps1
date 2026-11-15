# Isolated Inno Setup integration test. No mImageViewer executable is launched.
# Run with Windows PowerShell 5.1; installers and logs remain under target.
[CmdletBinding()]
param(
    [string] $IsccPath = 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$testRoot = Join-Path $repoRoot 'target\installer-swap-test'
$token = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path $testRoot ('run-' + $token)
$installDir = Join-Path $runRoot 'install'
$launcherName = 'miv-swap-test.exe'
$launcherPath = Join-Path $installDir $launcherName
$processes = New-Object 'System.Collections.Generic.List[System.Diagnostics.Process]'
$mutex = $null
$lockStream = $null
$savedTemp = $env:TEMP
$savedTmp = $env:TMP
$exitCode = 1

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

function Assert-UnderTestRoot([string] $Path) {
    $full = [IO.Path]::GetFullPath($Path)
    $prefix = [IO.Path]::GetFullPath($testRoot).TrimEnd('\') + '\'
    Assert-True ($full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) `
        "Refusing path outside test root: $full"
    # Reject reparse points in every existing ancestor before launching or deleting.
    $entry = $full
    while ($entry) {
        if (Test-Path -LiteralPath $entry) {
            $item = Get-Item -LiteralPath $entry -Force
            Assert-True (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) `
                "Refusing reparse point: $entry"
        }
        $entry = [IO.Path]::GetDirectoryName($entry)
    }
}

function Remove-TestInstall {
    Assert-UnderTestRoot $installDir
    if (Test-Path -LiteralPath $installDir) {
        $pending = New-Object 'System.Collections.Generic.Stack[string]'
        $pending.Push($installDir)
        while ($pending.Count -gt 0) {
            foreach ($item in Get-ChildItem -LiteralPath $pending.Pop() -Force) {
                Assert-True (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) `
                    "Refusing cleanup containing reparse point: $($item.FullName)"
                if ($item.PSIsContainer) { $pending.Push($item.FullName) }
            }
        }
        Remove-Item -LiteralPath $installDir -Recurse -Force
    }
}

function Invoke-TestProcess([string] $Exe, [string[]] $Arguments, [string] $Label) {
    Assert-UnderTestRoot $Exe
    Write-Host ("RUN {0}: {1} {2}" -f $Label, $Exe, ($Arguments -join ' '))
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo.FileName = $Exe
    $process.StartInfo.Arguments = $Arguments -join ' '
    $process.StartInfo.UseShellExecute = $false
    $process.StartInfo.CreateNoWindow = $true
    Assert-True ($process.Start()) "Could not start $Label"
    $processes.Add($process)
    Assert-True ($process.WaitForExit(60000)) "$Label timed out after 60 seconds"
    $process.Refresh()
    Write-Host ("EXIT {0}: {1}" -f $Label, $process.ExitCode)
    return $process.ExitCode
}

function Build-TestInstaller([string] $Source, [string] $Name, [string] $Extra = '') {
    $arguments = @('/Qp', "/DTestRoot=$runRoot", "/DTestToken=$token",
        "/DLauncherSource=$Source", "/DTestOutputName=$Name")
    if ($Extra) { $arguments += $Extra }
    $arguments += (Join-Path $repoRoot 'installer\tests\launcher_swap.iss')
    $compileLog = Join-Path $runRoot ("compile-$Name.log")
    $compileErrors = Join-Path $runRoot ("compile-$Name.stderr.log")
    Write-Host ("ISCC {0}" -f ($arguments -join ' '))
    $quotedArguments = @($arguments | ForEach-Object { '"' + $_ + '"' })
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo.FileName = $IsccPath
    $process.StartInfo.Arguments = $quotedArguments -join ' '
    $process.StartInfo.UseShellExecute = $false
    $process.StartInfo.CreateNoWindow = $true
    $process.StartInfo.RedirectStandardOutput = $true
    $process.StartInfo.RedirectStandardError = $true
    Assert-True ($process.Start()) 'Could not start ISCC'
    $processes.Add($process)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    Assert-True ($process.WaitForExit(60000)) "ISCC timed out; see $compileLog"
    [IO.File]::WriteAllText($compileLog, $stdout.Result)
    [IO.File]::WriteAllText($compileErrors, $stderr.Result)
    Assert-True ($process.ExitCode -eq 0) "ISCC failed; see $compileErrors"
    return (Join-Path $runRoot "output\$Name.exe")
}

function Install-Version([string] $Installer, [string] $Label, [bool] $Success = $true) {
    $log = Join-Path $runRoot ("$Label.log")
    $result = Invoke-TestProcess $Installer @('/VERYSILENT', '/SUPPRESSMSGBOXES',
        '/NORESTART', "/DIR=`"$installDir`"", "/LOG=`"$log`"") $Label
    if ($Success) { Assert-True ($result -eq 0) "$Label failed; see $log" }
    else { Assert-True ($result -ne 0) "$Label unexpectedly succeeded; see $log" }
    return $log
}

function Assert-Launcher([string] $Source) {
    Assert-True (Test-Path -LiteralPath $launcherPath -PathType Leaf) 'Launcher is missing'
    Assert-True ((Get-FileHash -LiteralPath $launcherPath).Hash -eq
        (Get-FileHash -LiteralPath $Source).Hash) 'Launcher content differs'
}

function Assert-OldCount([int] $Count) {
    $old = @(Get-ChildItem -LiteralPath $installDir -Filter "$launcherName.*.old" -File)
    Assert-True ($old.Count -eq $Count) "Expected $Count old files, found $($old.Count)"
}

function Start-TestLauncher {
    Assert-UnderTestRoot $launcherPath
    $process = Start-Process -FilePath $launcherPath -ArgumentList '-n 120 127.0.0.1' `
        -WindowStyle Hidden -PassThru
    $processes.Add($process)
    Assert-True (-not $process.WaitForExit(300)) 'Test launcher exited during startup'
    return $process
}

function Assert-Running([Diagnostics.Process] $Process) {
    $Process.Refresh()
    Assert-True (-not $Process.HasExited) 'Running launcher was closed (Restart Manager?)'
}

function Stop-TestLauncher([Diagnostics.Process] $Process) {
    if (-not $Process.HasExited) {
        $Process.Kill()
        Assert-True ($Process.WaitForExit(10000)) 'Test process did not stop'
    }
}

try {
    Assert-True (Test-Path -LiteralPath $IsccPath -PathType Leaf) "ISCC not found: $IsccPath"
    Assert-UnderTestRoot $runRoot
    New-Item -ItemType Directory -Path (Join-Path $runRoot 'temp') -Force | Out-Null
    $env:TEMP = Join-Path $runRoot 'temp'
    $env:TMP = $env:TEMP
    Write-Host "Artifacts: $runRoot"
    Write-Host "PowerShell: $($PSVersionTable.PSVersion)"

    # PE overlays distinguish versions without changing the harmless PING program.
    $sources = @()
    foreach ($version in 1..3) {
        $source = Join-Path $runRoot ("launcher-v$version.exe")
        Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\PING.EXE') -Destination $source
        $stream = [IO.File]::Open($source, [IO.FileMode]::Append)
        try {
            $marker = [Text.Encoding]::ASCII.GetBytes("`r`nMIV-SWAP-TEST-V$version`r`n")
            $stream.Write($marker, 0, $marker.Length)
        } finally { $stream.Dispose() }
        $sources += $source
    }
    $v1 = Build-TestInstaller $sources[0] 'v1'
    $v2 = Build-TestInstaller $sources[1] 'v2'
    $v3 = Build-TestInstaller $sources[2] 'v3'
    $fail = Build-TestInstaller $sources[2] 'fail' '/DFailAfterLauncher'

    # Updates (including fresh installs) must ignore the MCP mutex entirely.
    $mutex = New-Object System.Threading.Mutex($false, "Local\miv-swap-test-mcp-$token")

    Install-Version $v1 '1-fresh' | Out-Null
    Assert-Launcher $sources[0]
    Assert-OldCount 0
    Write-Host 'PASS 1: fresh install'

    Install-Version $v2 '2-idle-update' | Out-Null
    Assert-Launcher $sources[1]
    Assert-OldCount 0
    Write-Host 'PASS 2: idle update'

    $running = Start-TestLauncher
    Install-Version $v3 '3-running-update' | Out-Null
    Assert-Running $running
    Assert-Launcher $sources[2]
    Assert-OldCount 1
    $old = @(Get-ChildItem -LiteralPath $installDir -Filter "$launcherName.*.old" -File)
    Assert-True ((Get-FileHash -LiteralPath $old[0].FullName).Hash -eq
        (Get-FileHash -LiteralPath $sources[1]).Hash) 'Wrong running backup content'
    Write-Host 'PASS 3: running update and Restart Manager exclusion'

    Install-Version $v1 '4-second-update' | Out-Null
    Assert-Running $running
    Assert-Launcher $sources[0]
    Assert-OldCount 1
    Stop-TestLauncher $running
    Install-Version $v2 '4-stopped-update' | Out-Null
    Assert-Launcher $sources[1]
    Assert-OldCount 0
    Write-Host 'PASS 4: consecutive update and cleanup after exit'

    $running = Start-TestLauncher
    $log = Install-Version $fail '5-failure-cancel' $false
    Assert-Running $running
    Assert-Launcher $sources[1]
    Assert-OldCount 0
    $text = Get-Content -LiteralPath $log -Raw -Encoding UTF8
    $hash = (Get-FileHash -LiteralPath $sources[2]).Hash.ToLowerInvariant()
    Assert-True ($text.Contains("Swap test: installed launcher SHA256=$hash")) `
        'Failure did not reach launcher installation'
    # The missing external file defaults to Abort under /SUPPRESSMSGBOXES.
    Assert-True ($text.Contains('User canceled the installation process.')) `
        'Failure did not use the cancellation/rollback path'
    Assert-True ($text.Contains('Launcher swap: restored ')) 'Restore path was not executed'
    Stop-TestLauncher $running
    Write-Host 'PASS 5: failure/cancellation restored original running launcher'

    # A non-delete-sharing handle makes the rename fail before installation.
    $lockStream = [IO.File]::Open($launcherPath, [IO.FileMode]::Open,
        [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try { Install-Version $v3 'rename-denied' $false | Out-Null }
    finally { $lockStream.Dispose(); $lockStream = $null }
    Assert-Launcher $sources[1]
    Assert-OldCount 0
    Write-Host 'PASS extra: rename denial preserves original launcher'

    $uninstaller = Join-Path $installDir 'unins000.exe'
    # The same test-only mutex must block uninstall. No production mutex is used.
    try {
        $result = Invoke-TestProcess $uninstaller @('/VERYSILENT', '/SUPPRESSMSGBOXES',
            '/NORESTART', "/LOG=`"$(Join-Path $runRoot '6-mcp-blocked.log')`"") '6-mcp-blocked'
        Assert-True ($result -ne 0) 'Uninstall was not blocked by test MCP mutex'
        Assert-Launcher $sources[1]
        Write-Host 'PASS 6a: uninstall blocked by test MCP mutex'
    } finally { $mutex.Dispose(); $mutex = $null }

    Copy-Item -LiteralPath $sources[0] -Destination ($launcherPath + '.unused.old')
    $result = Invoke-TestProcess $uninstaller @('/VERYSILENT', '/SUPPRESSMSGBOXES',
        '/NORESTART', "/LOG=`"$(Join-Path $runRoot '6-uninstall.log')`"") '6-uninstall'
    Assert-True ($result -eq 0) 'Uninstall failed'
    Assert-True (-not (Test-Path -LiteralPath $launcherPath)) 'Uninstall left launcher'
    Assert-True (-not (Test-Path -LiteralPath ($launcherPath + '.unused.old'))) 'Uninstall left old file'
    Write-Host 'PASS 6b: uninstall removed launcher and unused backup'
    $exitCode = 0
} catch {
    Write-Host ("FAIL: {0}" -f $_.Exception.Message)
    Write-Host $_.ScriptStackTrace
} finally {
    if ($null -ne $lockStream) { $lockStream.Dispose() }
    if ($null -ne $mutex) { $mutex.Dispose() }
    foreach ($process in $processes) {
        try { Stop-TestLauncher $process }
        catch { Write-Host "Cleanup process failed: $_"; $exitCode = 1 }
        finally { $process.Dispose() }
    }
    try { Remove-TestInstall }
    catch { Write-Host "Cleanup directory failed: $_"; $exitCode = 1 }
    $env:TEMP = $savedTemp
    $env:TMP = $savedTmp
    Write-Host "Logs retained: $runRoot"
}
exit $exitCode
