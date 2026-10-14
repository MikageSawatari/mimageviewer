# Fixed output paths keep process protection, clean, signing and embedding aligned.
function Assert-MivFixedCargoOutput {
    param(
        [string[]] $CargoArgs = @(),
        [string] $WorkingDirectory = (Get-Location).Path,
        [switch] $Portable,
        [switch] $SmokeTestScript
    )

    if (Test-Path Env:CARGO_BUILD_TARGET) {
        throw '[build-output] CARGO_BUILD_TARGET is not supported: fixed native output paths are required.'
    }
    if ($null -eq $CargoArgs) { $CargoArgs = @() }
    $metadataArgs = @('metadata', '--format-version', '1', '--no-deps', '--offline')
    for ($index = 0; $index -lt $CargoArgs.Count; $index++) {
        $arg = $CargoArgs[$index]
        if ($arg -match '^--(target-dir|build-dir|target|profile|artifact-dir|out-dir|manifest-path)(=|$)' -or
            $arg -match '^-[CZ]') {
            throw '[build-output] Cargo output/layout arguments are not supported: fixed output paths are required.'
        }
        # Forward Cargo configuration verbatim; Cargo, not this guard, parses it.
        if ($arg -eq '--config') {
            $index++
            if ($index -ge $CargoArgs.Count) {
                throw '[build-output] --config requires a value.'
            }
            $metadataArgs += @('--config', $CargoArgs[$index])
        } elseif ($arg.StartsWith('--config=')) {
            $metadataArgs += $arg
        } elseif ($arg -in @('--locked', '--frozen')) {
            $metadataArgs += $arg
        }
    }

    Push-Location -LiteralPath $WorkingDirectory
    try {
        $targetName = if ($Portable) {
            if ($SmokeTestScript) { 'target-portable-test-script' } else { 'target-portable' }
        } else { 'target' }
        $expected = [IO.Path]::GetFullPath((Join-Path (Get-Location).Path $targetName)).TrimEnd('\', '/')
        if ($Portable) {
            # metadata has no --target-dir option; use Cargo's CLI config for the
            # fixed value. Legacy CARGO_TARGET_DIR still wins and is rejected if
            # it redirects metadata away from the expected portable directory.
            $tomlPath = $expected.Replace('\', '/')
            $fixedConfig = 'build.target-dir="{0}"' -f $tomlPath
            # Preserve TOML quotes through Windows PowerShell's native argv rules.
            if ($PSVersionTable.PSVersion -lt [version]'7.3' -or $PSNativeCommandArgumentPassing -eq 'Legacy') {
                $fixedConfig = $fixedConfig.Replace('"', '\"')
            }
            $metadataArgs += @('--config', $fixedConfig)
        }
        try {
            $oldOutputEncoding = [Console]::OutputEncoding
            try {
                [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false)
                $json = & cargo @metadataArgs
            } finally {
                [Console]::OutputEncoding = $oldOutputEncoding
            }
            if ($LASTEXITCODE -ne 0) { throw 'metadata failed' }
            $metadata = ($json -join "`n") | ConvertFrom-Json -ErrorAction Stop
            if (-not $metadata.target_directory) { throw 'missing target_directory' }
            $actual = [IO.Path]::GetFullPath($metadata.target_directory).TrimEnd('\', '/')
        } catch {
            throw '[build-output] Cargo metadata failed: fixed output paths could not be verified.'
        }
        if (-not $actual.Equals($expected, [StringComparison]::OrdinalIgnoreCase)) {
            throw '[build-output] Cargo target_directory differs from the fixed output path. Remove the output override before building; protection, clean, signing and packaging require fixed output paths.'
        }
    } finally {
        Pop-Location
    }
}
