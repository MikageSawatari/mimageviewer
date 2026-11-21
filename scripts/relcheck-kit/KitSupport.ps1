# Function-only support; dot-sourcing performs no I/O or process/UI work.
function Assert-ReleaseCheckKitNoReparsePath {
    param([string]$Path)
    $current=[IO.Path]::GetFullPath($Path)
    while($current){
        if(Test-Path -LiteralPath $current){
            if((Get-Item -LiteralPath $current -Force -ErrorAction Stop).Attributes -band [IO.FileAttributes]::ReparsePoint){
                throw "Kit path contains a reparse point: $current"
            }
        }
        $parent=[IO.Path]::GetDirectoryName($current)
        if($parent -eq $current){break}
        $current=$parent
    }
}

function Get-ReleaseCheckKitFileRecords {
    param([string]$KitRoot)
    $root=[IO.Path]::GetFullPath($KitRoot).TrimEnd('\','/')
    Assert-ReleaseCheckKitNoReparsePath $root
    $pending=New-Object Collections.Stack
    $pending.Push((Get-Item -LiteralPath $root -Force -ErrorAction Stop))
    $files=New-Object 'Collections.Generic.Dictionary[string,object]' ([StringComparer]::Ordinal)
    while($pending.Count){
        $item=$pending.Pop()
        if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw "Kit contains a reparse point: $($item.FullName)"}
        if(($item.PSIsContainer -and $item.Name -eq '__pycache__') -or
            (-not $item.PSIsContainer -and $item.Name -like '*.pyc')){
            throw "Kit contains extra Python bytecode (__pycache__ / *.pyc): $($item.FullName)"
        }
        if($item.PSIsContainer){
            foreach($child in (Get-ChildItem -LiteralPath $item.FullName -Force -ErrorAction Stop)){$pending.Push($child)}
            continue
        }
        $relative=$item.FullName.Substring($root.Length+1).Replace('\','/')
        # Still inspect excluded trees for reparse points; never follow links.
        if($relative -in @('release-check-kit.json','release-check-kit.sha256') -or
            $relative -match '^(target/portable-smoke/|target/ui-smoke/)'){continue}
        $files.Add($relative,[pscustomobject][ordered]@{path=$relative;sha256=(Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant();size=$item.Length})
    }
    [string[]]$paths=@($files.Keys)
    [Array]::Sort($paths,[StringComparer]::Ordinal)
    foreach($path in $paths){$files[$path]}
}

function Read-ReleaseCheckKitManifest {
    param([string]$KitRoot)
    $root=[IO.Path]::GetFullPath($KitRoot).TrimEnd('\','/')
    Assert-ReleaseCheckKitNoReparsePath $root
    if(Test-Path -LiteralPath (Join-Path $root '.git')){throw 'FromKit refuses a repository root (.git exists).'}
    # Bounded sampler validation: inspect only manifest/record paths, never the tree.
    foreach($relative in @('release-check-kit.json','release-check-kit.sha256','source-fingerprint-records.txt','target\portable-smoke-package\.test-script-build.json')){
        Assert-ReleaseCheckKitNoReparsePath (Join-Path $root $relative)
    }
    $jsonPath=Join-Path $root 'release-check-kit.json'
    $digest=Get-Content -LiteralPath (Join-Path $root 'release-check-kit.sha256') -Encoding ASCII -Raw -ErrorAction Stop
    if($digest -cnotmatch '\A[0-9a-f]{64}(\r?\n)?\z' -or
        $digest.TrimEnd("`r","`n") -cne (Get-FileHash -LiteralPath $jsonPath -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()){
        throw 'Kit manifest SHA-256 mismatch.'
    }
    $kit=Get-Content -LiteralPath $jsonPath -Raw -Encoding UTF8 -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
    if($kit.schema_version -ne 1){throw 'Unsupported kit schema.'}
    $buildRoot=$kit.artifact.PSObject.Properties['build_root']
    if($null -eq $buildRoot -or $buildRoot.Value -isnot [string] -or
        [string]::IsNullOrWhiteSpace($buildRoot.Value) -or
        $buildRoot.Value -notmatch '^(?:[A-Za-z]:[\\/]|[\\/]{2}[^\\/]+[\\/][^\\/]+(?:[\\/]|$))'){
        throw 'Kit artifact.build_root must be a nonempty absolute path; recreate the kit.'
    }
    $null=[IO.Path]::GetFullPath($buildRoot.Value)
    $fingerprint=[string]$kit.artifact.source_fingerprint_sha256
    if($fingerprint -cnotmatch '\A[0-9a-f]{64}\z' -or $fingerprint -cne
        (Get-FileHash -LiteralPath (Join-Path $root 'source-fingerprint-records.txt') -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()){
        throw 'Kit source fingerprint records mismatch.'
    }
    $package=Join-Path $root 'target\portable-smoke-package'
    $manifest=Get-Content -LiteralPath (Join-Path $package '.test-script-build.json') -Encoding UTF8 -Raw -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
    $recorded=$kit.artifact.manifest
    foreach($m in @($manifest,$recorded)){
        if($m.schema_version -ne 1 -or $m.artifact_flavor -cne 'portable-test-script' -or
            $m.cargo_profile -cne 'release' -or @($m.features).Count -ne 2 -or
            $m.features[0] -cne 'portable' -or $m.features[1] -cne 'test-script' -or
            $m.source_fingerprint_sha256 -cne $fingerprint){throw 'Invalid kit diagnostic manifest.'}
    }
    foreach($field in @('source_head','source_fingerprint_sha256','core_sha256','remote_sha256','epub_worker_sha256','mcp_sha256','musicvideo_sha256')){
        if([string]::IsNullOrWhiteSpace([string]$manifest.$field) -or $manifest.$field -cne $recorded.$field){throw "Kit artifact manifest mismatch: $field"}
    }
    if(($manifest | ConvertTo-Json -Depth 20 -Compress) -cne ($recorded | ConvertTo-Json -Depth 20 -Compress)){
        throw 'Kit artifact manifest content mismatch.'
    }
    return $kit
}

function Assert-ReleaseCheckKitRunRoot {
    param([string]$KitRoot)
    $root=[IO.Path]::GetFullPath($KitRoot).TrimEnd('\','/')
    $kit=Read-ReleaseCheckKitManifest $root
    $buildRoot=[IO.Path]::GetFullPath($kit.artifact.build_root).TrimEnd('\','/')
    if(-not $root.Equals($buildRoot,[StringComparison]::OrdinalIgnoreCase)){
        throw "Kit must be placed at $buildRoot (the diagnostic build validates evidence paths against its build root); current root is $root."
    }
}

function Assert-ReleaseCheckDiagnosticBuildRoot {
    param([string]$ExePath,[string]$BuildRoot)
    Assert-ReleaseCheckKitNoReparsePath $ExePath
    if(-not ('MivReleaseCheckKitBytes' -as [type])){
        # Search raw bytes in native code; PowerShell per-byte loops are too slow
        # for a diagnostic core. The executable is read once, never launched.
        Add-Type @'
public static class MivReleaseCheckKitBytes {
    public static bool Contains(byte[] bytes, byte[] needle) {
        if (needle.Length == 0) return false;
        for (int offset = 0; offset <= bytes.Length - needle.Length; offset++) {
            if (bytes[offset] != needle[0]) continue;
            int n = 1;
            while (n < needle.Length && bytes[offset + n] == needle[n]) n++;
            if (n == needle.Length) return true;
        }
        return false;
    }
}
'@
    }
    $bytes=[IO.File]::ReadAllBytes($ExePath)
    $needle=[Text.Encoding]::UTF8.GetBytes($BuildRoot)
    if(-not [MivReleaseCheckKitBytes]::Contains($bytes,$needle)){
        throw 'diagnostic exe does not embed the recorded build root'
    }
}

function Get-ReleaseCheckKitExpectedFingerprint {
    param([string]$KitRoot)
    $kit=Read-ReleaseCheckKitManifest $KitRoot
    return [string]$kit.artifact.source_fingerprint_sha256
}

function Assert-ReleaseCheckKit {
    param([Alias('Kit')][string]$KitRoot)
    $root=[IO.Path]::GetFullPath($KitRoot).TrimEnd('\','/')
    Assert-ReleaseCheckKitNoReparsePath $root
    if(Test-Path -LiteralPath (Join-Path $root '.git')){throw 'FromKit refuses a repository root (.git exists).'}
    $actual=@(Get-ReleaseCheckKitFileRecords $root)
    $kit=Read-ReleaseCheckKitManifest $root
    $expected=@($kit.files)
    if($expected.Count -ne $actual.Count){throw 'Kit file count mismatch (missing or extra files).'}
    for($i=0;$i -lt $actual.Count;$i++){
        if($expected[$i].path -cne $actual[$i].path -or $expected[$i].sha256 -cne $actual[$i].sha256 -or
            $expected[$i].size -ne $actual[$i].size){throw "Kit file mismatch: $($actual[$i].path)"}
    }
    if(@($actual | Where-Object {$_.path -ceq 'tools/ffmpeg/ffmpeg.exe'}).Count -ne 1){throw 'Kit ffmpeg.exe missing.'}
    $package=Join-Path $root 'target\portable-smoke-package'
    $manifest=$kit.artifact.manifest
    $executables=[ordered]@{core='mimageviewer.exe';remote='mimageviewer-remote.exe';epub_worker='mimageviewer-epub-pdf.exe';mcp='mimageviewer-mcp.exe';musicvideo='mimageviewer-musicvideo.exe'}
    foreach($entry in $executables.GetEnumerator()){
        if($manifest.($entry.Key+'_sha256') -cne (Get-FileHash -LiteralPath (Join-Path $package $entry.Value) -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()){
            throw "Kit executable hash mismatch: $($entry.Value)"
        }
    }
    return [string]$kit.artifact.source_fingerprint_sha256
}
