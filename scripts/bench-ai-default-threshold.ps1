# AI benchmark for default skip-threshold-safe image sizes.
#
# This script generates deterministic synthetic manga-like JPEG inputs
# (800x600 and 1920x1080), runs bench_ai on all shipping upscale/denoise
# models, and exports CSV files that can be pasted into the manual table.
#
# Usage:
#   .\scripts\bench-ai-default-threshold.ps1 -Label RTX4060Ti
#   .\scripts\bench-ai-default-threshold.ps1 -Label RTX4090
#
# Optional:
#   .\scripts\bench-ai-default-threshold.ps1 -Backend tensorrt -Label RTX4090
#   TensorRT requires target\release\mimageviewer.exe to exist already.
#   .\scripts\bench-ai-default-threshold.ps1 -SkipBuild

[CmdletBinding()]
param(
    [ValidateSet('directml', 'tensorrt', 'cpu')]
    [string] $Backend = 'directml',

    [int] $Warmup = 1,

    [int] $Runs = 3,

    [switch] $SkipBuild,

    [switch] $PrepareOnly,

    [switch] $BuildOnly,

    [string] $OutRoot = 'bench_results',

    [string] $Label = '',

    [string] $Models = 'realesr_general_v3,realcugan_4x,realesrgan_anime6b,realesrgan_x4plus,nmkd_siax_4x,denoise_realplksr'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent $ScriptDir
Set-Location $RepoRoot

$Stamp = Get-Date -Format 'yyyy-MM-dd_HHmmss'
$OutDir = Join-Path $RepoRoot (Join-Path $OutRoot "ai_default_threshold_$Stamp")
$InputDir = Join-Path $OutDir 'inputs'
New-Item -ItemType Directory -Force -Path $InputDir | Out-Null

$MasterLog = Join-Path $OutDir 'master.log'

function Log {
    param([string] $Msg)
    $line = "[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss'), $Msg
    Write-Host $line
    Add-Content -Path $MasterLog -Value $line -Encoding utf8
}

function Get-GpuInfo {
    try {
        return @(Get-CimInstance Win32_VideoController | ForEach-Object {
            [pscustomobject]@{
                name = $_.Name
                adapter_ram = $_.AdapterRAM
                driver_version = $_.DriverVersion
            }
        })
    } catch {
        return @()
    }
}

function Write-JsonFile {
    param(
        [object] $Value,
        [string] $Path
    )
    $Value | ConvertTo-Json -Depth 8 | Set-Content -Path $Path -Encoding utf8
}

function Invoke-LoggedProcess {
    param(
        [string] $FilePath,
        [string[]] $Arguments,
        [string] $LogPath,
        [string] $WorkingDirectory
    )

    $oldErrorActionPreference = $ErrorActionPreference
    $pushedLocation = $false
    try {
        # Avoid cmd.exe here. Cmd cannot use UNC paths such as \\tsclient\...
        # as its current directory, while PowerShell can push-location to them.
        $ErrorActionPreference = 'Continue'
        Push-Location -LiteralPath $WorkingDirectory
        $pushedLocation = $true
        & $FilePath @Arguments *> $LogPath
        return $LASTEXITCODE
    } finally {
        if ($pushedLocation) {
            Pop-Location
        }
        $ErrorActionPreference = $oldErrorActionPreference
    }
}

function Save-SyntheticMangaJpeg {
    param(
        [int] $Width,
        [int] $Height,
        [string] $Path,
        [int] $Seed
    )

    Add-Type -AssemblyName System.Drawing

    $bmp = $null
    $g = $null
    $blackPen = $null
    $grayPen = $null
    $lightPen = $null
    $dotBrush = $null
    $shadeBrush = $null
    $whiteBrush = $null
    try {
        $bmp = New-Object System.Drawing.Bitmap $Width, $Height, ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::None
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
        $g.Clear([System.Drawing.Color]::FromArgb(246, 246, 246))

        $scale = [Math]::Max(1.0, [Math]::Min($Width / 800.0, $Height / 600.0))
        $border = [Math]::Max(2, [int][Math]::Round(3 * $scale))
        $blackPen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(18, 18, 18)), $border
        $grayPen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(95, 95, 95)), ([Math]::Max(1, [int][Math]::Round(1.5 * $scale)))
        $lightPen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(180, 180, 180)), 1
        $dotBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(118, 118, 118))
        $shadeBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(222, 222, 222))
        $whiteBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 255, 255))
        $random = New-Object System.Random $Seed

        $margin = [Math]::Max(12, [int][Math]::Round(18 * $scale))
        $gap = [Math]::Max(8, [int][Math]::Round(14 * $scale))
        $panelW = [int](($Width - ($margin * 2) - $gap) / 2)
        $panelH = [int](($Height - ($margin * 2) - $gap) / 2)
        $panels = @(
            [System.Drawing.Rectangle]::new($margin, $margin, $panelW, $panelH),
            [System.Drawing.Rectangle]::new($margin + $panelW + $gap, $margin, $panelW, $panelH),
            [System.Drawing.Rectangle]::new($margin, $margin + $panelH + $gap, $panelW, $panelH),
            [System.Drawing.Rectangle]::new($margin + $panelW + $gap, $margin + $panelH + $gap, $panelW, $panelH)
        )

        foreach ($panel in $panels) {
            $g.FillRectangle($whiteBrush, $panel)
            $g.DrawRectangle($blackPen, $panel)
        }

        foreach ($panel in $panels) {
            $toneStep = [Math]::Max(7, [int][Math]::Round(10 * $scale))
            $dot = [Math]::Max(2, [int][Math]::Round(3 * $scale))
            for ($y = $panel.Top + $toneStep; $y -lt $panel.Bottom - $toneStep; $y += $toneStep) {
                for ($x = $panel.Left + $toneStep; $x -lt $panel.Right - $toneStep; $x += $toneStep) {
                    $v = (($x * 13 + $y * 7 + $Seed) % 29)
                    if ($v -lt 12) {
                        $g.FillEllipse($dotBrush, $x, $y, $dot, $dot)
                    }
                }
            }
        }

        foreach ($panel in $panels) {
            $shadeH = [Math]::Max(20, [int]($panel.Height * 0.22))
            $shadeRect = [System.Drawing.Rectangle]::new($panel.Left + $border, $panel.Bottom - $shadeH - $border, $panel.Width - ($border * 2), $shadeH)
            $g.FillRectangle($shadeBrush, $shadeRect)
            for ($x = $shadeRect.Left; $x -lt $shadeRect.Right; $x += [Math]::Max(5, [int][Math]::Round(7 * $scale))) {
                $g.DrawLine($lightPen, $x, $shadeRect.Top, $x + $shadeH, $shadeRect.Bottom)
            }
        }

        $cx = [int]($Width * 0.52)
        $cy = [int]($Height * 0.47)
        $lineCount = [Math]::Max(70, [int][Math]::Round(($Width + $Height) / 18))
        for ($i = 0; $i -lt $lineCount; $i++) {
            if ($random.NextDouble() -lt 0.5) {
                $x0 = $random.Next(0, $Width)
                $y0 = if ($random.NextDouble() -lt 0.5) { 0 } else { $Height - 1 }
            } else {
                $x0 = if ($random.NextDouble() -lt 0.5) { 0 } else { $Width - 1 }
                $y0 = $random.Next(0, $Height)
            }
            $x1 = $cx + $random.Next(-[int]($Width * 0.10), [int]($Width * 0.10))
            $y1 = $cy + $random.Next(-[int]($Height * 0.10), [int]($Height * 0.10))
            $g.DrawLine($grayPen, $x0, $y0, $x1, $y1)
        }

        foreach ($panel in $panels) {
            $faceW = [Math]::Max(42, [int]($panel.Width * 0.28))
            $faceH = [Math]::Max(54, [int]($panel.Height * 0.36))
            $faceX = $panel.Left + [int](($panel.Width - $faceW) / 2)
            $faceY = $panel.Top + [int]($panel.Height * 0.16)
            $g.FillEllipse($whiteBrush, $faceX, $faceY, $faceW, $faceH)
            $g.DrawEllipse($blackPen, $faceX, $faceY, $faceW, $faceH)
            $eyeY = $faceY + [int]($faceH * 0.42)
            $eyeR = [Math]::Max(2, [int]($faceW * 0.05))
            $g.FillEllipse($dotBrush, $faceX + [int]($faceW * 0.32), $eyeY, $eyeR, $eyeR)
            $g.FillEllipse($dotBrush, $faceX + [int]($faceW * 0.62), $eyeY, $eyeR, $eyeR)
            $g.DrawArc($grayPen, $faceX + [int]($faceW * 0.35), $faceY + [int]($faceH * 0.58), [int]($faceW * 0.30), [int]($faceH * 0.18), 0, 180)
        }

        $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Jpeg)
    } finally {
        if ($whiteBrush) { $whiteBrush.Dispose() }
        if ($shadeBrush) { $shadeBrush.Dispose() }
        if ($dotBrush) { $dotBrush.Dispose() }
        if ($lightPen) { $lightPen.Dispose() }
        if ($grayPen) { $grayPen.Dispose() }
        if ($blackPen) { $blackPen.Dispose() }
        if ($g) { $g.Dispose() }
        if ($bmp) { $bmp.Dispose() }
    }
}

function Convert-BenchJsonToCsv {
    param(
        [string] $JsonPath,
        [string] $DetailCsvPath,
        [string] $ManualCsvPath,
        [string] $BackendName,
        [string] $RunLabel
    )

    $summary = Get-Content -Path $JsonPath -Encoding UTF8 -Raw | ConvertFrom-Json
    $records = @($summary.records)

    $details = $records | ForEach-Object {
        [pscustomobject]@{
            label = $RunLabel
            backend = $BackendName
            model = $_.model
            image_size = ('{0}x{1}' -f $_.image_w, $_.image_h)
            avg_sec = [Math]::Round(($_.wall_total_avg_ms / 1000.0), 3)
            avg_ms = [Math]::Round($_.wall_total_avg_ms, 1)
            min_ms = [Math]::Round($_.wall_total_min_ms, 1)
            max_ms = [Math]::Round($_.wall_total_max_ms, 1)
            stddev_ms = [Math]::Round($_.wall_total_stddev_ms, 1)
            n_tiles = $_.n_tiles
            runs = $_.runs
            tile_size = $_.tile_size
        }
    }
    $details | Export-Csv -Path $DetailCsvPath -NoTypeInformation -Encoding utf8

    $modelRows = @(
        [pscustomobject]@{ label = 'Fast general'; key = 'realesr_general_v3'; mode = 'Light / HighQuality' },
        [pscustomobject]@{ label = 'Manga tone'; key = 'realcugan_4x'; mode = 'Light / HighQuality' },
        [pscustomobject]@{ label = 'Illustration'; key = 'realesrgan_anime6b'; mode = 'HighQuality' },
        [pscustomobject]@{ label = 'Photo/CG strong'; key = 'realesrgan_x4plus'; mode = 'HighQuality' },
        [pscustomobject]@{ label = 'Photo texture'; key = 'nmkd_siax_4x'; mode = 'HighQuality' },
        [pscustomobject]@{ label = 'JPEG denoise'; key = 'denoise_realplksr'; mode = 'HighQuality' }
    )

    function Find-Record {
        param(
            [string] $Model,
            [int] $Width,
            [int] $Height
        )
        return ($records | Where-Object {
            $_.model -eq $Model -and [int]$_.image_w -eq $Width -and [int]$_.image_h -eq $Height
        } | Select-Object -First 1)
    }

    function Format-Sec {
        param([object] $Record)
        if ($null -eq $Record) { return '' }
        return ('{0:0.00}' -f ($Record.wall_total_avg_ms / 1000.0))
    }

    function Format-Ms {
        param([object] $Record)
        if ($null -eq $Record) { return '' }
        return ('{0:0}' -f $Record.wall_total_avg_ms)
    }

    $manual = $modelRows | ForEach-Object {
        $small = Find-Record -Model $_.key -Width 800 -Height 600
        $fullhd = Find-Record -Model $_.key -Width 1920 -Height 1080
        [pscustomobject]@{
            label = $RunLabel
            backend = $BackendName
            model_label = $_.label
            model_key = $_.key
            mode = $_.mode
            sec_800x600 = Format-Sec $small
            sec_1920x1080 = Format-Sec $fullhd
            ms_800x600 = Format-Ms $small
            ms_1920x1080 = Format-Ms $fullhd
        }
    }
    $manual | Export-Csv -Path $ManualCsvPath -NoTypeInformation -Encoding utf8
}

Log "AI default-threshold benchmark"
Log "  worktree: $RepoRoot"
Log "  output:   $OutDir"
Log "  backend:  $Backend"
Log "  warmup:   $Warmup"
Log "  runs:     $Runs"
Log "  models:   $Models"

$GpuInfo = Get-GpuInfo
$RunLabel = $Label
if ([string]::IsNullOrWhiteSpace($RunLabel)) {
    if ($GpuInfo.Count -gt 0 -and -not [string]::IsNullOrWhiteSpace($GpuInfo[0].name)) {
        $RunLabel = ($GpuInfo[0].name -replace '\s+', ' ').Trim()
    } else {
        $RunLabel = 'unknown'
    }
}
Log "  label:    $RunLabel"

$RunInfoPath = Join-Path $OutDir 'run_info.json'
Write-JsonFile ([pscustomobject]@{
    timestamp = $Stamp
    label = $RunLabel
    repo_root = $RepoRoot
    backend = $Backend
    warmup = $Warmup
    runs = $Runs
    models = $Models
    threshold_note = 'Default 2048px skip threshold processes images with width < 2048 and height < 2048. 800x600 and 1920x1080 are both included.'
    gpu = $GpuInfo
}) $RunInfoPath
Log "Run info: $RunInfoPath"

$ImgSmall = Join-Path $InputDir 'synthetic_manga_800x600.jpg'
$ImgFullHd = Join-Path $InputDir 'synthetic_manga_1920x1080.jpg'
Save-SyntheticMangaJpeg -Width 800 -Height 600 -Path $ImgSmall -Seed 4060
Save-SyntheticMangaJpeg -Width 1920 -Height 1080 -Path $ImgFullHd -Seed 4090
Log "Generated inputs:"
Log "  $ImgSmall"
Log "  $ImgFullHd"

if ($PrepareOnly) {
    Log "PrepareOnly was set; benchmark was not run."
    Log "Use the same command without -PrepareOnly to measure this PC."
    exit 0
}

$BenchExe = Join-Path $RepoRoot 'target\release\bench_ai.exe'
$MainExe = Join-Path $RepoRoot 'target\release\mimageviewer.exe'

if (-not $SkipBuild) {
    $BuildLog = Join-Path $OutDir 'build.log'
    Log "Building bench_ai (release)..."
    $buildArgs = @('build', '--release', '--bin', 'bench_ai')
    $buildExit = Invoke-LoggedProcess `
        -FilePath 'cargo' `
        -Arguments $buildArgs `
        -LogPath $BuildLog `
        -WorkingDirectory $RepoRoot
    if ($buildExit -ne 0) {
        Log "ERROR: cargo build failed (exit $buildExit)"
        Log "  see: $BuildLog"
        exit 1
    }
    Log "Build OK"
    if ($Backend -eq 'tensorrt') {
        Log "TensorRT mode also needs target\\release\\mimageviewer.exe; build the release launcher first if the next check fails."
    }
} else {
    Log "Skipping build"
}

if ($BuildOnly) {
    Log "BuildOnly was set; benchmark was not run."
    Log "Use the same command without -BuildOnly to measure this PC."
    exit 0
}

if (-not (Test-Path $BenchExe)) {
    Log "ERROR: bench_ai.exe not found: $BenchExe"
    Log "Run without -SkipBuild first."
    exit 1
}
if ($Backend -eq 'tensorrt' -and -not (Test-Path $MainExe)) {
    Log "ERROR: TensorRT bench requires sibling mimageviewer.exe: $MainExe"
    Log "Build the release launcher first, for example: .\\scripts\\build-release.ps1"
    exit 1
}

$JsonPath = Join-Path $OutDir "bench_$Backend.json"
$LogPath = Join-Path $OutDir "bench_$Backend.log"
$DetailCsvPath = Join-Path $OutDir "bench_${Backend}_detail.csv"
$ManualCsvPath = Join-Path $OutDir "manual_table_values_${Backend}.csv"

Log "Running benchmark..."
Log "  json: $JsonPath"
Log "  log:  $LogPath"
$Start = Get-Date

$benchArgs = @(
    '--backend',
    $Backend,
    '--models',
    $Models,
    '--warmup',
    [string]$Warmup,
    '--runs',
    [string]$Runs,
    '--image',
    $ImgSmall,
    '--image',
    $ImgFullHd,
    '--json',
    $JsonPath
)
$rc = Invoke-LoggedProcess `
    -FilePath $BenchExe `
    -Arguments $benchArgs `
    -LogPath $LogPath `
    -WorkingDirectory $RepoRoot
$Elapsed = ((Get-Date) - $Start).TotalSeconds
Log "Benchmark finished in $([Math]::Round($Elapsed, 0)) s, exit=$rc"

if ($rc -ne 0) {
    Log "ERROR: benchmark failed"
    Log "  see: $LogPath"
    exit 1
}
if (-not (Test-Path $JsonPath)) {
    Log "ERROR: benchmark finished but JSON was not produced"
    Log "  see: $LogPath"
    exit 1
}

Convert-BenchJsonToCsv `
    -JsonPath $JsonPath `
    -DetailCsvPath $DetailCsvPath `
    -ManualCsvPath $ManualCsvPath `
    -BackendName $Backend `
    -RunLabel $RunLabel

Log "CSV exported:"
Log "  detail: $DetailCsvPath"
Log "  manual: $ManualCsvPath"
Log "Done. Send run_info.json, bench_$Backend.json, bench_$Backend.log, and manual_table_values_$Backend.csv from each PC."
