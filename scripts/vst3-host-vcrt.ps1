# Lightweight direct-host preflight; never launches, copies, or signs anything.
param([string] $ValidateRepo)

function Assert-MivVst3HostVcrt {
    param([Parameter(Mandatory = $true)] [string] $RepoRoot)
    $recover = 'Run cmake --build crates/vst3-host/build --config Release (requires the configured VST3 SDK), or restore the current vendor host together with its canonical vcrt/ directory. No host cache or System32 fallback is used by this preflight.'
    foreach ($name in @('msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
        $canonical = Join-Path $RepoRoot "vendor\vcrt\$name"
        $staged = Join-Path $RepoRoot "vendor\vst3-host\vcrt\$name"
        foreach ($path in @($canonical, $staged)) {
            $current = [System.IO.Path]::GetFullPath($path)
            $leaf = $true
            while ($current) {
                $item = Get-Item -LiteralPath $current -Force -ErrorAction SilentlyContinue
                if ($null -eq $item) { throw "[vst3-vcrt] Required CRT path missing/unreadable: $current. $recover" }
                if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
                    ($leaf -and -not ($item -is [System.IO.FileInfo]))) {
                    throw "[vst3-vcrt] CRT path must be regular and have no reparse ancestor: $current. $recover"
                }
                $leaf = $false
                $current = [System.IO.Path]::GetDirectoryName($current)
            }
        }
        if ((Get-FileHash -LiteralPath $canonical -Algorithm SHA256).Hash -ne
            (Get-FileHash -LiteralPath $staged -Algorithm SHA256).Hash) {
            throw "[vst3-vcrt] Staged CRT differs from canonical vendor/vcrt: $name. $recover"
        }
    }
    Write-Host '[vst3-vcrt] Direct-host canonical CRT set verified (4 files)'
}

if ($ValidateRepo) {
    $ErrorActionPreference = 'Stop'
    Assert-MivVst3HostVcrt -RepoRoot $ValidateRepo
}
