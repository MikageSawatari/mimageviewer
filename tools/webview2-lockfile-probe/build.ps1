param(
    [string]$ChocHeader = 'G:\home\effetune-mixwright-v015\third_party\choc\choc\gui\choc_WebView.h',
    [string]$VcVars = 'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$out = Join-Path $repo 'target\webview2-lockfile-probe\bin'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-ProbePath $out
[IO.Directory]::CreateDirectory($out) | Out-Null
$header = [IO.File]::ReadAllText($ChocHeader)
$start = $header.IndexOf('#ifndef __webview2_h__')
$end = $header.IndexOf('#endif // __webview2_h__', $start)
if ($start -lt 0 -or $end -lt 0) { throw 'CHOC COM declaration boundary not found; no download attempted.' }
$declarations = $header.Substring($start, $end - $start) + "`n#endif`n"
$declarations = "#include <eventtoken.h>`n" + $declarations.Replace('struct EventRegistrationToken { __int64 value; };', '')
[IO.File]::WriteAllText((Join-Path $out 'choc_webview2_declarations.h'), $declarations)
$blobs = [regex]::Matches($header, 'static constexpr unsigned char dllData\[(\d+)\] = \{([\s\S]*?)\};')
$x64 = @($blobs | Where-Object { $_.Groups[1].Value -eq '162392' })
if ($x64.Count -ne 1) { throw 'Expected CHOC x64 loader blob missing; no download attempted.' }
$bytes = [byte[]]@([regex]::Matches($x64[0].Groups[2].Value, '\d+') | ForEach-Object { [byte]$_.Value })
if ($bytes.Length -ne 162392) { throw 'CHOC loader byte count mismatch.' }
$pe = [BitConverter]::ToInt32($bytes, 60)
if ([BitConverter]::ToUInt16($bytes, $pe + 4) -ne 0x8664) { throw 'Loader is not x64.' }
[IO.File]::WriteAllBytes((Join-Path $out 'WebView2Loader.dll'), $bytes)
# Preserve the notices adjacent to the loader, without invoking any CHOC host code.
$noticeMatch = [regex]::Match($header, 'CHOC_REGISTER_OPEN_SOURCE_LICENCE \(WebView2Loader, R"\(([\s\S]*?)\)"\)')
$license = $header.Substring(0, $header.IndexOf('#ifndef CHOC_WEBVIEW_HEADER_INCLUDED'))
$license += "`nOriginal CHOC header: $ChocHeader`nWebView2 loader: 1.0.2365.46 (embedded x64 bytes).`n"
if (-not $noticeMatch.Success) { throw 'CHOC WebView2Loader license boundary not found.' }
$license += $noticeMatch.Groups[1].Value
[IO.File]::WriteAllText((Join-Path $out 'THIRD-PARTY-NOTICES.txt'), $license)
$source = Join-Path $PSScriptRoot 'probe.cpp'
$exe = Join-Path $out 'probe.exe'
$buildCmd = Join-Path $out 'compile.cmd'
# Paths come from trusted build parameters; cmd metacharacters are rejected.
foreach ($p in @($VcVars, $out, $source)) {
    if ($p -match '[%&|<>^!\r\n"]') { throw "Unsupported command path: $p" }
}
$installer = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer'
$lines = @('@echo off', 'set MSBUILDDISABLENODEREUSE=1')
if ([IO.File]::Exists((Join-Path $installer 'vswhere.exe'))) { $lines += ('set "PATH={0};%PATH%"' -f $installer) }
$lines += @(('call "{0}"' -f $VcVars),
    'if errorlevel 1 exit /b %errorlevel%',
    ('cl /nologo /std:c++17 /EHsc /W4 /WX /utf-8 /DUNICODE /D_UNICODE /DNOMINMAX /I"{0}" "{1}" /Fe:"{2}" /Fo:"{0}\probe.obj" /Fd:"{0}\probe.pdb" /link user32.lib ole32.lib shell32.lib shlwapi.lib rstrtmgr.lib bcrypt.lib wbemuuid.lib oleaut32.lib' -f $out, $source, $exe),
    'exit /b %errorlevel%')
[IO.File]::WriteAllLines($buildCmd, $lines, [Text.Encoding]::ASCII)
& $env:ComSpec /d /c $buildCmd
if ($LASTEXITCODE -ne 0) { throw "Probe build failed: $LASTEXITCODE" }
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'page.html') -Destination (Join-Path $out 'page.html')
$metadata = [ordered]@{ built_utc = [DateTime]::UtcNow.ToString('o'); choc_header = $ChocHeader;
    choc_sha256 = (Get-FileHash -LiteralPath $ChocHeader -Algorithm SHA256).Hash;
    source_sha256 = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash;
    snapshot_sha256 = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'snapshot.h') -Algorithm SHA256).Hash;
    resource_transport = 'WebResourceRequested (CHOC base interfaces only)';
    page_sha256 = (Get-FileHash -LiteralPath (Join-Path $out 'page.html') -Algorithm SHA256).Hash;
    loader_sha256 = (Get-FileHash -LiteralPath (Join-Path $out 'WebView2Loader.dll') -Algorithm SHA256).Hash;
    exe_sha256 = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash }
Save-Json (Join-Path $out 'build.json') $metadata
Write-Host "Built: $exe"

