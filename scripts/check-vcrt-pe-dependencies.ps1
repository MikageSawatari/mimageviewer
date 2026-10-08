[CmdletBinding()]
param(
    [string[]] $InputPaths = @(),
    [switch] $RequireCompanionRuntime,
    [switch] $SelfTest,
    [string] $ReportPath
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
. (Join-Path $PSScriptRoot 'sign-files.ps1')
$vcrtDir = Join-Path $repoRoot 'vendor\vcrt'
$manifestPath = Join-Path $vcrtDir 'provenance.json'
$allowedRuntime = @(
    'msvcp140.dll',
    'msvcp140_1.dll',
    'vcruntime140.dll',
    'vcruntime140_1.dll'
)

function Get-DumpbinPath {
    $command = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $candidates = @(Get-ChildItem -Path 'C:\Program Files (x86)\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe' -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending)
    if ($candidates.Count -eq 0) {
        throw '[vcrt-pe] dumpbin.exe was not found'
    }
    return $candidates[0].FullName
}

function Get-PeInfo {
    param([string] $Path, [string] $Dumpbin)
    $headers = @(& $Dumpbin /nologo /headers $Path)
    if ($LASTEXITCODE -ne 0) { throw "[vcrt-pe] dumpbin /headers failed: $Path" }
    $machine = if ($headers -match '8664 machine \(x64\)') {
        'x64'
    } elseif ($headers -match '14C machine \(x86\)') {
        'x86'
    } else {
        throw "[vcrt-pe] unsupported or unknown machine: $Path"
    }
    $linker = $null
    foreach ($line in $headers) {
        if ($line -match '^\s*([0-9]+\.[0-9]+) linker version\s*$') {
            $linker = $Matches[1]
            break
        }
    }
    $dependentOutput = @(& $Dumpbin /nologo /dependents $Path)
    if ($LASTEXITCODE -ne 0) { throw "[vcrt-pe] dumpbin /dependents failed: $Path" }
    $imports = @($dependentOutput | ForEach-Object {
        if ($_ -match '^\s*([A-Za-z0-9._-]+\.dll)\s*$') { $Matches[1].ToLowerInvariant() }
    } | Where-Object { $_ } | Sort-Object -Unique)
    [pscustomobject]@{
        path = [System.IO.Path]::GetFullPath($Path)
        machine = $machine
        linker_version = $linker
        imports = $imports
    }
}

function Test-VcRuntimeImportName {
    param([string] $Name)
    return $Name -match '^(msvcp|vcruntime|concrt)[a-z0-9_]*\.dll$'
}

function Get-MivGatePeFiles {
    param([string] $Path)
    $inputItem = Get-Item -LiteralPath $Path -Force
    if (-not $inputItem.PSIsContainer -and -not (Test-MivPeFile -Path $inputItem.FullName)) {
        throw "[vcrt-pe] explicit artifact is not a valid PE: $($inputItem.FullName)"
    }
    foreach ($file in Get-MivTreeFiles -Path $Path) {
        $isPe = Test-MivPeFile -Path $file.FullName
        # Known PE artifacts must fail closed if corrupted. Header enumeration
        # additionally includes valid .vst3/extensionless PEs among resources.
        if (-not $isPe -and $file.Extension -in @('.exe', '.dll', '.vst3')) {
            throw "[vcrt-pe] known PE artifact has an invalid header: $($file.FullName)"
        }
        if ($isPe) { $file }
    }
}

function Get-ExpectedMachineForArtifact {
    param([string] $Name)
    if ($Name.ToLowerInvariant() -in @('mimageviewer-susie32.exe', 'mimageviewer_setup.exe')) {
        return 'x86'
    }
    return 'x64'
}

function Assert-AllowedVcImports {
    param([object] $Pe)
    $vcImports = @($Pe.imports | Where-Object { Test-VcRuntimeImportName $_ })
    $unknown = @($vcImports | Where-Object { $allowedRuntime -notcontains $_ })
    if ($unknown.Count -ne 0) {
        throw "[vcrt-pe] unknown VC runtime import in $($Pe.path): $($unknown -join ', ')"
    }
}

function Assert-MicrosoftSignature {
    param([string] $Path, [string] $SubjectContains)
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    $subject = if ($signature.SignerCertificate) {
        $signature.SignerCertificate.Subject
    } else {
        ''
    }
    if ($signature.Status -ne 'Valid' -or -not $subject.Contains($SubjectContains)) {
        throw "[vcrt-pe] Microsoft signature is not valid: $Path status=$($signature.Status) subject=$subject"
    }
    return $signature
}

function Assert-CanonicalRuntimeFile {
    param([string] $Path, [object] $Entry, [string] $Dumpbin, [string] $SignerSubjectContains)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "[vcrt-pe] runtime file is missing: $Path"
    }
    $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant()
    if ($hash -ne $Entry.sha256.ToUpperInvariant()) {
        throw "[vcrt-pe] hash mismatch: $Path expected=$($Entry.sha256) actual=$hash"
    }
    $version = (Get-Item -LiteralPath $Path).VersionInfo.FileVersion
    if ($version -ne $Entry.version) {
        throw "[vcrt-pe] version mismatch: $Path expected=$($Entry.version) actual=$version"
    }
    $signature = Assert-MicrosoftSignature -Path $Path -SubjectContains $SignerSubjectContains
    $pe = Get-PeInfo -Path $Path -Dumpbin $Dumpbin
    if ($pe.machine -ne 'x64') { throw "[vcrt-pe] runtime is not x64: $Path" }
    Assert-AllowedVcImports -Pe $pe
    return [pscustomobject]@{
        path = $pe.path
        machine = $pe.machine
        linker_version = $pe.linker_version
        imports = $pe.imports
        sha256 = $hash
        version = $version
        signature = $signature.Status.ToString()
        signer_subject = $signature.SignerCertificate.Subject
    }
}

if ($SelfTest) {
    foreach ($name in $allowedRuntime) {
        if (-not (Test-VcRuntimeImportName $name)) { throw "self-test rejected $name" }
    }
    foreach ($name in @('msvcp140_2.dll', 'vcruntime140_threads.dll', 'concrt140.dll')) {
        if (-not (Test-VcRuntimeImportName $name)) { throw "self-test missed $name" }
        if ($allowedRuntime -contains $name) { throw "self-test allowed unknown runtime $name" }
    }
    if (Test-VcRuntimeImportName 'kernel32.dll') { throw 'self-test classified an OS DLL as VC runtime' }
    if ((Get-ExpectedMachineForArtifact 'mimageviewer-susie32.exe') -ne 'x86') {
        throw 'self-test lost the Susie x86 machine exception'
    }
    if ((Get-ExpectedMachineForArtifact 'mImageViewer_setup.exe') -ne 'x86') {
        throw 'self-test lost the Inno bootstrapper x86 machine exception'
    }
    if ((Get-ExpectedMachineForArtifact 'mimageviewer-core.exe') -ne 'x64') {
        throw 'self-test widened the normal x64 machine expectation'
    }
    $temp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
    $testDir = Join-Path $temp ('miv-pe-gate-test-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $testDir | Out-Null
    try {
        $bytes = New-Object byte[] 128
        $bytes[0] = 0x4d; $bytes[1] = 0x5a; $bytes[0x3c] = 0x40
        $bytes[64] = 0x50; $bytes[65] = 0x45
        foreach ($name in @('plugin.vst3', 'extensionless')) {
            [System.IO.File]::WriteAllBytes((Join-Path $testDir $name), $bytes)
        }
        [System.IO.File]::WriteAllText((Join-Path $testDir 'resource.txt'), 'resource')
        if (@(Get-MivGatePeFiles $testDir).Count -ne 2) {
            throw 'self-test missed header-based PE artifacts'
        }
        $rejected = $false
        try { $null = @(Get-MivGatePeFiles (Join-Path $testDir 'resource.txt')) } catch {
            if ($_.Exception.Message -notmatch 'explicit artifact is not a valid PE') { throw }
            $rejected = $true
        }
        if (-not $rejected) { throw 'self-test accepted an explicit non-PE with an unrecognized extension' }
        foreach ($name in @('corrupt.exe', 'corrupt.dll', 'corrupt.vst3')) {
            $corrupt = Join-Path $testDir $name
            [System.IO.File]::WriteAllText($corrupt, 'MZ')
            $rejected = $false
            try { $null = @(Get-MivGatePeFiles $testDir) } catch {
                if ($_.Exception.Message -notmatch 'known PE artifact has an invalid header') { throw }
                $rejected = $true
            }
            if (-not $rejected) { throw "self-test skipped corrupted artifact $name" }
            Remove-Item -LiteralPath $corrupt
        }
    } finally {
        $resolved = [System.IO.Path]::GetFullPath($testDir).TrimEnd('\')
        if (-not $resolved.StartsWith($temp + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'self-test cleanup escaped temp'
        }
        $null = @(Get-MivTreeFiles -Path $resolved)
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
    Write-Host '[vcrt-pe] parser self-test passed'
    return
}

if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "[vcrt-pe] provenance manifest is missing: $manifestPath"
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$dumpbin = Get-DumpbinPath
$manifestNames = @($manifest.files | ForEach-Object { $_.name.ToLowerInvariant() })
if (@(Compare-Object -ReferenceObject ($allowedRuntime | Sort-Object) -DifferenceObject ($manifestNames | Sort-Object)).Count -ne 0) {
    throw '[vcrt-pe] provenance manifest file set does not match the four-file runtime closure'
}
$manifestVersions = @($manifest.files | ForEach-Object { $_.version } | Sort-Object -Unique)
if ($manifestVersions.Count -ne 1) {
    throw "[vcrt-pe] the four canonical runtime files must declare one identical version: $($manifestVersions -join ', ')"
}
try {
    $canonicalVersion = [version]$manifestVersions[0]
    $minimumVersion = [version]$manifest.minimum_required_version
} catch {
    throw "[vcrt-pe] invalid runtime version in provenance manifest: $($_.Exception.Message)"
}
if ($canonicalVersion -lt $minimumVersion) {
    throw "[vcrt-pe] canonical runtime is older than the required version: actual=$canonicalVersion minimum=$minimumVersion"
}

$runtimeReports = @()
foreach ($entry in $manifest.files) {
    $path = Join-Path $vcrtDir $entry.name
    $runtimeReports += Assert-CanonicalRuntimeFile -Path $path -Entry $entry -Dumpbin $dumpbin `
        -SignerSubjectContains $manifest.signer_subject_contains
}

$inputFiles = @()
foreach ($inputPath in $InputPaths) {
    $resolvedInput = if ([System.IO.Path]::IsPathRooted($inputPath)) {
        $inputPath
    } else {
        Join-Path $repoRoot $inputPath
    }
    if (-not (Test-Path -LiteralPath $resolvedInput)) {
        throw "[vcrt-pe] input path is missing: $resolvedInput"
    }
    # Header-based enumeration includes plugin DLLs with a .vst3 extension and
    # future extensionless PE artifacts; resources are not passed to dumpbin.
    $inputFiles += Get-MivGatePeFiles -Path $resolvedInput
}
$inputFiles = @($inputFiles | Sort-Object FullName -Unique)
$peReports = @()
foreach ($file in $inputFiles) {
    $pe = Get-PeInfo -Path $file.FullName -Dumpbin $dumpbin
    $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToUpperInvariant()
    $expectedMachine = Get-ExpectedMachineForArtifact $file.Name
    if ($pe.machine -ne $expectedMachine) {
        throw "[vcrt-pe] unexpected machine for $($file.FullName): expected=$expectedMachine actual=$($pe.machine)"
    }
    Assert-AllowedVcImports -Pe $pe
    $version = $null
    $signatureStatus = $null
    $signerSubject = $null
    $lowerName = $file.Name.ToLowerInvariant()
    if ($allowedRuntime -contains $lowerName) {
        $entry = @($manifest.files | Where-Object { $_.name.ToLowerInvariant() -eq $lowerName })[0]
        $canonical = Assert-CanonicalRuntimeFile -Path $file.FullName -Entry $entry -Dumpbin $dumpbin `
            -SignerSubjectContains $manifest.signer_subject_contains
        $version = $canonical.version
        $signatureStatus = $canonical.signature
        $signerSubject = $canonical.signer_subject
    } elseif ($lowerName -like 'onnxruntime*.dll') {
        $signature = Assert-MicrosoftSignature -Path $file.FullName -SubjectContains 'Microsoft Corporation'
        $version = $file.VersionInfo.FileVersion
        $signatureStatus = $signature.Status.ToString()
        $signerSubject = $signature.SignerCertificate.Subject
    }
    if ($RequireCompanionRuntime -and $file.Extension -eq '.exe') {
        foreach ($name in $allowedRuntime) {
            $entry = @($manifest.files | Where-Object { $_.name.ToLowerInvariant() -eq $name })[0]
            $companionPath = Join-Path $file.DirectoryName $name
            $null = Assert-CanonicalRuntimeFile -Path $companionPath -Entry $entry -Dumpbin $dumpbin `
                -SignerSubjectContains $manifest.signer_subject_contains
        }
    }
    $peReports += [pscustomobject]@{
        path = $pe.path
        machine = $pe.machine
        linker_version = $pe.linker_version
        imports = $pe.imports
        sha256 = $hash
        version = $version
        signature = $signatureStatus
        signer_subject = $signerSubject
    }
}

$report = [ordered]@{
    generated_utc = [DateTime]::UtcNow.ToString('o')
    provenance = $manifestPath
    runtime = $runtimeReports
    pe = $peReports
}
if ($ReportPath) {
    $resolvedReport = if ([System.IO.Path]::IsPathRooted($ReportPath)) {
        $ReportPath
    } else {
        Join-Path $repoRoot $ReportPath
    }
    $parent = Split-Path -Parent $resolvedReport
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $resolvedReport -Encoding UTF8
    Write-Host "[vcrt-pe] report: $resolvedReport"
}
Write-Host ("[vcrt-pe] passed: runtime={0} pe={1}" -f $runtimeReports.Count, $peReports.Count)
