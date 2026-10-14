# Fixed output paths keep process protection, clean, signing and embedding aligned.
function Assert-MivFixedCargoOutput {
    param(
        [string[]] $CargoArgs = @(),
        [string] $WorkingDirectory = (Get-Location).Path
    )

    & python (Join-Path $PSScriptRoot 'check_build_output_config.py') $WorkingDirectory -- @CargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw '[build-output] Cargo output overrides are not supported. Remove the reported setting before building; protection, clean, signing and packaging require fixed output paths.'
    }
}
