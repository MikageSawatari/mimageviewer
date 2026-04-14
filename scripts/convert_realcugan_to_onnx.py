"""
Real-CUGAN 4x を PyTorch (.pth) から ONNX に変換するスクリプト。

使い方:
  python scripts/convert_realcugan_to_onnx.py

入力:  models/up4x-latest-conservative.pth
出力:  models/realcugan_4x_conservative.onnx
"""

import os
import sys
import torch
import torch.nn as nn
import torch.nn.functional as F

# upcunet_v3.py をインポートするためにパスを追加
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__))))
from upcunet_v3 import UpCunet4x


class UpCunet4xForExport(nn.Module):
    """ONNX エクスポート用ラッパー。
    tile_mode=0, cache_mode=0, alpha=1.0, pro=False を固定して、
    入力テンソルのみを受け取る forward() にする。
    出力を float [0, 1] に変換する（元の実装は byte [0, 255]）。
    """
    def __init__(self, model: UpCunet4x):
        super().__init__()
        self.unet1 = model.unet1
        self.unet2 = model.unet2
        self.ps = model.ps
        self.conv_final = model.conv_final

    def forward(self, x):
        n, c, h0, w0 = x.shape
        x00 = x
        # tile_mode=0 のパスを直接展開
        ph = ((h0 - 1) // 2 + 1) * 2
        pw = ((w0 - 1) // 2 + 1) * 2
        x = F.pad(x, (19, 19 + pw - w0, 19, 19 + ph - h0), 'reflect')
        x = self.unet1.forward(x)
        x0 = self.unet2.forward(x, alpha=1.0)
        x1 = F.pad(x, (-20, -20, -20, -20))
        x = torch.add(x0, x1)
        x = self.conv_final(x)
        x = F.pad(x, (-1, -1, -1, -1))
        x = self.ps(x)
        if w0 != pw or h0 != ph:
            x = x[:, :, :h0 * 4, :w0 * 4]
        x = x + F.interpolate(x00, scale_factor=4, mode='nearest')
        # [0, 1] 範囲にクランプして返す（byte変換はしない）
        return x.clamp(0.0, 1.0)


def main():
    script_dir = os.path.dirname(os.path.abspath(__file__))
    models_dir = os.path.join(os.path.dirname(script_dir), "models")

    pth_path = os.path.join(models_dir, "up4x-latest-conservative.pth")
    onnx_path = os.path.join(models_dir, "realcugan_4x_conservative.onnx")

    if not os.path.exists(pth_path):
        print(f"Error: {pth_path} not found")
        sys.exit(1)

    print(f"Loading weights from {pth_path}...")
    model = UpCunet4x(in_channels=3, out_channels=3)
    state_dict = torch.load(pth_path, map_location='cpu', weights_only=True)
    model.load_state_dict(state_dict, strict=True)
    model.eval()

    # ラッパーで forward を単純化
    export_model = UpCunet4xForExport(model)
    export_model.eval()

    print("Converting to ONNX...")
    # 偶数サイズの入力でテスト（2で割り切れる必要がある）
    dummy_input = torch.randn(1, 3, 64, 64)

    torch.onnx.export(
        export_model,
        dummy_input,
        onnx_path,
        opset_version=17,
        input_names=['input'],
        output_names=['output'],
        dynamic_axes={
            'input': {0: 'batch', 2: 'height', 3: 'width'},
            'output': {0: 'batch', 2: 'height', 3: 'width'},
        },
        dynamo=False,  # TorchScript ベースでエクスポート
    )

    size_mb = os.path.getsize(onnx_path) / (1024 * 1024)
    print(f"Done! Saved to {onnx_path} ({size_mb:.1f} MB)")

    # 検証: ダミー入力で推論して出力シェイプを確認
    import onnx
    onnx_model = onnx.load(onnx_path)
    for inp in onnx_model.graph.input:
        dims = [d.dim_value or d.dim_param for d in inp.type.tensor_type.shape.dim]
        print(f"  Input: {inp.name} shape={dims}")
    for out in onnx_model.graph.output:
        dims = [d.dim_value or d.dim_param for d in out.type.tensor_type.shape.dim]
        print(f"  Output: {out.name} shape={dims}")


if __name__ == '__main__':
    main()
