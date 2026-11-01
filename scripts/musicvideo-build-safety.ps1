# The editor owns unsaved work. Never terminate it to unlock build outputs.
# Call before stopping other companions, cleaning, building or replacing a package.
function Assert-MivMusicvideoBuildOutputsIdle {
    param(
        [Parameter(Mandatory = $true)]
        [string[]] $OutputPaths
    )

    $outputs = @($OutputPaths | ForEach-Object { [System.IO.Path]::GetFullPath($_) })
    foreach ($process in @(Get-Process -Name 'mimageviewer-musicvideo' -ErrorAction SilentlyContinue)) {
        $processPath = $null
        try { $processPath = $process.Path } catch { $processPath = $null }
        # An inaccessible image path cannot prove that overwriting is safe.
        if (-not $processPath -or $outputs -contains [System.IO.Path]::GetFullPath($processPath)) {
            throw ("[musicvideo-build] Editor is running (PID={0}): {1}. Save your work and close the editor before retrying the build." -f
                $process.Id, $processPath)
        }
    }
}
