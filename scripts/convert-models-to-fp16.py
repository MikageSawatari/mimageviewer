#!/usr/bin/env python3
"""
ONNX モデルを FP32 → FP16 に変換するスクリプト。

目的:
  vendor/models/ 配下の 8 個の .onnx を FP16 化し、DirectML 推論を 1.3〜1.8 倍
  高速化、ファイルサイズを約半分にする (exe 埋め込みサイズも半減)。

動作:
  各 .onnx を読み込み、convert_float_to_float16(keep_io_types=True) で
  内部演算のみ FP16 化する。入出力テンソルは FP32 のままなので、Rust 側の
  Tensor::from_array(f32) コードは変更不要 (モデル境界に Cast op が自動挿入される)。

  オリジナルは .fp32.onnx にリネームしてバックアップ。新しい FP16 版は
  元のファイル名 (例: realesrgan_x4plus.onnx) で保存するので model_manager.rs の
  パスは変更不要。

使い方 (PowerShell):
  cd C:\\home\\mimageviewer\\.claude\\worktrees\\dazzling-mcclintock-d0be64
  python scripts\\convert-models-to-fp16.py
  # → vendor/models/*.onnx が FP16 化、*.fp32.onnx が backup として残る

  もし戻したい場合:
  python scripts\\convert-models-to-fp16.py --restore
  # → *.fp32.onnx を *.onnx にリネームして元に戻す

事前準備 (一度だけ):
  pip install onnx onnxconverter-common

注意:
  - 一部のオペレータは FP16 サポートがないため、変換ツールが自動で FP32 のまま
    残すケースがある (op_block_list で制御可能)。デフォルトでも問題ないモデルが
    多いが、変換後の品質を mIV 上で目視確認することを推奨。
  - 変換後、TensorRT エンジンキャッシュは無効化される (モデルハッシュが変わるため)。
    次回 TRT 利用時に再ビルドが走る (5〜10 分目安、MI-GAN は別途 5〜10 分追加)。
"""

import argparse
import os
import sys
from pathlib import Path

try:
    import onnx
    from onnxconverter_common.float16 import convert_float_to_float16
except ImportError as e:
    print(f"[ERROR] 必要パッケージが入っていません: {e}", file=sys.stderr)
    print("以下を実行してください:", file=sys.stderr)
    print("  pip install onnx onnxconverter-common", file=sys.stderr)
    sys.exit(1)


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
MODELS_DIR = REPO_ROOT / "vendor" / "models"

# vendor/models/ 配下にある FP16 化対象ファイル一覧
MODEL_FILES = [
    "anime_classifier_mobilenetv3.onnx",
    "realesrgan_x4plus.onnx",
    "realesrgan_x4plus_anime_6b.onnx",
    "realesr_general_x4v3.onnx",
    "realcugan_4x_conservative.onnx",
    "4x_NMKD-Siax_200k.onnx",
    "dejpg_realplksr_otf.onnx",
    "migan.onnx",
]


def fp32_backup_path(model_path: Path) -> Path:
    """元 FP32 ファイルのバックアップパス (.fp32.onnx)。"""
    return model_path.with_suffix(".fp32.onnx")


def is_already_fp16(model_path: Path) -> bool:
    """既に FP16 変換済みか? (バックアップが存在するかで判定)"""
    return fp32_backup_path(model_path).exists()


def convert_one(model_path: Path) -> bool:
    """1 モデルを FP16 化する。成功で True、失敗で False。"""
    print(f"\n=== {model_path.name} ===")
    if not model_path.exists():
        print(f"  [SKIP] ファイルが見つかりません")
        return False

    backup = fp32_backup_path(model_path)
    if backup.exists():
        print(f"  [SKIP] 既に変換済み ({backup.name} が存在)")
        return True

    orig_size = model_path.stat().st_size

    print(f"  ロード中... ({orig_size / 1024 / 1024:.1f} MB)")
    try:
        model = onnx.load(str(model_path))
    except Exception as e:
        print(f"  [ERROR] ロード失敗: {e}", file=sys.stderr)
        return False

    print("  FP16 変換中...")
    try:
        # keep_io_types=True: 入出力テンソルの型は FP32 のまま、
        #   内部演算のみ FP16 化。Rust 側の f32 入出力コードを変更不要にするため。
        # disable_shape_infer=True: 一部の古い ONNX モデルで形状推論が失敗する
        #   ことがあるので無効化 (変換自体には不要)。
        fp16_model = convert_float_to_float16(
            model,
            keep_io_types=True,
            disable_shape_infer=True,
        )
    except Exception as e:
        print(f"  [ERROR] 変換失敗: {e}", file=sys.stderr)
        return False

    # オリジナルを .fp32.onnx にリネームしてバックアップ
    print(f"  バックアップ: {model_path.name} → {backup.name}")
    model_path.rename(backup)

    # FP16 版を元のファイル名で保存
    print(f"  保存中...")
    try:
        onnx.save(fp16_model, str(model_path))
    except Exception as e:
        print(f"  [ERROR] 保存失敗: {e}、バックアップから戻します", file=sys.stderr)
        backup.rename(model_path)
        return False

    new_size = model_path.stat().st_size
    saving = (orig_size - new_size) / orig_size * 100
    print(
        f"  完了: {orig_size / 1024 / 1024:.1f} MB → "
        f"{new_size / 1024 / 1024:.1f} MB ({saving:.1f}% 削減)"
    )
    return True


def restore_one(model_path: Path) -> bool:
    """バックアップから FP32 を復元する。"""
    backup = fp32_backup_path(model_path)
    if not backup.exists():
        print(f"  [SKIP] {model_path.name}: バックアップなし")
        return False

    print(f"  復元: {backup.name} → {model_path.name}")
    if model_path.exists():
        model_path.unlink()
    backup.rename(model_path)
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[2])
    parser.add_argument(
        "--restore",
        action="store_true",
        help="FP16 → FP32 に戻す (バックアップ .fp32.onnx を使用)",
    )
    args = parser.parse_args()

    if not MODELS_DIR.is_dir():
        print(f"[ERROR] {MODELS_DIR} が見つかりません", file=sys.stderr)
        print("setup-pdfium.sh / setup-ort.sh などのスクリプトでセットアップしたか確認してください")
        sys.exit(1)

    print(f"対象ディレクトリ: {MODELS_DIR}")
    print(f"操作: {'FP16 → FP32 復元' if args.restore else 'FP32 → FP16 変換'}")
    print(f"対象モデル: {len(MODEL_FILES)} 個")

    success = 0
    skipped = 0
    failed = 0
    total_before = 0
    total_after = 0

    for filename in MODEL_FILES:
        model_path = MODELS_DIR / filename
        if args.restore:
            r = restore_one(model_path)
        else:
            if model_path.exists():
                total_before += model_path.stat().st_size
            r = convert_one(model_path)
            if model_path.exists():
                total_after += model_path.stat().st_size

        if r is True:
            success += 1
        elif r is False:
            failed += 1
        else:
            skipped += 1

    print()
    print("=" * 50)
    print(f"成功: {success} / {len(MODEL_FILES)} (失敗: {failed}, スキップ: {skipped})")
    if not args.restore and total_before > 0:
        saving = (total_before - total_after) / total_before * 100
        print(
            f"合計サイズ: {total_before / 1024 / 1024:.1f} MB → "
            f"{total_after / 1024 / 1024:.1f} MB ({saving:.1f}% 削減)"
        )
    print()

    if not args.restore and success > 0:
        print("次のステップ:")
        print("  1. cargo build --release でリビルド (新モデルが exe に埋め込まれる)")
        print("  2. アプリを起動して AI アップスケール / デノイズ / 消しゴムが動くか確認")
        print("  3. 問題があれば: python scripts/convert-models-to-fp16.py --restore")
        print("  4. TRT 利用時: 既存エンジンキャッシュは無効化、次回利用時に自動再ビルド")

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()
