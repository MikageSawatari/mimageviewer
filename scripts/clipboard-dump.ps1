<#
.SYNOPSIS
  Development tool for the clipboard capture design (docs/clipboard-capture-plan.md, S0).
  Waits for the next clipboard change, then records every clipboard format, its size,
  the sequence numbers, the HTML Format text, PNG bytes and DIB header facts.
  It keeps watching for a few seconds to record follow-up changes from the same copy.

.EXAMPLE
  .\scripts\clipboard-dump.ps1 -Label chrome-copy-image
  (then copy something in the target application within the wait time)

.EXAMPLE
  .\scripts\clipboard-dump.ps1 -Label current -Now
  (records what is on the clipboard right now; no sequence-change log)

.NOTES
  Output: <OutDir>\<Label>-<timestamp>\  (default OutDir = Desktop\clipdump)
  Reads the clipboard only. ASCII only on purpose (PowerShell 5.1 reads non-BOM files as ANSI).
#>
param(
    [Parameter(Mandatory = $true)][string]$Label,
    [string]$OutDir = (Join-Path ([Environment]::GetFolderPath('Desktop')) 'clipdump'),
    [int]$WaitSeconds = 60,
    [int]$FollowSeconds = 3,
    [switch]$Now
)

$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class ClipDump {
    [DllImport("user32.dll", SetLastError = true)] static extern bool OpenClipboard(IntPtr hWnd);
    [DllImport("user32.dll", SetLastError = true)] static extern bool CloseClipboard();
    [DllImport("user32.dll", SetLastError = true)] static extern uint EnumClipboardFormats(uint format);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClipboardFormatName(uint format, StringBuilder name, int max);
    [DllImport("user32.dll", SetLastError = true)] static extern IntPtr GetClipboardData(uint format);
    [DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();
    [DllImport("user32.dll")] static extern IntPtr GetClipboardOwner();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
    [DllImport("kernel32.dll")] static extern IntPtr GlobalLock(IntPtr h);
    [DllImport("kernel32.dll")] static extern bool GlobalUnlock(IntPtr h);
    [DllImport("kernel32.dll")] static extern UIntPtr GlobalSize(IntPtr h);

    static readonly Dictionary<uint, string> Std = new Dictionary<uint, string> {
        {1,"CF_TEXT"},{2,"CF_BITMAP"},{3,"CF_METAFILEPICT"},{4,"CF_SYLK"},{5,"CF_DIF"},{6,"CF_TIFF"},
        {7,"CF_OEMTEXT"},{8,"CF_DIB"},{9,"CF_PALETTE"},{10,"CF_PENDATA"},{11,"CF_RIFF"},{12,"CF_WAVE"},
        {13,"CF_UNICODETEXT"},{14,"CF_ENHMETAFILE"},{15,"CF_HDROP"},{16,"CF_LOCALE"},{17,"CF_DIBV5"}
    };

    public class Entry {
        public uint Id; public string Name; public long Size = -1; public byte[] Data;
    }

    public static string FormatName(uint f) {
        string s;
        if (Std.TryGetValue(f, out s)) return s;
        var sb = new StringBuilder(256);
        return GetClipboardFormatName(f, sb, sb.Capacity) > 0 ? sb.ToString() : ("#" + f);
    }

    static bool Open() {
        for (int i = 0; i < 20; i++) {
            if (OpenClipboard(IntPtr.Zero)) return true;
            System.Threading.Thread.Sleep(25);
        }
        return false;
    }

    // Lists formats without reading data (no delayed rendering is forced).
    public static List<Entry> ListFormats() {
        var list = new List<Entry>();
        if (!Open()) return null;
        try {
            uint f = 0;
            while ((f = EnumClipboardFormats(f)) != 0) list.Add(new Entry { Id = f, Name = FormatName(f) });
        } finally { CloseClipboard(); }
        return list;
    }

    // Reads data of HGLOBAL-backed formats. GDI handle formats (CF_BITMAP, palettes, metafiles) are skipped.
    public static void ReadData(List<Entry> list, long keepLimit) {
        if (!Open()) return;
        try {
            foreach (var e in list) {
                if (e.Id == 2 || e.Id == 3 || e.Id == 9 || e.Id == 14) continue;
                IntPtr h = GetClipboardData(e.Id);
                if (h == IntPtr.Zero) continue;
                long size = (long)GlobalSize(h).ToUInt64();
                e.Size = size;
                if (size <= 0 || size > keepLimit) continue;
                IntPtr p = GlobalLock(h);
                if (p == IntPtr.Zero) continue;
                try { e.Data = new byte[size]; Marshal.Copy(p, e.Data, 0, (int)size); }
                finally { GlobalUnlock(h); }
            }
        } finally { CloseClipboard(); }
    }

    public static string OwnerProcess() {
        IntPtr hwnd = GetClipboardOwner();
        if (hwnd == IntPtr.Zero) return "(none)";
        uint pid; GetWindowThreadProcessId(hwnd, out pid);
        try { return System.Diagnostics.Process.GetProcessById((int)pid).ProcessName + " pid=" + pid; }
        catch { return "pid=" + pid; }
    }
}
'@

function Write-Ascii([string]$path, [string[]]$lines) {
    [IO.File]::WriteAllLines($path, $lines, [Text.Encoding]::UTF8)
}

function Describe-Dib([byte[]]$d) {
    if ($null -eq $d -or $d.Length -lt 40) { return 'dib: too short' }
    $hdr = [BitConverter]::ToUInt32($d, 0)
    $w = [BitConverter]::ToInt32($d, 4); $h = [BitConverter]::ToInt32($d, 8)
    $bpp = [BitConverter]::ToUInt16($d, 14); $comp = [BitConverter]::ToUInt32($d, 16)
    $amask = 0
    if ($hdr -ge 56) { $amask = [BitConverter]::ToUInt32($d, 52) }
    $s = "dib: headerSize=$hdr width=$w height=$h bpp=$bpp compression=$comp alphaMask=0x{0:X8}" -f $amask
    if ($bpp -eq 32) {
        $off = $hdr
        if ($comp -eq 3 -and $hdr -eq 40) { $off += 12 }
        $zero = 0; $full = 0; $other = 0; $n = 0
        for ($i = $off + 3; $i -lt $d.Length; $i += 4) {
            $a = $d[$i]; $n++
            if ($a -eq 0) { $zero++ } elseif ($a -eq 255) { $full++ } else { $other++ }
        }
        $s += " alpha: pixels=$n zero=$zero opaque=$full partial=$other"
    }
    return $s
}

$log = New-Object System.Collections.Generic.List[string]
if ($Now) {
    $log.Add('mode: -Now (current clipboard, no change log)')
} else {
    $startSeq = [ClipDump]::GetClipboardSequenceNumber()
    Write-Host "Waiting up to $WaitSeconds s for a clipboard change (current sequence $startSeq). Copy now."
    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    $seq = $startSeq
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 50
        $seq = [ClipDump]::GetClipboardSequenceNumber()
        if ($seq -ne $startSeq) { break }
    }
    if ($seq -eq $startSeq) { Write-Host 'No clipboard change. Nothing recorded.'; exit 1 }

    # Record every sequence change during the follow window (notification count / delayed rendering).
    $t0 = Get-Date
    $log.Add(("first change: sequence {0} -> {1}" -f $startSeq, $seq))
    $last = $seq
    $followEnd = (Get-Date).AddSeconds($FollowSeconds)
    while ((Get-Date) -lt $followEnd) {
        Start-Sleep -Milliseconds 20
        $s2 = [ClipDump]::GetClipboardSequenceNumber()
        if ($s2 -ne $last) {
            $log.Add(("+{0,6:N0} ms: sequence {1} -> {2}" -f ((Get-Date) - $t0).TotalMilliseconds, $last, $s2))
            $last = $s2
        }
    }
}

$formats = [ClipDump]::ListFormats()
if ($null -eq $formats) { Write-Host 'Could not open the clipboard.'; exit 1 }
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$dir = Join-Path $OutDir ("{0}-{1}" -f $Label, $stamp)
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$seqBeforeRead = [ClipDump]::GetClipboardSequenceNumber()
[ClipDump]::ReadData($formats, 300MB)
$seqAfterRead = [ClipDump]::GetClipboardSequenceNumber()

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("label: $Label")
$lines.Add("owner: " + [ClipDump]::OwnerProcess())
$lines.Add("sequence before read: $seqBeforeRead / after read: $seqAfterRead")
$lines.Add('--- sequence changes ---')
$log | ForEach-Object { $lines.Add($_) }
$lines.Add('--- formats (enumeration order) ---')
foreach ($e in $formats) {
    $lines.Add(("{0,6} {1,-50} size={2}" -f $e.Id, $e.Name, $e.Size))
}
$lines.Add('--- details ---')
foreach ($e in $formats) {
    if ($null -eq $e.Data) { continue }
    switch ($e.Name) {
        'HTML Format' {
            [IO.File]::WriteAllBytes((Join-Path $dir 'html-format.txt'), $e.Data)
            $lines.Add('HTML Format: saved to html-format.txt')
        }
        'PNG' {
            [IO.File]::WriteAllBytes((Join-Path $dir 'clipboard.png'), $e.Data)
            $lines.Add('PNG: saved to clipboard.png')
        }
        'CF_DIB' { $lines.Add('CF_DIB ' + (Describe-Dib $e.Data)) }
        'CF_DIBV5' { $lines.Add('CF_DIBV5 ' + (Describe-Dib $e.Data)) }
        'CF_UNICODETEXT' {
            $text = [Text.Encoding]::Unicode.GetString($e.Data).TrimEnd([char]0)
            $preview = if ($text.Length -gt 200) { $text.Substring(0, 200) } else { $text }
            $lines.Add(("CF_UNICODETEXT: length={0} preview={1}" -f $text.Length, ($preview -replace "`r?`n", ' / ')))
        }
        'CanIncludeInClipboardHistory' {
            $lines.Add('CanIncludeInClipboardHistory: value=' + [BitConverter]::ToUInt32($e.Data, 0))
        }
        default { }
    }
}
Write-Ascii (Join-Path $dir 'formats.txt') $lines
Write-Host "Recorded to $dir"
