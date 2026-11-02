# Make downscaled PNG copies of the V0 evaluation images.
# Used by the local VLM V0 evaluation (docs/local-vlm-v0-procedure.md).
# The EXIF orientation is applied before scaling. Transparent pixels become white.
# Images are never upscaled. The source folder is only read.
param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Dest,
    [Parameter(Mandatory = $true)][int]$LongSide
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force -Path $Dest | Out-Null

$files = Get-ChildItem -LiteralPath $Source -File |
    Where-Object { $_.Extension -match '^\.(jpe?g|png)$' } |
    Sort-Object Name

foreach ($f in $files) {
    $img = [System.Drawing.Image]::FromFile($f.FullName)
    try {
        if ($img.PropertyIdList -contains 0x0112) {
            $o = [int]$img.GetPropertyItem(0x0112).Value[0]
            $flip = switch ($o) {
                2 { [System.Drawing.RotateFlipType]::RotateNoneFlipX }
                3 { [System.Drawing.RotateFlipType]::Rotate180FlipNone }
                4 { [System.Drawing.RotateFlipType]::Rotate180FlipX }
                5 { [System.Drawing.RotateFlipType]::Rotate90FlipX }
                6 { [System.Drawing.RotateFlipType]::Rotate90FlipNone }
                7 { [System.Drawing.RotateFlipType]::Rotate270FlipX }
                8 { [System.Drawing.RotateFlipType]::Rotate270FlipNone }
                default { $null }
            }
            if ($null -ne $flip) { $img.RotateFlip($flip) }
        }
        $w = $img.Width
        $h = $img.Height
        $scale = [Math]::Min(1.0, $LongSide / [double][Math]::Max($w, $h))
        $nw = [Math]::Max(1, [int][Math]::Round($w * $scale))
        $nh = [Math]::Max(1, [int][Math]::Round($h * $scale))
        $bmp = New-Object System.Drawing.Bitmap $nw, $nh, ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        try {
            $g.Clear([System.Drawing.Color]::White)
            $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $g.DrawImage($img, 0, 0, $nw, $nh)
        } finally { $g.Dispose() }
        $out = Join-Path $Dest ([System.IO.Path]::GetFileNameWithoutExtension($f.Name) + ".png")
        $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
        Write-Output ("{0} {1}x{2} -> {3}x{4}" -f $f.Name, $w, $h, $nw, $nh)
    } finally { $img.Dispose() }
}
