# Shared output-path policy for build scripts. Dot-sourcing has no side effects.
# Launcher/MCP processes can be long-lived children of an AI client. Only an
# exact executable that this build writes may be stopped or block that build.

function Assert-MivReleaseOutputContract {
    param([string[]] $CargoArgs = @())
    for ($index = 0; $index -lt $CargoArgs.Count; $index++) {
        $argument = $CargoArgs[$index]
        if ($argument -cin @('--all-features', '--no-default-features', '--locked', '--frozen', '--offline', '-v', '--verbose')) {
            continue
        }
        if ($argument -cmatch '^--features=.+$') { continue }
        if ($argument -ceq '--features' -or $argument -ceq '-j' -or $argument -ceq '--jobs') {
            $index++
            if ($index -lt $CargoArgs.Count -and $CargoArgs[$index] -and -not $CargoArgs[$index].StartsWith('-')) {
                if ($argument -ceq '--features' -or $CargoArgs[$index] -cmatch '^[1-9][0-9]*$') { continue }
            }
        }
        throw "[build-release] release outputs are fixed at <repo>\target\release; additional Cargo argument is not allowed (or its value is missing): $argument"
    }
}

function Assert-MivCargoArtifact {
    param(
        [AllowEmptyCollection()][string[]] $Messages,
        [Parameter(Mandatory = $true)][string] $ExpectedExecutable
    )
    $expected = [IO.Path]::GetFullPath($ExpectedExecutable)
    $found = $false
    foreach ($line in $Messages) {
        try { $message = $line | ConvertFrom-Json -ErrorAction Stop }
        catch { throw '[build-release] Cargo did not emit the required JSON artifact stream' }
        if ($message.reason -eq 'compiler-artifact' -and $message.executable) {
            $actual = [IO.Path]::GetFullPath([string] $message.executable)
            if (-not $actual.Equals($expected, [StringComparison]::OrdinalIgnoreCase)) {
                throw "[build-release] Cargo executable output mismatch: $actual (expected $expected)"
            }
            $found = $true
        }
    }
    if (-not $found) { throw "[build-release] Cargo did not report the expected executable: $expected" }
}

function Test-MivExactBuildProcess {
    param(
        [Parameter(Mandatory = $true)] $Process,
        [Parameter(Mandatory = $true)][string[]] $OutputPaths
    )
    $imagePath = $null
    try { $imagePath = $Process.Path } catch { return $false }
    if (-not $imagePath) { return $false }
    try { $imagePath = [IO.Path]::GetFullPath($imagePath) } catch { return $false }
    foreach ($outputPath in $OutputPaths) {
        if ($imagePath.Equals([IO.Path]::GetFullPath($outputPath), [StringComparison]::OrdinalIgnoreCase)) {
            return $true
        }
    }
    return $false
}
