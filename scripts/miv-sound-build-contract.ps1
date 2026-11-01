# Nonexecuting build/configuration inspection shared by testing and approval.
function Get-SoundCTestInventory {
    param([string] $Source, [string] $CTest, [string[]] $Arguments)
    $savedPreference = $ErrorActionPreference
    Push-Location $Source
    try {
        $ErrorActionPreference = 'Continue'
        $output = & $CTest @Arguments -N --show-only=json-v1 2>&1 | Out-String
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $savedPreference; Pop-Location }
    if ($code -ne 0) { throw "[miv-sound] Cannot list registered ctest tests: $output" }
    $listing = $output | ConvertFrom-Json
    # Backtrace indices/graphs are diagnostics, not test definitions. Preserve
    # commands and properties too, including DISABLED and test environments.
    return @($listing.tests | ForEach-Object {
        if (-not $_.PSObject.Properties['command']) { throw "[miv-sound] Test executable missing: $($_.name)" }
        [ordered]@{ name = $_.name; command = @($_.command); properties = @($_.properties) }
    })
}

function Get-SoundBuildContract {
    param([string] $Source, [string] $CMake)
    $buildScript = [IO.File]::ReadAllText((Join-Path $Source 'build-miv.bat'))
    if ($buildScript -notmatch '(?m)^cmake --preset windows-release[^\r\n]*') { throw '[miv-sound] Cannot record configure command from build-miv.bat' }
    $configureCommand = $Matches[0]
    # Include all typed cache values, not a whitelist that could miss a new
    # test/feature option. Ignore comments, line endings and entry ordering.
    $cache = [string[]]@([IO.File]::ReadAllLines((Join-Path $Source 'build/windows-release/CMakeCache.txt')) |
        Where-Object { $_ -match '^[^/#][^=]*:[^=]+=.*$' })
    [Array]::Sort($cache, [StringComparer]::Ordinal)
    $ctest = Join-Path (Split-Path $CMake) 'ctest.exe'
    $registered = @(Get-SoundCTestInventory $Source $ctest @('--test-dir', (Join-Path $Source 'build/windows-release')))
    $selected = @(Get-SoundCTestInventory $Source $ctest @('--preset', 'windows-release'))
    if ($registered.Count -eq 0 -or $selected.Count -ne $registered.Count -or
        ($registered | ConvertTo-Json -Depth 30 -Compress) -cne ($selected | ConvertTo-Json -Depth 30 -Compress)) {
        throw '[miv-sound] Test preset must select the complete registered test inventory'
    }
    $userPresets = Join-Path $Source 'CMakeUserPresets.json'
    return [ordered]@{
        preset = 'windows-release'; configure_command = $configureCommand
        cmake_presets = [IO.File]::ReadAllText((Join-Path $Source 'CMakePresets.json'))
        cmake_user_presets = $(if (Test-Path -LiteralPath $userPresets) { [IO.File]::ReadAllText($userPresets) } else { $null })
        cache_entries = $cache
        registered_tests = $registered; selected_tests = $selected
    }
}

function Get-SoundBuildContractHash {
    param([object] $Contract)
    return Get-MivSoundTextHash ($Contract | ConvertTo-Json -Depth 30 -Compress)
}

function Assert-SoundBuildContract {
    param([object] $Item)
    $property = $Item.Record.provenance.PSObject.Properties['build_contract']
    if (-not $property) { throw '[miv-sound] Candidate lacks build/test contract; recreate it with the current build script' }
    $expected = Get-SoundBuildContractHash $property.Value
    $actual = Get-SoundBuildContract $Item.Record.source_root $Item.Record.provenance.tools.cmake_path
    if ((Get-SoundBuildContractHash $actual) -cne $expected) {
        throw '[miv-sound] Candidate build configuration or registered test inventory changed'
    }
    return $expected
}
