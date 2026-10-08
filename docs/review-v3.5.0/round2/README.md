# v3.5.0 再レビュー (第2回)

開始: 2026-09-03。前回指摘への修正と、その変更で生じる退行を確認する。アプリコードは変更しない。

**再レビュー完了**。全体 gate は通過したが、残件・追加指摘が15件 (P2:14 / P3:1) ある。
詳細は [指摘一覧](findings.md)、[検証・確認範囲](verification.md) を参照。
最終 HEAD: `cf4a3ca502000bb65e5d0af5c85cd0ba305e01cb` (実コードの最終変更は `8632af6a9`)。
基点から17 commits / 31 files / +2,310 -255。開始後のマニュアル更新2 commitsも照合した。

- 基点: `512b49d4dbb1fb3d64801458c629d769331bf881`。
- 開始 HEAD: `57fc2bc36`。21 files、2,250 insertions / 249 deletions。
- 開始時の状態: 前回の `docs/review-v3.5.0/` が未追跡。tracked file の未コミット差分なし。
- 実装者の完了報告・テスト報告は照合対象とし、そのまま解消の根拠にはしない。
- 通常 profile のアプリ起動・設定データ操作・署名・配布物作成は行わない。

## 確認状況

| 前回ID | 観点 | 状態 |
|---|---|---|
| F01 | export integration / 全体 gate | 解消。追加 commit `8632af6a9` で lease の integration producer と公開経路を修正。全体 gate PASS |
| F02 | 関連付けの path ごとの結果と temp ownership | 解消。成功済みを後段へ再投入せず、失敗 path と一時ファイルの所有を追跡 |
| F03 | AI / 単枚 / Merged の段の接続 | 部分解消。製本・一括・外部個別を接続。単枚/Merged、AI cache identity、注釈寸法、runtime 初期化失敗が残る |
| F04 | 一括 export 準備の UI thread 境界 | 未完。6ms は処理後の判定で、個別の DB/展開/初期化は UI。フレーム間の index 保持で別対象へ変わり得る |
| F05 | Merged の合成と hash の全体 | 部分解消。不要な hash 削除は適切。合成・回転・crop・コピーは UI に残る |
| F06 | gamepad OFF / 切断 / overlay / 再開 | 保持入力の原症状は解消。新しい終了処理が live rating の Undo を作らず picker を破棄。stop の UI join も残る |
| F07 | 保存 span と各復元経路 | 解消。toggle/直接指定へ保存 span を渡し、visible/current と hidden/remembered を区別 |
| F08 | context / sibling invalidation / 消滅 / undo | 所有 context への配送は解消。同じ page key を持つ sibling の mutation 失効は未実装 (計画にも明記) |
| F09 | AI resource lifetime / cancel / Remote acquire | 解消。両 worker の要求/入口へ mandatory lease、closure の寿命まで保持。実機接続は未検証 |
| F10 | effective params / LUT / 入れ子お気に入り | 外部ツールは解消。worker が確定 params と LUT を同時解決。製本/一括exportの同型の親設定 resolver は未修正 (R15) |
| F11 | 共通 panel geometry / 狭幅 | 解消。確保済み幅を渡し、縮んだ幅からの逆算を廃止 |
| F12 | 負原点 / tie / 連結 / DPI / crop | 未完。等寸法の負 tie は改善。奇数/偶数高さの混在で gap ±1px を現関数の実行で再現 |
| F13 | 関連付けの先行列挙 / miss / error / 開閉 | 部分解消。hit は高速化。miss は UI 同期列挙、cache の更新/失効経路がない |
| F14 | scan worker / 世代 / discard / 再要求 | 部分解消。走査は worker 化。実行中通知の破棄、世代未検査、所有投影変更時の結果破棄が残る |
| F15 | 移行失敗 → save → reload | 解消。通常保存は未完移行 marker を進めず、障害解除後の load で移行できる |
| F16 | 再割当 / 未割当のメニュー表示 | 解消。解決済み shortcut label を snapshot へ渡し、未割当は省略 |

## 追加版・検証結果

- `8632af6a9781caaf44e909ccfc9444bb74bd1cf0` を追加確認。基点から 15 commits / 23 files。
- 初回 gate は同時編集途中の `LocalAiActivityLease` 公開前に integration test をコンパイルし失敗。`test-full.log` は途中版の記録。現版の製品エラーとして数えない。
- `test-full-current.log`: 上記 commit で全体 gate PASS。8,159 passed / 0 failed / 36 ignored。
- `test-fmt.log`: `cargo fmt --check` PASS。`test-glyphs.log`: 危険文字0。
- `test-bulk.log`: 狭い一括編集テスト 19 passed。
- `geometry_probe.py` / `geometry-probe.log`: 現ソースの実関数・型を抽出してコンパイル。2,880 境界中 464 で設定 gap と 0.01px 超の差。代表例は 1000/1001px 混在、gap0 → `[1,-1,1,-1]`。同寸法 control は改善を確認。
- `annotation_probe.rs` / `annotation_probe.log`: 公開production compositorで16×16→32×32の決定的AI結果を合成し、元寸法なしの注釈位置ずれを再現。機器・GPU/AI推論での実測とは区別する。
- 同時に追加された `docs/README.md` / `docs/similar-image-search-research.md` はこのレビューの変更ではない。
