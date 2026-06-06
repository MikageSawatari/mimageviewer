#!/usr/bin/env bash
# 編集用追加パック (オノマトペ向け OFL フォント + 被写体分離モデル) を
# GitHub Releases へ配信するスクリプト。
#
# 使い方:
#   bash scripts/publish-editing-pack.sh build         # pack をビルドするだけ (アップロードしない)
#   bash scripts/publish-editing-pack.sh check         # 現在 Release に上がっているアセットを表示
#   bash scripts/publish-editing-pack.sh publish       # ビルド → GitHub Releases へアップロード
#
# build / publish には build_editing_pack への追加引数を渡せる:
#   bash scripts/publish-editing-pack.sh build --pack-version 2026.07.0
#   bash scripts/publish-editing-pack.sh publish --models vendor/editing-pack/models
#
# 前提:
#   - gh (GitHub CLI) が認証済みであること (publish / check)
#   - cargo (release ビルド可能な環境) であること (build / publish)
#   - 被写体分離モデル (BiRefNet 等) を同梱する場合は vendor/editing-pack/models/ に
#     *.onnx + ライセンス txt を置いてから実行する (無ければフォントのみの pack になる)
#
# 配信先:
#   - REPO の Release タグ EDITING_PACK_TAG (= editing_addon_download.rs の
#     DEFAULT_PACK_BASE_URL のタグと一致させること)
#   - アセット: editing-pack-<version>.zip + editing-pack-index.json

set -euo pipefail

cd "$(dirname "$0")/.."

# editing_addon_download.rs の DEFAULT_PACK_BASE_URL と一致させる:
#   https://github.com/<REPO>/releases/download/<EDITING_PACK_TAG>
REPO="MikageSawatari/mimageviewer"
EDITING_PACK_TAG="editing-pack-v1"

# build_editing_pack の出力先 (バージョンに依存しない固定 dir にしてアップロードを簡単に)。
OUT_DIR="dist/editing-pack-publish"

mode="${1:-publish}"
shift || true   # 残りは build_editing_pack へ forward

usage() {
    echo "usage: bash scripts/publish-editing-pack.sh {build|check|publish} [build_editing_pack args...]" >&2
}

build_pack() {
    echo "[publish-editing-pack] building pack into $OUT_DIR ..."
    cargo run --release --bin build_editing_pack -- --out "$OUT_DIR" "$@"
}

find_assets() {
    # ビルド済み成果物 (zip + index.json) のパスを ZIP / INDEX に設定する。
    ZIP="$(ls "$OUT_DIR"/editing-pack-*.zip 2>/dev/null | head -n1 || true)"
    INDEX="$OUT_DIR/editing-pack-index.json"
    if [ -z "$ZIP" ] || [ ! -f "$INDEX" ]; then
        echo "ERROR: 成果物が見つかりません ($OUT_DIR)。先に build を実行してください。" >&2
        exit 1
    fi
}

case "$mode" in
    build)
        build_pack "$@"
        find_assets
        echo
        echo "[publish-editing-pack] build OK:"
        echo "  zip:   $ZIP"
        echo "  index: $INDEX"
        echo "アップロードするには: bash scripts/publish-editing-pack.sh publish"
        ;;

    check)
        echo "[publish-editing-pack] repo: $REPO  tag: $EDITING_PACK_TAG"
        if gh release view "$EDITING_PACK_TAG" --repo "$REPO" >/dev/null 2>&1; then
            echo "--- 現在 Release に上がっているアセット ---"
            gh release view "$EDITING_PACK_TAG" --repo "$REPO" \
                --json assets --jq '.assets[] | "  \(.name)  (\(.size) bytes)"'
        else
            echo "(Release タグ $EDITING_PACK_TAG はまだ存在しません)"
        fi
        ;;

    publish)
        build_pack "$@"
        find_assets
        echo
        echo "[publish-editing-pack] uploading to $REPO ($EDITING_PACK_TAG) ..."
        echo "  zip:   $ZIP"
        echo "  index: $INDEX"

        # Release が無ければ作る。本体リリース (Latest) を奪わないよう prerelease で作成
        # する (TensorRT pack と同方針)。--latest=false で Latest 昇格も明示的に防ぐ。
        if ! gh release view "$EDITING_PACK_TAG" --repo "$REPO" >/dev/null 2>&1; then
            echo "[publish-editing-pack] Release $EDITING_PACK_TAG が無いので作成します (prerelease)"
            gh release create "$EDITING_PACK_TAG" --repo "$REPO" \
                --prerelease --latest=false \
                --title "Editing add-on pack" \
                --notes "mImageViewer の編集用追加パック (オノマトペ向けフォント + 被写体分離モデル) 配信用。アプリが editing-pack-index.json を参照してダウンロードします。"
        fi

        # --clobber で同名アセットを上書き (= pack 更新時の再アップロード)。
        gh release upload "$EDITING_PACK_TAG" --repo "$REPO" --clobber "$ZIP" "$INDEX"
        echo
        echo "[publish-editing-pack] 完了。本体を起動してテキスト編集に入り、DL フローを確認してください。"
        ;;

    -h|--help|help)
        usage
        ;;
    *)
        echo "ERROR: 未知のモード: $mode" >&2
        usage
        exit 2
        ;;
esac
