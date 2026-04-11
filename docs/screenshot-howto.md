# 製品ページ スクリーンショット撮影手順

## 概要

mImageViewer は wgpu（DirectX）でレンダリングするため、通常の GDI ベースの
スクリーンキャプチャ（`CopyFromScreen`、`PrintWindow` 等）ではウィンドウ内容を
正しく取得できない。DXGI Desktop Duplication API を使う必要がある。

**動作確認済みの方法：Python `mss` ライブラリ（DXGI使用）**

---

## 前提条件

- Python 3.x がインストール済み
- `mss` と `Pillow` がインストール済み

```
pip install mss pillow
```

- サンプル画像が `C:\temp\miv-samples\` に揃っていること（後述）
- リリースビルドが最新であること

```
cd H:\home\mimageviewer
cargo build --release
```

---

## サンプル画像の準備

初回のみ必要。CC0 ライセンスの JPEG 画像と AI メタデータ付き PNG を用意する。

### 通常写真（15枚）
`C:\temp\miv-samples\photo01.jpg` ～ `photo15.jpg`

[picsum.photos](https://picsum.photos/) などから CC0 画像を取得する。

### AI メタデータ付きサンプル画像

`C:\temp\miv-samples\ai_sample.png` を作成する（A1111 形式の tEXt チャンク付き PNG）。

```python
# C:\temp\make_ai_sample.py
import struct, zlib

def make_png_chunk(name, data):
    name = name.encode('ascii')
    crc = zlib.crc32(name + data) & 0xFFFFFFFF
    return struct.pack('>I', len(data)) + name + data + struct.pack('>I', crc)

# 400x300 のグラデーション画像を生成
width, height = 400, 300
pixels = []
for y in range(height):
    row = []
    for x in range(width):
        r = int(180 + 60 * x / width - 40 * y / height)
        g = int(100 + 40 * y / height)
        b = int(200 - 60 * x / width + 40 * y / height)
        row += [min(255, max(0, r)), min(255, max(0, g)), min(255, max(0, b))]
    pixels.append(bytes([0] + row))

compressed = zlib.compress(b''.join(pixels))

metadata = (
    "masterpiece, best quality, 1girl, solo, long hair, white dress, "
    "standing in a field of flowers, golden hour, soft lighting, bokeh, depth of field, film grain\n"
    "Negative prompt: lowres, bad anatomy, bad hands, text, error, missing fingers, "
    "extra digit, fewer digits, cropped, worst quality\n"
    "Steps: 28, Sampler: DPM++ 2M Karras, CFG scale: 7, Seed: 3141592653, "
    "Size: 512x768, Model hash: a1b2c3d4, Model: CounterfeitV30"
)

with open(r'C:\temp\miv-samples\ai_sample.png', 'wb') as f:
    f.write(b'\x89PNG\r\n\x1a\n')
    ihdr = struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0)
    f.write(make_png_chunk('IHDR', ihdr))
    text_data = b'parameters\x00' + metadata.encode('latin-1')
    f.write(make_png_chunk('tEXt', text_data))
    f.write(make_png_chunk('IDAT', compressed))
    f.write(make_png_chunk('IEND', b''))

print("ai_sample.png 作成完了")
```

---

## デフォルト設定ディレクトリの準備

スクリーンショット用にクリーンな設定（お気に入りなし、個人設定なし）を使う。

```
mkdir C:\temp\miv-default
```

初回起動時に空の設定が自動生成される（手動操作不要）。

---

## スクリーンショット 1: グリッド表示（ss_grid.png）

グリッドは PrintWindow で取得できる（GDI でも OK）。

```powershell
# C:\temp\take_ss_grid.ps1
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition '
using System; using System.Runtime.InteropServices;
public class WG {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool r);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
    [DllImport("user32.dll")] public static extern void mouse_event(int f, int dx, int dy, int data, int extra);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    public struct RECT { public int L, T, R, B; }
    public const uint PW_RENDERFULLCONTENT = 2;
}
'
Stop-Process -Name mimageviewer -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = 'H:\home\mimageviewer\target\release\mimageviewer.exe'
$psi.Arguments = '--data-dir C:\temp\miv-default --window-size 1400x860'
$psi.UseShellExecute = $true
$proc = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds 5

$hw = $proc.MainWindowHandle
[WG]::ShowWindow($hw, 9) | Out-Null
[WG]::BringWindowToTop($hw) | Out-Null
[WG]::SetForegroundWindow($hw) | Out-Null
[WG]::MoveWindow($hw, 80, 30, 1400, 860, $true) | Out-Null
Start-Sleep -Milliseconds 800

# フォルダを開く
[WG]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait('^o')
Start-Sleep -Milliseconds 1200
[System.Windows.Forms.SendKeys]::SendWait('^a')
[System.Windows.Forms.SendKeys]::SendWait('C:\temp\miv-samples')
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
Start-Sleep -Seconds 7  # サムネイル読み込み待ち

# PrintWindow で取得
$r = New-Object WG+RECT
[WG]::GetWindowRect($hw, [ref]$r) | Out-Null
$w = $r.R - $r.L; $h = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[WG]::PrintWindow($hw, $hdc, [WG]::PW_RENDERFULLCONTENT) | Out-Null
$g.ReleaseHdc($hdc)
$bmp.Save('C:\temp\miv-samples\ss_grid.png', [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Host "ss_grid.png 保存完了"
```

実行（コンソールは表示されたままでよい）:

```
powershell -ExecutionPolicy Bypass -File C:\temp\take_ss_grid.ps1
```

---

## スクリーンショット 2 & 3: フルスクリーン・AIメタデータパネル

フルスクリーン表示は **DXGI** でしかキャプチャできない。
手順:

1. PowerShell を `-WindowStyle Hidden`（コンソール非表示）で起動してフルスクリーンを表示
2. Python `mss`（DXGI）でキャプチャ

### 2-A. フルスクリーン単独（ss_fullscreen.png）

```powershell
# C:\temp\setup_fullscreen.ps1  ← -WindowStyle Hidden で実行すること
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition '
using System; using System.Runtime.InteropServices;
public class WF {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool r);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern void mouse_event(int f, int dx, int dy, int data, int extra);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    public struct RECT { public int L, T, R, B; }
}
'
[System.IO.File]::WriteAllText('C:\temp\ss_status.txt', 'starting')
Stop-Process -Name mimageviewer -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = 'H:\home\mimageviewer\target\release\mimageviewer.exe'
$psi.Arguments = '--data-dir C:\temp\miv-default --window-size 1400x860'
$psi.UseShellExecute = $true
$proc = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds 5

$hw = $proc.MainWindowHandle
[WF]::ShowWindow($hw, 9) | Out-Null
[WF]::BringWindowToTop($hw) | Out-Null
[WF]::SetForegroundWindow($hw) | Out-Null
[WF]::MoveWindow($hw, 80, 30, 1400, 860, $true) | Out-Null
Start-Sleep -Milliseconds 800

[WF]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait('^o')
Start-Sleep -Milliseconds 1200
[System.Windows.Forms.SendKeys]::SendWait('^a')
[System.Windows.Forms.SendKeys]::SendWait('C:\temp\miv-samples')
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
Start-Sleep -Seconds 7

# グリッドをクリックしてフォーカス取得
$r = New-Object WF+RECT
[WF]::GetWindowRect($hw, [ref]$r) | Out-Null
[WF]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Milliseconds 400
[WF]::SetCursorPos($r.L + 400, $r.T + 450) | Out-Null
Start-Sleep -Milliseconds 150
[WF]::mouse_event(2, 0, 0, 0, 0); Start-Sleep -Milliseconds 80; [WF]::mouse_event(4, 0, 0, 0, 0)
Start-Sleep -Milliseconds 500

# 2枚目の写真を選択（HOMEで先頭 → RIGHTで次）
[System.Windows.Forms.SendKeys]::SendWait('{HOME}')
Start-Sleep -Milliseconds 300
[System.Windows.Forms.SendKeys]::SendWait('{RIGHT}')
Start-Sleep -Milliseconds 500

# マウスを中央付近に置いてからフルスクリーンへ（パネルが出ないように左側）
$sw = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width
$sh = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height
[WF]::SetCursorPos([int]($sw * 0.3), [int]($sh * 0.5)) | Out-Null
Start-Sleep -Milliseconds 400
[WF]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Milliseconds 200
[System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
Start-Sleep -Seconds 5
[System.IO.File]::WriteAllText('C:\temp\ss_status.txt', 'ready')
Start-Sleep -Seconds 20  # キャプチャ待ち
```

### 2-B. AIメタデータパネル付き（ss_metadata.png）

上記と同じ構成で、フルスクリーンを開く前に**マウスを右端に移動**しておく:

```powershell
# フルスクリーンを開く直前：
$mx = [int]($sw * 0.88)   # 画面右端 12% の位置
$my = [int]($sh * 0.50)
[WF]::SetCursorPos($mx, $my) | Out-Null
Start-Sleep -Milliseconds 400
# ↑ この状態でフルスクリーンを開くと、マウスが右端にあるのでパネルが自動表示される
[System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
Start-Sleep -Seconds 7
[System.IO.File]::WriteAllText('C:\temp\ss_status.txt', 'ready')
Start-Sleep -Seconds 20
```

### キャプチャ（Python mss）

```python
# C:\temp\capture_primary.py
import mss, mss.tools, sys

output = sys.argv[1] if len(sys.argv) > 1 else r'C:\temp\screenshot.png'

with mss.mss() as sct:
    # プライマリモニター = left=0, top=0 のモニター（インデックス 0 は仮想全画面）
    primary = next(
        (m for m in sct.monitors[1:] if m['left'] == 0 and m['top'] == 0),
        max(sct.monitors[1:], key=lambda m: m['width'] * m['height'])
    )
    shot = sct.grab(primary)
    mss.tools.to_png(shot.rgb, shot.size, output=output)
    print(f"Saved: {output}  ({shot.size.width}x{shot.size.height})")
```

### まとめて実行するコマンド例

```
# フルスクリーン（ss_fullscreen.png）
powershell -ExecutionPolicy Bypass -WindowStyle Hidden -File C:\temp\setup_fullscreen.ps1 &
# status が ready になるまで待つ（約 25 秒）
python C:\temp\capture_primary.py C:\temp\miv-samples\ss_fullscreen.png

# メタデータパネル付き（ss_metadata.png）
powershell -ExecutionPolicy Bypass -WindowStyle Hidden -File C:\temp\setup_metadata.ps1 &
python C:\temp\capture_primary.py C:\temp\miv-samples\ss_metadata.png
```

---

## キャプチャ後の処理

### 解像度の統一（必要な場合）

mss は物理ピクセルで取得するため、4K モニター（3840×2160）だと大きすぎる場合がある。
2560×1440 に縮小:

```python
from PIL import Image
img = Image.open(r'C:\temp\miv-samples\ss_metadata.png')
img.resize((2560, 1440), Image.LANCZOS).save(r'C:\temp\miv-samples\ss_metadata.png')
```

### Web サイトへのコピー

```
cp C:\temp\miv-samples\ss_grid.png     H:\home\mimageviewer\htdocs\mimageviewer\
cp C:\temp\miv-samples\ss_fullscreen.png H:\home\mimageviewer\htdocs\mimageviewer\
cp C:\temp\miv-samples\ss_metadata.png  H:\home\mimageviewer\htdocs\mimageviewer\
```

---

## トラブルシューティング

| 症状 | 原因 | 対処 |
|------|------|------|
| CopyFromScreen でパネルが写らない | GDI は DirectX コンテンツ非対応 | mss (DXGI) を使う |
| PrintWindow でも写らない | 同上（egui のオーバーレイ部分） | mss (DXGI) を使う |
| mss のモニター番号が合わない | 物理座標で left=0, top=0 を探す | `capture_primary.py` の自動検出を使う |
| パネルが表示されない | マウスが右端にいない / キー未受信 | フルスクリーンを開く**前に**マウスを右端（x=88%）に移動しておく |
| コンソールがパネルを隠す | PowerShell ウィンドウが右側に重なる | `-WindowStyle Hidden` で起動する |
| フルスクリーン後にメインウィンドウが前面へ | z-order 操作との競合 | ENTER 送信後は BringWindowToTop / SetForegroundWindow を呼ばない |
