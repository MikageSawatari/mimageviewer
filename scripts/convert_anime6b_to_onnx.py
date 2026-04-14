"""
Real-ESRGAN x4plus_anime_6B を PyTorch (.pth) から ONNX に変換するスクリプト。

使い方:
  pip install torch onnx --index-url https://download.pytorch.org/whl/cpu
  python scripts/convert_anime6b_to_onnx.py

入力:  models/RealESRGAN_x4plus_anime_6B.pth
出力:  models/realesrgan_x4plus_anime_6b.onnx
"""

import os
import sys
import torch
import torch.nn as nn

# -----------------------------------------------------------------------
# RRDBNet アーキテクチャ定義 (Real-ESRGAN / BasicSR から抜粋)
# -----------------------------------------------------------------------

class ResidualDenseBlock(nn.Module):
    def __init__(self, num_feat=64, num_grow_ch=32):
        super().__init__()
        self.conv1 = nn.Conv2d(num_feat, num_grow_ch, 3, 1, 1)
        self.conv2 = nn.Conv2d(num_feat + num_grow_ch, num_grow_ch, 3, 1, 1)
        self.conv3 = nn.Conv2d(num_feat + 2 * num_grow_ch, num_grow_ch, 3, 1, 1)
        self.conv4 = nn.Conv2d(num_feat + 3 * num_grow_ch, num_grow_ch, 3, 1, 1)
        self.conv5 = nn.Conv2d(num_feat + 4 * num_grow_ch, num_feat, 3, 1, 1)
        self.lrelu = nn.LeakyReLU(negative_slope=0.2, inplace=True)

    def forward(self, x):
        x1 = self.lrelu(self.conv1(x))
        x2 = self.lrelu(self.conv2(torch.cat((x, x1), 1)))
        x3 = self.lrelu(self.conv3(torch.cat((x, x1, x2), 1)))
        x4 = self.lrelu(self.conv4(torch.cat((x, x1, x2, x3), 1)))
        x5 = self.conv5(torch.cat((x, x1, x2, x3, x4), 1))
        return x5 * 0.2 + x


class RRDB(nn.Module):
    def __init__(self, num_feat, num_grow_ch=32):
        super().__init__()
        self.rdb1 = ResidualDenseBlock(num_feat, num_grow_ch)
        self.rdb2 = ResidualDenseBlock(num_feat, num_grow_ch)
        self.rdb3 = ResidualDenseBlock(num_feat, num_grow_ch)

    def forward(self, x):
        out = self.rdb1(x)
        out = self.rdb2(out)
        out = self.rdb3(out)
        return out * 0.2 + x


class RRDBNet(nn.Module):
    def __init__(self, num_in_ch=3, num_out_ch=3, scale=4, num_feat=64,
                 num_block=6, num_grow_ch=32):
        super().__init__()
        self.scale = scale
        num_upsample = 2  # 4x = 2^2

        self.conv_first = nn.Conv2d(num_in_ch, num_feat, 3, 1, 1)
        self.body = nn.Sequential(
            *[RRDB(num_feat=num_feat, num_grow_ch=num_grow_ch)
              for _ in range(num_block)]
        )
        self.conv_body = nn.Conv2d(num_feat, num_feat, 3, 1, 1)

        # upsample
        self.conv_up1 = nn.Conv2d(num_feat, num_feat, 3, 1, 1)
        self.conv_up2 = nn.Conv2d(num_feat, num_feat, 3, 1, 1)
        self.conv_hr = nn.Conv2d(num_feat, num_feat, 3, 1, 1)
        self.conv_last = nn.Conv2d(num_feat, num_out_ch, 3, 1, 1)
        self.lrelu = nn.LeakyReLU(negative_slope=0.2, inplace=True)

    def forward(self, x):
        feat = self.conv_first(x)
        body_feat = self.conv_body(self.body(feat))
        feat = feat + body_feat

        # upsample 2x twice = 4x
        feat = self.lrelu(self.conv_up1(
            torch.nn.functional.interpolate(feat, scale_factor=2, mode='nearest')))
        feat = self.lrelu(self.conv_up2(
            torch.nn.functional.interpolate(feat, scale_factor=2, mode='nearest')))

        out = self.conv_last(self.lrelu(self.conv_hr(feat)))
        return out


def main():
    script_dir = os.path.dirname(os.path.abspath(__file__))
    models_dir = os.path.join(os.path.dirname(script_dir), "models")

    pth_path = os.path.join(models_dir, "RealESRGAN_x4plus_anime_6B.pth")
    onnx_path = os.path.join(models_dir, "realesrgan_x4plus_anime_6b.onnx")

    if not os.path.exists(pth_path):
        print(f"Error: {pth_path} not found")
        print("Download from: https://github.com/xinntao/Real-ESRGAN/releases/download/v0.2.2.4/RealESRGAN_x4plus_anime_6B.pth")
        sys.exit(1)

    print(f"Loading weights from {pth_path}...")

    # anime_6B: num_block=6, num_feat=64, num_grow_ch=32
    model = RRDBNet(num_in_ch=3, num_out_ch=3, scale=4,
                    num_feat=64, num_block=6, num_grow_ch=32)

    # 重みをロード (params_ema キーがある場合はそちらを使う)
    state_dict = torch.load(pth_path, map_location='cpu', weights_only=True)
    if 'params_ema' in state_dict:
        state_dict = state_dict['params_ema']
    elif 'params' in state_dict:
        state_dict = state_dict['params']

    model.load_state_dict(state_dict, strict=True)
    model.eval()

    print("Converting to ONNX...")

    # ダイナミック入力サイズ対応
    dummy_input = torch.randn(1, 3, 64, 64)

    # torch 2.11+ では dynamo=False で旧式 TorchScript ベースのエクスポートを使う
    torch.onnx.export(
        model,
        dummy_input,
        onnx_path,
        opset_version=17,
        input_names=['input'],
        output_names=['output'],
        dynamic_axes={
            'input': {0: 'batch', 2: 'height', 3: 'width'},
            'output': {0: 'batch', 2: 'height', 3: 'width'},
        },
        dynamo=False,
    )

    size_mb = os.path.getsize(onnx_path) / (1024 * 1024)
    print(f"Done! Saved to {onnx_path} ({size_mb:.1f} MB)")

    # 後片付け: .pth ファイルは削除
    os.remove(pth_path)
    print(f"Removed {pth_path}")


if __name__ == '__main__':
    main()
