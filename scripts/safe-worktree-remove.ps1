# safe-worktree-remove.ps1
#
# Wrapper for `git worktree remove` that handles NTFS junctions safely.
#
# WHY THIS EXISTS
# ---------------
# A worktree under this repo may contain `vendor/` (or sub-paths) as a
# junction (NTFS reparse point) that points back to the main worktree's
# vendor/. Running `git worktree remove <path>` on such a worktree makes
# git follow the junction and recursively delete the link TARGET's
# contents -- i.e. it wipes out the main repo's vendor/ files.
#
# This was observed:
#   2026-05-13  --  vendor/ wiped (originally documented in CLAUDE.md)
#   2026-05-14  --  vendor/ wiped again despite the documentation
#
# Documentation alone is not enough. This wrapper:
#   1. Recursively scans the target worktree for any reparse points,
#      WITHOUT descending into them.
#   2. Unlinks each junction with `cmd /c rmdir` (which always treats a
#      junction as just the link and never follows it).
#   3. Only then runs `git worktree remove <path>`.
#
# USAGE
# -----
#   .\scripts\safe-worktree-remove.ps1 <worktree-path>
#   .\scripts\safe-worktree-remove.ps1 <worktree-path> -Force
#   .\scripts\safe-worktree-remove.ps1 <worktree-path> -DryRun
#   .\scripts\safe-worktree-remove.ps1 -Audit
#
#   -Force   Forward --force to `git worktree remove` (for dirty worktrees).
#   -DryRun  Show what would happen. No changes.
#   -Audit   List all worktrees and the junctions found inside each one.
#            Does not remove anything. Useful as a pre-flight check.

[CmdletBinding()]
param(
    [Parameter(Position=0)]
    [string]$WorktreePath,

    [switch]$Force,
    [switch]$DryRun,
    [switch]$Audit
)

$ErrorActionPreference = 'Stop'

function Find-JunctionsSafe {
    # Recursive scan that does NOT descend into reparse points.
    # Returns an array of FullName strings for every junction / symlink
    # encountered below (and including) $Root.
    #
    # $SkipDirs (optional): list of absolute paths to skip entirely. Used
    # by audit mode so the main worktree does not double-count junctions
    # that live inside .claude/worktrees/ (those get scanned as their own
    # worktree).
    param(
        [Parameter(Mandatory)][string]$Root,
        [string[]]$SkipDirs = @()
    )

    $found = New-Object System.Collections.Generic.List[string]
    $item = Get-Item -LiteralPath $Root -Force -ErrorAction SilentlyContinue
    if (-not $item) { return ,$found.ToArray() }

    foreach ($skip in $SkipDirs) {
        if ($item.FullName -ieq $skip) { return ,$found.ToArray() }
    }

    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        $found.Add($item.FullName)
        # CRITICAL: do NOT recurse into reparse points -- that is the very
        # bug this script exists to avoid.
        return ,$found.ToArray()
    }

    if ($item.PSIsContainer) {
        $children = Get-ChildItem -LiteralPath $Root -Force -ErrorAction SilentlyContinue
        foreach ($c in $children) {
            $sub = Find-JunctionsSafe -Root $c.FullName -SkipDirs $SkipDirs
            foreach ($s in $sub) { $found.Add($s) }
        }
    }
    return ,$found.ToArray()
}

function Remove-Junction-Safely {
    param([Parameter(Mandatory)][string]$Path)
    # `cmd /c rmdir` deletes a junction without following it.
    # `Remove-Item -Recurse` on PowerShell 5.1 may follow junctions in
    # some edge cases (known issue), so we deliberately use rmdir.
    $quoted = '"' + $Path + '"'
    cmd /c "rmdir $quoted" 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "rmdir failed for ${Path} (exit ${LASTEXITCODE})"
    }
    # Verify it is actually gone.
    if (Test-Path -LiteralPath $Path) {
        throw "junction still present after rmdir: $Path"
    }
}

function Get-WorktreePaths {
    # Parses `git worktree list --porcelain` to extract worktree paths.
    $output = git worktree list --porcelain 2>&1
    $paths = @()
    foreach ($line in $output) {
        if ($line -match '^worktree\s+(.+)$') {
            $paths += $Matches[1]
        }
    }
    return $paths
}

# ---- Audit mode ----------------------------------------------------------

if ($Audit) {
    Write-Host "Scanning all git worktrees for junctions..."
    Write-Host ""
    $worktrees = Get-WorktreePaths
    if ($worktrees.Count -eq 0) {
        Write-Host "No git worktrees found (run from inside a git repo)."
        exit 0
    }
    # When scanning the main worktree, skip its `.claude/worktrees/`
    # subtree so we do not double-count junctions that belong to other
    # worktrees (those get scanned independently below).
    $otherWorktrees = @()
    $mainResolved = $null
    $topLevel = (git rev-parse --show-toplevel 2>$null)
    if ($topLevel) {
        $mainResolved = (Resolve-Path -LiteralPath $topLevel).Path
        foreach ($wt in $worktrees) {
            $r = (Resolve-Path -LiteralPath $wt -ErrorAction SilentlyContinue)
            if ($r -and $r.Path -ine $mainResolved) {
                $otherWorktrees += $r.Path
            }
        }
    }

    $bad = 0
    foreach ($wt in $worktrees) {
        $skip = @()
        $wtResolved = (Resolve-Path -LiteralPath $wt -ErrorAction SilentlyContinue)
        if ($wtResolved -and $mainResolved -and ($wtResolved.Path -ieq $mainResolved)) {
            $skip = $otherWorktrees
        }
        $junctions = Find-JunctionsSafe -Root $wt -SkipDirs $skip
        if ($junctions.Count -gt 0) {
            $bad++
            Write-Host "WARN  $wt"
            foreach ($j in $junctions) { Write-Host "        -> $j" }
        } else {
            Write-Host "OK    $wt"
        }
    }
    Write-Host ""
    if ($bad -gt 0) {
        Write-Host "$bad worktree(s) contain junctions. Use:"
        Write-Host "  .\scripts\safe-worktree-remove.ps1 <path>"
        Write-Host "to remove any of them safely."
    } else {
        Write-Host "All worktrees are junction-free."
    }
    exit 0
}

# ---- Remove mode ---------------------------------------------------------

if (-not $WorktreePath) {
    Write-Host "Usage:"
    Write-Host "  .\scripts\safe-worktree-remove.ps1 <worktree-path> [-Force] [-DryRun]"
    Write-Host "  .\scripts\safe-worktree-remove.ps1 -Audit"
    exit 2
}

if (-not (Test-Path -LiteralPath $WorktreePath)) {
    Write-Error "Worktree path does not exist: $WorktreePath"
    exit 1
}

$resolved = (Resolve-Path -LiteralPath $WorktreePath).Path

# Safety: refuse to operate on the main worktree (= cwd of the git repo).
$mainWorktree = (git rev-parse --show-toplevel 2>$null)
if ($mainWorktree) {
    $mainResolved = (Resolve-Path -LiteralPath $mainWorktree).Path
    if ($resolved -ieq $mainResolved) {
        Write-Error "Refusing to operate on the main worktree: $resolved"
        exit 1
    }
}

Write-Host "Scanning $resolved for junctions..."
$junctions = Find-JunctionsSafe -Root $resolved
if ($junctions.Count -gt 0) {
    Write-Host ""
    Write-Host "Found $($junctions.Count) junction(s) inside worktree:"
    foreach ($j in $junctions) { Write-Host "  $j" }
    Write-Host ""

    if ($DryRun) {
        Write-Host "[DRY-RUN] Would unlink each junction with: cmd /c rmdir <junction>"
        Write-Host "[DRY-RUN] Then: git worktree remove $resolved$(if ($Force) {' --force'})"
        exit 0
    }

    Write-Host "Unlinking junctions (cmd /c rmdir -- never follows reparse points)..."
    foreach ($j in $junctions) {
        try {
            Remove-Junction-Safely -Path $j
            Write-Host "  unlinked: $j"
        } catch {
            Write-Error "Aborting: $_"
            exit 1
        }
    }
} else {
    Write-Host "No junctions found."
    if ($DryRun) {
        Write-Host "[DRY-RUN] Would run: git worktree remove $resolved$(if ($Force) {' --force'})"
        exit 0
    }
}

Write-Host ""
$gitArgs = @('worktree', 'remove', $resolved)
if ($Force) { $gitArgs += '--force' }
Write-Host "Running: git $($gitArgs -join ' ')"
& git @gitArgs
if ($LASTEXITCODE -ne 0) {
    Write-Error "git worktree remove failed with exit $LASTEXITCODE"
    exit 1
}
Write-Host "OK: worktree removed."
