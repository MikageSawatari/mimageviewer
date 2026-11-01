# Read the same distribution identity used by Rust (no build/sign on import).
function Get-MivSoundConfig {
    $definition = Join-Path $PSScriptRoot '..\config\miv-sound.rs'
    $values = @{}
    foreach ($line in Get-Content -LiteralPath $definition -Encoding UTF8) {
        if ($line -match '^pub const ([A-Z_]+): &str = "([^"\\]+)";$') {
            $values[$Matches[1]] = $Matches[2]
        }
    }
    foreach ($key in @('VERSION', 'BUNDLE_NAME', 'SOURCE_ROOT', 'NOTICES_ROOT', 'UPSTREAM_TAG', 'DOCS_VERSION')) {
        if (-not $values.ContainsKey($key)) { throw "[miv-sound] Missing identity constant: $key" }
    }
    $values.Manifest = "$($values.NOTICES_ROOT)/$($values.VERSION)/manifest.sha256"
    $values.Plugin = "$($values.BUNDLE_NAME)/Contents/x86_64-win/$($values.BUNDLE_NAME)"
    return $values
}
$MivSound = Get-MivSoundConfig
