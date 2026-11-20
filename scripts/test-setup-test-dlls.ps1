# File-only FFmpeg staging regression checks. No Cargo or application launches.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$fixture = Join-Path $repo ('target/setup-test-dlls-regression-' + [guid]::NewGuid().ToString('N'))
$target = Join-Path $fixture 'custom target'
$vendor = Join-Path $fixture 'vendor/ffmpeg/bin'
$setup = Join-Path $PSScriptRoot 'setup-test-dlls.ps1'
$names = @('avcodec-61.dll', 'avformat-61.dll', 'avutil-59.dll',
    'avfilter-10.dll', 'swscale-8.dll', 'swresample-5.dll')
function Assert-That {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw "[test-setup-test-dlls] $Message" }
}
[void] [IO.Directory]::CreateDirectory($vendor)
try {
    foreach ($name in $names) { [IO.File]::WriteAllText((Join-Path $vendor $name), "fixture:$name") }
    $deps = Join-Path $target 'debug/deps'
    [void] [IO.Directory]::CreateDirectory($deps)
    $sentinel = Join-Path $deps 'avcodec-61.dll'
    [IO.File]::WriteAllText($sentinel, 'legacy deps must stay unchanged')
    $stamp = [DateTime]::UtcNow.AddDays(-2)
    [IO.File]::SetLastWriteTimeUtc($sentinel, $stamp)
    $sentinelStamp = [IO.File]::GetLastWriteTimeUtc($sentinel)

    # Omitted CargoProfile means test -> debug. Explicit TargetDir avoids Cargo.
    & $setup -RepoRoot $fixture -TargetDir $target
    foreach ($name in $names) {
        $destination = Join-Path $target "debug/$name"
        Assert-That ([IO.File]::ReadAllText($destination) -ceq "fixture:$name") "wrong copy: $name"
        Assert-That (-not ((Get-Item -LiteralPath $destination).Attributes -band [IO.FileAttributes]::ReparsePoint)) "copy is a link: $name"
        [IO.File]::SetLastWriteTimeUtc($destination, $stamp)
    }
    # Deny writes as well as checking mtime: equal content must never be copied.
    $locks = @()
    try {
        foreach ($name in $names) {
            $locks += [IO.File]::Open((Join-Path $target "debug/$name"), 'Open', 'Read', 'Read')
        }
        & $setup -RepoRoot $fixture -TargetDir $target -CargoProfile test,dev
    } finally { foreach ($handle in $locks) { $handle.Dispose() } }
    foreach ($name in $names) {
        Assert-That ([IO.File]::GetLastWriteTimeUtc((Join-Path $target "debug/$name")) -eq $stamp) "unchanged copy was written: $name"
    }

    [IO.File]::WriteAllText((Join-Path $vendor $names[0]), 'updated vendor bytes')
    & $setup -RepoRoot $fixture -TargetDir $target -CargoProfile test,dev,bench,release,dev-runtime,custom-profile
    foreach ($dir in @('debug', 'release', 'dev-runtime', 'custom-profile')) {
        foreach ($name in $names) {
            Assert-That ([IO.File]::ReadAllText((Join-Path $target "$dir/$name")) -ceq
                [IO.File]::ReadAllText((Join-Path $vendor $name))) "profile mapping/copy failed: $dir/$name"
        }
        if ($dir -ne 'debug') {
            Assert-That (-not [IO.Directory]::Exists((Join-Path $target "$dir/deps"))) "created deps: $dir"
        }
    }
    foreach ($dir in @('test', 'dev', 'bench')) {
        Assert-That (-not [IO.Directory]::Exists((Join-Path $target $dir))) "unmapped profile directory: $dir"
    }
    Assert-That ([IO.File]::ReadAllText($sentinel) -ceq 'legacy deps must stay unchanged') 'changed a deps DLL'
    Assert-That ([IO.File]::GetLastWriteTimeUtc($sentinel) -eq $sentinelStamp) 'wrote a deps DLL'
    Assert-That (@(Get-ChildItem -LiteralPath $deps -Force).Count -eq 1) 'added files in deps'

    # Every required DLL is fatal when missing, before even creating outputs.
    foreach ($name in $names) {
        $source = Join-Path $vendor $name
        $content = [IO.File]::ReadAllText($source)
        [IO.File]::Delete($source)
        $missingTarget = Join-Path $fixture ("missing-$name")
        $message = ''
        try { & $setup -RepoRoot $fixture -TargetDir $missingTarget }
        catch { $message = $_.Exception.Message }
        finally { [IO.File]::WriteAllText($source, $content) }
        Assert-That ($message.Contains($name) -and $message.Contains('bash scripts/setup-ffmpeg.sh')) "missing input did not fail with recovery instructions: $name"
        Assert-That (-not [IO.Directory]::Exists($missingTarget)) "partially staged missing input: $name"
    }
    Write-Host '[test-setup-test-dlls] PASS: copy, unchanged bytes, replacement, profiles, deps isolation, missing inputs'
} finally {
    $fixtureFull = [IO.Path]::GetFullPath($fixture)
    $targetRoot = [IO.Path]::GetFullPath((Join-Path $repo 'target')).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    Assert-That ($fixtureFull.StartsWith($targetRoot, [StringComparison]::OrdinalIgnoreCase)) 'cleanup escaped target'
    $directories = New-Object 'System.Collections.Generic.Stack[string]'
    $directories.Push($fixtureFull)
    while ($directories.Count) {
        $directory = $directories.Pop()
        Assert-That (-not ((Get-Item -LiteralPath $directory).Attributes -band [IO.FileAttributes]::ReparsePoint)) 'refusing cleanup through a reparse point'
        foreach ($item in (Get-ChildItem -LiteralPath $directory -Force)) {
            Assert-That (-not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) 'refusing cleanup through a reparse point'
            if ($item.PSIsContainer) { $directories.Push($item.FullName) }
        }
    }
    Remove-Item -LiteralPath $fixtureFull -Recurse -Force
}
