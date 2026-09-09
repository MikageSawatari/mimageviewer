[CmdletBinding()]
param(
    [switch] $Probe,
    [string] $ProbeOutput
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$buildScript = Join-Path $PSScriptRoot 'build-portable.ps1'

function Import-FunctionFromScript {
    param(
        [string] $Path,
        [string] $Name
    )

    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref] $tokens, [ref] $errors)
    if ($errors.Count -ne 0) {
        throw "failed to parse ${Path}: $($errors[0].Message)"
    }
    $definition = $ast.FindAll({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -ceq $Name
        }, $true)
    if ($definition.Count -ne 1) {
        throw "expected exactly one function ${Name} in ${Path}, found $($definition.Count)"
    }
    Set-Item -Path "Function:script:$Name" -Value $definition[0].Body.GetScriptBlock()
}

foreach ($functionName in @(
        'Get-NormalizedPath',
        'Get-MivOrdinalUniquePaths',
        'Get-MivSourceFingerprintRecords',
        'Get-MivSourceFingerprint'
    )) {
    Import-FunctionFromScript $buildScript $functionName
}

$representativeInput = @('x/a-b', 'x/ab', 'x/a_b', 'x/A', 'x/a', 'x/A')
$representativeExpected = @('x/A', 'x/a', 'x/a-b', 'x/a_b', 'x/ab')

function Assert-ExactSequence {
    param(
        [object[]] $Actual,
        [object[]] $Expected,
        [string] $Label
    )

    if ($Actual.Count -ne $Expected.Count) {
        throw "${Label}: count $($Actual.Count), expected $($Expected.Count)"
    }
    for ($index = 0; $index -lt $Expected.Count; $index++) {
        if (-not [System.StringComparer]::Ordinal.Equals([string] $Actual[$index], [string] $Expected[$index])) {
            throw "${Label}: item ${index} '$($Actual[$index])', expected '$($Expected[$index])'"
        }
    }
}

Assert-ExactSequence @(Get-MivOrdinalUniquePaths $representativeInput) $representativeExpected 'ordinal representative'

$originalCulture = [System.Threading.Thread]::CurrentThread.CurrentCulture
$originalUICulture = [System.Threading.Thread]::CurrentThread.CurrentUICulture
try {
    foreach ($cultureName in @('ja-JP', 'en-US', 'tr-TR')) {
        $culture = [System.Globalization.CultureInfo]::GetCultureInfo($cultureName)
        [System.Threading.Thread]::CurrentThread.CurrentCulture = $culture
        [System.Threading.Thread]::CurrentThread.CurrentUICulture = $culture
        Assert-ExactSequence @(Get-MivOrdinalUniquePaths $representativeInput) $representativeExpected "ordinal $cultureName"
    }
}
finally {
    [System.Threading.Thread]::CurrentThread.CurrentCulture = $originalCulture
    [System.Threading.Thread]::CurrentThread.CurrentUICulture = $originalUICulture
}

$records = @(Get-MivSourceFingerprintRecords $repoRoot)
$fingerprint = Get-MivSourceFingerprint $repoRoot
if ($Probe) {
    if (-not $ProbeOutput) {
        throw '-ProbeOutput is required with -Probe'
    }
    $payload = [ordered]@{
        shell = [string] $PSVersionTable.PSVersion
        representative = $representativeExpected
        records = $records
        fingerprint = $fingerprint
    }
    $json = $payload | ConvertTo-Json -Depth 3 -Compress
    [System.IO.File]::WriteAllText($ProbeOutput, $json, (New-Object System.Text.UTF8Encoding($false)))
    return
}

$shells = @(
    [ordered]@{ Name = 'Windows PowerShell 5.1'; Path = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" },
    [ordered]@{ Name = 'PowerShell 7'; Path = (Get-Command pwsh.exe -ErrorAction Stop).Source }
)
$probeResults = @()
foreach ($shell in $shells) {
    if (-not (Test-Path -LiteralPath $shell.Path -PathType Leaf)) {
        throw "missing $($shell.Name): $($shell.Path)"
    }
    $probePath = Join-Path ([System.IO.Path]::GetTempPath()) ("miv-source-fingerprint-{0}.json" -f [Guid]::NewGuid().ToString('N'))
    try {
        & $shell.Path -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -Probe -ProbeOutput $probePath
        if ($LASTEXITCODE -ne 0) {
            throw "$($shell.Name) probe failed with exit $LASTEXITCODE"
        }
        $probeResults += (Get-Content -Raw -Encoding UTF8 -LiteralPath $probePath | ConvertFrom-Json)
    }
    finally {
        if (Test-Path -LiteralPath $probePath) {
            Remove-Item -LiteralPath $probePath -Force
        }
    }
}

Assert-ExactSequence @($probeResults[0].representative) @($probeResults[1].representative) 'cross-shell representative'
Assert-ExactSequence @($probeResults[0].records) @($probeResults[1].records) 'cross-shell source records'
if (-not [System.StringComparer]::Ordinal.Equals($probeResults[0].fingerprint, $probeResults[1].fingerprint)) {
    throw "cross-shell fingerprint mismatch: $($probeResults[0].fingerprint) != $($probeResults[1].fingerprint)"
}

Write-Host ("[source-fingerprint-test] PASS records={0} sha256={1} shells={2}/{3}" -f
    $probeResults[0].records.Count,
    $probeResults[0].fingerprint,
    $probeResults[0].shell,
    $probeResults[1].shell)
