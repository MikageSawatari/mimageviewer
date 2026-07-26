# v2.8.1 前 全体点検 — 領域別 docs↔コード整合監査

v2.8.0 リリース後、`src/` が 45 万行を超えたことを受けて実施した全体点検の記録。
領域ごとに「設計ドキュメントの記述と実装が一致しているか」と「設計上見直したほうが
良い構造」を同じパスで洗い出した。

- 実施日: 2026-07-26
- 基準コミット: `2cb405c7`
- 実施方法: 領域ごとに 1 セッション (read-only)。オーケストレーション = ClaudeCode、
  調査 = Codex Sol。
- 対象領域の選定根拠: 先行して行った `docs/` 棚卸し監査の「優先検証リスト」上位。
  「文書の最終更新後に関連コードへ入ったコミット数」「被参照数」「分量」で順位付けした。

## 第 1 波の結果

| レポート | 領域 | 不一致 | リファクタ候補 | バグ |
| --- | --- | ---: | ---: | ---: |
| [s1-local-adjust.md](s1-local-adjust.md) | ローカル調整レイヤー / マスク / 隠蔽 / 補正合成 | 14 | 7 | 2 |
| [s2-detached.md](s2-detached.md) | detached viewer ライフサイクル | 19 | 7 | 3 |
| [s3-display.md](s3-display.md) | フルスクリーン表示 / display pipeline / パノラマ | 11 | 5 | 3 |
| [s4-video.md](s4-video.md) | 動画エンジン / native presenter / grade / 音声モード | 16 | 8 | 2 |
| 合計 | | **60** | **27** | **10** |

## 横断テーマ

個別の指摘より、複数領域に共通して現れた構造の方が重要。

### 1. 「実表示の幾何」を複数箇所が独立に再計算している

`fs_image_draw_rect_for_size` という正本ヘルパがあるのに、実表示矩形の生産系が **12 系統**
あり、ルーペ / 比較 / crop / capture / 編集 4 系統の**少なくとも 8 箇所が contain 式を
再実装**している。バックログ 1.23 (連結読みルーペ)、1.24 (Z ズームルーペ)、および新たに
見つかった編集座標ずれ (S3 Bug 3) は、すべてこの重複が原因。

### 2. 「最後に表示したフレーム」の所有者が存在しない

`present_retire` / `hidden_latest_frame` / `source.queue` の 3 つが「使えそうな最近の
フレーム」を部分的に表現しているだけで、表示内容の永続的な所有者がない。バックログ
1.25 (一時停止中の grade 未反映)、1.26 (placement prime 失敗) の共通原因。

**この監査でバックログ 1.25 の当初対応案が誤りと判明した。** `present_retire.back()` を
再 present する案は、(a) `present_retire` が廃棄待ちキューであること (b) 一度 present した
GPU frame は keyed mutex が writer key に戻り `AcquireSync(1)` が通らないこと、の 2 点で
成立しない。

### 3. context 固有リソースの所有境界が漏れている

`ViewerContextBundle` による分離は計画より実装が進んでいるが、次の 3 つが境界の外に残る。

- ローカル調整の bypass/prefix preview キャッシュ (S1 Bug 1) — キーが item index と
  generation だけで、ソース/context の識別子を持たない
- terminal close 時に cancel されない pending producer (S2 Bug 1)
- 動画 F12 OFF 時に削除されない manager runtime (S2 Bug 2)

いずれも detached リワークの BA-7 (terminal/transition state と producer ownership の分離)
に帰属する。

### 4. UI スレッドでの同期 I/O が残っている

local/conceal の materialization が SQLite 読み込み・JSON 復元・deflate・マスク raster・
blur 合成・テクスチャアップロードを描画経路から同期実行する (S1 Bug 2)。
`conceal-feature-plan.md` 自身が worker 化を要求しており、`ui-responsiveness.md` の
不変条件にも違反している。

### 5. 巨大ファイルへの責務集中

| ファイル | 行数 |
| --- | ---: |
| `src/ui_fullscreen.rs` | 26,235 |
| `crates/local-adjust-core/src/lib.rs` | ~24,000 |
| `src/video/native_presenter/mod.rs` | 9,054 |
| `src/video/decoder.rs` | 8,297 |
| `src/video/mod.rs` | 6,996 |
| `src/video/native_presenter/overlay_draw.rs` | 6,107 |

いずれも P3 として分割候補に挙がったが、**先に幾何・状態の所有者を確定してから分割
しないと、重複構造がファイルをまたいで残るだけ**という評価で一致している。

### 6. 文書が「計画 + 実装記録 + 将来案」の追記式になっている

現行仕様として読めない文書が領域ごとに存在する。共通の症状は、冒頭が着手前の計画のまま、
中盤に実装済み追補、末尾に将来案が同居し、同じ機能について相反する記述があること。

## バグ一覧 (10 件)

いずれもコード経路は静的に確定。実機再現は未実施 (S3 Bug 1/2 のみバックログに実機確認済み)。

| # | 内容 | 分類 | 既存バックログ |
| --- | --- | --- | --- |
| S3-3 | 非 Page fit / 表示トリム時に編集・crop overlay の座標が画像本体とずれる。誤った画素にマスクや crop を確定しうる | **新規** | — |
| S4-2 | 一時停止中の placement 切替で新ウィンドウの prime が失敗し、黒表示になり得る | **新規** | 1.26 として追加 |
| S1-2 | local/conceal の materialization が UI スレッドで同期実行される | 不変条件違反 | — |
| S2-1 | terminal close 後に in-flight の open/navigation が適用される | BA-7 | — |
| S2-2 | 動画 F12 OFF 後に `Closing` runtime が残留する | BA-7 | — |
| S2-3 | detached キー入力の subclass が window identity ではなく rect で選ばれる | BA-1 | — |
| S1-1 | ローカル preview キャッシュが viewer context をまたいで衝突しうる | BA-7 系 | — |
| S3-1 | 連結読み中のルーペがカーソル直下ページを解決しない | 既知 | 1.23 |
| S3-2 | Z ズーム中のルーペが通常 zoom/pan の矩形を使う | 既知 | 1.24 |
| S4-1 | 一時停止中に grade/LUT 変更が画面へ反映されない | 既知 | 1.25 |

## リファクタ候補 (27 件)

### P1 (5 件)

| 出典 | 内容 | 規模 | テスト担保 |
| --- | --- | --- | --- |
| S3 | 単一ページの実表示 transform を `DisplayedImageTransform` へ一元化 | Medium | 大半が純関数テーブルテスト可。最終確認は実機 |
| S1 | edit materialization を generation/cancel 付き worker へ | Medium | 状態遷移は単体テスト可。4K 素材で実機確認 |
| S1 | bypass/prefix preview lane を viewer context 所有へ | Medium | 二 context 衝突の回帰テスト可。detached 実機推奨 |
| S2 | terminal transition と全 pending producer の typed owner 化 | Medium | reducer は単体テスト可。multi-window 実機必須 |
| S4 | visible/hidden/retire を統合した `FramePresentationState` | Medium | 遷移は単体テスト可。keyed mutex は実機必須 |

### P2 (10 件)

| 出典 | 内容 | 規模 |
| --- | --- | --- |
| S3 | Single / 見開き / 連結読みを `FullscreenPageLayout::hit_test()` に集約 | Medium |
| S1 | キャッシュ無効化を typed cause/state graph に集約 | Medium |
| S1 | overlay/edit mode を相互排他的な typed state に | Medium |
| S1 | mask/conceal の永続化 codec を共通化 | Small |
| S1 | 360° source fallback を effective parameter version で管理 | Medium |
| S2 | キー入力 subclass の HWND 所有を registry に統合 | Small |
| S2 | park 時の thumbnail pipeline pause/resume protocol | Medium |
| S2 | 全 mounted/parked context を横断する VRAM budget | Large |
| S4 | BufferReady anchor payload と MasterClock 書き込み境界の型固定 | Small |
| S4 | VST GUI/owner/HUD teardown の単一 policy 化 | Medium |

### P3 (12 件)

責務分割 (`ui_fullscreen.rs` / `video/mod.rs` / `native_presenter/mod.rs` / `DspBridge` /
effect family)、typed reducer 化 (detached lifecycle / panorama session / 動画→音声 /
folder nav burst)、`ViewerContextBundle` 分割、`display_px` の context 所有化、
EngineActor と AvClock の二重 clock 解消。詳細は各レポート参照。

## 文書の信頼度

### 現行仕様としては信頼できない

| 文書 | 症状 |
| --- | --- |
| `panorama-360-view-plan.md` | 冒頭が「着手前・承認後に実装」のまま。XMP・V キー・WGSL・GPU upload まで実装済み。2,761 行に実装前レビュー / 実装済み追補 / 未更新チェックリスト / 将来案が同居 |
| `local-adjustment-layer-v1.1.0-plan.md` | `Status: Draft` のまま。合成順序・データモデル・DB スキーマがすべて一世代古い |
| `video-engine-redesign.md` | 「草案 v0」のまま。EngineState schema / Readiness 式 / anchor wall / 閾値 / pacing / actor topology のいずれも冒頭を現行仕様として読めない |
| `conceal-feature-plan.md` | settings.json 前提、ツール 8 種 (実際は 9 種)、DB スキーマ違い、worker 実装状況が古い |
| `vst3-integration.md` | 後半は比較的新しいが、冒頭・module 表・実装状況・負債表が旧 per-plugin bridge 時代のまま。私有 `file:///` リンクも残る |

### 部分的に訂正が必要

| 文書 | 訂正箇所 |
| --- | --- |
| `display-pipeline.md` | **crop の位置が恒久正本として重大な誤り** (S1・S3 が独立に同じ結論)。crop は `EditResultKey` に含まれず、表示 overlay + 出力時適用が正しい |
| `detached-rework-plan.md` | §9 と §10 の進捗表が互いに矛盾。実際は R2b 部分完了 / R3 実質完了 / R4 未完、BA-5 未解消・BA-7 と BA-1 が部分解消 |
| `preset-and-adjustment.md` | final-stage 一覧が節によって異なる。Book/headless に smart sharpen / post-filter が入るという記述が誤り |
| `async-architecture.md` | 動画 channel の型・容量、VST GUI worker、normalize scan worker の記述が現状と違う |
| `video-architecture.md` | 恒久正本としては最も信頼できるが、channel 表・モジュール規模・抽象化評価が v0.9.0 時点 |
| `local-adjust-testing.md` | 冒頭の進捗 matrix と `app.rs` の絶対行番号が追いついていない |
| `auto-thumb-aspect-plan.md` / `fullscreen-side-panel-mode-plan.md` | ターゲット設計は正しいが実装状況が未実装扱い |
| `dpi-multimonitor-issue.md` | viewport command の棚卸しとモジュール位置 (`main.rs` → `lib.rs`) が古い |
| `fullscreen-navigation-consistency.md` | マウス hook の所在が `main.rs` → `lib.rs` |
| `playback-speed-design.md` | UI 節が撤去済みの legacy egui path を要求。存在しないファイルを参照 |

## 第 2 波 (未実施)

優先検証リスト 6〜10 位をカバーする領域。

- 検索 / インデクサ / タグ / カラーフィルタ (`tag-catalog-redesign-plan.md`, `search-architecture.md`)
- 最上位一覧 ownership / ★固定 / 履歴 / rating / サブ展開 / スタック (`star-lock-snapshot-design.md`, `top-level-grid-view.md`)
- 設定永続化 / 復元 / バックアップ / data-dir (`settings-sqlite-migration.md`)
- キー入力 / KeySlot / KeyAction / native 動画転送 (`keymap-spec.md`)
- ZIP / RAR / 変換アーカイブ、PDF pool (`virtual-folders.md`)

全 9 領域をサブ分割した完全な監査には合計 18 セッション前後が必要という見積もり。
v2.8.1 リリース後に継続する。
