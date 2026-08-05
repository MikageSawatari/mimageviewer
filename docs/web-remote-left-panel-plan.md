# mIV Remote 左パネル 設計メモ

状態: **段 3a（カラー化の remote 操作）まで実装済み**。親の正本は
[web-remote-plan.md](web-remote-plan.md)。
関連: [display-pipeline.md](display-pipeline.md) / [preset-and-adjustment.md](preset-and-adjustment.md) /
[adjustment-scope-selector-plan.md](adjustment-scope-selector-plan.md) /
[fullscreen-side-panel-mode-plan.md](fullscreen-side-panel-mode-plan.md) /
[web-remote-video-streaming-plan.md](web-remote-video-streaming-plan.md) /
[web-remote-ai-plan.md](web-remote-ai-plan.md)

---

## 1. 目的

**デスクトップとスマホで操作感を揃える。** 見た目は端末に合わせて変えるが、
**処理の実体は mIV 本体側**に置き、リモートはその配線に徹する。

特に重要度が高いのは **カラー化**と **AI アップスケール**。

## 2. 対象範囲

### 2.1 静止画

フルスクリーン左パネルの 3 タブ (`FullscreenLeftPanelTab`) を対象にする。

| タブ | 対象 |
| --- | --- |
| 画像補正 (`Adjustment`) | **編集系 6 ボタンを除く全部** |
| 表示トリム (`ViewTrim`) | 全部 |
| ブックマーク (`Bookmarks`) | 全部 |

画像補正タブの内訳 (`AdjustmentSettingsTab`): **色調 / AI / カラー化 / フィルタ** の 4 サブタブ。
これは全部入れる。

**除外する編集系 6 ボタン** ([ui_adjustment_panel.rs:13561](../src/ui_adjustment_panel.rs) のヘッダ 2 行目):

消しゴム / 補正レイヤー / 隠蔽加工 / 切り取り / テキスト注釈 / エクスポート

理由: これらは**元データを書き換える編集**であり、参照系とは設計の重みが違う。
リモートの第一段では持たない。

### 2.2 動画

| タブ | 対象 |
| --- | --- |
| ジャンプ | 全部 (ピン / ブックマーク / チャプター) |
| 画像補正 | 全部 |

---

## 3. ⚠ 中心的な問題 — リモートの画像は補正パイプラインを通っていない

**これが本設計の主題**であり、UI を作る前に決める必要がある。

### 3.1 静止画: いま何が適用されているか

リモートのページ画像は [remote_ipc/container.rs:1231](../src/remote_ipc/container.rs) の
`process_load_request()` が作る。ここに渡されるのは:

- `context.adjustment_db` — ただし [thumb_loader.rs:969](../src/thumb_loader.rs) を見ると
  **`pinned_page_adjustment` にしか使われていない**。これは「編集済みページの
  edit-preview に対して、ページ個別の色調を載せ直す」ための限定的な経路

つまりリモートが今返しているのは:

| 段 | リモートに乗っているか |
| --- | --- |
| **編集結果 (消しゴム / 隠蔽加工 / 切り取り / テキスト等)** | ❌ **乗らない** (§3.1.1) |
| **色調** | ✅ 段 1a で適用 |
| **smart sharpen** | ✅ 段 1a で適用 (`used_upscale = false` 相当) |
| **AI アップスケール / デノイズ** | ❌ **乗らない** |
| **カラー化** | ✅ 段 1a で適用 |
| **Creative LUT** | ✅ 段 1a で適用 |
| **ポストフィルタ** | ✅ 段 1a で適用 |

ここでの ✅ は「PC で永続化済みの補正を remote の Page 応答へ反映する」意味であり、
remote から即時補正値を変更する UI は段 2a、カラー化の書き込み UI は段 3a で実装済み。
AI の書き込みと実行は段 3b の対象である。

### 3.1.1 ⚠ 編集結果が乗らないのは既存の不具合 (2026-08-03 判明)

当初この表は「編集結果は edit-preview 経由で乗る」と書いていたが、**誤りだった**
(Codex の指摘で判明、コードで確認済み)。実際には 3 重に外れている:

1. [container.rs:1248](../src/remote_ipc/container.rs) の `LoadRequest` は
   `..Default::default()` で **`edit_preview_key` を設定しない**
2. `process_load_request` の呼び出しで **`edit_preview_db` に `None`** を渡している
   ([container.rs:1357](../src/remote_ipc/container.rs))
3. full-page は `skip_cache: true` で、[thumb_loader.rs:971](../src/thumb_loader.rs) は
   `if req.skip_cache { None }` と **edit-preview を明示的に短絡**する

> **隠蔽加工や消しゴムで隠したページを、リモートは隠す前の状態で表示する。**

補正が乗らないのは「機能がまだ無い」だが、**編集結果が乗らないのは既存の不具合**であり、
隠すために使った操作が外向きの経路で無効になっている点で性質が違う。

**段 0 の対象に含める。** 補正の共有化より先に、既存の編集結果をリモートへどう供給するかを
確定する。

フルスクリーンの最終合成は `App::ensure_final_composite_pixels(&mut self, ctx, idx)`
([app.rs:54461](../src/app.rs)) が持っているが、これは **`&mut App` + `egui::Context` を要求し、
フルスクリーンの `idx` に紐づく** = UI スレッド専用で、リモートの IPC ワーカーからは呼べない。

この制約は段 1 で、idx に依存しない remote adapter が共有 final-composite executor を
呼ぶ構造へ直した。段 3a は同じ経路へカラー化の未確定値 / 永続値を渡し、別の描画経路を
追加せずに操作を接続している。

### 3.2 動画: さらに深い

動画の補正 (`VideoGradeSnapshot`) は **D3D11 の描画パイプラインで適用**される
([render_core.rs:2988](../src/video/native_presenter/render_core.rs) の
`pipeline.update_grade(...)`) = **表示時**。

一方リモート配信の video tap は [decoder.rs:4109](../src/video/decoder.rs) = **デコード直後**で、
表示より**前**。

> **今のままだと、動画の補正はリモート配信に一切乗らない。**

しかも remote-headless では presenter が hidden なので、「描画結果を横取りする」形も取れない。
**エンコーダへ渡すフレームに対して、独立した grade 適用段が要る。**

## 4. 方針

### 4.1 headless composite をリモートセッションが所有する

動画配信で学んだ形をそのまま使う。

> PC 側のフルスクリーンを乗っ取らない。**リモートセッションが自分の headless な
> 合成を持つ。**

動画配信の増分 8〜15 で、PC 側の fullscreen を再利用しようとして
「PC が白画面になる / ローカル利用者が閉じられなくなる」を引き起こし、
最終的に **headless player をリモートが所有する**形に落ち着いた経緯と同じ。
静止画の合成でも同じ判断をする。

### 4.2 ⚠ ただし「2 つ目の実装」を作らない

利用者から一覧の件で受けた指摘をそのまま適用する:

> 2 箇所にわかれていると他にも設定の動作の違いなどがおこることを懸念しています

幸い、**変換そのものは既に UI 非依存の純関数**になっている:

- `crate::adjustment::apply_adjustments_fast(&base, params) -> ColorImage`
- `crate::colorize::apply(&src, params) -> ColorImage`
- AI は `src/ai/` のワーカー経由

App が持っているのは**キャッシュと段取り**であって、変換ではない。

したがって守るべき不変条件はこうなる:

> **段の順序とパラメータ解決 (2 段スコープの合成) は 1 つの共有関数にする。**
> App の最終合成もリモートの headless 合成も、その同じ関数を呼ぶ。
> 各段を呼び直す形で「同じような順序」を再実装しない。

これを最初に切り出すのが、この作業の実質的な第 1 段になる。

### 4.3 動画は別の段として扱う

静止画と違い、動画は**エンコーダ入力に対する新しい grade 段**が要る。
`vendor/ffmpeg` に avfilter を同梱しているので `lut3d` 等を使う道もあるし、
tapped frame に対して Rust 側で適用する道もある。**どちらにせよ静止画とは別作業**で、
静止画より重い。

## 5. スコープ (適用範囲) の扱い

v2.9.0 で補正の解決は **2 段**になっている
([adjustment-scope-selector-plan.md](adjustment-scope-selector-plan.md)):

```
この画像の個別設定  >  その場所の標準設定
```

リモートの UI は**この 2 段をそのまま見せる**。「どこに書くか」を隠して
「とりあえずこのページに書く」にすると、デスクトップと挙動が食い違う
(= 本設計の目的そのものに反する)。

**リモートからの変更は本物の永続データを書き換える。** `adjustment.db` の
`page_params` / `favorite_params`、`settings.global_preset` に対する書き込みであり、
PC 側の表示も変わる。これは「操作感を揃える」の必然的な帰結だが、
**利用者に確認すべき論点**として §8 に挙げる。

## 6. 応答時間の 2 階層

左パネルの操作は、コストが 2 桁違う。

| 階層 | 例 | 目標 | 扱い |
| --- | --- | --- | --- |
| 即時 | 色調スライダー、カラー化、トリム、フィルタ | 数十 ms | ドラッグ中はプレビュー、確定時に本適用 |
| 秒 | **AI アップスケール / デノイズ** | 数秒 | 進捗と取り消しが要る |

カラー化はトーン密度を含むため当初「秒」側と見積もっていたが、段 3a の 768px 実測では
最も重い Gaussian でも共有 executor 内 5.45ms（最適化 profile、576x768、8 回中央値）だった。
したがって段 2a と同じ in-flight 1 本 + 最新値 coalesce で扱う。

秒側は動画配信で作った規律をそのまま使う:

- **1 つの予算を最初に取り、全段で持ち回る** (`VIDEO_STREAM_START_BUDGET` と同じ形)
- **世代 (generation)** を持ち、より新しい要求が古いものを置き換える
- **待機中と失敗を区別する** (準備中を失敗として畳まない)

**スマホは画面が消える。** バックグラウンド復帰で「数秒かかる処理の途中だった」
状態から復帰できることが要件になる。

## 7. 段階

| 段 | 内容 | 価値 |
| --- | --- | --- |
| **0 ✅** | 段の順序とパラメータ解決を共有関数へ切り出す (§4.2) | これ自体は無変化。以降すべての土台 |
| **1 ✅** | **1a 補正を共有関数に通す** / **1b 編集結果を materialize** | スマホで見る絵を PC と同じ段・値へ揃える |
| **2a ✅** | 静止画の向き対応パネル UI と、画像補正の即時階層（スコープ、色調 8 項目、自動補正、リセット） | 絵を見ながら本体と同じ値・保存先を操作できる |
| **2b (2b-1 ✅)** | 表示トリム、ブックマーク、画像補正の残る軽量操作。2b-1 でブックマーク完了 | 静止画パネルの軽量操作を完成させる |
| **3a ✅** | カラー化をリモートから操作できるようにする | 描画済みだったカラー化をスマホから設定できる |
| **3b ✅** | AI アップスケール / デノイズをリモートから起動できるようにする | 利用者が最重要と挙げた残る重処理 |
| **4a ✅** | 動画の向き対応パネル shell とジャンプタブ (読み取り) | 既存データを worker で読むだけなので軽い |
| **4b** | ジャンプの追加・削除・改名と、未保存サムネイルの生成 | スマホから場面を記録できる |
| **5** | 動画の補正 (エンコーダ入力への grade 段) | 重い。単独で判断する |

段 1 の合成経路を土台として、段 2 以降の UI / 操作を接続する。

段 3b は remote 接続中の PC modal 排他、接続取得 / 切断 drain barrier、
stable remote key、HTTP job lifecycle、画面消灯復帰、重複 AiRuntime、VRAM / 撤退条件までを
[web-remote-ai-plan.md](web-remote-ai-plan.md) に分離した。実装結果は §7.3。

### 7.1 段 2a 実装結果 (2026-08-03)

`RemoteWriteRequest`（protocol v22）へ補正の現在値取得と確定書き込みを追加した。Web は
`adjustment.db` を直接開かず、本体 UI thread 上で既存の page / favorite / global writer と
キャッシュ無効化を通る。即時値だけを wire 型に出し、カラー化、AI、post-filter、LUT、
smart sharpen は完全な `AdjustParams` の base に残したまま上書きする。

ページ個別 writer は `PageAdjustmentTarget`（page key、標準解決用 location、sidecar 座標、
製本判定）を正本にした。従来の `set_page_params(idx, ...)` はこの target を作る薄い入口で、
idx は mounted map / texture / generation の反映対象を探す用途だけに残る。remote のページが
現在の `items` に無くても DB と sidecar へ保存でき、mount 済みなら同じ key の idx に
`clear_caches_for_param_change` まで反映する。key target 自体を持つ Undo scope も追加し、
remote のドラッグ確定 1 回を Ctrl+Z の 1 件として戻せる。

標準スコープへの切替はデスクトップと同じく対象ページの個別設定を解除する。以降の変更は、
現在地に ON のお気に入り標準があればそこへ、なければ global へ書く。見開きは remote の
`PageGroup.pages` が物理的な画面左→右の順である契約を使い、2 ページ時だけ「左 / 右」を出す。
読書方向の anchor を補正対象には流用しない。

既存 Page API は `target_px` をすでに持ち 1px 以上の任意の低解像度を要求できた。
`PagePriority` は Foreground / Prefetch の scheduling だけで画質指定ではなかったため、新しい
画質 enum は作らず、未確定の即時値を `PageRequest.adjustment_preview` として最小追加した。
Web はドラッグ中 768px を要求し、同じ `FinalCompositePlan` / executor を通す。実行中は常に
1 本、待機値は最新 1 件へ coalesce し、`pointerup` / `pointercancel` / `touchend` /
`touchcancel` で 1 回だけ確定する。確定後は URL revision を進めて通常解像度を再取得する。

iOS の native range はトラック押下をドラッグとして扱わないため、range 自体を位置・ARIA・
キーボード操作の正本に残したまま pointer capture で相対ドラッグを所有する。押下時には値を
変えず、以後の横移動 1 トラック幅を正規化位置 0〜1 全体へ写像して step に丸める。ガンマと
中間点は egui 0.33 と同じ正の対数写像（log-space の線形補間）で位置と実値を往復し、range の
つまみも既定値 1.0 で中央に置く。移動が無ければプレビューも確定も発生させない。各スライダーの
`↩` は本体と同じ既定値・自動補正中の非表示・
ガンマ / 中間点の 0.001 許容を使い、非表示時も固定 slot を残して行レイアウトを動かさない。
個別リセットは共通リセットと同じ確定 writer へ即時に流す。

段 1a の説明にあった「remote は常に page → location → global」とする記述には例外の記載漏れが
あった。本体の製本フォルダ直下ページは page override が無ければ favorite / global ではなく
**無補正固定**である。remote resolver もこの規則へ合わせ、製本ページでは標準スコープを無効化した。

### 7.2 段 3a 実装結果 (2026-08-04)

protocol v23 で `RemoteAdjustmentValues` に本体 `ColorizeParams` と同形の型付き値を追加した。
mode / mono tolerance / palette / control points / luminance weight / density normalization /
tone method / radius / strength をすべて round-trip する。Web は mode、許容値、濃さ補正、
palette、輝度保持、トーン密度を編集できる。Custom の control points は失わず保持するが、
配色編集は PC に残す。

デスクトップのカラー化保存スロットは global Settings 所有で、対象スコープの adjustment writer
とは別責務である。段 3a は既存スロットを状態応答に含め、読み込み時だけ保存済み
`ColorizeParams` 一式を既存 `SetAdjustment` へ渡す。スロット保存のために別 writer や
`SetAdjustment` の二重責務は追加しない。

プレビューは段 2a と同じ 768px、in-flight 1 本、最新値 coalesce を使う。最適化 profile の
576x768 合成実測（初回除外、8 回中央値）は、通常 1.26ms、濃さ補正 100% 2.15ms、Fast
2.02ms、LocalMean 3.86ms、Gaussian 5.45ms。MonochromeOnly の PCA 判定込みでも 1.54ms
だったため、専用 queue / debounce は設けない。カラー化はサムネイル段に含まれないため、
page writer の thumbnail/pinned thumbnail 無効化も発生させない。

**段 4 (動画ジャンプ) は段 0〜3 と独立**なので、段 3b より先に実装した。
既存の `video_bookmarks` / `video_chapter_thumbs` を読んで返すだけで、
補正パイプラインに触れない。

### 7.3 段 3b 実装結果 (2026-08-04)

AI の入力は `src/canonical_image_loader.rs` に切り出した共有 decoder が作る。デスクトップの
フルスクリーンと remote の AI 入力が同じ経路で読むため、対象になる画像の判定がずれない。
通常ファイルの GIF / APNG は animated、ZIP 内の同じ拡張子は静止 1 枚目、WebP は両方で
animated という**既存の非対称**は仕様として保存した。ここを揃えると、デスクトップが処理する
画像をスマホだけ拒否する退行になる。

ジョブは要求した画面より長生きする。`RemoteAiJobLease` を `SessionOperation` の drain 側に、
`LocalAiActivityLease` を接続取得の barrier 側に置き、接続前から残る local AI と切断後の
remote AI を直列化する。native cache key は `target_px` を含めず、結果の identity には含める。
これにより同じ推論結果を複数の要求解像度で使い回せる。

見開きは 1 ページずつ判定する。当初は 1 ページが対象外だと同じ見開きのもう 1 ページの結果も
捨てていたため、UI ではなく protocol の水準で直した。

AI の 5 経路は fail-closed の認証 guard の下に置く。開始は GPU 推論を起こし、state / result は
画像そのものを返すためで、`every_video_stream_and_ai_route_is_below_the_fail_closed_auth_guard`
が固定している。

### 7.4 段 4a 実装結果 (2026-08-05)

動画メニューを静止画と同じ透明 input shield の向き対応パネル shell へ移し、縦持ちは下 50%、
横持ちは左 40%・全高にした。動画要素は作り直さず、残り領域へ `object-fit: contain` で収めるため
パネル表示中も再生を継続する。既存メニューは「機能」タブへ残し、「ジャンプ」タブを追加した。
動画面の上スワイプはこの段で新設し、パネルが先頭にある時の下スワイプで閉じる。左右スワイプ、
中央タップ、☰、`?` の既存操作は変更しない。

protocol v26 に session 指定のジャンプ一覧と token 指定の WebP 取得を追加した。stream 開始時に
ジャンプ用 DB は読まず、動画 path と player 所有の chapter snapshot だけを session に保持する。
最初の一覧要求を `IpcClass::Heavy` / core heavy worker へ流し、session-owned `OnceLock` が pin、
bookmark、chapter thumbnail を一度だけ読み、WebP の content hash を含む opaque token と一覧を作る。
そのため catalog 構築用の NotReady 応答や Web の自動再試行は持たない。pin は
`thumb_pts_secs` が現在位置と一致する時だけサムネイルを公開する。

HTTP は `GET /api/video/jumps?session=...` を `no-store` の JSON として返し、
`GET /api/video/jump-thumbnail?session=...&token=...` は該当 WebP を
`private, max-age=60` で返す。Web は全種を通した同秒判定を含む本体共通の時刻文字列をそのまま表示し、
`IntersectionObserver` で可視行のサムネイルだけを最大 2 本並列で取得する。行選択は既存の absolute seek command を
使い、パネルを閉じずに移動する。題名は本体の描画と同じく最大 5 行まで折り返す。

`OnceLock` は session の寿命の間 catalog を作り直さない。seek は generation を進めるだけで
session を維持するため、1 本の動画を見ている間は一貫した一覧と token を返す。**段 4b で
スマホから追加・削除できるようにするときは、この `OnceLock` を無効化できる owner へ置き換える
こと。** 置き換えないと、自分で足したブックマークが一覧に出ない。PC 側の変更が session 中に
反映されない点も同じ理由で、4a では読み取り専用なので許容している。

### 7.5 段 2b-1 ブックマーク実装結果 (2026-08-05)

protocol v27 で、現在の本の一覧、題名変更、ID 指定削除を `RemoteWriteRequest` の同じ bounded
write FIFO へ追加した。追加は既存 `SetBookmark { bookmarked: true }` を使う。一覧は UI thread
ではなく write worker 上で読み取り専用 DB と物理コンテナを解決し、DB の
`page_index_hint ASC, created_at_ms ASC` 順を変えずに返す。HTTP は新設せず既存 `POST /api/write`、
サムネイルも既存 `GET /api/thumb` を使うため、既存の fail-closed 認証 guard がそのまま覆う。

wire 行は表示用 `page_index_hint` と、解決できた時だけ存在する `target` を分ける。`target` は
page address、collapse 後の context address、実 item index を持ち、別の `resolved` bool は持たない。
Web は実 index を表示し、未解決行だけ hint を表示して「ページが見つかりません」で止める。
タップ後にも取得したコンテナに page address が残っていることを再確認し、消えていれば現在の
viewer state を切り替えない。

ZIP は現在 prefix だけでなく、列挙済み `ZipTree` のコンテナ全体から保存 identity の親階層を求め、
冗長 wrapper collapse と本体の表示順を通して target を作る。同じ純粋解決 helper を PC 本体の
事前解決とジャンプにも使うため、別 prefix の正常行をスマホだけ解決済みにする差は残さない。
PC で別 prefix の行は、移動前には現在階層の thumbnail slot が無いので `…` だが、欠落を示す
橙文言は出さない。

remote の題名変更・削除は、現在ページの検証済み `context_address` から container path を求め、
単一 SQL の `WHERE id = ? AND container_key = ?` で原子的に制限する。他の本の ID を指定した要求は
該当なしとして失敗し、その行を変更しない。remote mutation 完了時は該当本の
`current_book_bookmarks` を即時無効化して本体側も service の正本を再取得する。

## 8. 決定事項 (2026-08-02、利用者)

1. **補正変更は PC 側と共有。両方同時に変わる。**
   他の機能と同じ扱いにする。リモートからの変更は本物の永続データを書き換え、
   PC 側の表示にも即座に反映される。「外出先用の一時的な見え方」は作らない。
2. **動画の補正 (§7 の段 5) を第一段に含める。** ただし §10 の撤退条件つき。
3. **スマホの UI はメニューのタブ化。** → §9
4. **メニューの入口は上スワイプ。** 中央タップは使わない (§9.1)。
5. **静止画パネルは縦持ちで下 50%、横持ちで左 40%・全高にする**
   (画像を残す、2026-08-03 の追加合意。§9.3)。
6. **動画フィルタは初回のリモート対応リリースに必須ではない。**
   静止画のフィルタより需要が低いという判断。§10 参照。

## 9. スマホでの見せ方

### 9.1 入口を上スワイプに揃える

静止画メニューは既に **上スワイプ / ☰ ボタン / `?` キー** で開ける。動画は段 4 より前には
上スワイプを処理しておらず、段 4a で同じ入口を追加した。☰ ボタンと `?` キーは既存のまま使う。

向きにかかわらずジェスチャーは上スワイプのままにする。横持ちでも左端スワイプにはせず、
iOS Safari の戻るジェスチャーと競合させない。上スワイプは画面端を起点に要求せず、静止画の
content 内から開始する。実装では上下端 48 CSS px と既存の左端 32 CSS px を除外するため、
ホームインジケータぎりぎりから始める必要はない。

**中央タップは使わない。** 両ビューアで既に埋まっており、どちらも奪うと損をする:

| ビューア | 中央タップの現在の割り当て |
| --- | --- |
| 静止画 | `TOGGLE_VIEWER_BARS` (バー表示切替) |
| 動画 | `MEDIA_TOGGLE_PLAY` (再生 / 一時停止) |

### 9.2 メニューをタブ化する

現在の drill-down (`main` → `controls`、`menu_back` で戻る) を**タブ**に変える。
タブ名は**デスクトップの左パネルと同じ**にする (これが「揃える」の実体)。

```
機能 | 画像補正 | 表示トリム | ブックマーク      ← 静止画
機能 | ジャンプ | 画像補正                        ← 動画
```

現在のメニュー内容は **機能**タブへ入れる。
静止画で選択したタブはページ切替で破棄しない session state に置き、再読み込みまでは維持する。

### 9.3 ⚠ 全画面モーダルのままにしない

現在のメニューは `command-menu-layer` = **スクリム付きの全画面モーダル**
(`role="dialog"` / `aria-modal="true"`)。

**補正と表示トリムは「動かしながら絵を見る」面**であり、画像がスクリムの下に
隠れていると judgement ができない。**カラー化と AI アップスケールは、
まさに結果を見て判断するもの**なので致命的になる。
デスクトップの左パネルが画面の左端だけを占めて画像を残しているのは同じ理由。

2026-08-03 の実機検証前合意により、静止画の器はタブではなく**端末の向き**で形を決める。

| 向き | パネル | 残す画像領域 |
| --- | --- | --- |
| 縦持ち | 下から高さ 50% | 上 50% |
| 横持ち | 左から幅 40%・全高 | 右 60% |

横持ちを下半分にすると 844×390 程度の端末で双方が約 195px 高になり、縦に並ぶ操作列と画像が
同時に潰れる。左側は PC 本体の左パネルと操作感を揃えるためである。この規則は段 2 の静止画だけに
適用した。段 4a で動画ビューアにも同じ shell 寸法、透明 shield、上下スワイプを適用した。

パネル外は暗いスクリムで隠さず画像をそのまま見せるが、透明な input shield として扱い、タップや
スワイプを背面へ通さない。外側タップ、閉じるボタン、パネルが先頭までスクロールされた状態での
下スワイプで閉じる。機能一覧をスクロールしている最中の下方向 drag は panel scroll を優先する。

open / close / resize は単一の panel transition から panel rect と image rect を導出する。開閉時と、
開いたまま縦横が変わった時は、画像を残り領域へページ全体フィットし直す。ピンチ拡大または
ダブルタップ原寸の状態から開いた場合も倍率・pan と fit mode を page fit へ戻す。これにより補正中に
画像の一部だけが隠れた状態を残さない。拡大画像の 1 本指 drag は従来どおり pan を優先するため、
拡大中に開く場合は ☰ / `?` を使う。

2026-08-03 の実機確認で寸法・配置・再フィットは確定した。開く時は画像を残り領域へ**即時に**
再フィットし、その後にパネルだけを 180 ms で下から上へせり上げる。横持ちでも左から滑らせず、
左 40% の列へ下から入れる。閉じる時はパネルを上から下へ退場させてから画像領域を即時に戻す。
`prefers-reduced-motion` ではアニメーションを実質無効化する。

開いたまま回転した場合は同じ panel / viewer shell を維持して配置だけを変える。
`portraitSinglePage` により有効な見開き構成も変わる回転では、コンテナを再取得しても viewer を
作り直さず、現在ページを新しい group へ対応付けて同じ viewer 内で再表示する。したがって
パネルの open 状態と選択中タブを失わない。

拡大中は上スワイプを pan として扱い、パネルを開かない仕様で確定した。☰ / `?` から開いた場合は
従来どおり拡大を解除して page fit に戻す。

### 9.4 動画再生中

シートが半分の高さのときは**動画を止めない**。動画は上半分に残り、再生は続く。
補正の効果を再生しながら確認できることが要件
(静止画で「絵を見ながら」が要るのと同じ理由)。

---

## 10. 動画 grade をどう共有するか (A / B) と撤退条件

### 10.1 問題

動画の補正は `VideoGradeSnapshot` を **D3D11 の描画パイプライン**へ渡して適用する
([render_core.rs:2988](../src/video/native_presenter/render_core.rs) の
`pipeline.update_grade(...)`) = **表示時 / GPU**。

リモート配信の video tap は [decoder.rs:4109](../src/video/decoder.rs) = **デコード直後**で
表示より前。remote-headless では presenter が hidden なので描画結果を横取りする形も取れない。

→ **エンコーダへ渡すフレームに対する独立した grade 段が要る。**

### 10.2 2 つの道

| | 内容 | 懸念 |
| --- | --- | --- |
| **A** | 同梱済み FFmpeg の avfilter (`lut3d` 等) をエンコード前に噛ませる | **本体の D3D11 実装と別実装**になり、見え方がずれ得る |
| **B** | grade を UI / GPU 非依存の関数として切り出し、**D3D11 側とエンコーダ側の両方が使う** | 実装量が多い (シェーダと CPU 実装の一致) |

### 10.3 方針: B

利用者から一覧の件で受けた指摘をそのまま当てはめる:

> 2 箇所にわかれていると他にも設定の動作の違いなどがおこることを懸念しています

カラーグレーディングは「PC で見た色とスマホで見た色が違う」が**最も起きてはいけない場所**。
また将来フィルタ機能を拡充する可能性を考えると、共有した実装の上に足す方が安い。

A で始めて後から B へ寄せるのではなく、**最初から B で行く**。

### 10.4 撤退条件 (2026-08-02、利用者)

**B の実装量が非常に多いと判明した場合**:

1. **静止画の補正を先に完成させる** (§7 の段 0〜3)
2. **動画はジャンプ (ブックマーク) タブまで** (§7 の段 4) で止める
3. その段階でスケジュールを見て、動画フィルタ (段 5) を入れるか判断する

**動画フィルタは初回のリモート対応リリースに必須ではない。**
静止画のフィルタに比べて需要が低いという利用者の判断による。

→ 実装順は「静止画 → 動画ジャンプ → (判断) → 動画フィルタ」とし、
動画フィルタが落ちても他が出荷できる形に保つ。

### 10.5 Codex の見積もり (2026-08-03) と結論

読んだうえでの回答。要点:

#### B を CPU 実装にすると「1 つの実装」にはならない

> Rust 関数を HLSL から直接呼べるわけではない。B を CPU 実装にすると、実際には
> 「共有パラメータ + 同じ仕様の 2 バックエンド」であり、差を golden test で拘束する設計になる。

**B を選ぶ動機は「2 つの実装を避ける」ことだった**ので、これは前提を崩す。

#### 障害は関数ではなく grade 直前の色

| | grade に入る直前 |
| --- | --- |
| PC 表示 | D3D11 Video Processor 通過後の **BGRA8 UNORM** (full-range G22 BT.709)。**線形光ではなくガンマ符号化 8bit 上で演算**している |
| リモート tap | VPP より**前**。NV12 / P010 |

CPU 版 B では `NV12/P010 → BT.709 RGB → PC と同じ境界で BGRA8 量子化 → grade →
studio-range NV12 → encoder` が要る。matrix / range / transfer / クロマ位置 /
8-10bit 丸めをすべて明示する必要があり、現在の scaler wrapper は
`sws_setColorspaceDetails` 相当を明示していない。

**見積もり: SDR 中心で 3〜6 人日 / 600〜1,200 行。HDR まで厳密に合わせるなら 5〜8 人日以上**
(それでもドライバ依存の差が残る)。

#### A のずれは軽微とは限らない

- FFmpeg `lut3d` の既定補間は **tetrahedral**、mIV は **trilinear** (`interp=trilinear` 明示が要る)
- FFmpeg `eq` は YUV plane 上で処理し、彩度をクロマ倍率として実装。mIV は RGB 上の
  HSL lightness 基準。**式も順序も違う**
- black / white point / midtone、temperature、LUT strength の線形合成に相当する既成フィルタが無い

creative LUT だけ / SDR / BT.709 固定 / trilinear 明示 / grade 前に BGRA8 量子化、
まで揃えれば近づく。**全 8 調整を既成フィルタへ置き換える案は、強い彩度・コントラスト・
midtone で目視できる差が出る**と考えるべき。

#### 第 3 の道 — これを採る

**現在の D3D11 grade を swapchain 非依存の offscreen GPU 段として切り出す。**

```
decoder / D3D11 VPP
  → BGRA8 offscreen texture
  → 既存と同じ grade shader
  ├→ ローカル presenter
  └→ readback → swscale → remote encoder
```

VPP による色変換・HLSL の式・LUT テクスチャと補間・clamp と量子化境界が**すべて同一**になる。
remote-headless で presenter が非表示でも、swapchain を作らず offscreen render target へ
描けば成立する。**規模は 3〜5 人日で CPU 版 B と同程度、かつ長期的に優れている**
(CPU 全画素処理を増やさない / PC との差が最小)。

### 10.6 結論

**動画 grade は §10.4 の撤退条件を発動する。**

- 静止画 (段 0〜3) を先に完成させる
- 動画はジャンプ (段 4) まで
- **動画 grade は独立フェーズとし、実装形は「第 3 の道 = offscreen GPU 段」とする**

A を急いで入れて後から直すより、後で共有 GPU 段として正しく作る方が価値が高い。

### 10.7 静止画側への助言 (採用)

`egui::Context` は本質的な障害ではない。`ensure_final_composite_texture` が
`&mut App` + `Context` を要るのは画素変換ではなく、texture upload / repaint 要求 /
worker poll / pending・cancel・generation 管理 / App 所有キャッシュ /
PDF 再レンダや編集結果の materialize のため。

したがって「1 つの同期関数」に詰めず、次のように分ける:

1. ページ個別・場所の標準・global を解決する**共有 resolver**
2. 適用順と解決済みパラメータを表す **`FinalCompositePlan`**
3. tone / sharpen / colorize / LUT / postfilter の**共有 CPU executor**
4. App と remote が**それぞれ所有する** cache / cancellation / 出力 adapter。
   CPU final composite worker は別所有だが、段 3b の GPU inference は App 所有の
   singleton Runtime / typed bridge を通し、接続取得と切断の barrier で local と直列化する

> 共有すべきものは**段取りを表す plan と変換 executor**。`egui::Context` は App 側 adapter に残す。

#### 段 0 実装結果 (2026-08-03)

`src/final_composite.rs` に次を切り出した:

- `resolve_effective_params(page, location, global)` — 値だけを受け、ページ個別 > 現在地標準 >
  global の参照を返す。`App::effective_params(idx)` はお気に入り最深一致や製本ページ標準を集める
  adapter となり、従来と同じ優先順位でこの resolver を呼ぶ
- `FinalCompositePlan` — tone、実行済み AI を踏まえた smart sharpen 強度、colorize、解決済み
  Creative LUT、post_filter を保持する
- `execute_final_composite` — 旧 `run_final_effect_job` の段順、cancel 境界、`should_apply` 判定を
  そのまま移した共有 CPU executor

元画像の選択は plan に含めない。raw / edit result の materialize は DB、generation、pending、
cache の所有権を必要とし、選択済み pixels が executor の入力境界だからである。final AI も plan に
含めない。現行どおり tone 後・smart sharpen 前に独立 cache / worker として実行し、adapter が
AI 結果の有無と `used_upscale` を解決してから plan を作る。
段 3b の再調査で、remote 接続中は PC の main / fullscreen 入力が modal で停止するため、
PC 優先 preemption は不要と確定した。一方、接続前から残る local AI と切断時の remote AI は
境界をまたぎ得るため、App 所有の singleton Runtime / typed bridge と接続 lifecycle の
acquire / drain barrier で直列化する。adapter 固有なのは remote stable-key cache、cancel owner、
最終出力とする。詳細は [web-remote-ai-plan.md](web-remote-ai-plan.md)。

#### 段 1a 補正適用の実装結果 (2026-08-03)

この回は段 1 のうち補正だけを実装し、編集結果の materialize は次節の契約どおり次回へ残した。
remote の Page worker は raw decode 後の `ColorImage` を次へ通す:

```
resolve_effective_params(page, location, global)
  → build_final_composite_plan_without_ai
  → execute_final_composite
  → WebP encode
```

`build_final_composite_plan_without_ai` は App の final AI 無し経路と remote が共有し、smart sharpen は
`effective_smart_sharpen(false)` で組み立てる。AI upscale / denoise は実行せず、tone → smart sharpen →
colorize → Creative LUT → post-filter の段順、cancel 境界、`should_apply` は段 0 の executor に一任する。

値は remote session 開始時に焼き付けない。各 Page 要求の合成準備時に、worker 上で次の順に読む:

1. 稼働中の `settings.db` handle から favorites、`global_preset`、Creative LUT 登録を targeted read
2. `WorkerContext.adjustment_db` から canonical page key の `page_params` を exact read
3. page 個別値が無い場合だけ `favorite_params` を読み、現在地 path に対する最深の ON favorite を選択

したがって PC 側の操作が各 DB へ commit された後に始まる次の Page 要求は、新しい page / location /
global を読む。page 個別がある要求で favorite DB 全走査を起こさない性質も
`resolve_effective_params` の lazy location 契約と揃える。

favorite の選択は App の idx 経路と remote の path 経路が
`active_favorite_default_id_for_path` を共有する。`app_and_remote_resolve_the_same_nested_favorite_and_page_scopes`
は同じ通常画像について global → 外側 favorite → 内側 favorite → page 個別 → 内側 OFF の各状態を
両 adapter で解決し、同じ `AdjustParams` になることを固定する。ZIP / PDF の page key は次節の
canonical key と同じで、location path は App と同じくコンテナ本体である。

remote 固有の所有物は次のとおり:

- 合成済み CPU pixels の LRU: 最大 8 件かつ 128 MiB。key は page key、source mtime / size、target_px、
  **解決済み全パラメータ**、LUT entry を含むため、PC 側の補正変更は cache miss になる
- parsed Creative LUT の LRU: 最大 16 件。App の LUT library / cache は触らず、同じ parser と builtin
  generator だけを共有する
- Page cancel token: 新しい prefetch は古い prefetch を、foreground は進行中 prefetch を cancel する。
  cancel は decode と共有 executor に渡し、置換された要求は `Busy` と明示的な理由で返す

合成時間は各 cache miss で `final_composite elapsed_ms` と tone / sharpen / colorize check+apply /
Creative LUT / post-filter の内訳をログへ出す。executor 自体には固定 deadline を置かない。
remote-web の IPC 応答期限は Page だけ 10 秒から **60 秒**へ分離し、metadata / thumbnail の 10 秒は
変えない。60 秒を超えた transport timeout は既存どおり HTTP へ返し、内部 retry で同じ重い合成を
再実行しない。これにより遅い full-page colorize / post-filter を従来の raw decode 用 10 秒で
失敗扱いにせず、真の timeout、DB / LUT 読み出し失敗、cancel を区別できる。
debug build の合成単体計測では、2048x1365 の synthetic page に tone / smart sharpen /
Gaussian tone-density colorize / built-in Creative LUT / post-filter をすべて通して約 0.84 秒だった。
これは deadline ではなく参考値であり、実時間は入力サイズ・補正内容・実行環境に依存する。

回帰テストは `plan_without_ai_matches_the_app_non_ai_formula` で App の AI 無し plan、
`remote_default_adjustment_preserves_pixels` で既定 global の画素不変、
`adjustment_render_settings_are_read_from_each_committed_snapshot` で commit 後の live 再読込も固定する。


#### 段 1 の編集結果供給契約

remote adapter は `LoadRequest` の edit-preview 分岐を full-page の正本にはせず、次の安定キーを
1 回導出して、補正・編集の全 DB lookup で共有する:

- 通常画像: `adjustment_db::normalize_path(path)`
- ZIP entry: `adjustment_db::zip_entry_key(zip_path, entry_name)`
- PDF page: `adjustment_db::zip_entry_key(pdf_path, "page_{page_number}")`

raw decode 後、App の `ensure_edit_result_pixels` と同じ順で headless materialize する:

1. `mask.db` の bitmap + vector mask を source 寸法へ復元し、マスクがあれば MI-GAN inpaint
2. `local_adjust.db` の layer JSON を読み、erase 結果を入力に `local-adjust-core` で合成
3. `conceal.db` の bitmap + vector mask と現在の conceal preset を読み、local-adjust 結果へ合成

この結果が `FinalCompositePlan` executor へ渡す edit-result source になる。各 DB handle、MI-GAN / local /
conceal worker、cancel、generation、CPU result cache は remote session が所有し、App cache や
`egui::Context` を共有しない。未完成の上位 layer がある間に下位 pixels から完成 result を作らない
点も App と同じ契約にする。

段 1 ではこの順序を remote に書き直さない。`App::assemble_edit_result_pixels` の layer 選択と、
MI-GAN / local-adjust / conceal の UI 非依存 worker body を typed な edit-source request / executor として
共有層へ出し、App adapter もそこへ接続し直す。App と remote の違いは DB snapshot・cache・worker
queue・generation の所有者だけにする。これは元画像選択を `FinalCompositePlan` へ混ぜず、別の
materialize 境界として共有するという段 0 の判断の後半である。

`ensure_edit_result_pixels` 自体にはテキストと crop は入らない。テキストは complete な final
composite の後段で `comic.db` の scene を同じ font / stamp resolver で source 座標から出力寸法へ
scale してベイクする。crop は `export_crop.db` の設定を読み、テキスト合成後の最終出力へ
`export_crop_rect_for_pixels` と同じ座標変換で適用する。したがって段 1 の headless 出力順は
`raw → erase → local-adjust → conceal → shared final composite → comic → export crop → WebP encode`
とする。

`edit_preview_cache` は最大辺 2048px・下地 WebP q=90 で、保存も編集画面を閉じた境界に限られる。
full-page の正本にすると解像度と鮮度の両方で App と一致しないため、段 1 の canonical source には
使わない。`edit_preview_key` / `edit_preview_db` / `skip_cache` の 3 重外れを個別に直す配線も作らず、
上記 headless materializer へ置き換える。

#### 段 1b 編集結果 materialize の実装結果

`src/edit_source.rs` に typed な `EditSourceRequest` / `EditLayer` と共有 executor を置き、
App の `assemble_edit_result_pixels` も同じ layer 選択へ接続した。App が所有していた
local-adjust / conceal の worker body と MI-GAN の model-load / inference / diffusion fallback
選択も共有 body を呼ぶ。`Pending` は完成結果に昇格せず、順序は
`raw → erase → local-adjust → conceal` のまま固定する。

remote は heavy worker ごとに mask / local-adjust / conceal / comic / export-crop DB の
read-only handle を所有する。worker 起動時 open が失敗して `None` でも Page ごとに再 open し、
再 open 失敗は Page の明示的な Internal error とログへ出すため、一過性の失敗を worker の
寿命へ latch しない。各 Page は安定 page key で committed snapshot を読み、snapshot 内容の
SHA-256 を source stat・target・補正と共に CPU result cache key に含める。

出力は共有 edit executor の後に共有 `FinalCompositePlan`、App と同じ comic font-key / stamp
resolver、共有 crop 座標変換を通して WebP 化する。MI-GAN は erase row があるページだけ
runtime を初期化し、Page ログへ edit 全体と erase / local / conceal の時間、diffusion fallback
有無を記録する。MI-GAN は 512px tile を 48px 深さごとの複数 pass で処理するため、通常の
小マスクは UI の既存想定どおり数秒でも、大きな穴は旧 60 秒を超え得る。Page の transport
timeout は 10 分へ広げる。prefetch は後続要求で core cancel されるが、foreground は transport
timeout が core 作業を cancel する protocol ではないため、この時間内で完走させる。
