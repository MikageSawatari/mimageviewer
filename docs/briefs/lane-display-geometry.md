# レーン 2: 表示ジオメトリの丸めと、フルスクリーンの表示領域

v3.5.0 の並行レーンの 1 本。**このブリーフを最初に読み、そのうえで
[docs/README.md](../README.md) から該当領域の設計ドキュメント
(特に [display-pipeline.md](../display-pipeline.md)) を開くこと。**

## 作業ツリーとブランチ

- 作業ツリー: `C:\home\mimageviewer-pano` (このツリー)
- ブランチ: `display-geometry-rounding` (master `ea233160` から分岐)
- **master へ merge しない。** master では別セッションがリリース作業中。
  区切りごとにこのブランチへコミットし、完了したら報告するところまでが担当。
- 他の worktree (`-extlaunch` / `-r2e` / `-video-strip` / `-export`) と
  `C:\home\mimageviewer` のファイルは**読むのも書くのも行わない**。
- `git worktree remove` を使わない (junction 再帰削除の事故があるため)。

## 担当する項目 (この順で)

正本は [docs/next-release-backlog.md](../next-release-backlog.md) の各節。**着手前に必ず
その節を読む** (ここは要約であって正本ではない)。3 件は同型 —
「寄せ終わってから 1 つの所有者が置く」— なので、**回帰テストは 1 本目で全走査の形に作り、
以降で拡張する**。

### 1. §1.161 端数倍率の拡大で、貼り先と出力テクセル数が合わずボケる

- **常用域で踏む。** ppp 1.25 の環境では論理 0.9 倍表示が物理 1.125 倍の拡大になるので、
  ウィンドウよりわずかに小さい画像を出すだけでこの分岐に入る。
- `RectSnapMode::None` では矩形を寄せないが、visible-region Lanczos は**整数サイズの出力**を
  作る。貼り先が未量子化なので egui/wgpu がもう一度 Linear で伸ばす。
- **寄せる単位が縮小時と違う。** 全体矩形ではなく**可視領域の出力サイズと貼り先**を整数で
  揃える契約にする。`paint_source_region_texture` が `full_image_rect` 基準で貼っているので
  そこから見直し、`gpu_lanczos` の `target_and_source_region_for_branch` と対で決める。
- [displayed_image_transform.rs](../../src/displayed_image_transform.rs) の
  `snap_rect_to_physical_pixels` の doc に、なぜ端数拡大を触っていないかが書いてある。
  **その理由を読んでから契約を変える。**
- 回帰は「現在の走査が明示的に除外している `RectSnapMode::None` を含める」形で書ける。

### 2. §1.159 連結読みのユニット間の間隔が、丸めの混在で最大 1 物理 px ばらつく

- ユニットの可視幅・高さとスクロール offset は**未量子化の `content_bbox`** から、後段の
  transform は**量子化した辺**から出る。累積 offset は `quantize_points_to_physical_pixels` を
  通るので中心間距離は揃うが、**端と端 (= 見える間隔) が揃わない**。
- 既定 20px では 19〜21px で気付きにくいが、`continuous_reading_gap_px` を 0 / 1px に
  している利用者では §1.154 と同じ形の報告になる。**gap 0 の密着も保証されない。**
- v3.4.0 で入れた合わせは**ユニット内の左右だけ**。ユニット間は対象外。
- 縦方向 × N ユニットで、位置がスクロール範囲の計算にも効く。慎重に。

### 3. §1.154 見開きの継ぎ目合わせが detached の凍結スナップショットに入っていない

⚠️ **detached リワークの凍結ルール対象。着手前に
[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) を読むこと。**
症状パッチ (guard / delay / retry / 追加 repaint / 一括 reset / silent fallback) を入れない。
構造的修正として進めるなら、**ClaudeCode と Codex の双方から「これは症状パッチではなく
構造的修正である」ことへの合意を取り**、触れた範囲と判断理由を同 plan へ記録する。

- 対象は `detached_spread_frozen_pages_for_snapshot` と
  `detached_continuous_frozen_pages_for_snapshot` の 2 つ。
- **live 経路と同じ helper をそのまま当てるのは誤り** (2026-08-31 に Codex が不同意)。
  frozen 経路は transform を `content_bbox: None` で組み、可視範囲を
  `left_clip_rect` / `right_clip_rect` として別に持つ。この経路の `paint_rect` は
  **見える端ではなくフルページ矩形**なので `align_spread_pages_for_gap` が測る所が違う。
  移動するのは transform だけで clip は旧 `rects` 由来のまま残り、直そうとした二段構造が
  そのまま残る。
- 既存テスト `detached_spread_snapshot_preserves_trim_uv_and_background`
  ([app/tests.rs](../../src/app/tests.rs)) は、トリム時に**フル矩形が重なり clip は重ならない**
  ことを要求しており、gap 0 でフル矩形同士を合わせる当て方と正面から矛盾する。
  **テストの意図を読んでから動かす。**
- 正しい直し方は **clip / UV の導出を配置と同じ所有者へ寄せること**で、この関数の契約変更に
  なる。現状を固定するテストは足さない (直っていない挙動を仕様にしてしまう)。
- 3 件が片付いたら、**v3.4.0 の更新履歴と
  [known-issues.html](../../htdocs/mimageviewer/manual/known-issues.html) の
  「一部の条件で継ぎ目の太さがわずかにずれる」を消す。**

### 4. §1.158 右情報パネルをロックし、前後移動中も表示を維持する

- 静止画・本の右情報パネルのヘッダへ鍵ボタン。**ロック ON では画像へ重ねず、右側に
  パネル幅の領域を確保して content rect を残りへ再解決する** (上下 HUD のロックと同じ考え方)。
  旧 Pinned の「重ねたまま開状態だけ固定」は戻さない。
- **ロック状態は viewer context ごとの一時状態。** App-global bool にしない
  (別ウィンドウの操作で他方のパネルが開閉しないこと)。永続設定にもしない。
- **右パネル幅・上下 HUD のロック領域・画像 content rect を 1 つの layout snapshot で決める。**
  描画・ズーム・パン・hit-test・ルーペ・ナビゲータが同じ矩形を見ること。見た目だけ画像を
  左へ押して入力座標を旧位置に残さない。
- detached 経路へ触れる場合は §1.154 と同じ凍結ルールに従う。

### 5. §1.145 360 ビューの ON 意図と投影方式が、一覧へ戻ると失われる (+ R-19)

- リセットは 1 か所: `close_fullscreen` の `panorama_state = None` (`app.rs` 51887 行付近)。
  一覧へ戻る / フォルダ移動はどのモードでも close を通るので、モードによらず起きる。
- 単純な ON/OFF ではなく **「ON の意図を覚えて、次が対象素材なら復帰」**。XMP による自動 ON の
  判定 (`app.rs` 61875 行付近) へセッションの意図を足す。
- 投影方式は **`Settings::panorama_projection` を書き換えない**。現在の実装は
  「今見ているこの 1 枚を別の写り方で見る操作であって既定の変更ではない」(`app.rs` 63387 行付近)
  を明示的に選んでいる。**セッション内で引き継ぐ値を別に持つ。**
- ⚠️ **この状態をどこに持つかは §1.158 と同じ問題** (viewer context ごとの一時状態)。
  2 回別々に決めない。App グローバルに置くと窓ごとに 360 の状態が混ざる。
- 併せて **R-19: 360 動画の roll が描画へ渡らない** (v3.3.0 レビュー持ち越し) を見る。
  shader 配線 + 実機検証が要る。該当素材でのみ再現。

## 共有登録簿 — A が着地するまで触らない

レーン A (`external-tool-launch` worktree、右クリックメニューと外部ツール起動) が、
`src/ui_dialogs/context_menu.rs` (+新設 `context_menu_model.rs`) /
`src/ui_dialogs/preferences.rs` + `preferences/pages.rs` /
`src/settings.rs` + `src/settings_db.rs` / `src/keymap.rs` +
`docs/keymap.ini.default` を全面的に書き換えている。**先に触ると解決不能なコンフリクトになる。**

このレーンは本来どれも触らないはず。必要になったら、**そのレーンの最後に専用コミット 1 本**へ
まとめること。`src/app.rs` は A も触るが、A のハンクは 10701–15786 / 26212 / 60220–60549 /
66930–67594 で、このレーンの対象 (51887 / 61875 / 63387 と凍結スナップショット) とは重ならない。

## 進め方

- 修正前に、観測された失敗・守るべき不変条件・違反を作った経路をログ / テスト /
  source inspection で特定する。症状を消す guard を根本原因の代わりに置かない。
- **1 物理 px の話なので、目視の前に数値で固定する。** 走査テストで倍率・DPI・トリム・
  見開き / 連結の組み合わせを回し、期待値を式で書く。
- 実装を Codex へ出すなら**出す前にコミットする**。**1 worktree につき Codex は 1 本まで。**
- コミット前に `cargo fmt` (引数なし・ワークスペース全体)。
- テストは `cargo test -p mimageviewer --lib <filter>` / `--test <name>` で最小に。

## 実機確認の頼み方

`.\scripts\build-dev.ps1` を回し、
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` を利用者へ渡す。
**エージェント自身は起動しない。** DPI 100 / 125 / 150%、見開き / 連結 / 単ページ、
gap 0 / 1 / 20px のように**確認すべき組み合わせを具体的に**書く。

**実機確認は利用者 1 人しかいない直列資源で、いま 4 レーンが並行している。**
細かく何度も頼まず、区切りでまとめて 1 回にする。

## 他のレーン (参考)

| レーン | ツリー | 中身 |
| --- | --- | --- |
| A | `-extlaunch` | 外部ツール起動 §1.117 (進行中) |
| 1 | `-r2e` | §1.142 → §1.143 → §1.150/§1.151 |
| 2 | **`-pano` (ここ)** | §1.161/§1.159/§1.154 → §1.158 / §1.145 / R-19 |
| 3 | `-video-strip` | 動画シークストリップ §1.155 |
| 4 | `-export` | エクスポート §1.144 → §1.148 |
