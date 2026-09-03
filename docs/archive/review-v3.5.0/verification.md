# 検証記録と残る確認

2026-09-03、Windows、最終対象 `512b49d4dbb1fb3d64801458c629d769331bf881`。
通常の `%APPDATA%\mimageviewer` は使用していない。アプリの main を起動する GUI 検証は行っていない。

## 実行結果

| 検証 | 結果 | 証拠 |
|---|---|---|
| `cargo test -p mimageviewer --lib external_tool` | 70 passed | 初期の絞り込み実行。全体 gate でも再実行 |
| 初回 `scripts/test-full.ps1` | exit 101、E0063。F01 | [test-full.log](test-full.log) |
| `cargo test --workspace --lib --no-fail-fast` | 7,711 passed / 0 failed / 31 ignored | [test-libs.log](test-libs.log) |
| 更新後 `scripts/test-full.ps1` | **PASS、8,133 passed / 0 failed / 36 ignored** | [test-full-current.log](test-full-current.log) |
| vendored egui-wgpu / eframe の先行個別実行 | 9 / 15 passed。後の全体 gate にも含まれる | test-egui-wgpu.log / test-eframe.log |
| Remote Web 9 files の直接実行 | **382 passed / 0 failed / 0 skipped** | [test-remote-web-direct.log](test-remote-web-direct.log) |
| `cargo fmt --check`、最終確認 | PASS (exit 0) | [test-fmt-final.log](test-fmt-final.log) |
| `python scripts/check_ui_glyphs.py` | dangerous glyph 0 | [test-glyphs.log](test-glyphs.log) |
| geometry の数値再現 | F11 / F12 確認 | [geometry-probe.log](geometry-probe.log) |
| migration の障害注入 | F15 確認、probe exit 0 | [settings-migration-probe.log](settings-migration-probe.log) |

8,133 は 51 個の harness summary の合計で、独立した条件の網羅数ではない。mimageviewer lib 7,209、UI snapshot integration 43、export integration 17、Remote executable 118、IPC 54 等を含む。先行実行を合算して水増ししていない。`#[test]` 重複の警告があるため、同名関数の重複実行も報告値に含まれる。

初回 gate の失敗はテスト runner の timeout ではなく、`BakedEditSnapshot` の field 追加に追随しなかった integration initializer の型エラー。レビュー中に別作業の `512b49d4d` が修正し、再実行は成功した。

### Remote Web の実行方法と制約

通常の `node --test` は子プロセス spawn の `EPERM` で失敗した。この環境の Node は in-process 用の追加 flag も受理しなかったため、既存の各 `.test.mjs` を Node で直接実行した。各ファイル内の `node:test` assertion と TAP summary は実行されており、skip へ置き換えてはいない。launcher の子プロセス隔離や全ファイルを一度に動かす挙動は未検証。

実行対象 (すべて `crates/remote-web/web/`):

| ファイル | passed |
|---|---:|
| app-runtime.test.mjs | 100 |
| command-core.test.mjs | 128 |
| document-double-tap.test.mjs | 7 |
| local-settings.test.mjs | 11 |
| page-coordinator.test.mjs | 27 |
| page-timings.test.mjs | 6 |
| pwa.test.mjs | 41 |
| video-stream.test.mjs | 50 |
| viewer-position.test.mjs | 12 |

元の runner 失敗は test-remote-web.log、未対応 flag は test-remote-web-inprocess.log に残している。Web テストの成功は、スマートフォンの実ブラウザや実 DirectML/動画 stream の動作保証ではない。

## 再現資料

### F11 / F12: geometry

`geometry_probe.py` は実ソースから quantize / physical extent / snap / reading offsets の関数を抽出し、既存ビルドの egui をリンクして小さな計算プログラムを作る。`target/review-v350` が作業先。アプリは起動しない。

- F11: music full width 640 / 800 / 860 / 1200pt を代入。誤った逆算による右端超過は 55 / 15 / 0 / 0pt。
- F12: 501×1001 の縦画像、等倍、先頭 unit の中心0、gap 0/1/20、pixels-per-point 1/1.25/1.5/2。負/正の .5 origin を跨ぐ時、隣接辺が最大1px離れる具体例をログ化。
- 各 DPI のすべての gap が悪化するわけではない。例えば 125% の gap1 はほぼ期待通り。これは tie 条件に依存する不具合であり、通常の画像で一度見ただけでは見逃しやすい。
- 現在のプログラムは単ページ・trim なしの arithmetic を再現。修正時は production の描画経路で screenshot / vertex rect の回帰テストも追加する。

再実行: リポジトリ直下から `python docs/review-v3.5.0/geometry_probe.py`。既存 egui rlib と Rust toolchain が必要。修正後は抽出関数名や API 変更に合わせて probe も更新する。

### F15: migration

`settings_migration_probe.rs` は公開 SettingsDb API と使い捨て DB を使用する。旧登録1件を用意し、migration の INSERT のみ trigger で拒否 → load → trigger 解除 → 無関係な設定保存 → reload を実行した。

実測:

```text
after failed migration: legacy=1 external=0
after ordinary save and reload: legacy=1 external=0 migration_marker=1
```

実行時だけ source を `examples/review_v350_settings_migration.rs` に置き、`cargo run --example review_v350_settings_migration --features pack-build-tools` で実行。その後 source は本レビュー directory へ移動した。正常アプリの entrypoint は呼ばず、通常の settings.db は開かない。

この probe の assert は「現状の不具合を確認する」向きになっている。修正後の回帰テストでは、legacy 登録が external_tools に現れ、成功した移行だけに marker が付くことを期待する形へ変更する。

## 優先して追加する回帰テスト

| 指摘 | 境界・入力 | 合格条件 |
|---|---|---|
| F08 | viewer A で開始、B を mount した frame で completion を drain。A/B に同じ page と違う page の両方 | DB・A の runtime・必要な sibling 失効が一致し、B の無関係な編集/undo/cache は不変 |
| F09 | batch / materializer が erase runtime を保持したまま Remote acquire、cancel、worker の後着完了 | RemoteActive はローカル AI の停止後だけ。活動数は worker resource の実寿命に一致 |
| F15 | migration の INSERT / marker / commit 失敗後、障害解除、通常 save、restart | 旧登録を失わず、未完了移行を完了扱いにしない |
| F06 | 非ゼロ軸、repeat 待ち button、リング保持から OFF → 数 frame → ON | OFF 中の action/repaint が消え、ON は新規入力から再開 |
| F02 | Shell の path 別 success / failure 混在、全失敗 | 成功済みは再実行せず、件数と temp ownership が path ごとの結果に一致 |
| F03 / F10 | 単枚/製本/一括/外部、3段、ページ LUT、入れ子お気に入り | 同じ有効設定の同じ段が同じ意味の出力。AI/表示補正の除外も実際の画素で確認 |
| F07 | WholeWave/WholeThumb → key で隠す → toggle/direct action/drag で復元 | 保存した範囲の復元規則が全入力で一致 |
| F11 / F12 | 狭い music viewport、負原点の奇数ページ、gap、DPI、scroll | panel は予約矩形内、隣接辺の距離は指定の physical gap |
| F04 / F05 / F13 / F14 | 遅い DB/Shell/scan を注入し UI の進行を観測 | dialog / repaint / cancel の処理が worker の終了待ちで止まらない |
| F16 | 回転/選択解除を再割当・解除、native/fallback | メニューの表示が実際の keymap と一致 |

## 実機確認のマトリクス (今回未実施)

以下は確認済み一覧ではなく、修正後・出荷前の残作業。通常 profile を agent が起動しない規則を守り、agent の UI 確認を追加する場合は `scripts/prepare-portable-smoke.ps1` で用意した `target/portable-smoke/mimageviewer.exe` と、その `data` のみを使う。実ユーザー設定が必要なケースは利用者が実行する。

| 領域 | ケース | 確認する結果 |
|---|---|---|
| 外部ツール | Exe / 関連付け、未起動/起動済み、Each / Batch、空白・日本語・引用符を含む対象名 | 正しい引数/件数/対象。二重起動、意図しないファイルの起動がない |
| 外部ツール | 外部先が一部を拒否、起動先なし、起動中に取り消し | 成功と失敗を区別し、一時ファイルの寿命が結果に一致 |
| 仮想対象 | ZIP/CBZ/PDF (password 有無)、入れ子書庫、スタック、動画、音声 | 元パス/実体化/非対象の区別と一時ファイル内容が正しい |
| 外部変更 | 外部 editor で同名上書き、連続保存、大量フォルダ、共有フォルダ | 内容更新を検出し、入力・描画を止めない |
| メニュー | native/fallback、主窓/F12、異なる DPI、Shell submenu、Esc/選択後 | 一致した機能、焦点と z-order の復帰、残る tooltip/owner がない |
| 一括編集 | A/B が別フォルダ、同じページ、片側 navigate/close、部分失敗・cancel | F08 の context 分離、保持編集、回転、undo、cache を確認 |
| 書き出し | single/batch/book/merged、3段、AI の有無、LUT、隠蔽 preset、注釈、crop | 表示名だけでなく画像の寸法/画素/適用編集が選択段に一致 |
| 書き出し | 大量対象で dialog を開く、巨大見開きを合成 | 準備中も UI が動き、段階を追った進捗と cancel が利用できる |
| 端数描画 | 501×1001 と偶数サイズ、等倍/0.333/0.5/0.75/1.25/2倍、DPI 100/125/150/175/200% | crop 辺、1px 線、文字、格子が不意に欠けず、等倍の位置とサイズが安定 |
| 端数描画 | 0/90/180/270°、左右/上下反転、crop/trim、単ページ/見開き | 表示とクリック座標が一致。左右ページの接合と端がずれない |
| 端数描画 | 縦/横連結、LTR/RTL、gap0/1/20、負原点へスクロール、zoom焦点維持 | F12 の不意な1px間隔、重複/欠落、スクロール中のちらつきがない |
| texture | source/縮小版/AI/final composite の差替え、freeze、各 GPU filter | 位置と可視範囲が飛ばず、幅・高さの端数処理が経路で一致 |
| 情報パネル | 静止画/動画/音楽、幅640/800/860pt、lock/unlock、最大化/復元 | 帯の予約と描画が一致し、鍵ボタンと右端が見える |
| 情報パネル | hoverから固定→カーソルを左へ→panel内部へ、touch handle | cursor hide と hover、click/drag/wheel/seek が干渉しない |
| 動画strip | 5状態、cycle候補変更、toggle・direct action・上drag、restart | 範囲/内容/高さの復元が一貫し、非表示からWholeを失わない |
| 動画strip | wholeの端/短い動画/長尺、mouse/touch、wheel、windowへの切替 | seek地点、release commit、背後操作の抑止、波形末尾が正しい |
| ゲームパッド | stick非中立・D-pad押下・リング保持→OFF、切断/再接続 | OFFで操作/再描画が止まり、解除後に古い入力を再生しない |
| カスタマイズ | 新actionをキー/対応操作へ設定、再割当/解除、共有import/export | 実行先とメニュー表示が一致。旧bundleでpadが勝手にOFFにならない |
| crop | 比率固定で四象限へ新規drag、画像端、panel横断、タッチ/ペン | anchorと比率を保ち、releaseで確実に終了、panelのクリックが誤発火しない |
| 360 | V/ボタン/閉じて再開、非360素材を挟む、2窓で別投影、見開き/連結 | viewer間で意図が混ざらず、global defaultを勝手に更新しない |
| ★日時 | rating/smart folder/通常一覧/履歴を往復、仮想/欠損/同時刻 | 並び、列、sort復元、Remoteの順序が一致 |
| Remote AI | erase付き batch/export/外部実体化の実行中にスマホからacquire | F09 の停止確認と ownership。AIの競合/長時間無応答がない |
| Remote状態 | 本体で一括編集中の接続、戻す/切断/再接続、ページを跨ぐ後着結果 | DB mutation・表示cache・command世代が一致し、別ページへ反映しない |
| Remote画像 | 通常画像/ZIP/PDF、見開き/分割、回転/crop/補正、★一覧 | 表示/選択/ページ数/操作結果が本体設定と整合 |
| Remote動画 | 再生/seek/終了/再接続、縦横端末、ブラウザbackground復帰 | stream、音声、操作応答の既存動作を維持 |

## 未確定事項と検証の穴

- 情報パネルの旧 cursor/touch 述語は残るが、click 消費や主要 classifier の修正もある。click-through の実発生としては報告していない。
- Remote acquire と一括 DB mutation の競合は、F09 の AI barrier とは別の未確認事項。
- `seek_strip_wheel_is_consumed_and_becomes_one_range_step` の `#[test]` 欠落は v3.4.0 にもある。新規退行ではないが、そのテスト名を根拠に wheel の自動保証を主張できない。
- Windows Shell extension、実 IME、ゲームパッド/タッチ機器、D3D11、複数 DPI の実測は未実施。Linux CI、署名、配布物の launcher/extraction、インストーラーも今回の確認範囲外。
- レビュー対象より後の commit は含まれない。修正後は関連する境界の回帰を先に実行し、最後に全体 gate と必要な実機確認を行う。
