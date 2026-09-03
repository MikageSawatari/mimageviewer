# 次リリース検討バックログ

このファイルは、まだ着手していない作業候補だけを置く恒久バックログ。
完了した項目はコミット履歴・リリースノート・個別設計メモに任せ、このファイルからは削除する。

運用ルール:

- 着手前に `docs/README.md` から該当領域の設計ドキュメントを読む。
- 着手中のものだけ `対応中` と明記してよい。完了したらこのファイルから削除する。
- 判断保留・見送りの理由は、次に再判断する人が困らない最小限だけ残す。
- 依存ライブラリ更新は `CLAUDE.md` のリリース手順チェックリスト Phase 2 と整合させる。

---

## 1. 優先候補

### 1.0i Remote の寸法 probe を「近傍だけ」にするか — R-23 の残り (2026-08-30)

**一括・並列化は v3.3.1 で済んだ** (R-23)。実ファイル画像の寸法読みが 1 件ずつだった
のを、ZIP (書庫を 1 回開く) / PDF (worker 1 往復) と同じ「まとめて 1 回」へ揃え、
rayon の既定プールで並べた。**答えは 1 つも変えていない** (等価性テストあり)。

**残っているのは「件数そのものを減らすか」という判断で、これは答えが変わる。**
`build_remote_spread_page_groups` は横長ページを境に以降の組み方が変わるので、
近傍だけ読んで残りを後から埋めると、**埋まった瞬間に見開きの組が組み替わる**。

**方針決定 (2026-08-30、利用者判断)**: **B は採らない。** 表示中にページの組が変わるのは
体験として悪い。直すなら **C (永続化)** か、待たせる間の**進捗表示**で体験を改善する方向。

- **A. 現状維持**: 全件読む。並列化で 3〜4 倍速いが件数に比例する。**通常のフォルダでは
  実質ゼロ**なので、当面はこれで足りる。
- **B. 近傍だけ + 非同期補完**: ~~初期応答は一定~~ → **不採用**。横長ページを境に以降の
  組み方が変わるので、補完が届いた瞬間に見開きが組み替わる。
- **C. 永続化**: 一度読んだ寸法を残す。**答えを変えない**のでこちらが本命。
  設計上の論点は「どこへ書くか」— カタログ DB には寸法列があるが、remote service は
  別プロセスなので所有権をどうするか。サムネイル blob を持たない**寸法だけの行**を
  許すかどうかも含めて決める。
- **D. 進捗表示**: 待ち時間そのものは減らないが、無言で固まるのをやめる。C と併用可。

**行が無い条件を正確に**: 既定の `Auto` 方針は「2MB 以上」または「decode+display が 25ms
以上」でキャッシュする。つまり**小さくて速い画像は、一度見た後もカタログ行が無い**。
「未閲覧のフォルダだけの問題」ではない。

**計測について**: 合成ファイル (1200x1800 PNG/JPEG、OS キャッシュ温) では 1 件 32µs、
並列化で 3.28 倍。レビューの実測は初回 454µs/件。**合成の数字を実機の見積もりに使わない**
(過去に 30 倍外している)。NAS のように I/O が支配する環境ほど並列化の効きは大きい。

### 実は大半が既にある

[post_operation_selection.rs](../src/post_operation_selection.rs) の `decide` は、
**要求を再読込またぎで保持し、集合が増えていれば再適用する**。`note_applied` が期限も
延ばすので、遅れて届く分は本来拾える。

失うのは 1 か所だけ — 「前回適用した集合から増えていない」ときの `Step::Drop`。これは
**利用者が自分で変えた選択を、無関係な再読込のたびに奪わない**ために置かれている
(コメントに明記あり)。守りたいものは正しいが、判定が代理になっている。

**利用者の言い方のほうが正確**: 止める条件は「集合が増えなかった」ではなく
「**利用者が選択を変えた**」。置き換えると:

| 状況 | 今 | 直した後 |
| --- | --- | --- |
| 集合が増えた | Apply | Apply |
| 増えていない・利用者は触っていない | **Drop** (= 以後を失う) | Wait (10 秒の期限で切れる) |
| 利用者が選択を変えた | Drop (偶然) | **Drop** (明示的に) |

**実装 (2026-08-30)**: 見立てとは置き場所が変わった。`decide` に現在の選択を渡す形には
できない — **再読込が `checked` を消す**ので、`decide` が走る時点の選択は利用者の意思を
表していない。判定は再読込より**前**、`check_external_folder_changes` が持つ。

3 つが噛み合っている:

1. `decide` の「増えていない」は `Drop` ではなく `Wait` (期限まで)。これが R-09 の本体
2. 再読込の直前に `still_owns_selection` を見て、違っていれば要求を取り下げる
3. 自動再読込の選択保存を止めるのは「**まだ 1 度も適用していない**要求がある間」だけに
   変えた。適用済みなら「元の選択」はこちらが置いたものなので、通常どおり保存して戻す

**テストが 1 つ設計欠陥を捕まえた**: 出力が見えている限り上の期限切れ判定は通らないので、
「増えていない = Wait」だけにすると要求が不死身になり、自動再読込の選択保存を永久に
止めてしまう。期限を明示的に見る形にした。

変異 4 種 (常に所有を主張 / カーソルを見ない / 件数だけ比べる / 期限切れしない) はすべて
落ちる。「件数だけ」は最初生き残ったので、**件数が同じまま別項目へ入れ替える**ケースを
足した。

### 分割到着はどれくらい起きるか

**通常は起きない。** フォルダ監視の debounce は 700ms で、しかも**イベントが来るたびに
期限が延びる** (`CURRENT_FOLDER_WATCH_DEBOUNCE_MS`、`poll_current_folder_watch`)。
コピー中はファイル作成と書き込みのイベントが続くので期限が延び続け、**700ms 静かに
なって初めて 1 回だけ再読込**する。普通の貼り付けは 1 回の tranche にまとまる。

分けるには**コピーの途中に 700ms 以上の空白**が要る。現実的なのは Shell の確認ダイアログ
(「置き換えますか?」) で利用者が考えている時間くらい。あとは、増えていない再読込
(既存ファイルの更新、一時ファイルの消滅、リネーム) が挟まると上の表の 2 行目に当たる。

**これは仕組みからの推論であって実測ではない。** 頻度を根拠に優先度を決めるなら、
実際に大量貼り付けをして再読込回数を数えるべき。

### 1.0k テキスト注釈の編集が重いのは「毎フレーム全画面を焼き直す」ため (2026-08-30 調査)

**発端**: 利用者から「テキスト注釈の編集が重い。1/2・1/4 解像度にする設定を入れて
しのいでいるが、解像度を落とさずに軽くならないか。R-07 / R-14 でそれが直るのか」。

**答えは直らない。R-07 / R-14 は別サブシステム。** 形は同じ (大きな値を UI スレッドが
持って複製する) だが、直す場所が重ならない。

| | R-07 / R-14 | この項目 |
| --- | --- | --- |
| 対象 | 補正レイヤーのマスク | テキスト注釈 (comic overlay) |
| 触るファイル | `ui_adjustment_panel.rs` / `local_adjust_db.rs` / `edit_bundle.rs` / `sidecar.rs` | `app.rs` の同期ベイク / `crates/comic-core/src/raster.rs` / `comic_overlay.rs` |
| 重さの正体 | マスクの直列化・圧縮・DB 保存、文書の複製 | 全画面のベイク + composite + GPU upload |

### 何が毎フレーム走っているか (コード確認、[app.rs](../src/app.rs) `ensure_comic_composite_texture`)

編集中 (`text_mode`) は **意図的に同期ベイク**している。非同期にすると、ドラッグ中は
毎フレーム `comic_generation` が進んで常に未完 → 下地 (注釈なし) にフォールバックし、
枠だけ動いてテキストが消える。その退行を避けるための同期である。**この判断自体は正しい。**

同期で走る中身は 4 つとも**全画面サイズ**:

1. `bake_annotation_layers` — レイヤーを画像と同じ w×h で確保して描く (Multiply が混ざると複数枚)
2. `composite_annotation_layers` — 下地と合成してもう 1 枚 w×h
3. `clamp_for_gpu(..).into_owned()` — さらに 1 枚コピー
4. `ctx.load_texture` — w×h を毎フレーム GPU へ upload

`text_preview_scale` (1/2 … 1/8) はこの 4 つ全部を 1/N&sup2; にする。よく効くのは当然で、
**裏を返すと「全画面を焼き直している」ことが重さの全部**ということでもある。

### 解像度を落とさず速くできるか — できる見込みはある

**ドラッグ中に実際に変わるのは、動かしているオブジェクトの旧位置 ∪ 新位置だけ**である。
下地 (`base`) は `Arc::ptr_eq` が保たれていて変わらない (= 作り直していない)。
つまり全画面を焼き直す必要が無い。

- 部分アップロードの手段は**ある**: epaint 0.33.3 に `TextureHandle::set_partial(pos, ..)`
  (`epaint/src/texture_handle.rs:78`、`ImageDelta::partial`)。確認済み
- 規模は**中**。`bake_annotation_layers` に「原点つきの窓」を渡せるようにする契約変更 +
  composite の窓化 + `app.rs` 側の dirty rect 追跡

**着手前に潰すこと**:

- 影 / 縁取り / ぼかしは幾何 bbox の外へ出る。padding は**推測せず bake 側から出す**
- 窓に重なる他オブジェクトは z 順で焼き直す必要がある (窓に clip して全件通す)
- Multiply レイヤーは画素ごとなので窓化しても結果は変わらない
- 文字の入力・改行は 1 オブジェクトの bbox が変わるだけなので同じ枠組みに乗る

**まだ測っていない。** §1.0 の表にある 70.6ms / 27.7ms は**補正レイヤーの数字**であって
この経路のものではない。着手時に、原寸で 1 フレームのどこに時間が行っているかを先に測る
(4 段のどれが支配的かで、窓化だけで足りるか upload の形まで変えるかが決まる)。

### 並行開発できるか — できる

`external-tool-launch` worktree が触る `src/` 20 ファイルのうち、この項目の対象
(`crates/comic-core/`、`comic_overlay.rs`、`ui_text.rs`) と R-07 の対象
(`ui_adjustment_panel.rs` ほか) は **0 件**。唯一重なるのは `app.rs` だが、向こうのハンクは
10701〜15786 / 26212 / 60220〜60549 / 66930〜67594 で、注釈ベイク (59300 台) とは重ならない。
他の 3 worktree (pano / r2e / video-strip) は master に対して `src/` の差分が無い (= マージ済み)。

### 1.0f 実機確認で見つかった 3 件 (2026-08-30)

**(a) F12 連打で動画再生が止まり、別ウィンドウが閉じる。** 利用者報告。
**v3.3.0 でも起きる (連打を続けると再現)。退行ではなく、起きやすくなった。**

**原因はログで確定した。私が最初に立てた仮説 (`push_detached_release` が移譲した lease を
畳む) は外れ。**実際は **自分で送った `ViewportCommand::Close` が、再利用された同じ
ViewportId の新しい viewport に届き、利用者が × を押したのと同じ扱いになる**。

実ログ (`mimageviewer.log`、transition 29 → 30):

```
1545.452  transition=29 target=fullscreen action=Destroy ... viewport_command=Close  ← 自分で送る
1545.532  F12 → transition=30 target=detached phase=begin host=0x0
1545.564  [detached-viewer] show viewport: ... host=hwnd=0        ← 同じ ViewportId を再表示
1545.564  [detached-viewer] viewport close_requested: presentation=None host=hwnd=0  ← ★
1545.566  active-detached-session action=clear reason=handle_fullscreen_close_request
1545.575  presentation-transition id=30 target=DetachedWindow effect=Destroy hwnd=0x0
1545.594  [decoder-lifecycle] video-decode thread exit: live_count=0                 ← 再生停止
```

★ の行は **`presentation=None` かつ `host=hwnd=0`** — まだ窓が無い viewport なので、
**利用者が閉じられるはずがない**。112ms 前に自分が送った Close が届いている。

[ui_fullscreen.rs:14591](../src/ui_fullscreen.rs) は
`ctx.input(|i| i.viewport().close_requested())` を無条件に `close_fs = true` にしており、
**自分が送った Close と利用者の × を区別できない**。

**なぜ v3.3.0 より起きやすいか**: R-27 の lease 移譲で同じ window_id / ViewportId が
そのまま後継へ渡るため、viewport の再表示が早く・同一 id で起きる。stale な Close が
新しい viewer に当たる確率が上がる。**Codex が案 B を否定したときに挙げた危険
(「`ViewportEvent::Close` を利用者の close と解釈して後継ごと閉じる」) が、
現行経路にも存在していた**ということ。

**v3.3.1 で修正済み** (`edf1c5ed`)。方針は Codex と合意 ([review §10.7](review-v3.3.0/README.md)、ブリーフ = [close-identity-brief.md](review-v3.3.0/close-identity-brief.md))。私が出した 2 案はどちらも Codex が証拠つきで否定した (egui では内部 Close と利用者の × が同じイベントになり照合不能 / incarnation と ViewportId の採番順が循環)。採るのは **内部 teardown で Close を送らず、terminal になった ViewportId を再利用しない** 第三案。

**(b) 動画を別ウィンドウで開いているとき、gamepad の十字キーでシーク操作ができない。**
利用者報告。**v3.2.0 / v3.3.0 でも同じなので退行ではない。** 別ウィンドウ側にキー入力が
届いていないか、動画面の分岐に入っていない。§1.0d の R-02 と同じ「gamepad の配り先」
まわりだが、静止画では動くので別の原因。

**(c) R-02 の症状は利用者環境では再現しなかった。** 別ウィンドウを開いたままメインを
前面にして十字キーを押すと、**v3.3.0 でもメイン一覧が動いて見えた**との報告。
§10.2 で確認したのは「配り先と面の判定が別の情報源を使っており、食い違い得る」ことまでで、
`active_detached_context_is_at_rest()` が真になる条件は限られる。修正 (`7f064a57`) は
2 つの判定を 1 つにするもので構造的には正しいが、**利用者に見えていた症状の説明としては
私の記述が証拠より強かった**。§10.2 の書き方を弱める。

### 1.0d v3.3.0 リリースタグの再レビュー — **v3.3.1 で決着**

正本は [docs/review-v3.3.0/README.md](review-v3.3.0/README.md) §9-10。指摘された P1 5 件は
R-27 / R-02 / R-26 を v3.3.1 で修正し、R-14 は R-07 の一部へ格下げした
(同じ関数から出ている。§10.4)。最後に残っていた R-07 (+R-14) も
**2026-09-01 に完了**した ([briefs/local-adjust-ownership-brief.md](briefs/local-adjust-ownership-brief.md)
が着手用ブリーフ、[briefs/local-adjust-ownership-progress.md](briefs/local-adjust-ownership-progress.md)
が結果)。**このレビュー由来の P1 は全件決着。**

### 1.0 v3.3.0 レビューからの持ち越し (2026-08-29 に判断)

正本は [docs/review-v3.3.0/README.md](review-v3.3.0/README.md)。v3.3.0 出荷時に
「複数ウィンドウの不具合解消」を優先し、以下を次版へ回した。**いずれも実測または
コード確認のうえで判断しており、推測で落としたものは無い。**

| ID | 内容 | 退行か | 実測 / 根拠 |
| --- | --- | --- | --- |
| R-07 (d) | 補正マスクの圧縮・DB 保存が UI スレッド | v3.2.0 以前から。**v3.3.0 が 5.8 倍速く**した後 | 24MP で 70.6ms。**ストローク単位ではなくフレーム単位だった** (`Slider::changed()` はドラッグ中毎フレーム真)。**対応済み** (`976aef4a` / `d6a591b2`) |
| (b) | 図形のキー移動 / 回転がキーリピートごとに全文書を保存 | v3.2.0 以前から | リピート 1 回ごとに 70ms + undo 2 文書。**対応済み** (`368499d9` / `9ccbb934` / `d75bed4c`) |
| (c) | 塗りの毎フレームに文書 1 複製が残る | v3.2.0 以前から | 24MP で 27.7ms/フレーム (もう 1 枚は `3315fd05` で削除済み)。**対応済み** (`116ca8cd` / `a9f69d14`) |
| R-18 | Remote の ZIP 寸法 probe が 64KiB で打ち切る | いいえ (経路ごと v3.3.0 の新規) | v3.2.0 は寸法を一切持たず見開きが効かなかった。**見送っても v3.2.0 より悪くならない** |
| R-12 | detached parked fallback が UI で `parent().is_dir()` | いいえ | クリック 1 回につき 1 syscall。コード側にも「恒常経路ではない」と明記。**v3.3.1 では見送り** — レビューの修正方向 (parked snapshot を async build/commit/abort まで保持) は detached の lifecycle 変更で凍結ルールの対象。1 syscall のために所有権境界を動かす取引が見合わない (切断された共有だけが遅い) |
| R-11 | トレイ退避が sidecar 書き込み完了を UI で待つ | いいえ | `df245720` で writer 側の複製 1 回分は減った。**対応済み・実機確認済み** (2026-09-01。編集直後に退避 → 復帰で編集が残ることを確認) — `PersistScope` で「待つ」を終了経路だけの選択にした。待っていた理由はプロセスが止まることだけで、トレイ退避では writer も読み手も生きたまま (読み手は pending を先に見る) |
| R-19 | 360 動画の roll が描画へ渡らない | いいえ | 該当素材のみ。shader 配線 + 実機検証が要る |
| R-10 / R-13 / R-16 / R-22 | P3 4 件 | いいえ | R-10 は §1.141 で既に P3 裁定済み |

**R-07 / R-14 の結果 (2026-09-01)**: 保存は worker へ移り (UI スレッドの同期 I/O は
p95 0.3ms)、**24MP の筆のフレームは 101ms → 9.2ms (8.5 → 73 fps)** になった。ただし
**原因は 3 回とも予想と違った** — 保存でも塗りでもなく、①マスク重ね描きの毎フレーム
複製 ②補正パネル編集器の毎フレーム複製 ③プレビューの直列再構築 だった。どれもこの
表にも review にも無く、計装を足して初めて見えた。経緯・実測・未修正で残した 1 件
(detached context と App-global な保留、BA-7 系として
[detached-rework-plan.md](detached-rework-plan.md) へ報告) は
[briefs/local-adjust-ownership-progress.md](briefs/local-adjust-ownership-progress.md)。

**(c) は見積もりをもう一度外していた** (2026-08-30 に調査。着手前に読むこと):

前提が違った。**あの複製は無駄ではなく、正しさを支えている。** 16 個の closure が通る
`mutate_local_adjust_layer_from_canvas_impl` は、文書を丸ごと複製してから closure に渡し、
closure が `false` を返したら複製ごと捨てる。つまり「`false` を返した = 文書は変わっていない」
という不変条件を、**複製の破棄そのものが作っている**。その場で書き換える形にすると、
`false` を返した編集の書き込みが残る。

書き込みは 16 か所に散らばってはいない。**1 か所に集まっている** —
`local_adjust_target_raster_vector_mask_mut`
([ui_adjustment_panel.rs](../src/ui_adjustment_panel.rs))。これが `false` の前に文書を書く
経路は 3 つあり、`create` 引数で止まるのは 1 つ目だけ:

1. `create = true` のときスロットへ空マスクを作る (バックログが元から挙げていたもの)
2. **`create` に関係なく** `Raster` を `RasterVector` へ変換する (形式の昇格)
3. **`create` に関係なく** `resize_to(width, height)` を呼ぶ

その後で呼び出し側は `source.size != [mask.width, mask.height]` や「塗ったが 1 画素も
変わらなかった」で `false` を返せる。つまり **`create=false` で呼んでいる経路も安全ではない。**

集約されているのは朗報 (直す先が 1 つ) だが、**必要なのは契約の変更**であって複製の削除
ではない。方向としては、材質化・昇格・リサイズを「それ自体が変更である」ものとして
closure の外へ出し、`mutate` は変更の有無だけを返す純粋な編集にする。そのうえで seam は
文書を **move** して (複製せずに) 渡す。

**(d) と R-26 は同じ境界の話。** (d) の「保存を worker へ出す」は、R-26 で入れた
「durable な保存の成功を公開の境界にする」(`b8cb3ce5`) を開け直さないよう設計する必要がある。
別々に考えない。

**(b) に着手するときの注意** (見積もりを 1 度外している):

- (c) の詳細は上へ移した (調査で前提が変わったため)。
- (b) はキーを離したときだけでなく、**ページ移動・モード終了・フルスクリーン終了など
  編集状態を畳む全箇所**で確定が要る。ブラシは破棄でよいがキー移動は完了した編集なので
  破棄できない。取りこぼし防止の監査テストを付けること。

### 1.7 detached 中の発火面解決の残り (? / トースト / スタック) — BA 報告

- 残り (P3、発火面の window_id 粒度が必要):
  1. **? キーのショートカット一覧**: detached 中にグリッドから開くと FS 用一覧が
     出る。修正には発火元の面情報を consume サイトから通す必要がある。
  2. **トーストの面粒度**: 発火面のビューアを閉じた後に完了したバッチのトーストが
     不可視のまま消える / active detached + main fullscreen 同時表示時に Viewer 面が
     両ビューアに出る (2 値 enum の粒度の限界)。許容中。
  3. **複数ウィンドウ×スタックの理想形**: スタックをクリックするとメイン一覧が
     フラット読書ビューに切り替わる (フル機能と同じ挙動)。理想は「メインは集約のまま、
     窓側だけフラット文脈を持つ」だが、窓が自前のフラット items 文脈 (bundle 化された
     stack 状態) を持つ設計が必要 (複数ウィンドウの bundle 化と同系統)。現行でも
     Shift+↓↑ ジャンプは動作する。
- 優先度: P3。

### 1.9 parked 窓のリソース制御 (サムネ pipeline 停止 / VRAM 合算予算) — 角度⑥送り

- 出典: v2.3.0 角度⑥レビュー (Sol/Terra 一致、docs/archive/review-v2.3.0/sol-angle-reviews.md)。
  いずれも bounded な効率問題でデータ喪失は無し。park/再活性ライフサイクルの構造に
  踏み込むため、リリース直前パッチではなく detached リワーク後続で設計対応する。
  1. **P2: park してもサムネ pipeline が止まらない**: `pause_background_work_keep_current_frame`
     は fullscreen/AI 系のみ cancel し、bundle の `cancel_token` / `reload_queue` /
     `heavy_io_queue` に触れない。park 時点で積まれていたデコードが走り切り、結果
     `ColorImage` が誰も poll しない rx に溜まる (窓の再活性化 / close まで保持)。
     対応案 = park 時に queue を drain (worker pool は殺さない)。ただし Requested 状態の
     サムネが再活性化時に再要求される仕組みの確認が必要 (state が Requested のまま
     queue から消えると復帰後にロードされない恐れ)。
  2. **P2: サムネ VRAM 上限が文脈単位**: `update_keep_range_and_requests` の予算は
     mounted 文脈にしか効かず、parked bundle N 個がそれぞれ上限近くまで保持し得る
     (動画サムネは eviction 対象外なのでフォルダサイズ分)。対応 = 全 bundle 合算の
     予算会計 + cross-bundle eviction (リワークの資源予算ステージ)。
  3. P3: `display_px_shared` が App-global のため、detached 文脈のデコード解像度が
     メイングリッドの表示密度に引きずられる (適用ミスは無し、品質/CPU の無駄のみ)。
- 関連: v2.3.0 で対応済みの境界 = bundle Drop の解放 (角度①でクリーン確認)、
  文脈別 channel/cancel/世代 (P2-9)。

### 1.10 終了時の削除 worker 未調整 — 角度⑤送り (P2、実害小)

- 出典: v2.3.0 角度⑤ Sol P2。数百件削除の実行中に終了すると、実行中の
  IFileOperation チャンクは完走するが後続チャンクは開始されず、部分削除の最終報告と
  完了後クリーンアップ (resume purge 等) が走らない。一覧は次回起動の走査で実態に
  収束するため破壊は無し。対応案 = 終了時に cancel を立てて現行チャンクの完了を
  短時間待つ + 次回起動時の注意トースト。v2.2.0 出荷時からの既存挙動。

### 1.12 detached 静止画窓から音声/動画へのフォルダ内ナビ不可 (メディア昇格導線)

- 出典: v2.3.0 実機検証 §5 #33 (2026-07-10)。別窓 (静止画) で ↑↓ ナビ中、音声/動画の
  アイテムへは移動できない (静止画窓はメディア再生セッションを持てない設計境界)。
  音楽ビュー→画像の方向は移動できるため非対称。ユーザー許容済み (「一旦これでもよさそう」)。
- 対応案: detached リワークの後続ステージで「静止画窓がメディアに到達したらメディア
  セッションへ昇格 (または既存メディア窓へ委譲)」の導線を設計。凍結ルール下では
  症状パッチを入れない。

### 1.13 duration 不明/不正 MPEG-PS のシークバー不能

- 背景: `sample.mpg` で実機確認済み。duration が不明または不正な MPEG-PS は
  シークバーで移動できない。VLC は同じファイルをバイトシーク + PTS 再スキャン方式で
  シークできる。
- 対応案: decode EOF で実尺を学習して `VideoInfo` / HUD の duration へ反映するか、
  duration を信頼できない場合にバイト位置シークへフォールバックする。
- 裁定: v2.2.0 以前から同挙動で、ユーザー裁定により次版送り。優先度 P3。
  final-report 追補 5 の P3 参照。

### 1.17 UI レイアウト崩れ (文字はみ出し / 見切れ) の自動検出 QA → その後に英語対応を再検討

- 背景: 英語対応 (UI ローカライズ) を検討したが、AlternativeTo は「英語 UI 必須」
  (FAQ: DB 登録アプリは英語対応が必須) のため、日本語のみの現状では掲載できない。
  一方、多機能ゆえに英語 UI を実機で全画面目視 QA するのは負担が大きく、しかも日本語でも
  文字はみ出し / 見切れのレイアウトバグが繰り返し発生している。そこで **まず言語非依存の
  「レイアウト崩れ自動検出」QA ツールに投資**し、日本語バグに即効かつ将来の英語化の
  「目視 2 倍」問題を先に解消する。英語対応の可否はその後に再判断する。
  (2026-07-13 相談。ユーザー方針: QA 投資を先行 → その後に英語対応を再検討。今は別作業中のため別途着手)
- 現状 (確認済み):
  - egui は文字配置時に galley サイズが取れ、painter は clip rect を持つため
    「galley 幅 > 割り当て幅」で見切れを機械判定できる。
  - UI スナップショット基盤 `tests/ui_snapshot.rs` (egui_kittest) が既にある (回帰検出向け)。
  - 文字列は現状ハードコード (i18n 未 externalize)。見切れ計装は現行日本語のままでも動く
    ので、i18n とは分離して先行できる。
- 方針 (QA ツール先行 → 英語再検討):
  - QA ツール (言語非依存・日本語バグにも効く):
    1. **見切れ計装**: 描画時に galley 幅 vs clip/available 幅を比較し、超過した文字列 +
       ウィジェット位置をログ列挙する。全画面を人が見なくても崩れ候補の一覧が出る。
    2. **疑似ローカライズ (幅ストレス)**: UI 文字列を ~1.4x 長い擬似文字列に置換したビルドで
       幅不足のウィジェットを炙り出す (英語が長くなる前提の先行テスト。日本語のままでも
       将来はみ出しやすい箇所を潰せる)。
    3. 既存 egui_kittest スナップショットを併用 (変化検出。最終は差分画像を目視)。
    4. (任意) スクショ自動キャプチャ + ビジョン判定で「切れ / 重なり」を triage。
    - 限界: 得意なのは見切れ / はみ出し / 行あふれ。位置ズレ・不格好な折り返し・パネルの
      重なりは取りこぼしがあり、多少の目視が残る。
  - 英語対応 (QA ツール整備後に可否判断):
    - 前工程 = 文字列の externalize (i18n 化)。
    - AI 一括翻訳 + **用語集** (見開き=Two-page spread 等を先に固定) + 各文字列の文脈付与で
      用語ゆらぎ / 品詞ズレを抑える。
    - **変数整合チェック**: `%s` / `{}` / 改行 の個数・種類が元と英訳で一致するか機械照合。
    - **逆翻訳 QA**: 重要文字列 (削除 / 上書き / 警告 / エラーダイアログ) に限定して英→日へ
      訳し戻し、元日本語と比較する。英語力が無くても意味ズレを検知できる (日本語同士の比較)。
    - 実機確認は見切れ計装 + 疑似ロカライズで大半を機械化し、目視は残差のみ。
    - "English is machine-assisted; feedback welcome" と明示し、OSS の Issue / PR で継続改善。
- AlternativeTo 連携: 英語 UI が入れば掲載可。申請素材 (Tagline / Description の v2.3.0 版
  = 音楽再生 + VST3 入り / Website / Source / License Free+OSS(MIT) / 画像 = icon.png + ss_*.png
  含む ss_music.png / Alternative to = NeeView・MangaMeeya・Leeyes・ZipPla・Honeyview・
  ImageGlass・IrfanView・XnView MP・nomacs・FastStone) は準備済み。GitHub Topics / 英語 README
  サマリ / egui Showcase / awesome-egui PR は UI 言語非依存の枠なので実施済み。
- 規模 / リスク: QA 計装 = Medium (描画経路への hook + 幅バジェット定義)。英語化 = Large
  (externalize + 翻訳 + QA 工程)。QA ツールは日本語バグにも効くため単独で投資回収がある。
- 優先度: P2 candidate (QA ツール)。英語対応は QA ツール整備後に再判断 (現時点は保留)。

### 1.21 自動選定した親コンテナ代表サムネイルに編集プレビューを反映

- 出典: v2.5.0 リリース前実機確認 (2026-07-17)。ZIP 内画像を編集しても、親階層の
  ZIP セルで自動選定された代表サムネイルには編集結果が反映されない。
- v2.6.0 で手動ピン経路は対応済み。cascade 解決後の固定 leaf (直接 Image / ZipEntry /
  PdfPage、ネスト ZipDir、変換アーカイブ) は canonical page key を親要求へ渡し、編集
  preview を catalog より優先する。固定 leaf の個別色調補正も注釈前の下地へ適用し、
  保存 / 無効化通知を親セルへ伝播する。
- 残件は **自動代表選定だけ**。worker 内で leaf を選定した後に page key を組み立て、
  対応する編集 preview があれば raw decode / catalog より優先する。自動選定結果と親
  キャッシュの無効化条件も同時に設計する。
- 不変条件:
  - 現行の編集 preview 内容 (erase / local_adjust / conceal / crop / comic 注釈) だけを
    自動選定 leaf へ反映する。自動代表へどの色調補正を適用するかは別途決め、手動固定済みの
    個別色調補正規則を変えない。
  - UI スレッドへ ZIP/PDF 列挙や SQLite 単件照会を追加しない。
- 規模 / 優先度: Medium〜Large / P3。

### 1.30 native video Stage 5 の再投入条件 (revert 済み、原因未確定)

- 出典: 2026-08-01。Stage 5 (`726f838d`) を入れたところ UI が完全停止する P1 が再現し、
  `737c5234` で **revert 済み**。v2.9.1 は Stage 5 を含めずに出す。§1.28 のカーソル問題は
  既知の問題として残る。
- 症状: 動画をフルスクリーン再生中に VST エディタを開閉すると UI が完全に固まる。**動画・音声の
  再生は継続し、EOF で次の動画へも進む。UI だけが死ぬ。**終了も右クリックも不能。2 回再現
  (t=178s / t=18s)。
- 停止点 (cdb `-pv` で採取): main スレッドが `SendMessageW` 経由の wndproc の中で
  `eframe run_ui_and_paint` → `egui_wgpu paint_and_update_textures` →
  `wgpu_hal::dx12::Surface::acquire_texture` → `WaitForSingleObjectEx`。
  他スレッドは全て健全 (`vst-owner-dispatch` は recv で idle、pump は sleep、render / demux /
  decode は稼働)。
- **原因は未確定。候補 2 つとも潰れていない**:
  1. **VST owner handoff**: owner / z-order / visibility の変更が同期メッセージを撃つ。
     ただし hidden anchor は published fullscreen host の破棄時のみ通り、C++ は
     `old_owner == new_owner` で早期 return し、owner 付け替え自体は Stage 5 以前から存在する。
  2. **カーソル所有**: 新実装は pump から **8ms ごとに `WindowFromPoint`** を呼ぶ。同 API は
     hit-test のため対象の wndproc へ **`WM_NCHITTEST` を同期送信**し、相手は UI スレッドの窓や
     **別プロセスの VST エディタ**になり得る。pump からの無制限な cross-thread / cross-process
     同期呼び出しであり、**Stage 4 の「pump は時間上限を保証できない処理を持たない」原則に抵触する**。
     「窓を作らないから同期メッセージを撃たない」は成立しない。
- 次回に取るべき証拠 (今回取れていない):
  - `acquire_texture` の wait が**本当に無限か**。wgpu-core 27.0.3 は **1000ms timeout** を渡し、
    wgpu-hal は `WAIT_TIMEOUT` を `Ok(false)` にして先へ進む。スタック 1 枚では
    「デッドロック」と「1 秒待ちを繰り返す飢餓」を区別できない。**wait の `dwMilliseconds` が
    `0x3e8` か `0xffffffff` かを読み、1.2 秒以上あけて複数回 break する**。
  - 停止直前に main が実際に受けた `msg` (どの同期メッセージが再入描画を起こしたか)。
    スタックにある `in_window_resize_subclass_proc` は全メッセージを通る常設 subclass なので、
    **resize が起きた証拠にはならない**。
  - `old_owner != new_owner` / anchor handoff / presenter retirement が実際に発生したかのログ。
- 再投入の条件: **同一 release profile** で 4 分割 A/B を通すこと。
  (a) Stage 4 baseline / (b) cursor のみ / (c) owner のみ / (d) Stage 5 全体。
  今回の A/B は `dev-runtime` 対 `release` で、出荷判断には十分だが**原因帰属としては profile 差が
  残る**。(b) が VST 開閉 soak を通ることを出荷条件にする。
- 診断用の回避策 (`MIV_WGPU_FRAME_LATENCY=2`、`WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT=DontWait`)
  は**出荷修正にしない**。症状が消えても wndproc 内 GPU wait と Win32 同期依存は残る。
- VST 側の長期案: editor を transient な presenter HWND へ付け替えるのではなく、**editor lifetime
  全体で安定した専用 owner proxy** を使い、topmost / focus / visibility を別の ordered transaction
  として扱う。owner request の dedupe と一括適用も要る。
- **保留 (2026-08-13、利用者判断)**。revert 後は**一度も再現していない**。原因未確定のまま
  再投入の A/B に工数を割く段階ではない、として棚上げする。再発したら上の「次回に取るべき
  証拠」から再開する (特に `dwMilliseconds` が `0x3e8` か `0xffffffff` かの確認)。
  §1.28 のカーソル問題は既知の問題として残したまま。
- 規模 / 優先度: Large / **P2 (保留)**。カーソル (§1.28) の修正はこれが片付くまで入らない。

### 1.31 wndproc の内側で GPU 待ちのある描画をする構造

- 出典: §1.30 の解析 (2026-08-01、Codex Sol の独立レビュー)。**v2.9.1 の範囲外**。
- **進捗 (2026-08-16)**: **§1.31-A は完了、§1.31-B は未着手**。A で vendored eframe に
  per-window / viewport / surface-generation 単位の typed scheduler と about_to_wait 外側
  paint drain を新設し、通常 RedrawRequested、bootstrap、AccessKit、不可視窓の要求済み work が
  message dispatch 中に renderer へ入る経路を除去した。inline 例外は event provenance が
  非ゼロ Resized の InteractiveResize frame と、その親 frame が作る immediate viewport render
  subtree だけ。reducer 11契約、既存 hidden throttle 3件、別 thread の SendMessageTimeoutW が
  RedrawWindow で同期 damage を発生させる Windows process test を gate 化した。
  **A が除去したのは「同期メッセージ自身が GPU 待ちを開始する経路」だけ**であり、外側 paint 中の
  GPU 待ちに後着の同期メッセージが巻き込まれる問題は B のまま残る。
- 何が問題か: winit は `WM_PAINT` 内で `RedrawRequested` を**同期 dispatch** し、eframe はそこで
  `run_ui_and_paint` を直接呼ぶ。さらに Windows の resize では flicker 防止のため意図的に同期 paint
  する。結果として **wndproc の内側で swapchain acquire と Present を行う**。Microsoft も
  `Present` が message-pump thread を待ち得ると明記している。
  main surface は `AutoVsync` / maximum frame latency **1** なので、前フレームが retire しない限り
  次の acquire が待たされる。
- 効果: 窓の owner / z-order / visibility を動かす操作、DWM 合成の切替 (Independent Flip / MPO)、
  device-loss 周辺のいずれかが絡むと、**同期メッセージ 1 通で UI が任意時間停止し得る**。
  Stage 4 で閉じた側 (presenter スレッドが窓を持ったままブロックする) に対し、こちらは
  「UI スレッドが自分の窓のメッセージ処理中に GPU で止まる」。**同じ上位の破綻の別の面**で、
  Stage 4 の pump 分離では届かない。
- Stage 4 以降の運用制約 (維持すること): pump thread では GPU API を呼ばない。held frame の
  再提示にも re-arm 用 `AcquireSync` を追加せず、reader key の維持と producer 側回復を使う
  ([video-architecture.md](video-architecture.md) の `FramePresentationState` 節)。
- 方向 (Codex 案):
  - wndproc は damage / resize / redraw 要求を記録して**即 return** する。
  - 実描画は外側の event-loop 境界へ post して coalesce する。
  - `Idle / Painting / Pending` の単一 typed render state を置き、**再入 acquire を禁止**する。
  - surface acquire は短時間・nonblocking にし、間に合わなければその frame を捨てる。
  - msg / paint depth / wait handle / timeout / Present 前後 / cloak / visibility を 1 つの trace に残す。
  - 同期 `SendMessage` 中に presentation availability を故意に止め、**wndproc が有限時間で返る**
    Windows 回帰テストを追加する。
  - 必要なら eframe / winit / wgpu を vendor patch し、upstream へ最小再現を出す。
- 併せて直す観測の穴: 今回 `ui-heartbeat-watchdog` は生きていたのに `panic.log` へ何も書かれ
  なかった。**active native fullscreen 中は main HWND が隠れていても watchdog を suspend しない**
  ようにする (危険な操作の最中こそ黙る、という現状は逆)。
- **v3.0.0 の判断 (2026-08-13)**: **構造修正は v3.0.0 をブロックしない**。
  - 根拠: この問題が実際に牙を剥いた唯一の事例は §1.30 で、それは **VST エディタの開閉が
    窓の owner / z-order を動かす**ことが引き金だった。v3.0.0 の Web 配信は、HTTP サーバと
    トランスコードを**UI スレッド外**で回すもので、**窓の所有関係にも同期メッセージにも
    手を出さない**。§1.31 の露出面を新たに増やさない。
  - その §1.30 も revert 後は再現していない (上記のとおり保留)。
- **観測の穴は解消済み (v3.0.0 前)**。`ui_heartbeat_should_stay_active_while_hidden`
  ([app.rs](../src/app.rs)) が keep-alive の述語に **mounted media session** (フルスクリーンの
  動画 / 音声、動画→音声モード、`FsCacheEntry::Video`) を含めるようになり、ネイティブ動画
  フルスクリーン中に main HWND が隠れても watchdog が armed のままになる。以下は当時の記録。
- ~~ただし観測の穴だけは v3.0.0 の前に埋める (Small)~~。§1.30 のとき
  `ui-heartbeat-watchdog` は生きていたのに `panic.log` へ何も残らなかった。
  [app.rs:62646 付近](../src/app.rs:62646) を見ると、main HWND が不可視になった時点で
  watchdog を suspend し、**例外は `viewer_session_is_detached_or_switching()` だけ**。
  ネイティブ動画フルスクリーン中は main HWND が隠れるため、**最も危ない操作の最中に
  watchdog が黙る**。keep-alive の述語にアクティブなメディアセッションを含める。
  これだけ入れておけば、次に野生で固まったときにログが残る。
- message-dispatch / render 位相分離は §1.31-A として完了。nonblocking acquire / Present と
  message service latency の上限制約は §1.31-B として後続する。
- **着手順の確定 (2026-08-16、ClaudeCode / Codex Sol 合意)**:
  `§1.85-A → §1.86 → §1.31 本体` の 3 段。根拠と逆順の失敗モードは
  [codex-texture-delivery-test-brief.md](briefs/codex-texture-delivery-test-brief.md) §0。
  §1.31 は frame drop を通常経路にするので、その経路の delta 配送を先に契約化する。
  **§1.86 は障害影響としては P3 のままだが、本項の依存関係上は必須前提**として扱う。
- **前提充足 (2026-08-16)**: §1.86 の単一 delta transaction owner と non-submit outcome の
  finalization 契約が完成し、その上で §1.31-A の位相分離まで完了。通常 frame drop と readiness
  wake を足す §1.31-B は未着手。
- **本体を A / B に分割 (2026-08-16、ClaudeCode / Codex Sol 合意)**:
  - **§1.31-A**: message-dispatch 位相と render 位相の分離。**同期メッセージ自身が GPU 待ちを
    開始する経路**を除去する。vendored eframe だけで完結する (winit / wgpu は vendor しない)。
    ブリーフ = [codex-render-phase-separation-brief.md](briefs/codex-render-phase-separation-brief.md)。
  - **§1.31-B**: presentation と message service latency の上限制約。
    **A を merge しても §1.31 は完了しない**。UI スレッドが外側の paint で GPU を待つ間も
    message pump は止まるため、他スレッドの `SendMessage` は依然ブロックする。A が消すのは
    「同期メッセージが GPU 待ちを**開始させた**」経路だけ。
  - **B の実現可能性は確認済み (2026-08-17、コードで実地確認)**。acquire 側は
    **wgpu の vendor 不要**で組める:
    - `Dx12UseFrameLatencyWaitableObject` の既定は **`Wait`**
      ([wgpu-types instance.rs](https://docs.rs/wgpu-types/27.0.1/))。つまり現状の
      `acquire_texture` は frame-latency waitable object を待ち、wgpu-core の
      `FRAME_TIMEOUT_MS = 1000` で打ち切られても**その結果を捨てて先へ進む**。
    - `DontWait` は doc に「**This is useful if the application wants to wait for the
      waitable object itself**」と明記されており、まさに B の用途。swapchain は
      waitable flag 付きで作られ、handle は取れるが wgpu は待たない。
    - handle への到達経路も public: `wgpu::Surface::as_hal::<Dx12>()` (surface.rs) →
      `wgpu_hal::dx12::Surface::waitable_handle()`。
    - → 設計: 起動時に `DontWait` を設定し、paint 前に `WaitForSingleObject(handle, 0)` で
      非ブロッキング判定。未 signal なら damage を保持したまま `DeferredNotReady` として
      その frame を捨て、signal されたら wake する。**時間窓ではなく OS の事実で判定する**
      ので憲法 5 に適合する。
    - 注意: DX12 専用。backend が DX12 でない場合 `as_hal` は None を返すので、その場合は
      現行挙動へフォールバックする (新しい失敗経路を作らない)。
    - 参考: mIV は既に `MIV_WGPU_FRAME_LATENCY` で `desired_maximum_frame_latency` を
      設定しており ([lib.rs](../src/lib.rs))、native presenter は自前 swapchain で
      `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT` を既に使っている
      ([render_core.rs](../src/video/native_presenter/render_core.rs))。機構は既知。
  - **Present 側は別問題で、これは vendor 不要では閉じない**。`SurfaceTexture::present()` は
    戻り値が `()` で、HAL の FIFO Present は interval 1 かつ `DXGI_PRESENT_DO_NOT_WAIT` 無し
    (`DO_NOT_WAIT` は blocked 時に `DXGI_ERROR_WAS_STILL_DRAWING` を返す契約)。
    厳密に bounded にするなら wgpu / core / hal の patch か render thread 分離が要る。
  - ~~**B は acquire だけ先に閉じて、Present は A 後の実測を見てから判断する**のが妥当。~~
    **2026-08-17 撤回。acquire も先に測る**。下記参照。
- **B は「計測 (B0) が先」に変更 (2026-08-17、Codex Sol の設計レビューで差し戻し)**。
  正本 = [codex-acquire-readiness-gate-brief.md](briefs/codex-acquire-readiness-gate-brief.md)。
  差し戻しの決め手 2 つ (どちらもコードで実地確認済み):
  - **acquire を閉じても message service latency は上限化されない**。非ゼロ `Resized` は
    message-dispatch 中に `on_window_resized` → `configure_surface` を通り、wgpu-hal DX12 の
    `configure` は `ResizeBuffers` の前に `wait_for_present_queue_idle()` を呼ぶ。その中身は
    **`WaitForSingleObject(event, INFINITE)`** (`wgpu-hal-27.0.4/src/dx12/device.rs`)。
    unconfigure / drop 側にも同じ待ちがある。**§1.31-A が残した inline resize 例外は、
    acquire より手前で無期限 GPU wait に入れる。** 「acquire は最大 1 秒」も強すぎた
    (1000ms は frame-latency wait の timeout であって acquire 全体の上限ではない)。
  - **§1.31-A の後、acquire が実害を起こしている証拠が無い**。§1.30 の実スタックが本当に
    この wait だったかも未確定のまま。`configure` の `INFINITE` / acquire / Present の
    3 候補があり、どれが効いているか不明な状態で 1 つを選んで直さない。
  - 旧 acquire gate 案には他に P0 が 4 件あった (ブリーフ §3 に訂正付きで保存):
    `waitable_handle` は `unsafe` で「swap chain が生きている間だけ有効」→ worker へ raw handle を
    渡すのは未定義動作 (`DuplicateHandle` が要る) / UI と worker を同時 waiter にすると signal の
    取り合いになる / §1.31-A の `surface_generation` は handle generation の正本ではない
    (`RecreateSurface` で増えず、`set_window(None)` は全 surface を clear する) /
    immediate viewport は親の pass 内で自前 surface を acquire するので outer gate では捕まらず、
    gate の実体は `egui-wgpu::Painter` の per-surface seam に要る。
  - **API 到達性の結論は正しかった**: wgpu 27 に `hal` feature は無いが native build では
    `cfg(wgpu_core)` で `wgpu::hal` が re-export され、`Surface::as_hal` も public (`unsafe`)。
    mIV の依存構成で `wgpu/dx12` / `wgpu-hal/dx12` が有効、`wgpu-hal 27.0.4` 一つに解決される。
    **acquire handle へ到達するための wgpu vendoring は不要**。
  - **`NativeEguiOverlay` は別の `wgpu::Instance`** (`src/video/native_presenter/render_core.rs`)
    で既定 `Wait` のまま動く。`src/lib.rs` の設定は届かないので、
    「mIV の acquire 全体を直した」とは言えない。
- **§1.31-A 着手前の 2026-08-16 に訂正した認識**
  (前セッションの読み違い。同じ誤りを繰り返さないため履歴として残す):
  - **外側の paint 位相は現在存在しない**。`check_redraw_requests` は
    [run.rs:198](../vendor/eframe/src/native/run.rs:198) `handle_event_result` の末尾と
    [run.rs:396](../vendor/eframe/src/native/run.rs:396) `new_events` から呼ばれ、
    `WinitAppWrapper` には `about_to_wait` の実装が無い (trait 既定の no-op)。
    不可視窓の direct paint を「既に外側」と読まないこと。
  - **`RepaintNow` は resize 専用ではない**。producer は初回 `resumed` (bootstrap) /
    非ゼロ `Resized` / AccessKit initial tree の 3 つで、`EventResult::RepaintNow(WindowId)` は
    理由を失っている。位相を分けるには reason の型付けが要る。
  - **無限 `WM_PAINT` の懸念は撤回してよい**。winit は `RedrawRequested` を送った後に
    `DefWindowProcW(WM_PAINT)` を呼び、これが update region を validate する。
    eframe が描画せず戻っても再送ループにはならない。
  - **厳密な `MARKER_IN_SIZE_MOVE` 限定は採らない**。winit の非公開状態であり、
    取りに行くと winit の vendor patch か全 HWND subclass が要る。A の inline 例外は
    「非ゼロ `Resized`」とする (現行は全 redraw が inline なので、これでも厳密に狭い)。
- **設計時に確定すべき点 (2026-08-16 に追加で判明したもの)**:
  - **`WM_PAINT` 内の同期 `RedrawRequested` だけでは足りない**。Windows の resize では
    [eframe run.rs:121](../vendor/eframe/src/native/run.rs:121) が `RepaintNow` を受けて
    その場で `run_ui_and_paint` を直接呼んでいる (flicker 対策)。同期 paint の入口は 2 つある。
  - **acquire を短くしても Present 側の上限が未規定**。wgpu-core の acquire timeout は
    `FRAME_TIMEOUT_MS = 1000` 固定 (present.rs)。DX12 backend は frame-latency waitable object の
    `wait(timeout)` が返す `false` を捨てて先へ進む (`unsafe { sc.wait(timeout) }?;` で bool を破棄)。
    Present は FIFO なら interval 1 で `DXGI_PRESENT_DO_NOT_WAIT` 無し。
    **公開 wgpu API の設定だけでは短時間 try-acquire にできない**。lower-layer patch /
    waitable-object の明示 readiness gate / 非ブロッキング Present 戦略のいずれかまで設計に含める。
    なお §1.30 の「wait は本当に無限か」は、コード上は 1000ms 上限側と整合する
    (= デッドロックより飢餓)。ただし実際に観測された `WaitForSingleObjectEx` がこの wait かは
    `latency_waitable_object` の設定次第なので、live evidence の項目は開いたままにする。
  - 通常の frame drop は `Skipped` と**別の outcome** (`DeferredNotReady` 等) にする。
    通常動作なので warning 対象ではなく、damage の保持と readiness wake が要る。
  - `Idle / Painting / Pending` を global scalar にすると複数 viewport の dirty 状態を失う。
    viewport ID / dirty set / 最新 resize / surface 世代を持たせる。
  - drop 後に即 repost すると spin する。waitable signal / event / 明示的 scheduler wake で再 arm する。
  - `WM_PAINT` を即 return する際の `BeginPaint` / `EndPaint` または validation 規則を決めないと
    無限 `WM_PAINT` になる。
  - immediate viewport は UI callback 中に同期再帰する
    ([wgpu_integration.rs](../vendor/eframe/src/native/wgpu_integration.rs) の nested 経路)。
    フレーム全体を単純に `Painting` にすると detached / immediate viewport を塞ぐので、
    state の適用範囲を定義する。
  - dropped frame でも `textures_delta` は配送する (= §1.86 の契約)。screenshot request は
    成功まで保持 / 再投入する。
  - Windows 回帰テストは 2 つの性質を分ける: (a) `WM_PAINT` / 同期 message handler が有限時間で
    返ること (b) presentation 不能中も外側 render step と message-pump の service latency が有限で
    あること。ハング防止に raw `SendMessage` ではなく `SendMessageTimeout` を使い、実 DWM stall では
    なく決定的な readiness gate を止めて再現する。
- 規模 / 優先度: **残るのは §1.31-B の上限制約 = Large / P1 candidate (基盤)**。
  watchdog の穴 (Small / P2) は上記のとおり解消済み。

### 1.35 リネーム移行ジャーナルの部分失敗が再試行されない

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、Codex)。source inspection で確認済み。
- 機構 (仕様判断あり): worker が `panicked` を返したときだけ
  ジョブを `rename_migration_boot_retry` へ戻す。`report.errors` に個別ストアの失敗が
  入った場合はトーストを出すものの、ジョブは完了扱いでジャーナルから消える。一時的な
  DB ロックや I/O エラーだと、編集結果やパス依存設定の一部が旧パスに残ったまま再起動でも
  回復しない。
  - **これは現状のコードコメントが明記している意図的な選択**である
    (「per-store エラーは通常経路と同じ best-effort = 再試行しない」、v2.3.0 角度⑤ 検収時の判断)。
    したがって着手前に「一時エラーと恒久エラーを区別できるか」「再試行の上限をどう置くか」を
    決めること。無条件の再試行は、決定的に失敗するストアで無限ループになる。
  - 移行処理自体は冪等に設計されているので、失敗ストアを保持して再試行する形にはできる。
- 規模 / 優先度: Medium / P2 (仕様判断を先に決める)。もう 1 件あった「UI スレッドでの同期 I/O」は
  v2.10.0 で latest-value worker へ移して close 済み。

### 1.36 ゲームパッドの文字入力ゲートは 3 面の union が必要 (2026-08-01 訂正)

- 前段の誤った前提: viewer 宛なら fullscreen / detached viewport、そうでなければ root viewport
  という排他選択で十分と考えた。しかし gilrs は App が直接 poll するグローバル入力で、
  OS の keyboard routing を通らない。正しい不変条件は **mIV 内のどこかで文字入力中なら止める**。
- 文字入力面は 3 つある。
  1. App root viewport (一覧・ダイアログ)。
  2. App fullscreen / detached viewport (静止画パネル)。
  3. native video / music presenter overlay。これは `egui::Context::default()` で独自 Context を持ち、
     App の `Context::data` からは viewport id を変えても見えない。実機ログでも detached 動画の
     ブックマーク名入力は overlay 側 ROOT (`FFFF`) として観測され、App detached viewport (`39E5`) と別だった。
- keyboard は presenter の `NativeOverlayInputRouting` が `wants_keyboard_input` / `text_input_active` を見て
  App 転送を止めるが、ゲームパッドは presenter を経由しないため以前からこの保護を素通りしていた。
- 対応: root と対象 viewer viewport の IME state を常に union し、さらに presenter が既に公開する
  `NativeOverlayInputRouting::wants_keyboard_input` を既存 output event bus の latest-value snapshot として
  App へ一方向 publish する。3 条件は `gamepad_text_input_active` だけが所有し、呼び出し側へ分散させない。
  presenter Context の統合、presenter→App 直接参照、新しい逆向き channel は行わない。
- 回帰テスト: root composing、viewer composing、presenter `wants_keyboard_input`、全て inactive の 4 状態を
  1 つの predicate test で固定し、snapshot publish が semantic event として App へ漏れないことも固定する。
- 規模 / 優先度: Small / P2。前段差分を本訂正で置換。

### 1.37 トレイ常駐再生まわりの所有境界 2 件

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
  v2.9.1 で「トレイ格納中も再生を続ける」経路を新設したことで、いずれも**この版から初めて
  実際に通る**組み合わせになった。
  1. **P3: 再生継続中に video processor cache を落とす**。
     [tray_integration.rs](../src/tray_integration.rs) の `release_gpu_resources` は
     detached 早期 return より前で `release_idle_pools()` を呼び、その中の
     `processor_cache.take()` は**無条件**である (`hw_frames_pool.clear()` と
     `shared_output_pool.retain(in_use)` は refcount 上安全)。
     ⚠ **呼び出し順自体は v0.9.1 (`287eea9f`, 2026-05-19) からで、v2.9.1 で動かしたわけではない**。
     新しいのは「格納中も再生が続く」ことのほうで、その結果この経路が再生中に走るようになった。
     トレイ格納の瞬間に video processor の作り直しが 1 回入るはずで、実害は小さい。
  2. **P3: 外部 hide 検出が tray 所有権に追随していない**。[app.rs](../src/app.rs) の
     `IsWindowVisible` 追従分岐は `viewer_session_is_detached_or_switching()` だけで heartbeat
     suspension を決め、`sync_media_presenter_visibility_for_tray()` /
     `sync_retained_viewport_visibility_for_tray()` を呼ばない (これらは `hide_to_tray` /
     復帰経路にしか無い)。同フレーム後半の `sync_tray_resident_media_wake()` が**値が変化した
     ときだけ**自己修復するので実害は出にくいが、hide の所有者が 2 つある状態である。
- 規模 / 優先度: どちらも Small / P3。3 件目だった「WM_PAINT ブリッジに背圧が無い」は
  v2.10.0 で claim / ack 構造へ変えて close 済み。

### 1.38 ネイティブ名前ダイアログが `App::update` の内側でモーダルを回す

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
- 何が問題か: [rename_item.rs](../src/ui_dialogs/rename_item.rs) の
  `show_rename_dialog_window` は `loop` の中で `native_name_dialog::prompt_name`
  (`DialogBoxIndirectParamW` = 独自のモーダルメッセージループ) を呼び、バリデーション失敗の
  たびに再表示する。`App::update` は `WM_PAINT` → `RedrawRequested` の同期 dispatch から
  呼ばれるので、**wndproc の内側で無制限長のモーダルが回る**。
  §1.31「UI スレッドが自分のメッセージ処理中に任意時間止まる」構造の一種なので、
  **§1.31 の設計を決めるときに一緒に見る**のが筋が良い。
  なお rename 自体は `rename_item_async` で worker 化されており、ここは**ダイアログの
  モーダル span だけ**の問題である。
- **副次的な退行は対応済み (2026-08-01)**: 既存 `rename_pending` を左下の進捗 overlay に接続し、
  シェル rename 実行中の「変更中...」表示を復帰した。モーダル構造は変更していない。
- 規模 / 優先度: Medium / P2 (§1.31 に合流)。残るのはモーダル span の構造だけ。

### 1.39 `fs_page_load_state` が PDF ページで毎フレーム String を作る (`9590b661` の続き)

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
- 機構: `has_retained_pdf_final_ai_for_current_params` → `retained_pdf_page_key_for` →
  `metadata_cache_key(idx)` が String を生成し、HashMap を引く。
  ⚠ **レビュー報告より実コストは小さい**: `effective_params` / `final_ai_key_for_pixels` は
  retained cache に**該当エントリがあるときだけ**走る (無ければ `?` で抜ける)。
  非 PDF は `items.get` + `matches!` で即 return なので無コスト。
  とはいえ PDF フルスクリーンでは `update_prefetch_window` の current、prefetch ターゲット
  全件、連結読み keep 範囲ループ全件、見開きパートナーから毎フレーム走るため、
  key を 1 回作って使い回す形にできるかは見ておく。
- 規模 / 優先度: Small / P3。

### 1.42 右クリックメニューの表示が遅い (シェル拡張を UI スレッドで同期ロード)

- 出典: 2026-08-02 の v2.10.0 実機確認中の利用者報告。「右クリックメニューを開くのが
  かなり遅い」ため、連続リネームのテストが現実的に行えなかった。
- 壊れている前提: [native_context_menu.rs](../src/native_context_menu.rs) の
  `query_shell_context_menu` は `IContextMenu::QueryContextMenu` を**メニュー構築時に
  同期実行**する ([native_context_menu.rs:288](../src/native_context_menu.rs) と
  [872](../src/native_context_menu.rs))。同 module に worker は無く、**UI スレッドで走る**。
  `QueryContextMenu` はサードパーティのシェル拡張 (ウイルス対策 / クラウドストレージ /
  書庫ソフト等) を列挙してロードするため、環境によっては数百 ms〜秒単位かかる。
- 「Windows のせい」は半分正しい。遅いのは拡張側だが、**それを UI スレッドで待っているのは
  mIV 側**であり、その間アプリ全体が固まる。CLAUDE.md「UI スレッドでの同期 I/O は即
  worker 化する」の対象。
- 未計測: 実際に何 ms かかっているか、どの拡張が支配的かは未測定。**着手前に計測すること**
  (`perf::event` を `query_shell_context_menu` の前後に入れる)。環境差が大きいので、
  遅い環境のログが取れると判断が早い。
- 対応案 (計測してから選ぶ):
  1. **シェル部分を遅延構築**する。mIV 自身のメニューを即座に出し、シェル項目は
     サブメニューを開いた時点で初めて `QueryContextMenu` する。エクスプローラー互換の
     見た目からは離れるが、体感は最も改善する。
  2. mIV のメニューを先に表示し、シェル項目だけ**非同期に差し込む**。項目が後から
     増えるので、開いた直後にクリックすると位置がずれる問題を設計で潰す必要がある。
  3. 直近で使った拡張セットを**キャッシュ**する。COM オブジェクトの寿命と、
     拡張の追加 / 削除の検知が要る。
- ⚠ **症状パッチにしないこと**: タイムアウトで打ち切って一部の拡張を落とす、は
  「どの拡張が出るか環境ごとに変わる」という新しい非決定性を持ち込むので採らない。
- 完了条件 / 回帰テスト:
  - 右クリックからメニュー表示までの UI スレッド占有時間を計測し、改善を数値で示す。
  - シェル項目の実行 (`InvokeCommand`) が従来どおり動く。
  - 拡張が 1 つも無い環境でも従来と同じメニューが出る。
- 規模 / 優先度: Medium / P2。実害は「操作のたびに待たされる」で、データ喪失は無い。

### 1.47 動画の拡大縮小を mIV のシェーダで行う — 設計確定 / 未実装

- **これが入ると 1.112 (360 度動画) の前提が揃う。**投影を差し込む場所を作るのがこの項目であり、
  正距円筒投影は同じシェーダステージ上のもう 1 枚として載る。設計を変えるときは 1.112 も見る。

**正本は [video-upscale-shader-plan.md](video-upscale-shader-plan.md)。** 本項は要約と着手判断のみ。

§1.46 の動画版。**GPU 性能ではなく動画表示の構造が障害**だったので別案件として扱ってきたが、
2026-08-07 に構造・測定方式・UI まで設計を確定した。

- 現状: native presenter は **swap chain を動画解像度で作り**、`CopySubresourceRegion` で 1:1
  コピーし、**`IDCompositionVisual::SetTransform2` で拡大縮小している**。つまり拡大しているのは
  mIV ではなく DWM / DComp で、mIV のシェーダは通っていない (色補正が identity でないときだけ
  grade シェーダが走るが、それも動画解像度で動く)
- 採る構造: **swap chain を「映像の表示矩形の物理ピクセルサイズ」にし、シェーダでソース解像度
  から表示解像度へ直接解決する**。`compute_video_visual_transform` はサーフェスサイズを引数に
  取る作りなので**無改造で正しい**。リサイズ中は差し替えず既存サーフェスを DComp に伸ばさせ、
  静止後に 1 回だけ差し替える (= 全画面再生では差し替えが一度も起きない)
  - 当初案の「swap chain をウィンドウ解像度にする」は黒帯まで描くので上位互換の表示矩形サイズを
    採る。「動画解像度 × 2 の整数倍」も却下 (mIV の Anime4K / NIS は任意倍率へ直接解決できるため、
    整数倍に縛ると 1〜2 倍の中間倍率で情報を捨てる)
- **Phase A (標準 Lanczos3 / シャープ NIS / ニアレスト + 縮小)** と
  **Phase B (Anime4K)** に分ける。Phase A は 4K 出力で +0.4〜0.9ms と実質タダで全解像度に使え、
  **4K 動画をウィンドウで見たときの縮小モアレも同時に直る**。重いのは Anime4K だけ
- Phase B は変種 (S/M/L/VL/UL) を扱うため、**現行の VL 専用ハードコードを表駆動へ一般化**する
  必要がある ([gpu_anime4k.rs](../src/gpu_anime4k.rs) と
  [convert_anime4k_glsl_to_wgsl.py](../scripts/convert_anime4k_glsl_to_wgsl.py))。一般化すれば
  静止画側にも同じ選択肢が生える
- モデル選択は **利用者が Anime4K を選んだ瞬間にモデル × ソースサイズを実測して表を作り、
  以後は表引きで決める**。GPU 性能は 4090 とノート iGPU で 10 倍以上違い、解像度やフレーム
  レートからの固定しきい値では当てられないため。測定結果は GPU + ドライバ版をキーに永続化し、
  回復手段は再測定ボタン。**再生中はモデルを変えない** (自動昇降格はハンチングし、切替のたびに
  リソース再構築が走るため)
- 最重要の不変条件: **切り替えの瞬間にシェーダのコンパイルもテクスチャ確保も一切しない**。
  現行 grade pipeline はレンダースレッドで同期 `D3DCompile` しており、同じ作りで Anime4K UL
  (25 本 + 中間 24 枚) をやると設定を触った瞬間に数百 ms〜秒単位で固まる
- UI は動画左パネル →「画像補正」→「フィルタ」タブに置く (Creative LUT の隣)。制限で実行され
  ない場合は静止画と同じ `processing_size_outside_note` の書式で選択肢直下に出す
- VSR は [video-architecture.md](video-architecture.md) で**スコープ外と決定済み**なので、
  先例として使えるものが無い
- 性能の数字は**すべて静止画実測からの外挿**であり、これを根拠に既定値やしきい値を決めない。
  測定機構自体が「1080p VL が現実的か」を利用者ごとに答えるためのものである
- 規模 / 優先度: **Phase A = Large / P3 (約 2 週間、測定を待たずに着手可)**、
  **Phase B = Large / P3 (約 2 週間、Phase A の後)**。いずれも単独リリースで実機検証を厚く取る
  規模。detached リワーク ([detached-rework-plan.md](detached-rework-plan.md)) と presenter を
  共有するので、着手時期はリワークの進捗と調整する

### 1.59 360 度ビューに等距離魚眼投影を追加する (提案・採否判断が要る)

- 出典: 利用者メール (pattier、2026-08-06)。「現在の透視投影に加えて等距離魚眼投影があると、
  視野を引いたときに 360 度カメラの絵に近い見え方ができる」。
- 現状: [panorama_wgpu.rs](../src/panorama_wgpu.rs) のシェーダは透視投影固定
  (`tan_half = tan(fov_y * 0.5)` でカメラ方向を作る)。この式は原理的に 180 度へ近づくと発散するため、
  引いた画角そのものが表現できない。
- 見込み: シェーダ側は数行 (半径 → 角度を線形に対応させ、`sin/cos` で方向ベクトルを作る)。
  作業の本体は投影モードの uniform 追加、設定への永続化、切り替え UI / キー割り当て、
  画角上限の再定義 (魚眼なら 180 度超も扱える)、ドキュメント。
- **どの魚眼かを先に確かめること** (2026-08-07 調査): 「魚眼」には複数の写像がある。
  半径 `r` と入射角 `θ` の対応が、透視 `r=f·tan θ` / 立体射影 `r=2f·tan(θ/2)` /
  等距離 `r=f·θ` / 等立体角 `r=2f·sin(θ/2)` と異なるだけで、**シェーダ上はどれも 1 行の差**。
  利用者の言う「引いたときに 360 度カメラっぽい絵」は、周辺の伸びが最も穏やかな
  **立体射影 (いわゆるリトルプラネット)** の可能性が高い。krpano も little planet は
  stereographic を使う。等距離はレンズの物理仕様としての標準表記で、見た目は中央が
  やや膨らむ。**どちらか一方ではなく、方式を選ぶ形にするのが素直** (実装コストがほぼ同じため)。
- 判断が要る点: 投影方式を増やすと画角スライダの意味と上限が方式ごとに変わる。既定は透視のまま、
  切り替えを提供するのか、パノラマ設定に持たせるのかを先に決める。
- ~~**先送り (2026-08-13、利用者判断)**。提案として妥当だが、v3.0.0 では扱わない。
  却下ではないので、要望が重なったら再評価する。~~
- **実装済み (2026-08-27、レーン C、branch `panorama-projection`)**。着手可否を利用者へ再確認し、
  4 方式すべてを入れる / 既定は透視のまま / 切り替えはキー + 360 表示中の上バーのボタン、で確定。
  **仕様の正本は [panorama-360-view-plan.md §13](panorama-360-view-plan.md)**。要点:
  - 視野角の意味を全方式で共通化 (画面上下端の入射角 = `fov_y / 2`)。透視は導入前の式と恒等で、
    既定の見え方は変わらない (格子点での回帰テストあり)。
  - 画角上限は方式ごと。透視 = 約 149° (従来と同値)、非透視 = 約 340°。方式を戻すときに
    `clamp_fov` を通すので、広げた画角のまま透視へ戻しても発散しない。
  - 等距離 / 等立体角は広画角で画面隅が定義域を出るため、そこは不透明の黒 (魚眼のイメージ
    サークル外)。WGSL は seam 勾配の uniform control flow を守るため早期 return せず、最後の
    色選択でだけ判定する。
  - settle overlay の stale 判定キーを `PanoPose` へ型化 (投影方式の比較漏れを構造で防ぐ)。
  - **実機確認の状況** (2026-08-27):
    - 確認済み: 既定 (透視) の見え方が導入前と同じに見えること、4 方式で見え方が変わること、
      上バーの投影一覧が画面内に収まること。
    - **未確認**: 各方式が幾何的に正しいか。利用者が 4 方式を見比べた時点では違いを判別
      できなかった。**これは実装の問題ではなく、普通のパノラマ写真では原理的に判別
      できない**ため (差は周辺と広い画角にしか出ない)。判別用のチャートと手順を
      [panorama-360-view-plan.md §13.7](panorama-360-view-plan.md) に用意したので、
      次に触る人はそれで確認する。特に「水平を向いて画角を最大まで引くと、赤道が
      直線になるのは透視だけ」が最も差が出る。
- 規模 / 優先度: Medium / P3 (実装済み)。
- **次の一手 (§1.112)**: 数式・分岐位置・uniform の持ち方はここで確定した。動画側は
  `panorama.rs` の写像表と WGSL の `projection_theta` をそのまま実行時 HLSL へ移す。

### 1.62 お気に入り編集を開いている間、進捗が動いていなくても 100ms ごとに repaint する

- 出典: v2.13.0 の idle health 測定中に判明 (2026-08-11)。退行ではなく既存仕様。
- 観測: `favorites_editor.rs:752` が `ctx.request_repaint_after(100ms)` を**無条件で**呼ぶため、
  ダイアログを開いている間ずっと 11〜12 fps で `App::update` が回る。perf log で
  `prev_frame_causes=['src\\ui_dialogs\\favorites_editor.rs:752']` が連続して確認できる。
- 実害: 利用者は**起動後の索引作成が終わるのを確認するためにこのダイアログを開いたままにする**
  運用をしており、索引作成は 5 分ほどかかる。その間ずっと起きている。ノート PC やタブレットの
  電池には無視できない。v2.13.0 でタッチ対応を入れた以上、タブレット運用は増える。
- 現行コードのコメントは「active が空でも notify-rs が動き出した瞬間に拾えるよう常に呼ぶ」と
  理由を書いており、**意図的**である。直すなら意図を保ったまま頻度を落とす:
  - active が空の間は 100ms ではなく 500ms〜1s へ落とす (動き出しの検出遅れは許容範囲)
  - または watcher 側から `ctx.request_repaint()` を呼び、ポーリング自体をやめる (構造的)
- 同型の確認: 他にも「進捗表示のために無条件 `request_repaint_after`」を持つダイアログが
  無いか探すこと。あれば同じ方針で揃える。
- 規模 / 優先度: Small / P2。

### 1.69 変換対象アーカイブのキャッシュパスが未解決のとき、ZipDir タイルだけ黙って失敗し得る

- 出典: §1.66 の修正 (`ff56abea`) の周辺を洗って見つけた。**利用者報告ではなく、
  実際に到達するかも未確認**。今回の不具合ではなく以前からある形。
- 形: `converted_archive_cache_paths` (= 変換キャッシュ ZIP へのパス表) は worker が
  非同期に作るので、表が空の窓がある。この窓での振る舞いが item 種別ごとに違う。
  - `GridItem::ConvertibleArchive`: `?` で **LoadRequest を作らない** → 後のフレームで
    再要求される。安全。
  - フォルダピン経由: `ff56abea` で「表が届いたら依存タイルだけ組み直す」ようにした。
  - **`GridItem::ZipDir`: `unwrap_or(zip_path)` で元アーカイブのパスへ落ちる**。RAR は
    `zip_loader` が `rar_loader` へ振り分けるので読めるが、**7z / ソリッド RAR は ZIP として
    開こうとして失敗**し、そのサムネイルは失敗のまま残る。
- 到達性: コードのコメントによると、`ZipDir` が元 RAR/7z/LZH のパスを持つのは
  **レーティング一覧に復元されたとき**。実際にその状態を作って確認していない。
- 方針: **まず到達するかを確かめる。** 到達しないなら、3 経路で fallback の作法が違うこと
  自体をコメントで明示して閉じる。到達するなら `ff56abea` の依存追跡
  (`pin_archive_dependencies`) を ZipDir にも広げる。**確認前に直さないこと。**
- 規模 / 優先度: Small / P3。

### 1.79 比較表示中の元画像表示 (押している間だけ両側を元画像にする) — 利用者要望

- 出典: 利用者実機確認 2026-08-13。比較 (ワイプ) 表示中に元画像表示 (`FsOriginalPreviewHold`、
  既定 RightCtrl) が効かない、という報告。**期待する挙動は「ワイプはそのままに、ワイプ元と
  比較対象の両方が元画像になる」** (利用者判断)。片側だけ元画像にしたり、比較をやめて
  元画像 1 枚を出すのは求められていない。
- **現状 (source inspection、2026-08-13)**: 比較表示中は**全モードで**元画像表示が効かない。
  `compare_requested` は `compare_view_mode != Off` だけで決まり、
  `CompareFramePrimaryDraw::resolve` は元画像表示を入力に取らない
  ([ui_fullscreen.rs:11708](../src/ui_fullscreen.rs:11708))。今回の Ctrl まわりの変更以前からこう。
- **実装コストは Medium。小さくない。** 比較の合成は加工済みピクセルから作られており、
  「元画像版の対」を新たに用意する必要がある。
  1. **prepared pair は非同期 worker が作る**。identity は
     `(current_idx, pinned_source_idx, output)` なので、**加工済み / 元画像の次元を足す**必要がある。
  2. **比較対象 (pin した側) は、pin した時点の加工済みピクセルの snapshot** (`slot.pixels`) で
     保持している ([ui_fullscreen.rs:22982](../src/ui_fullscreen.rs:22982))。元画像版は存在しないので、
     **pin 時に未加工の snapshot も持つ (メモリ倍) か、押された時点で再デコードする (遅延)** かの
     選択が要る。再デコード対象は現在ページとは限らず、`fs_cache` に無いこともある。
- 方針の案:
  - 元画像版の対は**押されたときに遅延生成**し、準備できるまでは**加工済みの比較を出したまま**にする
    (途中で別の絵に切り替わるちらつきを作らない)。既存の `WaitingForSource` / 「比較表示を準備中」
    の枠組みに乗る。
  - 一度作った元画像版は、対の identity が変わるまで保持して 2 回目以降を即時にする。
- 着手時に読む: 上記 `ensure_compare_prepared_pair` / `ComparePrepareOutput` / `pinned_compare_slot`。
- 関連: 線を消す修飾は左 Ctrl 限定にしてあるので (`88b028e1`)、既定 RightCtrl の元画像表示とは
  物理的に競合しない。この項目を入れても修飾キーの整理は不要。
- **当面の決着 (2026-08-13、利用者判断、`42212e9d`)**: 実装コストが高いので、
  **比較表示中は元画像表示を明示的に無効**にした。有効なままだと画面は変わらないのに
  「比較表示を準備中」トーストが出るなど副作用だけが起きるため、`original_preview_active`
  の入口で断っている。本項目を実装するときはこの早期 return を外す。
- 規模 / 優先度: Medium / P3 (比較表示は加工結果を見比べる機能で、元画像表示が効かないことが
  データを壊すわけではない)。

### 1.95 OS 側でコピー・移動したファイルへ編集内容を引き継ぐ — 残りは同計画の未着手分

- 正本: [edit-content-identity-plan.md](edit-content-identity-plan.md)。
  Phase 1 (A1-A6) は v3.3.0 で出荷済み。
- **残り**: 同計画の「未着手」節 — アプリ内コピー / ページ移動の同じ失効漏れ、
  復元を通っていないファイルの注釈サムネイル。
- 規模 / 優先度: 小〜中 / P3 (実害は余計な確認ダイアログ。データは失われない)。

### 1.102 YouTube 型の精密シーク用サムネイル列 — 専用スレ >>271 / >>277

> **設計の正本は [video-seek-strip-plan.md](video-seek-strip-plan.md) へ移した** (2026-08-23)。
> 以下は着手前の記録。決定事項 (等幅キーフレーム軸 / キーフレームのみ抽出 / ラッチ開閉 /
> 本体のみ) は同書 §2 を見る。

- **1.113 (音声波形ストリップ) と同じ場所を使う。**ストリップの導線 (シークバーから上へドラッグ) を
  共有し、中身をサムネイル列と波形で切り替える前提で設計する。片方だけ先に作らない。

- 出典: 専用スレ >>271 (2026-08-20)。シークバーへサムネイルを並べたい要望。
- **意図確認済み (専用スレ >>277)**: YouTube でシークバーを上へドラッグしたときに出る、
  動画を見たまま複数の場面を横一列で選べる「精密シーク」相当が希望。常時表示する
  サムネイル付きシークバーではない。
- 既存機能との境界:
  - `S` / 上部 HUD のタイルボタンで、動画全体の場面を複数タイル表示してシークできる。
  - `B` の動画ブックマークは、左パネルへサムネイル・時刻・名前を並べる。
  - シークバー hover では、その位置のプレビューを 1 枚表示する。
- YouTube デスクトップ版の観測 (2026-08-20、実装仕様の公式保証ではなく参考値):
  - シークバーから上へドラッグすると、160x90px 程度のサムネイル列を現在位置中心に表示し、
    列を横へ動かすことで動画全体をたどる。通常幅では約 5〜6 枚、広い画面ほど表示枚数が増える。
  - ストーリーボードはキーフレーム列ではなく固定時間間隔。観測例は 3:33 = 2 秒間隔 / 108 枚、
    8:14 = 5 秒 / 100 枚、20:17 = 10 秒 / 123 枚、2:00:53 = 10 秒 / 727 枚。
  - YouTube は事前生成済み JPEG sprite を配信するため、ローカル動画からその場で作る mIV と
    抽出コストの前提が異なる。見た目だけをそのまま真似て全時間分を先に生成しない。
- mIV での UI 方向:
  - シークバーから上方向へ一定量ドラッグしたときだけ、下部 HUD 上へサムネイル列を開く。
    中央の時刻を選択位置とし、左右ドラッグで列を送る。固定シークバーでも通常時は列を隠す。
  - 初期生成は画面に見える枚数 + 少量の前後だけとし、移動方向へ逐次追加する。動画全体の
    サムネイル生成完了を待ってから表示する構造にはしない。
  - 既存 `tile_thumb_cache`、seek hover cache、タイル抽出 worker のどこまでを共有できるか確認する。
- **抽出方式は未決定 (性能との妥協を追加調査する)**:
  - 表示時刻どおりの任意フレームは、素材によって 1 枚約 1 秒かかる。一方、直前のキーフレームを
    そのまま採る場合は約 50ms。既存 §1.104 の実測でも精密側は最大約 1 秒、キーフレーム近傍は
    約 40〜80ms で、GOP 距離が支配項と確認済み。
  - キーフレームだけを列にすると高速だが、GOP によって時刻間隔が不均一になり、長い GOP では
    内容が粗くなる。最終 UI をキーフレーム列に固定するとはまだ決めない。
  - 比較候補は、(A) 固定間隔を精密復号、(B) キーフレームのみ、(C) キーフレームを先に表示して
    固定間隔の精密画像へ非同期差し替え、(D) 表示範囲の先頭側キーフレームへ 1 回 seek して
    順方向にまとめて復号、の 4 方式。素材 / GOP / HW decode 別に初回表示時間、列を送ったときの
    待ち、CPU / GPU 負荷を測って決める。
  - サムネイル画像が近似時刻でも、クリック後の本編 seek は選択した表示時刻へ行う。近似画像と
    ラベル時刻のズレをどこまで許容するかは §1.104 の設定と共有するかも含めて判断する。
- 規模 / 優先度: Medium〜Large / P3 (抽出方式の計測・設計後に着手)。

### 1.105 動画のフルスクリーン上部バーにも「…」オーバーフローを出す — 利用者要望

- 出典: 利用者要望 (2026-08)。静止画のフルスクリーン右上に「…」を追加したので、動画にも
  同じものがあると UI として揃う、という指摘。
- 現状: [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `overflow_available` が
  `!state.is_video && !is_music_view && !panorama_mode_active_now && ...` で、
  **画像のときだけ**「…」を出す。動画・音楽・パノラマでは出ない。
- 仕様の論点: 動画の上部バーには既に動画専用ボタン (タイル一覧 / Perf グラフ 等) が並ぶ。
  「…」に何を入れるかを決める必要がある。
- **1.104 とは切り離す (2026-08-20 決定)**。「ズレ許容」をフルスクリーン中から変えられると
  体感を確かめながら調整できるが、動画側の上部バーへ overflow を通す修正量が大きい。
  1.104 の初回リリースには**含めない**。設定は環境設定からのみ変更できる形で出す。
  報告者へもこの件は伝えていないので、実施を約束したものとして扱わない。
- 規模 / 優先度: Medium / P3。

### 1.106 リングショートカット / マウスジェスチャを左クリックで取り消す — 利用者要望

✅ 実装済み (2026-08-22)。グリッドセル / グリッド背景 / active viewer / passive detached
viewer の 4 開始面で、進行中の右ドラッグを左ボタン press から既存 cancel helper へ流す。
active 面では対応する左 release と右ボタンの後続 release を通常 click として再利用しない。
passive detached は通常どおり release で窓を activate するが、選択 / open / viewer action には
再利用しない。固定 mouse chord の理由は `keymap-spec.md` に記録した。

- 出典: 利用者要望 (2026-08)。右ドラッグで開始した後にやめたいとき、取り消す手段がない。
  現状の回避は「リングは中央の円へ戻して離す」「ジェスチャは割り当てのない軌跡にする」。
  提案は**右ドラッグ中に左クリックを押すと取り消し**。
- 実装の見通しは良い。取り消し自体は既にあり ([ui_main.rs](../src/ui_main.rs) の
  `cancel_mouse_ring_flick` / `cancel_mouse_gesture` をダイアログが開いたときに呼んでいる)。
  足すのは発火条件だけ。
- 確認すべき点:
  - 開始点は 3 か所。[ui_main.rs](../src/ui_main.rs) に 2 か所 (グリッドと、もう 1 か所)、
    [ui_fullscreen.rs](../src/ui_fullscreen.rs) に 1 か所 (フルスクリーン / native 動画)。
    **同型の入口をすべて塞ぐ** (片方だけ直すと「一覧では取り消せるがフルスクリーンでは効かない」になる)。
    タッチからの開始経路は無い (右ドラッグのみ。`touch_input.rs` / `touch_correlation.rs` に
    ring / gesture の参照が無いことを確認済み)。
  - 取り消しに使った左クリックが、離した時点で通常のクリック動作 (選択 / 開く) として
    発火しないこと。リング / ジェスチャが active な間は左ボタンの press と release を
    両方消費する必要がある。
  - 取り消し自体は既に多くの場所から呼ばれている (gamepad / native_video / app / fullscreen /
    main)。追加するのは左ボタン押下という発火条件だけで、取り消し処理は既存のものを使う。
- 規模 / 優先度: Small / P2。

### 1.107 Z 照準のカーソル写像を「画面帯」から「実際に描いている画像領域」へ — 利用者要望
 ✅ 実装済み (2026-08-21)

> 実装は [briefs/z-aim-cursor-mapping.md](briefs/z-aim-cursor-mapping.md)。写像と描画は
> `ZAimBasis` を 1 つ共有し、basis は **実際に描画しているスケール**を持つ (単ページは Z 中に
> contain を強制するので contain、見開きは表示モード由来の `fit_scale`)。見開きのカーソル写像も
> 同時に直った (それまで描画位置でない contain 矩形へ写像していた)。
> **実機確認済み (2026-08-21)**: 意図どおりの動作。以下は着手前の記録。

- 出典: 利用者報告 (2026-08)。3 パターンの切り分けまでいただいた。
  1. 上下左右に余白がない画像 → 枠とカーソルのズレは小さい (あっても枠内)
  2. 縦長画像 (左右に余白) → 上下端への枠移動は 1. と同じだが、**左右端へ動かすには
     ウィンドウ端付近までカーソルを運ぶ必要があり、左右方向に大きくずれる**
  3. パノラマ画像 (上下に余白) → 2. と逆で、左右は違和感なく、**上下のズレが大きい**
- 原因: [displayed_image_transform.rs](../src/displayed_image_transform.rs) の
  `z_cursor_image_px` が **`pan_band` (画面側の帯) 内の比率**で画像座標を決めている。
  帯は viewport 全体から上下の HUD 分だけ詰めたもので、**画像の縦横比を見ていない**。
  一方、照準枠を描く `z_aim_frame_rect` は `view_rect` に縦横比を保って収めた
  `content_rect` を基準にしている。**写像と描画で基準が違う**ので、余白が大きい方向ほど
  カーソルと枠が離れる。報告の 3 パターンはこの差でそのまま説明できる。
- 決定 (2026-08-20): **写像の基準を `content_rect ∩ pan_band` にする**。
  - `content_rect` = いま実際に描いている画像領域 (`view_rect` に縦横比を保って収めたもの)。
    `z_aim_frame_rect` が既に使っているものと**同一の値を共有する** (別々に計算すると
    また乖離する。共通 helper へ出す)。
  - `pan_band` との交差を取るのは、上下の HUD ホバー帯へカーソルが入る前に画像の上端・
    下端へ到達できるようにするため (実機 FB 2026-06-21 で入れた既存の意図)。これは維持する。
  - 効果: 縦長画像では横方向が画像領域基準になるので 2. が解消。パノラマでは
    `content_rect` が帯の内側に収まるので交差が `content_rect` そのものになり、3. も解消。
    1. は元から差が小さく、変化もほぼ無い。
  - 縮退: 交差が空、または極端に細い場合 (小さいウィンドウ + 大きい HUD 余白) は
    従来どおり `pan_band` を使う。狙えなくなる状態を作らない。
- **同型の入口**: [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `zip_cursor_image_px`
  (連結表示 / 見開き合成) が**同じ式の複製**になっている。片方だけ直さない。
  可能なら 1 つの helper に寄せる。
- カーソル非表示 ([ef9d8b0b](../src/ui_fullscreen.rs)、Z を押している間は隠す) とは独立。
  この写像を直した後にカーソルを出す方が自然かどうかは、実機で見てから判断する。
  当面は非表示のままでよい。
- 回帰確認:
  - 既存テスト `zip_cursor_image_px_maps_band_to_image_and_clamps` と
    `zip_pan_band_reaches_image_edge_before_top_hover_zone` は前提が変わるので更新する。
    後者が守っている「上部ホバー帯へ入る前に画像上端へ届く」性質は**新しい写像でも維持する**
    ことをテストで明示する。
  - 縦長・パノラマ・余白なしの 3 形状で、カーソル位置と照準枠の対応を確認する。
  - トリム表示中 (`content_bbox` あり) と回転ページでも枠と写像が一致すること。
- 規模 / 優先度: Small〜Medium / P2。

### 1.108 開くときは黒地、切り替えるときは前の画像 — 表示先の占有を routing 境界で判定する

- 出典: 2026-08-20 の利用者報告 (グリッドで PDF をダブルクリックすると、1 ページ目が遅いときに
  無反応に見える) と、それに続く仕様整理。
- **確定した規則 (利用者判断)**: **表示先に既に中身があるか**で分ける。
  - **A (中身が無い)**: 新しいウィンドウが開く / F12 デタッチで新規窓 / 同一ウィンドウ内で
    ファイルを開く → **黒地を挟んでフルスクリーンへ移る**。開く操作の結果が即座に返る
  - **B (中身がある)**: Ctrl+↑↓ / メイン側から既存 detached を切り替え → **前の画像を保持**。
    黒を挟むとちらつく
  - どちらも待ちが 500ms を超えたら中央に「読み込み中」(v3.1.3 で実装済み)
- **却下された導出**: `FsNavigationSequence::previous.is_none()` では分離できない。詳細と
  全経路の分類は [docs/briefs/fs-open-black-vs-holdover.md](briefs/fs-open-black-vs-holdover.md)
  の「第 1 段階の結果」。要点:
  - `previous` は「直前の表示単位を texture として捕捉できたか」であり、表示先の占有ではない
  - **A の production 経路のほとんどに navigation sequence が無い** (`open_fullscreen` を直接通る)
  - **B の多くにも sequence が無い** (Home/End、スタック Shift+↑↓、連結読みのシーク、
    スライドショー、native 動画の前後移動、passive snapshot クリック、remote 復元 等)
  - 反例: グリッド由来・`from_explicit_open == true` でも、linked detached の既存窓を
    更新するなら B
- **正しい判定地点**: teardown 後の `open_fullscreen` ではなく、**表示先の surface / context を
  選ぶ routing 境界**。`ViewerPresentation`、viewer session、detached runtime state、
  mounted bundle の `fullscreen_idx`、native presenter の owner を見る必要がある。
- **リワーク R2 (状態の集約) が所有する領域と重なる。**独立した作業として着手しない。
  `previous.is_none()` を viewport 側へ足す実装は §2 の症状パッチに当たる (Codex 判定)。
  条件を増やす形の部分対応も、単一の所有者を持たない routing 判断へ条件を足すことになるため
  採らない (利用者判断)。
- 現状: 中央の「読み込み中」により**無反応に見える問題は解消済み**。残るのは
  「グリッドが見えたまま待つ (黒地にならない)」という見た目の差のみ。
- 規模 / 優先度: 中〜大 / P3 (実害は見た目。R2 で routing の所有者が決まってから)。

### 1.110 別ウィンドウの題が、表示ページ不定のときに一覧の先頭ファイル名になる

- 出典: 2026-08-20、keep-alive backstop の所有権修正 (`detached-rework-plan.md` §11) の
  フェーズ 1 調査で Codex が指摘。**その修正とは別件なので、同時に直さず切り離した** (憲法 7)。
- 機構: detached の viewport builder は題の index を
  `self.fullscreen_idx.unwrap_or(0)` で決める ([ui_fullscreen.rs:12791](../src/ui_fullscreen.rs:12791))。
  **表示ページが定まっていない正規のギャップ** (列挙待ち、パスワード待ち、フォルダ移動の途中など) では
  `None` になり、**その一覧の先頭ファイル名**が題になる。
- 症状: 一瞬だけ、開いているものと無関係なファイル名がタイトルバーに出る。
  backstop の所有権修正で「別 context の名前が出る」方は消えたが、**こちらは残る**
  (同じ context の中で、たまたま先頭のファイル名が出る)。
- 直す方向: **`unwrap_or(0)` をやめる。** 表示ページが定まっていないことは「先頭ページ」ではない。
  ギャップ中は直前に表示していた題を保つか、アプリ名だけにするかを決める。
  **`Option` を握りつぶさずに、ギャップを型で表す**のが筋。
- 同型の入口: 静止画スナップショット窓の題
  ([app.rs:40534](../src/app.rs:40534) 付近の `detached_image_snapshot_title_for_idx`) も
  `items.get(idx)` が空振りしたとき `"mimageviewer"` に落ちる。こちらは先頭へ落ちないので
  挙動は穏当だが、**ギャップの表し方を揃えるなら一緒に見る**。
- 規模 / 優先度: 小 / P3 (実害は一瞬の見た目。所有権の方が先)。

### 1.111 フルスクリーンで動画へ入る瞬間に前面を失い、押しっぱなしのキーが他アプリへ流れる — 利用者報告

- 出典: 利用者報告 (2026-08-21、v3.1.2 で確認)。
  - 全画面モードで静止画を表示し、**上下キーを押しっぱなしで高速送り**している最中、
    次が動画だと**一瞬ウィンドウが消え、フォーカスが別のウィンドウへ移り**、押しっぱなしの
    上下キーがそのウィンドウへ入力される。
  - **ウィンドウモード / 別ウィンドウモード / 別ウィンドウのフルスクリーンでは起きない。**
  - 併発: ランダムに**カーソルが mIV 上で非表示のまま**になる。次の画像 / 動画へ移ると戻る。
- 原因の見当 (コード確認済み、未確定):
  - フルスクリーン表示では静止画が **egui フルスクリーンビューポート**、動画が
    **別 HWND の native presenter** (D3D11 + DirectComposition)。静止画→動画の遷移で
    前者を `ViewportCommand::Visible(false)` で隠し ([ui_fullscreen.rs:12739](../src/ui_fullscreen.rs:12739))、
    後者を別途 materialize する。
  - **この症状は既に疑われていて計装がある**。同じ場所の直前に
    `log_main_flash_probe("fs_visible_false", …)` ([ui_fullscreen.rs:12735](../src/ui_fullscreen.rs:12735))。
  - [native_video.rs:2842](../src/app/native_video.rs:2842) のコメントが症状そのものを書いている:
    「foreground 奪還まで 80ms 待つと、**その間だけ外部ウィンドウが見えることがある**」。
  - つまり **viewport を隠してから presenter が foreground を取るまで、mIV のどのウィンドウも
    前面を持っていない**。Windows は z-order 上の次のウィンドウへ前面を渡すので、キーリピートが
    そちらへ流れる。報告と一致する。
  - 他モードで起きない説明も付く。ウィンドウ内動画はメインウィンドウの子として扱われ
    (`set_in_window_video_child`、[native_window_host.rs:536](../src/video/native_window_host.rs:536))、
    専用フルスクリーンビューポートと presenter HWND を入れ替える構造になっていない。
  - **カーソル非表示の固着も同根**。`cursor_hidden` はラッチで、解除は (a) egui フルスクリーン
    フレーム内で観測したポインタ操作、(b) HUD が出て `clean` が false になったとき、の 2 つだけ。
    presenter 側は `update_cursor_icon` で別経路の解決をしており、**カーソルの所有者が 2 つある**。
    次の項目へ移ると直るのは、そこで `open_fullscreen` がラッチを落とすため。
- **2026-08-27 追試: 再現しなかった (利用者、v3.3.0 開発版)。** 単一ウィンドウの通常
  フルスクリーン、動画は全画面再生、背後にエディタを置いて ↓ 押しっぱなしで高速送り。
  **動画の再生がすぐ始まり、背後のエディタへキーは 1 つも入らなかった。**
  Windows のキーリピートは初回遅延の後およそ 30ms 間隔なので、1 つも漏れないということは
  前面の空白がほぼ無いことを意味する。
  - main window の cloaking (`native_video_main_cloaked`) は `f2215fd2` (2026-05-13) で
    **v3.1.2 に既に入っている**ので、これが後から効いたわけではない。
  - 未確定なもの: (a) v3.1.2 以降のどこかで解消した (b) より遅く開く動画 (大きい 4K/HEVC、
    低速ドライブ、起動後 1 本目) が必要 (c) 報告者の環境依存。
  - **確かめる手段は既にある**。`MIV_DETACHED_WINDOW_DEBUG=1` で起動すると
    `main_flash_probe stage=fs_visible_false` が窓の状態ごと出る
    ([app.rs](../src/app.rs) `log_main_flash_probe`)。抑制条件は
    `fullscreen_idx.is_some()` を含む広いものなので、この経路なら必ず出る。
    1 回の再現で前面の空白の有無が確定する。
  - **既知の問題ページには載せない** (見せられないものは載せない)。再現を取れたら載せる。
- **着手条件: 複数ウィンドウ / キー入力所有権の整理 (別 worktree、`docs/briefs/modifier-ownership-design.md`) の完了待ち。**
  症状パッチ (遷移前後の追加 `SetForegroundWindow`、遅延、キーリピート抑止) を入れない。
  問われているのは「viewer の遷移中、どのウィンドウが前面と入力とカーソルを所有するか」であり、
  1.100 や過去のキー所有権報告と同型。整理後に、遷移を**所有権の受け渡しが切れない 1 手**として
  設計し直す。
- 回帰確認の観点: 静止画→動画・動画→静止画の双方向、キー押しっぱなし中と単発、
  4 表示モード (フルスクリーン / ウィンドウ / 別ウィンドウ / 別ウィンドウのフルスクリーン)、
  遷移後のカーソル可視状態。
- 規模 / 優先度: Medium / P1 (実害あり)。**整理待ち**。
- **v3.2.0 からは外すと確定 (利用者判断 2026-08-22)。** 着手条件 (キー入力所有権の整理) が
  未達で、症状パッチを入れれば整理時に解きほぐす手間が増えるため。次リリースの筆頭候補として残す。

### 1.114 PDF の表示解像度合わせが毎フレーム自分を殺し合ってライブロックする

- 出典: 2026-08-21 の実機確認中に遭遇 (別ウィンドウで PDF の先頭 / 末尾へジャンプ)。
  症状は左下の「PDF 再レンダリング中...」が消えず、そのページの高解像度化が永久に完了しない。
- **今回の detached 修正が原因ではない。** 同じループが `mimageviewer.log.bak`
  (2026-08-20 12:09、detached 作業の着手前) に **2051 回**記録されている。
  別の PDF / 別の日 / 同じ機構。今日のログでは 93 秒間で **15883 回**。
- **機構 (ログで確定)**:
  1. ページを読み込み、保持ページキャッシュへ `long=1179` で格納する
  2. 表示解像度合わせが `target_px=1702` を要求 → キャッシュは 1179 なので
     `reason=request_pdf_rerender/resolution_mismatch` で miss
  3. [`request_pdf_rerender_at_target`](../src/app.rs:51535) が
     **進行中の再レンダを無条件にキャンセルして**新しいリクエストを投げる
     (「既に同じ (idx, target_px) が飛んでいるか」を見ていない)
  4. 呼び出し元の `ensure_pdf_display_resolution`
     ([ui_fullscreen.rs:13897](../src/ui_fullscreen.rs:13897) / [:13906](../src/ui_fullscreen.rs:13906)) は
     **毎フレーム**評価される
- PDF のレンダは数百 ms かかるのに、リクエストは約 6ms ごとに投げ直される。
  **どのリクエストも完了する前に次に殺される。**
  ログの裏付け: `pdf rerender done` が **1 回も出ておらず**、
  spawn された全スレッドが `pdf rerender: cancelled (channel closed)` で終わっている。
- **修正方針**: 「同じ `(idx, target_px)` のリクエストが既に in-flight なら投げ直さない」判定を入れる。
  時間窓ではなく、**進行中リクエストの identity** で判定する。
  in-flight を無条件に潰す現在の実装 ([app.rs:51535](../src/app.rs:51535) 付近) がライブロックの直接原因。
  - 副次的に確認すること: 1 回でも再レンダが完了すれば miss が解消するのか。
    今回は一度も完了していないため、要求解像度 1702 を満たす結果が返るかは**未確認**。
    dedup を入れた後に、完了した結果の long がまだ target を下回るなら別の原因が残っている。
- **⚠ 着手前に調整が要る**: 2026-08-20〜21 に master 側で PDF 周辺が活発に動いている
  (`f2c88357` 同時 open 数の上限 / `c817a0d3` 列挙の相乗り / §2.13〜§2.16)。
  同じ領域を触る別作業と衝突しないか確認してから着手する。
- 規模 / 優先度: Small / P1 (利用者の目に見える固着)。**別の PDF 修正が落ち着いた後に再確認する** (利用者判断 2026-08-21)。

### 1.112 360 度動画 (正距円筒) の 360° ビュー — 利用者要望

- 出典: 利用者要望 (2026-08-21)。静止画でできる 360° ビューを動画でも。報告者は
  insta360 / GoPro の素材を持っている。
- **現状で不可能な理由は「難しい」ではなく「投影を差し込む場所が無い」**。presenter は
  デコード済みフレームを動画解像度の swap chain へ 1:1 コピーし、拡大縮小は DComp が担当する
  ([video-upscale-shader-plan.md](video-upscale-shader-plan.md) §1)。シェーダが走るのは色調 /
  Creative LUT のときだけで、それも動画解像度で走る。
- **着手条件: §1.47 (動画の拡大縮小を mIV のシェーダで行う) が入ること。** §1.47 は
  swap chain を表示矩形サイズにし、シェーダでソース解像度から表示解像度へ解決する構造にする。
  **そのステージができれば、正距円筒投影は同じステージ上のもう 1 枚のシェーダ**になる。
- **段階と進捗** (レーン C、branch `panorama-projection`):

  | 段 | 内容 | 状態 |
  | --- | --- | --- |
  | 1 | **判定** — 球面メタデータ + 2:1 フォールバック + ステレオ排除 | **完了** (2026-08-27) |
  | 2 | **描画** — presenter へ投影パスを足す | **完了** (2026-08-27) |
  | 3 | **入力と導線** — 見回しドラッグ / FOV / 投影方式 / HUD | **完了** (2026-08-27、実機確認済み) |

  **実機確認済み (2026-08-27、利用者)**。360 動画を開いて見回し・画角・投影方式の切替・
  視点リセット・再生系 HUD の同居まで確認した。実機で挙がった 3 件はいずれも対応済み:

  1. **左右パネルが見回し中に出て邪魔** → 360 表示中は左右パネルを出さないようにした
     (右 / 左 / ClickToShow の呼び出しタブの 3 経路とも塞ぐ。タグピッカーが開いていると
     通常のホバー条件を迂回するので、そこも塞いだ)。
  2. **投影方式が静止画のプルダウンと揃っていない** → 一覧から選ぶ形にした。
  3. **`.webm` の素材が一覧に出ない** → mIV の `SUPPORTED_VIDEO_EXTENSIONS` に `webm` が
     無いため。**動画側 360 の不具合ではない。** テスト素材は無劣化で `.mkv` へ詰め替えた
     (WebM は Matroska のサブセットなので `-c copy` で済み、球面メタデータも残る)。
     **`webm` を対応拡張子に足すかは別途判断が要る** (サムネイル / 検索 / 変換へ波及する)。

  実機確認中に**動画が再生不能になるパニック**も出たが、**360 の退行ではなく波形
  ストリップの既存不具合**だった (panic.log に 2026-08-10 / 08-14 の同型が残っている)。
  幅の上限として修正済み。パニック後に復旧しない構造自体は §1.135 へ分けた。

- **第 1 段 (完了)**: [spherical_metadata.rs](../src/video/spherical_metadata.rs)。
  `display_metadata.rs` と同じ形で、FFmpeg の side data をこのモジュールだけで型に直す。
  `detect()` が `VideoPanoramaTrigger` (Auto / Hint) か `VideoPanoramaRejection`
  (未対応投影 / ステレオ / 平面) を返す。実素材 16 本を
  `cargo run --features dev-tools --bin probe_spherical -- <dir>` で通して確認済み。
  **presenter に触っていないので、レーン B と衝突しない。**

- **第 2 段 (描画) — 読んで確定した挿入点**:
  1. **`D3DCompile` はもう使っていない** (上の記述は古い)。presenter のシェーダは
     **build.rs が FXC で `.cso` 化**して `include_bytes!` する
     ([build.rs](../build.rs) の `compile_video_presenter_shaders`、backlog §1.122 で切替済み)。
     投影シェーダも `shaders/video_panorama.hlsl` を足して同じ表に 1 行足すだけでよい。
     **実行時コンパイルを復活させないこと** (placement 切替のたびに数秒固まる)。
  2. **投影は resample と同じステージに立つが、別パイプラインにする**。
     `VideoResamplePipeline::draw(source_tex -> target_tex)` が「ソース解像度 → 表示解像度」を
     解決する場所で、投影もそこを置き換える。ただし **`VideoResampleMode` の variant には
     しない**: あの enum は設定から決まる filter の選択で、`select_video_resample_mode` と
     多数の perf イベント名の match に紐づいている。**毎フレーム変わる pose を混ぜると
     意味が濁る**ので、`panorama_pipeline` を別に持ち、resolve 直前で分岐する。
     投影が有効な間は Lanczos3 / Anime4K は走らない (投影シェーダ自身が球面から
     表示解像度へ直接解決するため)。
  3. **pose の渡し方は `set_video_grade` の前例に合わせる**
     ([render_core.rs](../src/video/native_presenter/render_core.rs) の `set_video_grade`)。
     `presenter.set_panorama_pose(Option<...>)` を足し、`video/mod.rs` の再生スレッドが
     grade と同じ経路で流す。**静止画側の `PanoPose` をそのまま使う** (yaw/pitch/fov_y/投影方式)。
     型を分けると stale 判定と丸めが 2 つになる。
  4. **surface は既に表示解像度**。§1.47 の `decide_video_surface_size` が shader filter 時に
     表示矩形サイズの swap chain を作るので、投影でも同じ条件を満たせばよい
     ([surface_policy.rs](../src/video/native_presenter/surface_policy.rs))。
  5. **ミップは「まず無しで測る」**。静止画側はフルミップ + `textureSampleGrad` で品質を出すが、
     動画は毎フレーム生成になる。D3D11 の `GenerateMips` は使えるものの、5.7K で毎フレーム
     払えるかは**実測してから決める**。まず bilinear で出して、広い画角でのエイリアスが
     実用に耐えるかを見る。耐えないときだけミップを足す。
  6. **投影方式は静止画と同じ 4 種**。数式・分岐位置・uniform の持ち方は
     [panorama-360-view-plan.md §13](panorama-360-view-plan.md) で確定済みで、WGSL の
     `projection_theta` をそのまま HLSL へ移せる。**部分 FOV の UV 変換も静止画と同じ
     `PanoUvTransform`** なので、crop 時の軸別 clamp もそのまま移す。

- **第 3 段 (入力) — レーン B との衝突は「予告ほどではない」(2026-08-27 実測で訂正)**。
  [next-cycle-work-lanes.md §4](next-cycle-work-lanes.md) は「どちらも動画上のマウスドラッグを
  新規に定義するので正面衝突する」と予告していたが、実際のコードを読むと**取り合わない**:
  - **動画キャンバス上のドラッグは現在まったく使われていない**。動画 HUD 内のドラッグは
    seek bar / 音量スライダ / VST パネル移動の 3 つで、すべてウィジェット矩形内。
  - **動画のタッチもタップのみ** (中央 = HUD 切替、左右 = ±5 秒シーク)。キャンバス全体の
    パンジェスチャは無い。
  - レーン B のドラッグは**ストリップ矩形の中**の横スクラブと下方向クローズ。
  - 残る重なりは `ui_fullscreen.rs` というファイル (マージ衝突) と、HUD の場所の取り合い
    (レーン B が下部、360 は上部) だけ。**設計上の競合ではない。**

- **入力設計の決定** (2026-08-27、Codex Sol と相談のうえ確定):
  1. **360 は「再生の制限モード」ではなく「映像キャンバスの表示モード」**。静止画の 360 は
     他機能を止める制限モードだが、動画は再生 / 一時停止 / シーク / 音量が要るので**再生系
     HUD は維持する**。排他にするのは**同じキャンバスか最終リサンプルを所有する機能**だけ。
  2. **ホイールは 360 中、修飾キー不問で FOV** (静止画と同じ)。ファイル移動は ↑/↓ に残る。
     ⚠ **FOV が上下限に達しても必ずホイールを消費する**。未消費に戻すと、限界でもう一度
     回した瞬間にファイルが切り替わる。レターボックス部分も同じ扱いにする (画面端だけ
     挙動が変わると予測できない)。
  3. **排他にする機能**: タイル一覧 / 比較 / 表示スケーリング (Anime4K・Lanczos・NIS・nearest)。
     **投影シェーダとスケーラーは両方が「表示解像度の最終出力」を所有する**ので、UI だけで
     なく実効描画モードとして排他にする。前段に置くと投影の原画が加工済みになり、後段に
     置くと視点を動かすたびに再生成が要る。
     ⚠ **利用者の設定値は書き換えない**。「360 投影中のため一時停止しています」と出すだけに
     する (既存の `processing_size_outside_note` と同じ作法)。
     色調補正 / LUT は投影前の source-resolution 処理なので維持してよい。
     音声モード♪ は維持するが、音声画面の間は 360 入力を休止する (見えない FOV が動かない
     ようにする)。映像へ戻ったら視点を復元する。
  4. **タッチはドラッグ = 見回し、タップ = 既存のまま**。ただし release 時の距離判定だけでは
     不十分で、既存のタッチ認識器と同じ ownership latch が要る:
     - しきい値 (12 logical pt / 700 ms) を一度でも超えたら、その接触列は最後まで見回し。
       開始位置へ戻して離してもタップに戻さない。
     - 見回し確定フレームでは DOWN からの全移動量を反映する (最初の移動を捨てない)。
     - HUD 上から始まった接触は外へ出ても HUD 所有。逆も同じ。
     - 2 本目が入ったら pending tap を必ず取り消す。
     - **ダブルタップを視点リセットに割り当てない**。左右の連続タップ (シーク) が化ける。
       リセットは上バーのボタンに置く。
  5. **ON/OFF ボタンは上バー** (VST3 と全画面切替の間あたり)。静止画と同じ論理位置。
     ⚠ **静止画の「360 中はボタンを隠して × を解除に使う」方式は踏襲しない**。動画の × は
     常に動画 / ビューアを閉じる意味であり、360 状態で意味が変わると別ウィンドウで事故る。
     360 ボタンは ON 中も強調表示で残し、同じボタンで OFF にする。ON 中は隣に投影方式と
     視点リセットを出す。非対応時は同じスロットに理由付き disabled (隣のボタンを動かさない)。

- **決定: 360 ON はファイルをまたいで保持する (静止画に合わせる)** (2026-08-27、利用者判断)。
  Codex は source-scoped (別ファイルでは改めて明示 ON) を勧めたが、**静止画との一貫性を
  優先する**。静止画側の lifecycle をそのまま写す:
  - 明示 ON でセッション state を作り、**通常ナビでは視点 (yaw/pitch/fov/投影方式) を保持**する。
  - **360 でない動画へ移ったら非アクティブ化するが state は捨てない**。次に 360 と判定された
    動画へ移ると同じ視点で再開する。判定は `is_panorama_mode_active` と同じ形
    (`state.is_some() && detect(...).is_ok()`) にして、静止画と述語の形も揃える。
  - **明示 OFF とフルスクリーン退出では state を破棄する。**
  - 受け入れる副作用: Hint (2:1 のみ) の通常動画へ ↑/↓ で移ると、その動画も投影表示になる。
    **静止画側が既にそう振る舞っており出荷済み**なので、動画だけ別の境界にはしない。
    利用者が迷ったら明示 OFF で抜けられる。
- **第 3 段 (完了、2026-08-27)**: 静止画と共通の `App::panorama_state` を正本として、
  native presenter へ pose を同期する。動画キャンバスのマウス / タッチドラッグ、修飾キー不問の
  FOV ホイール、`FsPanorama` / `FsPanoramaProjection`、上バーの固定 360 スロット・投影方式・
  視点リセットを実装した。ホイールは FOV 上下限でも消費し、タッチは 12 logical pt の
  ownership latch で DOWN からの全移動量を初回ドラッグへ含め、2 本目で pending tap を取り消す。
  ダブルタップは既存のタップ操作のままでリセットには使わない。動画の × は常に close のまま。
  タイル一覧は 360 ON 時に閉じて再入場を抑止し、動画スケーラーは実効描画だけ休止して
  `VideoScaleFilter` の保存値を変更しない。音声モード中は入力だけを休止し、映像へ戻ると同じ
  pose を再開する。自動テスト済み、Windows native presenter の実機確認は残る。
- **自動判定は可能だが、それだけでは足りない**。FFmpeg は `AV_SPHERICAL_EQUIRECTANGULAR` を
  出す ([spherical.h](../vendor/ffmpeg/include/libavutil/spherical.h))。回転メタデータと同じ扱いに
  できる。ただし**実素材を集めて測った結果、次の 3 点が分かった** (2026-08-27、
  テストセットは `H:\home\mimageviewer_old\testimage\360d\movie\`、同梱の README.md に実測表):
  1. **実素材の大半は metadata を持たない。** Wikimedia から集めた実在の 360 動画 10 件のうち、
     spherical metadata があったのは **2 件だけ**。WebM トランスコードが Matroska の Projection
     要素を落とすため。→ **静止画側と同じ 2:1 アスペクト比のフォールバック
     (`PanoramaTrigger::Hint` 相当) が必須**。metadata 判定だけで作ると、利用者の手元の多くの
     ファイルで 360 ボタンが有効にならない。
  2. **部分 FOV は別の enum になる。** `equi` の projection_bounds が非ゼロだと FFmpeg は
     `AV_SPHERICAL_EQUIRECTANGULAR` ではなく **`AV_SPHERICAL_EQUIRECTANGULAR_TILE`** を返す。
     これは静止画側の GPano `CroppedArea*` (Phase 1.5 の `PanoUvTransform`) に相当する。
     **`EQUIRECTANGULAR` だけを見ると部分 FOV 素材を取りこぼす。**
     `av_spherical_tile_bounds()` が 0.32 固定小数を画素へ直してくれるので、静止画側の
     `PanoUvTransform::from_gpano` と同じ形へ落とせる。
  3. **上下分割ステレオ (3D 360) が実在する。** 集めた中の 1 件が該当し、モノラル equirect と
     して扱うと上下に同じ絵が 2 つ出る。`st3d` の stereo_mode を見て弾くか、片目だけ使うかを
     決める必要がある。**静止画側にはこの分岐が無い。**
- **テスト素材の作り方**: `ffmpeg` は MP4 出力へ spherical metadata を引き継がない (8.1 で
  `-c copy` / 再エンコードとも確認) ため、上記フォルダに `make_spherical_mp4.py` を置いてある。
  `st3d` / `sv3d` を自前で書き込み、全球と部分 FOV の MP4 を作れる (ffprobe で往復検証済み)。
- 規模 / 優先度: Large / P3。**§1.47 待ち**。

### 1.113 動画シークバー近傍のストリップを「サムネイル列 ⇄ 音声波形」で切り替える — 利用者要望

> **設計の正本は [video-seek-strip-plan.md](video-seek-strip-plan.md) へ移した** (2026-08-23)。
> 以下は着手前の記録。下の「全尺デコードが要る」「開いたときだけ解析を起動する」は、
> **窓オンデマンド解析** (同書 §5) で全尺デコード自体を起こさない形に置き換わった。

- 出典: 利用者要望 (2026-08-21)。動画視聴中に `Z` で出る音声波形を、シークバー近傍にも出して
  波形を手がかりにシークしたい。サムネイルと併せて場所を探せると便利、という趣旨。
- **§1.102 (YouTube 型のサムネイル列) と同じ場所を使う。** 2 つの UI を競合させず、
  **1 つのストリップのモード切替**にする。§1.102 の「シークバーから上へドラッグしたときだけ開く」
  導線をそのまま共有し、開いた中身をサムネイル列と波形で切り替える。
- データ面は好条件:
  - `TimelineAnalysis` ([crates/music-core/src/analysis.rs:186](../crates/music-core/src/analysis.rs:186)、
    bins + ビートグリッド) が**動画ファイルでも `Z` 波形モードで既に生成されている**。
  - 描画も `draw_music_timeline` のラスタキャッシュがある ([ui_music_timeline.rs](../src/ui_music_timeline.rs))。
  - **タイムライン全体あたりのコストはサムネイル列より安い**。1 回のデコードで全尺分の波形が
    得られるので、§1.102 が悩んでいる抽出方式 (1 枚 1 秒 vs 50ms) の問題が無い。
- コスト面で決めること:
  - **全尺の音声デコードが要る** (背景スレッド・progressive)。長い動画では埋まるまでの時間がある。
    途中経過をどう見せるか決める (progressive partial の既存挙動を流用できるか)。
  - **永続キャッシュを持たない設計** ([app.rs:8238](../src/app.rs:8238)「永続 DB はやめて直近 N 曲だけ
    メモリに載せる」)。開くたびに解析し直す。動画でも同じでよいかを判断する。
  - **ストリップを開いたときだけ解析を起動する** (常時ではない)。現状 `ensure_music_analysis` は
    音楽ビューが有効なときだけ呼ばれる ([app.rs:38518](../src/app.rs:38518)) ので、その条件を広げる形。
    常時起動にすると、動画を開くたび全尺デコードが走る。
- 規模 / 優先度: Medium / P3 (§1.102 と同時に設計する)。

### 1.115 別ウィンドウ表示で静止画を開閉すると、フルスクリーンと一覧が何度も入れ替わってちらつく — 利用者報告

> ⚠ detached viewer リワーク中の領域。**症状パッチ (delay / guard / 追加 repaint) を入れない**。
> BA-5 (gap フレームで passive 窓が破棄される) と BA-7 の境界にまたがる。

- 出典: 2026-08-23 (v3.2.0 公開後)。`g:\home\comfyui\202608-29_焼き肉レストラン` を開くと、
  フルスクリーンと一覧の切り替えが激しくちらつく。**ポータブル版では同じ操作で起きない。**
  これは初回比較時の観測で、その後タイトルバーから detached 窓を最大化するとポータブル版でも再現した
  (下記「最大化後の追試」で訂正)。
- **初回推定 (最大化後の追試で訂正)**: ポータブルで起きない理由は build flavor ではなく設定。ポータブルは既定設定なので
  フルスクリーンが別ウィンドウにならず、下の `detached_viewer_cleanup` 経路自体を通らない。
  `portable` feature が変えるのは native 依存の解決先と data_dir だけで、この経路に分岐は無い。
- **ログで確認できた事実** (`mimageviewer.log`、1 セッション 14 open / 10 cleanup):
  1. 別ウィンドウのフルスクリーンを閉じるたびに
     `[viewport] cleanup_visible_false: presentation=Some(DetachedWindow)` →
     `[ui-fonts] schedule main font atlas resync: detached_viewer_cleanup` →
     **連続 5 フレームの pass 破棄** (`discard pass for font atlas resync`、generation が
     +1 ずつ、`egui_frame` は連番)。実測 3.503s→3.761s = **258ms 分の描画が捨てられる**。
  2. この「cleanup 1 回 = discard 5 回」は **3 世代のログすべてで一定** (v3.1.3 期の
     `mimageviewer.log.bak` も 4 cleanup / 20 discard)。**5 フレーム破棄そのものは v3.2.0 で
     入ったものではない。**
  3. close の約 0.55 秒後、**キー入力の記録が無いまま `=== open_fullscreen ===` が再発火**し、
     別ウィンドウが**新しい HWND で作り直される** (3.341→3.903 / 37.311→37.927 /
     38.528→39.086)。再オープンは毎回**discard 窓が終わった 0.14〜0.17 秒後**に来ており、
     3 回とも同じ内部イベントからの一定オフセット。人が 2 度押ししたにしては揃いすぎている。
  4. `keys=Enter:up,Enter:up,Enter:up,Enter:up` のように**同一フレームに同じキーイベントが
     4 つ並ぶ行がある** (F12 も同様)。同じ入力が複数回配送されている。
- **コードで確認できた構造**:
  - `MAIN_FONT_ATLAS_RESYNC_REPEAT_FRAMES = 5` ([app.rs:392](../src/app.rs:392))。
    発行側のコメントが理由を明記している ([app.rs:37598](../src/app.rs:37598)):
    「close 直後の 1 フレームだけメインウィンドウの wgpu surface が消えて full upload が
    捨てられる。**数フレーム再発行して、surface が戻ったフレームで確実に届かせる**」。
    = **どのフレームで surface が戻るかを観測せず、時間窓で race を吸収している。**
    5 フレームぶんの描画破棄はその代償で、別ウィンドウ経路ではそれが目に見える。
  - `maybe_defer_for_main_font_atlas_resync` は 1 OS フレーム 1 発行にゲートされているので、
    5 回は必ず**別々のフレーム**を消費する ([app.rs:37855](../src/app.rs:37855))。
  - `should_defer_main_paint_for_font_atlas_resync(_reason) -> bool { true }`
    ([app.rs:3130](../src/app.rs:3130)) は**引数を無視して常に true**。reason で絞る余地が
    潰れている (縮退した経緯を先に読むこと)。
- **直す方向**:
  1. **surface が戻ったことを観測して 1 回だけ発行する。** 固定 5 フレームをやめる。
     no-surface early return の地点で「full upload を捨てた」ことを型で記録し、次に surface が
     ある フレームで 1 回だけ resync する。**時間窓で race を吸収しない** (この 5 フレーム窓は
     v1.8.0 の黒サムネ修正で入ったもので、同じ形が別の症状で再発している)。
  2. **再オープンの正体を先に確定させる。推測で直さない。** close 後に `open_fullscreen` を
     呼んだ呼び出し元と契機 (replay されたキーか、遅延していた要求か) をログに出す。
     事実 3 と 4 から「discard 窓中に同じキーが複数回配送され、main 側が Enter を
     もう一度 open として解釈している」が第一候補だが、**未確定**。
  3. 1 と 2 は独立に進められる。1 だけでも破棄フレームが 5 → 1 になり、ちらつきの大半は消える。
- ⚠ **2026-08-27 訂正: 上の方針 1 の前提はもう成り立たない。**
  「no-surface early return で full upload が捨てられる」ことを前提に「捨てたことを型で
  記録する」と書いてあるが、**捨てられなくなっている**。painter 側で 2026-08-14 に根治済み:
  - `ce6616ef` delta の適用を surface 取得より **前** へ移した
  - `e4a52d39` 誰も描かないフレームでも eframe が texture を渡すようにした
  - `0b645861` その配送をトランザクション化した

  `begin_delivery` ([winit.rs](../vendor/egui-wgpu/src/winit.rs)) は `render_state` があれば
  surface の有無に関係なく適用し、`render_state` は構築時以外 `None` に戻らない。
  detached teardown が prune するのは viewport / surface だけで、`Painter` は全 viewport 共有
  (Codex が read-only レビューで独立に確認)。surface 不在での full replacement を固定する
  テストも既にある (`paint_and_update_textures_delivers_set_and_free_without_surface`)。

  → **「捨てたことを記録する」機構は作る必要が無い。5 回の再発行は現状ほぼ vestigial** で、
  `MAIN_FONT_ATLAS_RESYNC_REPEAT_FRAMES` を入れた `e09c83d9` (2026-06-18) の理由は 2 か月後に
  消えている。その上に積んだ `d48982e5` (detached cleanup を保守経路へ戻した判断) も
  同じ前提の上にあるので、合わせて見直す。
- ⚠ **ちらつきへの因果はまだ確定していない。** 破棄フレームとの相関は取れているが、
  利用者のキャプチャでは **デスクトップ全体と他アプリまで** 塗り直されており、これは
  top-level HWND の破棄・focus / z-order 変更・DComp presenter detach による DWM の再合成で
  説明が付く。**破棄を減らすと停止時間は縮むが、閃光は残る可能性がある。**
  計器 `[ui-frame-gap]` / `[atlas-probe]` (`2f514a2d`) と再アームログ (`65e90871`) を入れた
  A/B ビルドで分離する。
- **2026-08-27 実測・実装結果 — option (d) で caller resync を撤去**:

  | discard reason | build A (現行 5 回) | build B (detached cleanup のみ 1 回) |
  | --- | ---: | ---: |
  | `native_video_backdrop_hide` | 14 | 10 |
  | `detached_viewer_cleanup` | 0 | 3 |

  2 build とも font-atlas delta は全件 `site=paint outcome=Submitted` で、
  `SurfaceAbsent`、`no_paint` site、`RenderStateAbsent` は 0 件だった。build A の
  detached cleanup は先行する backdrop-hide pending を re-arm しただけで、自分の reason の
  discard を 1 回も作っていない。支配的 producer は F12 toggle ごとの
  `native_video_backdrop_hide` であり、旧 repeat は毎回 5 frame を消費していた。

  画面録画の close 周辺は 20 luma 以上の swing が build A で 17 回 / 1.20 秒、build B で
  11 回 / 0.40 秒となり、repeat 短縮で約 3 倍短くなった。一方で閃光は残り、top-level HWND
  teardown による DWM 再合成という別原因の見立てと一致した。PDF / video の実機確認では
  glyph damage は出なかった。

  所有境界の結論は「surface acknowledgement 後に 1 回」ではなく、**viewport lifecycle の
  font firing は 0**。main viewport 群は `Context` / `Painter` / `render_state` /
  renderer texture namespace を共有し、viewport teardown は atlas texture を所有しない。
  native presenter は別 `Context` / renderer を持つため、main atlas resync は presenter を
  修復しない。そこで repeat / reason / safety / marker reload / discard-repass /
  thumbnail 先送りと全 lifecycle producer を撤去した。実際の UI font 設定変更だけは
  main context owner へ coalesce し、次 update で現在設定を 1 回 `set_fonts` する。
  正しさは painter の no-surface delivery test と、eframe の 4 early-return site を検査する
  caller-level test の両方で固定する。計測 probe は利用者の再測定まで残す。
- 別セッションからの引き継ぎ資料: `docs/detached-close-flicker-handoff.md` (video-strip worktree)。
  同じ機構を「ちらつき vs フォント崩れのトレードオフ」として記述しているが、上の訂正により
  **トレードオフではなくなっている**。同資料の見立て「再同期が毎フレーム自分を再予約している」
  は外れで、回数は定数 5 そのもの (同資料も未確認と明記していた)。
- **初回推定 (最大化後の追試で訂正): 再現条件は「設定」ではなく「実行時の状態」**
  (2026-08-23 追記、利用者の settings.db を読んで確認):
  報告者の永続設定は `detached_viewer_enabled=false` / `detached_viewer_open_images_in_window=false` /
  `fullfeature_media_window=true` / `video_in_window_mode=true`。**静止画を別ウィンドウで開く設定は
  どれも OFF** なのに、ログでは静止画 open のたびに `presentation=Some(DetachedWindow)` になっている。
  `requested_viewer_presentation_for_open` ([app.rs:43006](../src/app.rs:43006)) の 4 分岐のうち、
  永続設定によるもの (`detached_viewer_enabled` / `effective_media_in_media_window`= メディア限定) は
  すべて偽なので、**残る実行時分岐**しかない:
  `viewer_session_is_detached() && detached_viewer_independent_active` か、F12 が立てる一度きりの
  `detached_viewer_open_next_still_detached_once` ([app.rs:41528](../src/app.rs:41528))。
  → **環境設定を合わせても再現しない。F12 で作った独立 detached セッションが閉じても残っている状態が要る。**
  利用者がポータブルで同手順を試すと、閉じた後の再オープンで detached が維持されず
  `non_detached_viewer_presentation()` へ落ちた (= `video_in_window_mode=true` なので MainWindow)。
  **同じ操作で detached セッションの寿命が 2 環境で違う**こと自体が、この不具合と同じ所有権の問題。
- **追加観測の指定 (実施済み)**: `MIV_DETACHED_WINDOW_DEBUG=1` で起動すると `[ui-fonts][diag]` /
  `[detached-window-debug]` が `presentation` / `session` / `active_context` / `passive_windows` /
  `fullscreen_idx` を毎回出す ([app.rs:38773](../src/app.rs:38773))。**どの分岐で DetachedWindow に
  なっているかはこれで確定できる。** 再オープンの契機と併せて 1 回の再現で両方取れる。
- **最大化後の追試と訂正 (2026-08-23、Codex、ポータブル v3.2.0、
  `MIV_DETACHED_WINDOW_DEBUG=1`)**:
  1. **再現を可視化する条件はタイトルバーの最大化状態**。最初の通常サイズ open は
     `rect=(110,110 1462x1136)` / `window_id=20` / **16.1ms**。その窓をタイトルバーで最大化して
     close すると、settings.db は
     `detached_viewer_window_placement.maximized=true` を保存し、次の open は
     `rect=(-11,-11 3862x2110)` / `window_id=21` / **157.7ms**、その次も別 HWND の
     `window_id=22` / **147.6ms** になった。通常サイズの約 10 倍を UI thread の
     `pre_grid` で費やしている。builder が保存 placement の `maximized` を新 viewport へ適用する
     ([ui_fullscreen.rs:17331](../src/ui_fullscreen.rs:17331)) ためで、portable feature や F11 の
     borderless fullscreen は必要条件ではない。
  2. **1 close ごとに HWND / viewport identity を捨て、次の open で作り直している。** ログは毎回
     `cleanup_visible_false ... recreate=true` の後に host を clear し、generation を進める
     ([ui_fullscreen.rs:13083](../src/ui_fullscreen.rs:13083),
     [ui_fullscreen.rs:13111](../src/ui_fullscreen.rs:13111))。そのため最大化した新しい全面 HWND の
     生成・surface 初期化が、一覧との切替時にそのまま見える。
  3. close 後は既報どおり **5 回の font-atlas resync discard** が必ず走った。今回の 3 回は
     157〜161 = 229ms、162〜166 = 240ms、167〜171 = 231ms。つまり最大化 host の
     148〜158ms の再生成に加え、一覧側も約 0.23 秒を固定回数の race 吸収に使っている。
     **画像 decode は 10〜11msで完了しており主因ではない。**
  4. 上記「内部イベントによる自動再オープン」「同一 Enter の 4 重配送」は今回のログで訂正する。
     open 時の `input_seq` は 59 → 61 → 63。viewer の Enter close が 1 回 increment
     ([ui_fullscreen.rs:14144](../src/ui_fullscreen.rs:14144)) し、一覧の Enter open がさらに 1 回
     increment してから `open_fullscreen` を呼ぶ
     ([app.rs:35015](../src/app.rs:35015)) 契約と一致する。`[fs-key]` は fullscreen context の
     生存中しか一覧側 Enter を記録しないため、「キー行が無い」ことは内部 open の証拠にならない。
     4 個並んだのは `Enter:up` のみで、close/open を発火する `down` の重複は無かった。
- **確定した原因**: 最大化そのものは増幅条件で、根は次の 2 つの lifecycle 不整合
  (2 は 2026-08-27 の option (d) で解消済み)。
  1. 明示 close が linked detached host を terminal teardown し、次の明示 open が保存済み
     `maximized=true` の全面 HWND / surface を同期的に新規作成する。
  2. teardown が atlas texture を所有しないのに、固定 5 フレームの discard で font atlas の
     full upload を再送していた。利用者が Enter で viewer と一覧を往復するたび、
     「全面 host の作り直し」と「一覧側の 5 pass 破棄」が交互に露出していた。
- **構造的な解消方針 (実装前に detached R4 / ゲート C で方式を確定する)**:
  1. **font atlas 側は実装済み**。固定 5 回だけでなく lifecycle resync 自体を廃止し、生成済み
     delta は renderer の既存 delivery transaction で完結させる。teardown 用の新しい
     acknowledgment state、delay / retry / 追加 repaint は作らない。
  2. linked detached の content close と OS host lifetime を分離し、`DetachedWindowManager` が
     hidden host の再利用可否を所有する。再利用できる方式なら同じ `window_id` / HWND を
     hidden → content remount → visible と遷移させる。egui の制約で terminal teardown が必須なら、
     新 maximized host は HWND 作成・placement 適用・surface/content-ready acknowledgment が揃うまで
     hidden のまま保持し、`Visible(true)` を 1 回だけ発行する。App-level bool / Option、rect heuristic、
     固定待ち時間は追加しない。
  3. `maximized=false` への強制リセット、最大化 placement の保存停止、windowed 強制は機能劣化なので
     修正に使わない。タイトルバー最大化と F11 borderless は別状態のまま維持する。
- **回帰条件**: 保存 placement が `maximized=true` の linked 静止画窓で Enter close/open を反復し、
  (a) 1 操作につき visibility 遷移が 1 回、(b) 再利用方式なら `window_id` / HWND が不変、teardown
  方式なら ready 前に visible にならない、(c) cleanup / backdrop hide / recreate が font request /
  discard を 1 回も作らない、(d)通常サイズ・F11・folder-nav reopen・always-new / passive / ParkedLive の所有権を
  変えないことを state-transition test + debug log smoke で固定する。実装時は
  [detached-rework-plan.md §11](detached-rework-plan.md#11-リワーク外の変更ログ) に合意と変更境界を記録する。

#### 追記 (2026-08-23): 最大化が再現条件、原因は placement の毎フレーム書き戻し

`MIV_DETACHED_WINDOW_DEBUG=1` で取り直したログで確定した。**ポータブル版でも、別ウィンドウを
タイトルバーで最大化すれば再現する。**

- **訂正**: 前段の「キー入力の記録が無いまま再オープンしている」は**誤りだった**。`[key-debug]` を
  出すと、open も close も**すべて実際の Enter 押下**である (`raw main down vk=0x0D` →
  main 窓なら `pressed GridOpenSelected [Grid]`、detached 窓なら `consume FsClose [FsImage]`)。
  非 debug ログの `[fs-key]` はフルスクリーン viewport が見たキーしか出さないので、main 窓の
  Enter が「無い」ように見えていた。**出ていないログを根拠に推論していた。**
- **新しい事実**: **最大化された別ウィンドウが表示されている間、placement 更新が毎フレーム走る。**
  1 セッションで `runtime_placement reason=active_placement_update_maximized` が **82 回**、
  そのすべてが `from == to` (中身が変わらない書き込み)。同数の
  `placement_trace event=builder_no_position` が対になっている。**82 フレーム = 別ウィンドウが
  開いていた全フレーム**で、1 フレームも休んでいない。
- **構造**: maximized 分岐 ([app.rs:43782](../src/app.rs:43782)) は**実測した `outer_rect` を捨てて**
  `detached_window_seed_placement()` (= restore 用の w/h) を書き、`maximized=true` だけ立てる。
  実測 `rect=(-11,-11 3862x2110)` に対し placement は `w:2560, h:1369.33` のまま。
  つまり `DetachedViewerWindowPlacement` 1 個に**「今のサイズ」と「元に戻したときのサイズ」が
  同居**していて、最大化中は前者を記録する場所が無い。だから書き込みは**永久に収束しない**
  (毎フレーム同じ値を書き直す)。終端状態が無いという意味で
  v3.1.2 の `nav/archive_cache_peek` 退行と同型。
- **Codex の §2.20 分析と根が同じ。** あちらは「snapshot bake が placement の w/h で画像矩形を
  正規化するのに、passive draw は最大化 viewport の `full_rect` へ X/Y 独立で戻す」ことを
  指摘している。**症状 (縦横比の圧縮 / ちらつき) は違うが、原因はどちらも「最大化中の placement が
  実ジオメトリを表していない」こと**。片方だけ直しても、もう片方の入力は歪んだまま残る。
- **解消方法** (A が本命、B は A の一部として自然に消える):
  - **A. 現在ジオメトリと restore ジオメトリを別の場所に持つ。** maximized のとき実測 rect を
    捨てない。restore 用 seed は maximize に入った時点の値として別に保持する。これで §2.20 の
    bake / draw も同じ数字を見られるようになる。
  - **B. 変化が無ければ書かない。** 現状 82/82 が no-op。A を入れれば「実測値が変わったときだけ
    書く」が自然に成立する。**変化検出を後付けの guard として足すのではなく、A の帰結にする。**
  - **C. font atlas 側は option (d) で解消済み。** viewport teardown は atlas owner ではないため
    lifecycle resync / discard を 5 → 1 ではなく 0 にした。A/B と独立。
- **検証の観測点**: 最大化した別ウィンドウを開いている間、①`runtime_placement` が毎フレーム出ない
  こと ②`builder_no_position` が毎フレーム出ないこと ③close 後に font-atlas 起因の
  `discard pass` が 0 回であること ④最大化 → 復元でウィンドウが元のサイズに戻ること
  (A で restore 値を壊していない証明)。

#### Codex クロスチェック: 2 つの欠陥を同一原因として扱わない

ClaudeCode の追加ログと Codex の portable / normal ログを併記して source まで照合した結果、
**最大化が再現を可視化する条件**、**open / close は実入力**、**placement が毎フレーム同値更新される**
という観測は一致した。ただし、ちらつきへの因果は次のように分ける。

- `active_placement_update_maximized` の `from == to` は runtime manager 内の
  `runtime.placement = Some(placement)` であり
  ([detached_window_manager.rs:538](../src/app/detached_window_manager.rs:538))、viewport command、
  generation 更新、visibility 更新、repaint 要求を発行しない。ログの `builder_no_position` も
  `apply_placement=false`、すなわち生存中の窓へ position / maximize を再適用していない。
- settings への永続化は毎フレームではなく、runtime を remove する close 境界で 1 回だけ行う
  ([app.rs:2174](../src/app.rs:2174))。したがって「82 回の同値更新」だけから
  **ウィンドウが82回再表示された**、または**それがちらつきの直接原因**とは結論できない。
- 一方、当時の画面遷移と一対一に対応した事実は、各 close の `recreate=true` + 5 discard と、
  各 open の **新 `window_id` / 新 HWND / 113〜177ms の最大化 host 生成**である。font discard は
  option (d) で解消したため、§1.115 に残る直接原因は host lifecycle と DWM 再合成である。
- 現在 geometry と restore geometry を1値に混在させる placement model は、§2.20 の座標破綻、
  不要な runtime 書き戻し、将来の placement command 振動を招く**独立した構造欠陥**であり、
  ClaudeCode の A/B は同じ R4 設計で直す価値がある。ただし font discard を撤去しても close ごとの
  host 再生成が残る限り、§1.115 の完了条件は満たさない。
- 実装レビューでは (1) placement A/B のみで HWND 再生成が残るケース、
  (2) host lifecycle 修正で最大化 placement を維持し、font work は 0 のままのケースを分けて測り、
  「ログ量が減った」ことを「ちらつきが直った」ことの代用にしない。

- 規模 / 優先度: 中 / **P1** (常用操作で目に見える。ただし凍結領域なので構造修正が前提)。
- 関連: §2.20 (**同じ根**。最大化中の placement が実ジオメトリを表していない)、
  [docs/detached-rework-plan.md](detached-rework-plan.md) の BA-5 / findings-12 D1
  (旧 font resync の discard パスが passive 窓を破棄した実害、option (d) で解消) / findings-14
  (毎フレーム seed による振動を `builder_placement_latch` で止めた前例)。

### 1.116 メインウィンドウの起動状態を選べるようにする — 外部SNSでの移行検討者の指摘

- 出典: 2026-08-24。ZipPla からの移行を検討している利用者が、終了時のウィンドウサイズ復元、
  最大化状態の維持、最大化起動の設定が見つからないと外部SNSへ投稿した。直接受けた実装要望では
  ないため、需要候補として記録する。
- **現状の整理**: 通常ウィンドウの位置とクライアントサイズは既に `window_pos` / `window_size` へ
  保存し、起動時に復元している ([lib.rs:1124](../src/lib.rs:1124),
  [app.rs:54576](../src/app.rs:54576))。最大化中は restore 用の通常矩形を壊さないため
  `track_window_rect` の更新対象外だが、**最大化して終了したという状態自体は保存していない**。
  そのため「サイズ復元なし」ではなく、「次回も最大化」と「常に最大化で起動」が不足している。
- 仕様候補: 環境設定へ `起動時のウィンドウ状態 = 前回の状態 / 通常 / 最大化` を追加する。
  現行互換の既定値は `通常`。restore 用の通常位置・サイズと、終了時の最大化 flag を別々に持つ。
  必要性があれば `--window-state normal|maximized|restore` も同じ resolver へ通す。
- 実装条件:
  - `pending_initial_size` の mixed-DPI 起動補正が、最大化適用後に通常サイズへ戻さない順序にする。
  - 最小化終了は前回の有効な通常矩形を維持し、起動時に最小化は復元しない。
  - 設定 round-trip と起動状態 resolver の unit test に加え、複数モニター / 異なる DPI /
    トレイ終了で restore rect と最大化状態を手動確認する。
- 規模 / 優先度: Small (0.5〜1.5日程度) / P2。コード量より Windows / DPI 実機確認が主なリスク。
- **実装済み (2026-08-25、実機確認待ち)**:
  - `Settings.startup_window_state` (`Normal` 既定 / `Maximized` / `RememberLast`) と、
    終了時の最大化 flag `Settings.window_maximized` を追加。flag は復元矩形
    (`window_pos` / `window_size`) と**別フィールド**にして、最大化を解いたときの
    戻り先を残す (detached 側 §1.115 と同じ根を作らない)。
  - `resolve_startup_maximized` で起動状態を決め、`ViewportBuilder::with_maximized` へ渡す。
    初回フレームで `ViewportCommand::Maximized` を送る形にすると通常サイズが一度見えて
    ちらつくため、生成時に指定する。`--window-size` は設定より優先して通常ウィンドウ。
  - `pending_initial_size` の mixed-DPI 補正は、最大化起動では**最大化が解けるまで保留**する
    (`deferred_initial_size_ready`)。egui が「最大化ではない」と明示報告した最初のフレームで
    1 回だけ流し、そこで復元矩形を矯正する。報告が無い `None` を「最大化ではない」と
    読まないこと自体が条件。
  - 追跡側 (`tracked_window_maximized`) は最小化中と報告欠落時に直前の状態を保つ。
    Windows は最小化中の `GetWindowPlacement().showCmd` を `SW_SHOWMINIMIZED` にするため、
    素直に読むと最大化して最小化しただけで flag が落ちる。
  - 環境設定のページ名を「起動時に開く場所」→「起動時の動作」に変更し、同ページへ
    「起動時のウィンドウ状態」節と検索索引エントリを追加。
  - unit test: 起動状態 resolver / 設定 round-trip / 未知値の既定落ち / 環境設定 OK が
    選択を巻き戻さないこと / 補正の保留条件 / 最小化中の追跡 / 保存が復元矩形を触らないこと。
  - **既定は `RememberLast`** (2026-08-25 利用者判断)。v3.2.0 以前の設定には field も
    `window_maximized` も無いため、更新直後の初回起動は通常ウィンドウのまま。次に最大化して
    終了したときから効き始めるので、更新した瞬間に驚かせない。
  - 既定の変更なので `version_highlights.rs` へ `must_read` を追加済み。
    **⚠ 版数は暫定 `"3.3.0"`。次のリリース版数を決めるときに合わせること** (リリース手順 Phase 1)。
  - 実機確認済み (2026-08-25): 設定 UI / 検索、前回状態の復元と解除後のサイズ、別 DPI モニター、
    最小化終了、トレイ終了、通常ウィンドウ。

### 1.117 外部ツール連携を設定画面へ出し、引数・複数選択へ拡張する — 外部SNSでの移行検討者の指摘

- 出典: 2026-08-24。移行元の ZipPla / NeeView にある外部ツール設定が mIV に無いとの外部SNS投稿。
  直接受けた実装要望ではないが、既存機能の発見性不足と機能差の両方があるため候補として記録する。
- **起案時 (2026-08-24) の現状整理**: 右クリックの「アプリケーションで開く…」から Windows 関連付けアプリと任意 exe を
  追加・実行できる ([context_menu.rs:2037](../src/ui_dialogs/context_menu.rs:2037))。ただし登録場所が
  コンテキストメニュー内だけで見つけにくく、実行は `exe + 物理ファイル1件` 固定
  ([open_with.rs:142](../src/open_with.rs:142))。引数、作業フォルダー、複数選択、ZIP / PDF 内ページ、
  任意ツールの直接キー割り当ては未対応。動画の <kbd>Shift+Enter</kbd> は OS 既定アプリを開く別経路。
- 段階案:
  1. 環境設定の「起動と連携」に外部ツール管理を追加し、既存の任意 exe 登録を改名・並べ替え・削除
     できるようにする。操作カスタマイズにはまず動的 action を増やさず、「外部ツールを選んで開く」
     という汎用 picker action を追加する。
  2. `ExternalToolDefinition { id, name, executable, arguments, working_directory, selection_mode }` の
     typed 定義へ移行し、`{file}` / `{folder}` / `{files}` 等の明示 placeholder と複数物理項目を扱う。
  3. ZIP / PDF / 変換アーカイブ内ページを対象にする場合は、一時実体化、キャンセル、外部プロセスが
     読み終えるまでの lifetime、終了後 cleanup を別 phase で設計する。
- 安全性 / 応答性:
  - コマンド文字列を `cmd /c` へ渡さず、exe と `OsString` 引数列を分離して `Command` を使う。
  - ネットワーク上の exe / 作業フォルダー確認や仮想ページ抽出を UI スレッドで行わない。
  - 起動失敗を黙って捨てず、ツール名と OS error を通知する。
- 規模 / 優先度: 管理UIのみ Small〜Medium (1〜2日)、物理項目の引数・複数選択まで Medium
  (3〜6日)、仮想ページ込み Large (追加1〜2週間) / P2。
- **段階 3 まで一度に出す (2026-08-25、利用者判断)。** 段階 2 で切って出す案は採らない:
  - 段階 2 までだと現状 (実ファイル 1 件を関連付けで開く) から用途があまり広がらない。
  - **設定画面を一度出すと、それが互換の約束になる。** `selection_mode` と `{file}` の
    意味は仮想ページが入ると変わるので、後から段階 3 を足すと「同じ設定なのに挙動が違う」
    形になる。設定の形を決めるのは仮想ページまで見えてからにする。
  - 出典は要望ではなく不満の声なので**優先度は高くない**。急いで段階を刻む理由が無い。

- **正本は [external-tool-launch-plan.md](external-tool-launch-plan.md) へ移した (2026-08-29)。**
  他アプリ調査 (NeeView をソースから、ZipPla を公式 Tips から) と mIV 側のコードベース調査を踏まえた
  仕様案・段階分け・未決事項はそちらにある。作業は worktree `C:\home\mimageviewer-extlaunch` /
  ブランチ `external-tool-launch`。
- **実装状況 (2026-09-01): P2c まで実装済み。** 導線は全登録ツールを出す右クリックと
  固定キースロットに整理し、複数対象は既定 `Each`、ツール別の確認 / 上限で扱う。P3 以降は未実装。
- 調査で分かった重要な点を 2 つだけここに残す:
  - **発見性の問題の正体**: グリッド右クリックはネイティブコンテキストメニューが既定 ON。
    ネイティブ側にも mIV は自分の項目を先頭へ差し込んでいるが、差し込めるのは `NativeMivCommand` の
    固定 enum に載っているものだけで、「アプリケーションで開く…」は入っていない。つまり現行の
    open-with は、ネイティブメニューを出せなかったときのフォールバックにしか現れない。
    差し込み口自体はあるので、そこへ外部ツールを載せる (利用者判断 2026-08-29)。
  - **仮想パス (`C:\book.zip\page.jpg`) を渡す方針は採らない**。Win32 のファイル API では
    存在しない扱いになることを実測で確認した (シェル名前空間でのみ解決し、しかも ZIP 限定)。

### 1.118 任意の実ファイルを参照だけで束ねる「コレクション」 — 外部SNSでの移行検討者の指摘

- 出典: 2026-08-24。ZipPla / NeeView からの移行に「仮想ディレクトリ」が不足するとの外部SNS投稿。
  直接受けた要望ではなく、相手が期待する詳細操作は未確認。ZipPla の仮想フォルダや NeeView の
  プレイリストに相当する、手動で選んだ分散ファイルの参照一覧を想定して候補化する。
- **既存機能との差**:
  - スマートフォルダは複数 root と条件から毎回結果を作るため、ZipPla の「スマートフォルダ」相当。
  - 製本は追加時点のページを実ファイルとしてコピー / 焼き込みするため、元ファイルへの参照一覧ではない。
  - 任意項目を手動登録し、元の場所に置いたまま1フォルダのように見せる機能は無い。
- MVP 案:
  - 内部名も ZIP / PDF の `virtual folder` と衝突させず、利用者向け名称を「コレクション」とする。
  - `TopLevelGridSurface::Collection` を追加し、専用 DB worker が
    `CollectionDefinition` / `CollectionEntry { id, collection_id, source_path, order }` を所有する。
    設定 JSON の巨大配列や UI thread の同期存在確認にはしない。
  - 対象はまず実フォルダ / 実ファイル / アーカイブ本体のみ。コレクション入れ子と ZIP / PDF 内の
    仮想ページは対象外。追加、手動順序、コレクションから外す、元の場所を開くを提供する。
  - 「コレクションから外す」と「元ファイルをごみ箱へ移す」を明確に別操作・別確認にする。
- identity / lifecycle:
  - mIV 内の rename / move は既存 path migration の同じ transaction から entry を更新する。
  - OS 側で移動された項目は推測で別ファイルへ結び付けず、missing 表示と削除 / 再リンクを提供する。
  - 外部から任意パス一覧を読み込む機能は、UNC への意図しないアクセスや他人由来リストの危険があるため
    MVP に含めない。追加する場合は明示 import と確認を別途設計する。
- 規模 / 優先度: 物理項目だけの MVP Medium〜Large (1〜2週間)、仮想ページ・移動追跡・完全な
  D&D 管理まで Large (2〜4週間) / P3。着手前に利用者が期待する「仮想ディレクトリ」の操作例を確認する。

### 1.119 横長画像1枚を左右の表示ステップへ分割して読む — 外部SNSでの移行検討者の指摘

- 出典: 2026-08-24。ZipPla / NeeView からの移行に「見開き分割」が不足するとの外部SNS投稿。
  NeeView の「横長ページを分割する」(横長画像を左右へ分けて順番に読む) に相当する機能を想定する。
- **採用する簡略仕様 (2026-08-24)**: 分割した左右を永続的な論理ページにはしない。元の item index を
  正本のまま維持し、フルスクリーン内だけで `PageSlice::Full | Left | Right` に相当する一時的な表示位置を
  持つ。同じ texture の UV を左右半分へ crop し、1つの元ページを2回の表示ステップとして読む。
- 設定 / 分割判定:
  - 既存のページ構成プルダウンへ排他的な表示モードとして「横長分割 左→右」「横長分割 右→左」を
    追加する。「1ページ表示 / 通常の見開き」と組み合わせる独立 bool にはせず、組み合わせ状態を増やさない。
  - 保存済み回転を反映した後に横長となる静止画ページだけを 50% 位置で分割する。左→右は左半分から、
    右→左は右半分から表示する。分割率の手動調整は MVP に含めない。
  - 自動表示トリムは分割対象ページでは無効にする。分割の適用条件と UV crop は1つの resolver に集約し、
    ページ送り、描画、ナビゲータ、ルーペ等で左右の解釈を重複させない。
- 元ページ単位のまま維持する機能:
  - ★、タグ、ブックマーク、読書位置、補正、注釈、切り取り等はすべて分割前の元ページへ記録する。
    編集画面も分割前の画像全体で開き、左右別の編集データは持たない。
  - サムネイルは分割前の画像を使う。ブックマーク、履歴、検索、シークバー等から再び開いた場合は、
    選択した分割方向の最初の半分へ着地する。左右どちらを見ていたかは永続化しない。
  - シークバーのページ数とノブ位置は元ページ単位のままとし、左右間の移動では変えない。スクラブ時も
    対象ページの最初の半分へ着地する。表示中だけ「12ページ・左側」のように片側を示して混乱を避ける。
- ナビゲーション / 連結読み:
  - 通常のページ送りは `(source_idx, PageSlice)` の typed な一時状態を1か所で進める。分割状態を複数の
    bool / `Option` へ分散させず、同じ元ページ内の左右移動と次の元ページへの移動を区別する。
  - 縦連結では、分割対象ページを同じ texture 由来の2つの crop 領域として縦に並べる。幅フィットで
    スクロール読みする用途を対象とする。横連結と通常の見開き表示には MVP では分割を適用しない。
- **現状の構造差**: 現在の `SpreadDisplayUnit` は物理 item index だけを単位とし、ページ送り、通過表示、
  表示確定も `fullscreen_idx` の変化を前提にする ([ui_fullscreen.rs:8169](../src/ui_fullscreen.rs:8169))。
  元 index が同じ左右移動でも表示変更として扱う typed presentation step と、縦連結の layout-only な
  2領域展開が必要。ただし seek / resume / bookmark / DB / サムネイルを論理ページ化する必要はない。
- detached 制約: 別ウィンドウ経路へ到達する前に [detached-rework-plan.md](detached-rework-plan.md) §2 を読み、
  owner 外へ分割状態を足す症状修正にしない。通常の本体フルスクリーンを先に完成させ、detached は R4 後に
  同じ presentation step を所有できる場合だけ拡張する。Remote は当初 MVP 対象外としていたが、
  **スマートフォンの縦画面で一番効く**ため 2026-08-26 に実施した ([web-remote-plan.md](web-remote-plan.md) §15)。
- 回帰条件: 左→右 / 右→左の往復、横長と縦長の混在、回転後の分割判定、先頭 / 末尾、キー長押し、
  縦連結、シーク、編集画面への出入り、ブックマーク再表示で元 index と表示片の不変条件を固定する。
- 規模 / 優先度: 通常ページ表示 Medium、縦連結まで含めて Medium〜Large (1〜2週間程度)。当初の
  logical page 全面対応 (2〜8週間想定) より範囲を大きく限定できる / P3。

**実装プランは [page-split-plan.md](page-split-plan.md)** (当たりを付けた結果・範囲の根拠・
次に触る場所)。以下は着手時の記録。

#### 純ロジックだけ先行 (2026-08-25、[page_split.rs](../src/page_split.rs))

配線はレーン A の `app.rs` 一括切替の後に回す
([next-cycle-work-lanes.md](next-cycle-work-lanes.md) §6.2)。待つ間に、共有ファイルを
触らない部分だけ作った。単体テスト 9 本。

- `PageSlice { Full, Left, Right }` + `uv_rect()` (50% 固定) + `is_half()`
  (自動表示トリムを無効にする条件でもある)
- `SplitDirection { LeftFirst, RightFirst }` の `first()` / `second()`
- `PresentationStep { source_idx, slice }` — **元 index が正本**で、slice は表示だけ
- `presentation_steps(nav, direction, is_split_idx)` — nav をステップ列へ広げる。
  **既存の見開きユニット生成と同じ述語渡しの形**にした。回転を反映した縦横比と
  「静止画か」は `is_landscape` / `is_spread_pairable_item` が既に持っているので、
  ここで読み直さない。寸法が未取得の item は 1 ステップになり、届いた後に組み直される
  (既存の見開きが `is_landscape` に対して持つ性質と同じ。分割のために読み込みを待たせない)
- `landing_step` — しおり / 履歴 / 検索 / シークから**分割方向の最初の半分**へ着地
- `step_forward` / `step_backward` → `StepMove::{WithinPage, ToAnotherPage, AtEnd}`。
  同じ元ページ内の左右移動と別ページへの移動を**型で**区別する (呼び出し側で
  `before.source_idx != after.source_idx` を組み立てると判定が散る)
- 縦連結も同じステップ列を使う。「同じ texture 由来の 2 領域を縦に並べる」順序は
  ページ送りの順序と同じものなので、別の列を作らない
#### モードと per-viewer 状態まで (2026-08-25、レーン A マージ後)

- `SpreadMode::SplitLtr` / `SplitRtl` を追加。**`all()` には入れていない**ので
  プルダウンには出ない (選べるのに何も起きない状態を master へ置かない)。
  見開きとは排他で、`is_spread` / `is_rtl` / `has_cover` はすべて偽のまま。
- `App::fullscreen_page_slice` + bundle 側 + `swap_field!` を追加。**元ページと同じ
  context 所有**にした。片方だけ App に置くと、context を切り替えたときに前の viewer の
  左右が次の viewer に残る。`viewer_context_audit` は通過 (既知 1 件のみ)。
- **回転したページも分割する** (利用者判断で最初のリリースに含める。回転したページだけ
  分割されないのは、使う側からは不具合にしか見えず報告が来る)。
  - 当初は「型付き `SourceCrop` へ作り替えるしかない (215 箇所)」と見立てたが、**外れ**。
    実際の原因は `DisplayedImageTransform::resolve` が**同じ矩形を 2 つの座標系として
    使っていた**こと (fit 倍率では回転後の `display_size` に掛け、UV では元画像空間の
    まま渡す)。回転すると両立しないので丸ごと捨てていた。**UV 側は元々正しかった。**
  - 修正は用途ごとに座標系を分けるだけだった。`rotate_bbox_to_display` を足し、fit と
    paint rect は表示空間、UV は元画像空間にした。写像は screen ↔ source と同じ
    `forward_uv` を使う。**`content_bbox` の型も 215 箇所も動かしていない。**
  - `effective_bbox` が降ろすのは**自由回転中だけ**になった (傾いた矩形の外接が広がる分の
    拡大量を解けないため)。自由回転は保存しない一時値なので、やめれば戻る。
  - 旧テスト `fit_rotation_and_trim_share_paint_and_hit_geometry` は「回転時 UV は全体」を
    固定していた。**制限を仕様として固定していたテスト**なので期待値を更新した。
  - 詳細は [page-split-plan.md](page-split-plan.md) §2、正本は
    [display-pipeline.md](display-pipeline.md)「部分矩形 (content bbox) の座標系」。
- **表示トリムはまだ回転ページで効かない** (2026-08-25 訂正。一度「効くようになった」と
  書いたが誤り)。変換側で扱えるようになっただけで、**トリムを作る側に同じ規則の複製が
  4 か所残っている** (`capture_fs_display_unit_*` の単ページ / 見開き、Z ズーム、連結読み)。
  そのコメントは「描画側が bbox を使わない**ので**」と消費側を理由に挙げており、対の片方
  だけが変わった状態。**外すのは別件** —— 見開きの左右そろえが表示空間で定義されていて、
  180 度回転で左右が入れ替わるページに素直には適用できない。分割は自前の resolver
  (`fs_page_content_bbox`) から矩形を出すのでこのガードを通らない。
- **ページ送り / 描画 / 着地 / 縦連結まで実装済み** (2026-08-25)。通常表示は実機確認済み。
  - 描画の解決は `draw_fs_image` が所有する。最初は呼び出し側で解決させていて、
    `content_bbox` を作る 6 か所のうち**通常表示の 1 か所を通し忘れ**、ページ送りだけ
    半分ずつ進んで絵は横長のまま、という状態を実機で出した。数えて塞ぐのではなく、
    実際に描く 1 か所が所有する形へ変えた。
  - 縦連結は同じステップ列から段を組む。現在位置は左右まで見て選ぶ (同じ元 index の段が
    2 つ並ぶため)。連結の寸法計算にも `content_bbox` と座標系のずれがあり、同じ形で直した。
  - **スクロールで段が変わったときに左右を書く人がいなかった** (実機で発覚)。現在位置が
    毎フレーム元の段へ戻され、スクロールが引き戻されて先へ進めなくなっていた。
    表示は正しく入れ替わっていたので、**描画は合っていて現在位置の追従だけが
    取り残されていた**形。`reanchor_continuous_reading_viewer` が `fullscreen_idx` と
    同じ場所で左右も書くようにした。
  - **同じ形の取りこぼしを 2 回踏んだ**: 「元 index は変わらないが表示位置は変わった」
    という状態を、元 index しか見ていない既存コードが取りこぼす。1 回目は描画側
    (`content_bbox` の producer)、2 回目は現在位置側。分割のように**既存の識別子を
    細分化する機能**では、その識別子を読んでいる場所を先に数えるべきだった。
- **「分割を選んだのにページがつながったまま」の実機報告を解決** (2026-08-25)。
  再現手順が分からなかったので、**先に理由を型で残す計装を入れた** (`SplitDecision`)。
  次の再現で `[split] idx=0 decision=dimensions_unknown` が出て、そこから確定した。
  - `is_landscape` は寸法が未取得でも `false` を返す。見開きのペアリングでは困らないが、
    分割では「まだ分からない」と「縦長」が別の意味を持つ。探索と判定を分けた。
  - 真因は**収穫側**。PDF を開き直すと items 世代が変わり `page_dims_cache` が失効する。
    そのページは retained composite から復元されて表示できるので `fs_cache` にも
    サムネイルにも載らず、**供給源がゼロ**になっていた。寸法自体は retained 側が
    持っている (ログに `source=1512x1921` と出ていた)。
  - `harvest_page_dims_from_fs_cache` → `harvest_page_dims` に変え、**表示できる 2 経路**
    (live cache / retained composite) の両方から収穫するようにした。
- **残り**: 縦連結の実機確認、製品ページへの追記、リリース時の更新履歴。

### 1.122 F12 で動画をメイン ⇄ 別ウィンドウへ往復させると重い — 主因は解消、残り 1 件

- 主因 2 つ (16ms pump のイベント駆動化、`egui_overlay` の wgpu device 共有) は
  v3.3.0 で出荷済み・実機確認済み。switch total 231.8 → 198.1ms、動画を開いている間の
  UI フレーム 121.7/秒 → 48.0/秒。詳細な計測記録はコミット履歴を参照。
- **残り: `overlay_first_render` (37.2ms)**。overlay ごとに新しい `egui::Context` を
  作るので font atlas を毎回建て直している。次に見るならここか `publish` (`SetFocus`)。
- `d3d11_device` の共有は**見送り確定**。1 device に immediate context は 1 本しか作れず、
  `Present` まで同じ submission owner に寄せる必要がある。共有 `GpuVideoDevice` で
  immediate context 競合による hard-stuck を既に経験済みなので、26ms のために戻らない。
- ⚠ **guard / delay / retry で待ち時間を隠さない。**
- 規模 / 優先度: Medium / P3 (残りは体感上実用域)。

### 1.123 スレッド終了中の heap 例外を、例外ハンドラ自身が二次クラッシュで握り潰す — 実機クラッシュ

- 出典: 2026-08-25、R2e ステージ④の smoke 準備中。タグを付けて上のフォルダへ移動した直後に
  `mimageviewer-core.exe` が **0xc0000005 で異常終了**。`panic.log` に記録なし
  (= Rust panic ではない)。**R2e とは無関係の既存不具合**
  ([logger.rs](../src/logger.rs) も [lib.rs](../src/lib.rs) の handler も
  このブランチでは 1 行も変更していない)。
- **WER のフルダンプから取得したスタック** (`cdb -z <dump> -c ".ecxr; kn"`)。下から読む:

  ```
  0b ntdll!RtlUserThreadStart
  0a kernel32!BaseThreadInitThunk
  09 ntdll!RtlExitUserThread          ← スレッドが終了処理に入っている
  08 ntdll!LdrShutdownThread          ← DLL_THREAD_DETACH / TLS デストラクタ実行中
  07 ntdll!LdrShutdownThread
  06 ntdll!RtlFreeHeap+0x726          ← ここで一次例外が発生
  05 ntdll!KiUserExceptionDispatcher
  02 mimageviewer_core!mimageviewer::native_exception_handler+0xa0   ← 自前ハンドラが走る
  01 mimageviewer_core!mimageviewer::logger::current_thread_id_num+0x19
  00 mimageviewer_core!std::thread::current::current+0xf   ← 二次 AV (rcx=0)
  ```

- **問題は 2 つある。**

  **(A) 一次例外**: スレッド終了中の `RtlFreeHeap` で例外。ヒープの不整合か、
  TLS デストラクタ順序に依存した解放。直前のログは**フォルダ移動でワーカー 12 本を
  停止して 12 本を起動**した箇所 (`w0 stopped` … `spawning 10 regular + 2 I/O workers`)。

  **(B) 自前ハンドラが、その文脈で走れない**:
  [lib.rs:693](../src/lib.rs:693) の `native_exception_handler` は
  `logger::current_thread_id_num()` ([logger.rs:272](../src/logger.rs:272)) を呼び、
  その中の `std::thread::current()` が **TLS 依存**である。
  `LdrShutdownThread` 以降は TLS が破棄済みなので `rcx=0` を deref して二次 AV になる。
  **ハンドラが自分で死ぬので `panic.log` に何も残らない。**

- ⚠ **(B) を直しても (A) は直らない。だが (B) を直さないと (A) は永久に見えない。**
  今は例外ハンドラが証拠ごと消している。**(B) を先に直す。**
- (B) の直し方: 例外ハンドラの内側では **TLS に触れる API を使わない**。
  スレッド ID は `std::thread::current().id()` ではなく **`GetCurrentThreadId()`** を使う。
  ハンドラ経路が通るログ整形すべてを同じ基準で見直す (`format!` の allocator も
  ヒープ破損時は危険なので、固定バッファ + `write!` が望ましい)。
- (A) の手掛かり: 一次例外は `RtlFreeHeap` の中。`Application Verifier` の page heap か
  `_NT_GLOBAL_FLAG` の heap tail checking を有効にして再現すると、壊した側が特定できる。
  ダンプは `C:\Users\<user>\AppData\Local\CrashDumps\` に残る (今回は 59.6 GB)。
- 規模 / 優先度: Small (B) / Medium (A)。**P2** — 通常操作 (タグ付け → フォルダ移動) で落ちた。

### 1.128 ★固定で範囲外になった別窓を、閉じず自前の一覧へ切り出す — 仕様提案

- 出典: 2026-08-26、§1.125 の実機確認。複数ウィンドウモードで動画再生中に
  ★固定を押し、**そのファイルが固定範囲外だったため動画窓が閉じた**。
  **§1.125 の設計どおりの動作**であり、不具合ではない。ただし体験としては
  **グリッドのボタンを押したら別の窓が消えた**に見える。
- 利用者の希望: **再生はなるべく継続**し、前後移動も納得できる形で残す。

#### 前提の確認 (2026-08-26、コードで裏取り済み)

「★固定していないときは実 FS 順で前後移動する」という想定は **実装と違う**。

```rust
fn build_nav_indices(items: &[GridItem], visible_indices: &[usize]) -> Vec<usize>
```

`get_nav_indices()` は `current_grid_order()` (= `visible_indices`、Details なら `details_order`)
を渡す ([ui_fullscreen.rs:7621](../src/ui_fullscreen.rs:7621) /
[app.rs:46141](../src/app.rs:46141))。つまり **★固定の有無にかかわらず、
前後移動は常にグリッドの表示順・絞り込み結果を辿る**。実 FS 順を辿るモードは存在しない。

したがって ★固定は「別の並びに切り替える」操作ではなく、
**その時点の並びを凍結する**操作である。

#### 提案 — 別窓を自前の context へ切り出す

現状、複数ウィンドウモードの動画窓は **同じ context を別の窓で描いている**
(実機ログ: `main_fs_idx=Some(121) mounted=true`)。だから親のグリッド操作が直撃する。

**器は既にある**:

- 2026-08-26 に `snapshot` を `ViewerContextBundle` の field にしたので、
  **context ごとに別の並び・別の凍結状態を持てる**
- `fork_mounted_live_media_context` が「再生中メディアを自前の context へ切り出す」既存の仕組み

よって §1.125 の miss 経路を「閉じる」から「**切り出す**」へ変えることで、
窓は自分の items を持ち続け、再生も前後移動も維持される。

#### 詰めるべき点

- 切り出した context の一覧は **固定前の並び** を保持する。それを利用者にどう示すか
  (窓のタイトル / HUD に何か出すか、無言でよいか)
- 親が ★固定を**解除**したとき、切り出した窓を親へ戻すのか、独立のままにするのか
- 切り出した context の寿命 (窓を閉じるまで / 親のフォルダ移動まで)
- 静止画でも同じにするか、メディア限定にするか
- ⚠ detached 述語に触れるので §2 の手続きが必要

- 規模 \\ 優先度: Medium / P3 (不具合ではなく仕様改善)。

### 1.135 動画レンダースレッドがパニックすると、以後どの動画も再生できなくなる

- 出典: 2026-08-27 実機。シークバーのストリップを開いた直後に
  `native video render fault: native video render thread panicked` が出て、**以後どの動画を
  開いても同じエラー**になった。アプリを再起動するまで戻らない。
- **直接の原因になったパニックは §1.112 の作業中に直した** (波形テクスチャ幅が GPU 上限を
  超えていた。`WaveSpanRequest::pixel_width` に上限が無く 4K 幅 × 3 = 11520px を要求していた)。
  **本項はその先の構造の話**: パニックの原因が何であれ、一度死んだら復旧しないこと。
- 観測できた事実:
  - panic.log に**同じ形の `Device::create_texture` パニックが 2026-08-10 (8320px) と
    08-14 (9000px) にも残っている**。つまり以前から起きており、そのたびに再生不能に
    なっていたはず。利用者が「動画が再生できない」と気付いた時点では、原因のストリップ
    操作から離れているので、結び付けにくい。
  - スレッドは `catch_unwind` で包まれており ([video/mod.rs:1922](../src/video/mod.rs))、
    パニック自体は捕まえてログに残る。しかし**その後スレッドを作り直さない**ので、
    presenter が居ないまま `native video render fault` を返し続ける。
- 判断が要る点:
  1. **作り直してよいのか。** パニックの原因が GPU リソースの不整合だと、作り直しても
    同じ場所で落ちてループする。無条件の再生成は「症状パッチ」になり得る。
  2. 作り直すなら**回数を制限し、理由を型で持つ**必要がある (`VideoScaleFallbackReason` と
    同じ作法)。何度目で諦めるか、諦めたら何を見せるかを決める。
  3. **少なくとも利用者への見せ方は変えられる。** 現在の「動画を再生できません:
    native video render fault」は内部語で、再起動すれば直ることが伝わらない。
- 参考: 静止画側には同種の「GPU 経路が死んだら次のフレームで作り直す」機構が無いので、
  前例として使えるものは無い。
- 規模 / 優先度: Medium / P2 (実害はデータ喪失ではないが、**再起動するまで動画機能が
  丸ごと死ぬ**。原因側を潰しても別のパニックで再発し得る)。

### 1.130 font atlas resync 待ちが 16ms 固定でスピンする — §1.115 option (d) で解消

- 出典: 2026-08-26、§1.122 の repaint 要求元を分けているときに見つけた。
  **§1.122 と owner も原因も違う**ので別項目にした (憲法 §2 規則 7)。
- [`maybe_defer_for_main_font_atlas_resync`](../src/app.rs) は、main font atlas resync が
  pending の間 `!safety.is_settled()` だと
  `ctx.request_repaint_after(16ms)` して `false` を返す。
  つまり「**フレームが settled になるまで 16ms ごとに見に行く**」。
- 呼び出し元が **2 つ** (`update_early` / `pre_main_ui`) なので、
  1 フレームに **2 回**発行される。

#### 実測 (2026-08-26、§1.129 修正**後**のログ)

| | 値 |
| --- | --- |
| 発火したフレーム | **343 / 1930 (17.8%)** |
| 延べ発行回数 | 672 (= 1 フレーム 2 回) |
| 分布 | 連続ではなく **3 回のバースト** (0.3s / 0.3s / 3.8s、計 ≈ 4.4s) |

§1.122 の pump (98.7% を連続で埋める) と違って常時ではないが、
**atlas rebuild を止めた後でも残っている**ので、§1.129 では消えない別経路。

#### 解消 (2026-08-27)

§1.115 の実測と所有境界再監査により、viewport teardown は shared renderer の font atlas を
所有せず、`d48982e5` の保守経路が補っていた dropped-upload premise も backend 側で解消済みと
確定した。したがって settled event へ polling を置き換えるのではなく、lifecycle resync /
`safety` / 16ms polling 自体を撤去した。placement / cloak / opening / closing は font owner では
なくなり、実 UI font 設定変更だけを main context の one-shot pending が所有する。

- 規模 / 優先度: — (解消済み)。

### 1.134 common modal がツールバー / メニューバーのポインターナビを止めていない — 未対応

- 出典: 2026-08-27、§1.133 の調査中に Codex が発見。**archive convert 固有ではなく、
  全 common-modal window に関わる入力ゲートの不統一**。

`common_modal_dialog_open` に登録された window は、キーボードショートカット /
グリッド open / 背面 wheel / フルスクリーン入力を止めるが、**ツールバーの
お気に入りボタンなどのポインターナビは止まらない** (`any_dialog_open()` で
無効化されていない)。

変換進捗ウィンドウ表示中でもお気に入りクリックでナビできるのはこれが原因。
これ自体は §1.133 の修正対象ではない (preflight が拒否するので安全) が、
**モーダルの意味が入力種別で違う**のは設計として不揃い。

⚠ `archive_convert` を `archive_convert.is_some()` で一括登録するのは**誤り**。
意図的に非表示にしている `Scanning` 中まで操作不能になる。現行の
`archive_convert_dialog_visible()` 経由の登録が正しい。

- 規模 \\ 優先度: Medium / P3 (出荷ブロッカーではない)。

### 1.136 detached placement のテストが、実機のモニター配置に依存して揺れる — 未対応

- 出典: 2026-08-27、video-strip セッションがちらつき調査中に見つけ、
  `docs/detached-close-flicker-handoff.md` §8 で引き継いだもの。同資料は「壊れているのでは
  なく揺れている」と正しく区別した上で、**何が揺らしているかは未特定**としていた。

```
app::tests::still_window_mode_key_tests::detached_builder_placement_latch_does_not_follow_live_drag_updates
```

実行ごとに結果が変わる (単独実行で FAILED → 直後に 3 回連続 ok、フル実行でも両方あり)。
失敗時の値は `{x:80, y:80, w:960, h:720}` = 設定既定。

#### 原因 (2026-08-27 特定)

**共有状態でも実行順でもなく、テストが実機のディスプレイ構成を見ている。**

`set_detached_window_runtime_placement` は `detached_window_runtime_placement_is_usable` を通らない
placement を **無言で return** する ([app.rs](../src/app.rs) `runtime_placement_rejected` は
debug ログのみ)。その判定は:

```rust
placement.is_sane() && crate::monitor::title_bar_on_some_monitor(placement.x, placement.y, placement.w)
```

`title_bar_on_some_monitor` ([monitor.rs:61](../src/monitor.rs:61)) は **`MonitorFromPoint` を
`MONITOR_DEFAULTTONULL` で直接呼ぶ**。テスト用の差し替えは無い。

テストの `initial` は `x:1564, y:240.66667, w:1167` なので、検査点は
**(2147, 255)** 。ここがどのモニターにも乗らない瞬間 (ディスプレイのスリープ /
構成変更 / DPI 変更 / ロックなど) に実行されると placement が保存されず、
`active_detached_seed_placement` の `unwrap_or_else` が設定既定へ落ちる。
**観測された失敗値と一致する。**

#### 直す方向

- latch の振る舞いを見るテストが OS 問い合わせを通るのが問題。
  `title_bar_on_some_monitor` にテスト override を与えるか、placement 検証を注入可能にする。
- **無言の拒否を残さない**。`set_detached_window_runtime_placement` の拒否は debug ログだけなので、
  探す側には何も見えない。型で返すか、少なくともテストから見える形にする。
- 同形の依存が他のテストにも無いかを見る (`title_bar_on_some_monitor` /
  `MonitorFromPoint` / モニター列挙を通る検証全般)。

- 規模 \\ 優先度: Small ～ Medium / P2 (出荷ブロッカーではないが、
  **リリース前の全体テストが理由なく赤くなる**ので早めに閉じる)。

### 1.137 F12 で別ウィンドウへ入るときだけ、中身の無いホストが 380ms 先に見える — R2b + retire follow-up 実装済み、実機計測待ち

> ⚠ detached viewer リワーク中の領域。症状パッチ (delay / guard / 追加 repaint) を入れない。

- 出典: 2026-08-28、利用者の観察「F12 を押すと、動画のウィンドウサイズをいちど
  真っ白に表示してから、最大化して、その後再生しているように見える」を
  ログで裏付けたもの。**§1.115 の font atlas resync を撤去した後も残るちらつきの主因候補**。

#### 実測 (build C = font work 撤去後のログ)

```
33.192  [native-video] defer placement switch until detached host is ready: target=DetachedWindow
33.192  [native-video-key] F12 (seq=13)
33.489  [detached-viewer] registered host hwnd=0x2280fde        <- ホストはもう見えている
33.568  [native-video] resume deferred detached placement switch after 376.5ms
33.574  [native-video] window created-hidden: hwnd=0x4982e4e rect=(0,34 3840x2054)
33.701  [native-video] window shown: hwnd=0x4982e4e visible=true
```

**ON 側だけが非対称に遅い**。OFF (→ Fullscreen) には遅延が無く、押下から 7ms で
presenter window を作り、`placement switched ... total=139.0ms` で完了する。

内訳: ホスト viewport の生成・登録に約 297ms、登録を検知して resume するまでに約 79ms。

#### 見立て (未検証)

ホストの viewport は `activate=true` で**先に可視になり**、その時点では動画がまだ attach
されていないので中身が無い。376ms 後に全画面サイズの presenter window が上に乗るので、
「白 → 最大化 → 再生」と見える。直すなら**見せる順序の所有権** (presenter が publish するまで
ホストを見せない) を決める形になる。**遅延を短くする・待ちを入れる等の症状パッチは不可**。


#### 同じ根を持つもう一つの症状: タスクバーの明滅 (2026-08-28 利用者報告)

同じログ (約 115 秒の 1 セッション) でのウィンドウ生成回数:

| 契機 | ホスト生成 |
| --- | --- |
| 起動時の動画フルスクリーン | gen=1 |
| **F12 を 12 回 = 6 往復** | gen=2〜7 (**1 往復につき 1 個**) |
| その後の別のフルスクリーン操作 (F12 ではない) | gen=8〜13 |

native-video window は 15 (`detached-viewer-child` 9 + `fullscreen-borderless` 6)。

**回数が多いのではない** —— F12 1 往復につきホスト 1 個で、無意味な繰り返しは無い。
問題は **F12 OFF でウィンドウを捨てている**こと。そのため 1 往復でタスクバーボタンが
「消える」「現れる」の 2 回変化し、6 往復で 12 回の変化になる。隠して再利用すれば 0 回。

> 調査中に `open_fullscreen` が 155ms で 4 回出ているのを一度異常と見たが、**誤り**。
> `idx=1 → 3 → 5 → 7` で `input_seq` も 17→20 と進んでおり、PDF の見開きページ送り。
> ウィンドウは作られていない (この間のホストは gen=9 の 1 個だけ)。

detached ホストの builder は `.with_taskbar(true)` ([ui_fullscreen.rs](../src/ui_fullscreen.rs))。
つまり **F12 のたびにトップレベルウィンドウを破棄して作り直しており、タスクバーボタンが
消えては現れる**。白いウィンドウと同じ根 (= ウィンドウを再利用せず毎回新規に作る)
なので、別起票にせずここで扱う。

修正の方向も共通で、「**ウィンドウの寿命を F12 のトグルと切り離す**」か、
少なくとも「**見せる順序の所有権を決める**」ことになる。リワークの
`DetachedWindowRuntime` / 状態 enum が扱う領域なので、先にプラン §2 を読むこと。

#### 測定: タスクバー自体が 1 フレームごとに出入りする (2026-08-28)

利用者の「タスクバーが何度も明滅する。**アプリのアイコンではなくタスクバー自体**」という
指摘を受けて、キャプチャの下端 18px をフレームごとに分類した (明るい = タスクバー可視)。
**F12 1 往復で 16 回反転していた**。

| 遷移 | 区間 | 反転回数 |
| --- | --- | --- |
| F12 #1 (→ fullscreen) | 6.233ー6.567s (334ms) | **9** |
| (安定) | 6.567ー9.567s | 0 |
| F12 #2 (→ detached) | 9.600ー9.933s (333ms) | **7** |

ほぼ 1 フレーム (33ms) ごとの交互。利用者には「3 回くらいの点滅」と見えていた。

ログとの対応 (F12 間隔がログ 3.47s / キャプチャ 3.37s で一致):

- **F12 #1**: fullscreen 窓の生成→表示→`placement switched` (138ms) の前後に集中。
- **F12 #2**: **押下からホスト viewport が表示されるまで**の 333ms に集中し、
  動画の子ウィンドウが作られる前に終わっている。

つまり閃光の正体は「DWM が再合成している」という漠然としたものではなく、
**遷移中に「全画面を覆うウィンドウがある / ない」が毎フレーム入れ替わっている**こと。
上の白いホストと同じ 330〜380ms の窓で起きている。

対応する geometry: `fullscreen-borderless` = (0,0 3840x2160) はタスクバーを含む全面、
`detached-viewer-child` = (0,34 3840x2054) は含まない。遷移中は両方が短時間共存し、
z-order / show / raise / destroy のたびにどちらが手前かが入れ替わる。

> 注: タスクバーボタン (アイコン) の出入りではない。動画窓は 2 つとも
> `native_window_owner_for_placement` で owner 付き (= タスクバーに出ない)。
> ボタンを持つのは detached ホスト viewport (`with_taskbar(true)`) だけで、1 往復 2 回。
> 当初私はこのボタン側を数えており、**利用者の指摘で対象を間違えていたことが分かった**。
- 関連: §1.115 (font atlas 側。**別機構**。破棄フレームは 14 → 0 になったがちらつきは残った)。
- 規模 \ 優先度: Medium ～ Large / P2。

#### R2b 実装結果 (2026-08-28)

- `pending_detached_video_host_switch` と `native_video_mode_switch` を廃止し、context-owned
  `PresentationTransitionOwner` (`Stable → Preparing → Ready → Committing → Stable`) へ統合した。
  current / target / request generation / activation intent と candidate/prior HWND を 1 request が
  所有し、F12 再入力、failure、Esc、window close、player end、stale Ready/Commit を reducer で
  解決する。
- native contract は hidden candidate の create/attach/prime を `Ready` までに済ませ、`Commit` で
  初めて publish する。`NativeCommitted` で host `Visible/Focus` を先に発行し、同じ effect batch で
  outgoing を retire する。incoming host の OS visibility poll は retire の前提にしない。
  fixed-ms commit / forced recovery は無い。failure/abort は hidden candidate だけを cleanup する。
- host `Visible/Focus/Destroy` と native `Publish/Destroy` を reducer effect に限定し、遷移中の
  presenter/HUD/VST/focus recovery は同じ permit を読む。実 action は transition id / target / HWND
  付き `[presentation-transition]` ログに出る。既存 `[ui-frame-gap]` / `[atlas-probe]` は変更していない。
- 自動回帰は両方向、abort、F12 再入力、Esc、window close、player end、stale generation を含む。
  出荷判定前に実機 1 往復で **outgoing presenter raise 0 / cover change 各方向 1 /
  content-ready 前 host activation 0** を画面キャプチャとログで照合する。

#### build I: publish 済み outgoing presenter の retire が UI sleep に依存 (2026-08-28 follow-up)

```
18.726  Publish incoming presenter
18.727  placement committed
18.732  host Visible / Focus
19.216  shared output pool exhausted (以後約500msごと)
24.630  [ui-frame-gap] 5899.4ms
24.632  Destroy outgoing presenter        <- window click 直後
24.661  placement retired
```

`Committing::AwaitingHostVisible` が、次の `App::update` による `IsWindowVisible(host)` の level poll まで
`RetirePlacement` を出さなかった。native `PlacementCommitted` / `PlacementRetired` は lossless event bus
から root viewport を wake するが、その間の `HostVisible` は UI-only event である。poll 末尾の
`request_repaint` と visibility / focus / redraw の window event が次 pass を起こす想定だった。
build F の約 70ms 完了も第2 pass は必要で、incidental window event に起こされただけだった。

`DetachedHostDisposition` はこの依存を導入していない。問題の `Fullscreen → DetachedWindow` では
disposition は `None` で、変更差分にも `AwaitingHostVisible` の変更は無い。build I は偶発 wake の
無いスケジュールで既存の依存を露出させた。

candidate は `NativeCommitted` が届く時点で create / attach / current-frame prime / pump publish 済み。
ここを presenter ownership の cutover とし、commit effect を
`ApplyPresentation → Visible → Focus → RetireOutgoing` に変更した。host command は先行させるが、
OS visibility confirmation を retire の前提にはしない。`AwaitingHostVisible` / `HostVisible` poll は
撤去し、timeout / retry / settle window / 追加 repaint は入れていない。

GPU output pool は 16 slot、source queue は最大 8、copy-fence retire queue は最大 4 で、candidate
prime の短い二重生存は設計内。二 presenter の zero-overlap 制約ではなく、5.9秒残った旧 fence owner
が pool exhaustion を起こした。容量 tuning ではなく lifecycle ordering の correctness defect である。

回帰テスト
`fullscreen_to_detached_retires_on_native_commit_without_waiting_for_host_visibility` は commit batch の
順序と `AwaitingRetire` を固定する。retire を incoming host event の後ろへ戻す killing mutation では
commit batch から `RetireOutgoing` が消えて失敗する。

次 run では `shared output pool exhausted waiting for free slot` の連続、同区間の約5.9秒
`[ui-frame-gap]` が無いことを確認する。`Destroy outgoing` は `placement committed` の直後、
同じ UI pass で送った retire command の pump / WM teardown 分だけ後に並ぶ。固定msを受け入れ条件には
しないが、build F と同じ数十ms級で、秒単位や user input 待ちにならないことを timeline で照合する。

R4 に残るのは `show_viewport_deferred`、single render entry、host persistence。今回も F12 OFF は
terminal に host HWND を破棄するため、次の ON では約 300ms の hidden host 作成を待つ。
この待ち時間自体の短縮、F12 を跨ぐ taskbar button/host identity の永続化は本項の R2b close には
含めず、R4 gate C の仕様決定後に扱う。

### 1.138 F12 往復で `active detached backstop window N has no context binding` で落ちる — 実機クラッシュ

- 出典: 2026-08-28、利用者が F12 を何度か押してクラッシュ。**2 日で 2 回踏んでいる**。
- **今日の §1.137 (遷移所有者) の退行ではない。** 遷移所有者を含まないビルド C でも
  同一メッセージで落ちている (`panic.log` 2026-08-28 00:23:56、frame 10 = `App::update`)。
- アサーション自体は **2026-08-25 `bf391e6a`** (R2e の所有権整理) で入った。

```
PANIC at src\ui_fullscreen.rs:13611:21: active detached backstop window 1 has no context binding
```

#### ログ (ビルド F、遷移所有者あり)

```
16.131  [presentation-transition] id=3 target=Fullscreen effect=Publish  hwnd=0x7e0b0a
16.193  [native-video] placement committed placement=fullscreen-borderless request=3 generation=4
16.198  [presentation-transition] id=3 target=Fullscreen effect=Destroy  hwnd=0x382c58
16.402  PANIC ... active detached backstop window 1 has no context binding
```

3 回目の F12 (detached → fullscreen) の **204ms 後**。遷移そのものは成功しており
(`Raise` 0 件、`Publish` が `Visible`/`Focus` より先)、その直後に落ちている。

#### 観測で訂正した状態 (2026-08-28、次の実機 run)

前節の「session が生きたまま binding が先に消えた」という方向は、追加計装で**反証された**。
実際の session producer は 2 本あり、通常 open は binding 済み、native placement commit は
binding 無しで session を新規作成していた。

```
seq=1  set caller=src\\app.rs:43150              window 1 binding=ViewerContextId(0)/Mounted
seq=3  set caller=src\\app\\native_video.rs:3604 window 1 binding=none
seq=13 set caller=src\\app\\native_video.rs:3604 window 3 binding=none
```

seq=3 / 13 の前に unbind は無い。`apply_video_presentation_switched` が
`ensure_detached_viewer_window_id()` の後、context binding を確立せず
`begin_active_detached_session(id, Video)` を呼んでいた。backstop はその不正な session を数百 ms
観測し続け、mount へ到達した frame で初めて `None` を検出した。したがって 1.138 の実原因は
binding の早期解放ではなく、**binding の無い window を名指す active session の生成**である。

同じ run は別の独立した transition owner 退行も確定した。detached host resync の
`DetachedWindow → DetachedWindow` 遷移が current host を outgoing host として request に保存し、
native retire 完了後にその live host へ `DestroyHost` を発行していた。F11 は detached 窓の
borderless 切替だけだが、その後の同一 presentation host resync が窓を閉じ、close request が
fullscreen / player を終了させた。

#### 原因と修正 (2026-08-28、観測後の訂正版)

active session は backstop が描画 owner を mount できるという公開済み所有権なので、unbound
window を指す中間状態は正当ではない。通常 open (`prepare_viewer_presentation_open`) が従来から
行っていた `bind_window(mounted, id)` を共通 helper へ集約し、native commit と egui F12 経路も
session begin より前に通す。context build の binding は commit 前非公開という I8 を守るため、
book session begin は build closure 内から commit 後・repaint 前へ移した。production の
`begin_active_detached_session` も Mounted / AtRest binding が無い開始を所有境界で拒否する。
backstop の契約と追加計装は弱めず、そのまま残す。

host 側は request の raw `outgoing_host_hwnd` を、`None / KeepLive / RetireOutgoing` の typed
disposition に置換した。成功時に live detached host を退役できるのは
`DetachedWindow → MainWindow/Fullscreen` だけである。`DetachedWindow → DetachedWindow` は
presenter の再構築 / 再親付けなので host を保持する。detached 候補の abort / failure と terminal
close による host cleanup は、この成功時退役とは別の lifecycle effect として維持する。

前回入れた `unbind_window` assertion、`retire_context` 内の session finish、handoff-before-unbind
は、今回観測された 1.138 の原因修正ではなかった。ただし binding 確立後に反対向きの違反
(session を残した release) を作らせない補完的不変条件であり、撤去しない。これらのテストは
「今回の再現原因」ではなく release / retire ownership hardening の回帰証明として位置付け直す。
A (host disposition) と B (binding-before-session) は同じ commit effect の約 30µs 内に並んだが、
状態 owner も壊した不変条件も異なる独立 defect である。

- 規模 \\ 優先度: Small 〜 Medium / **P1 (即死する。§1.137 の実機確認もこれで止まる)**。

### 1.141 Susie のクラッシュ対象 ID が「エントリ名 + 長さ」止まり

- 出典: 2026-08-27 の出荷前レビュー (Codex、機能別評価の Susie 行)。
- 機構: [susie_loader.rs](../src/susie_loader.rs) の `decode_bytes` は、クラッシュした
  入力を二度と同じプラグインへ渡さないための識別子を
  `format!("{filename_hint}#{}", bytes.len())` で作る。ZIP 内画像はパスを持たないため、
  名前だけでは別書庫の同名エントリを巻き添えにする — その対策として長さを足した経緯が
  コメントに残っている。
- 残る穴: **同名かつ同サイズで中身が違う**エントリは、まだ同一視される。
  片方でプラグインが落ちると、もう片方も開かれなくなる。
- 実害の大きさ: 落ちるのは既にクラッシュした後の話で、結果は「開けたはずの 1 枚が
  開かれない」。クラッシュや破損ではない。**同名同サイズは再配布された同一ファイルである
  ことが多く、その場合は巻き添えではなく正しい判断**になる。
- 直し方の候補: バイト列のハッシュを識別子に含める。全体ハッシュは decode 経路 (数 MB) に
  数ミリ秒を足す。先頭 + 末尾の固定長だけを混ぜる案なら実質ゼロだが、**どちらも実際の
  クラッシュを再現できないと検証できない**。
- 2026-08-27 の判断: **出荷前には入れない。** 失敗は限定的で自己修復可能 (再起動で解除)、
  一方で修正は decode のホットパスに触れ、実機で確かめる手段が無い。
- 規模 / 優先度: 小 / **P3**。

### 1.140 超広幅ウィンドウで波形ストリップの鮮鋭さが落ちる (分割描画の検討)

- 出典: 2026-08-27 の出荷前レビュー (Codex P2) をきっかけに判明。
- 経緯: `WaveSpanRequest::pixel_width` は「可視幅そのものが GPU 上限 (8192px) を超えても
  丸めない」を明示的な設計としており、コメントは**「テクスチャ生成側の責務として残す」**と
  書いていた。しかし [render_core.rs](../src/video/native_presenter/render_core.rs) の
  `sync_seek_strip_textures` は分割も縮小もせず `load_texture` を呼ぶだけで、
  **その責務は実装されないまま残っていた**。到達すれば必ずレンダースレッドが
  パニックし、以後どの動画も再生できなくなる (§1.135 と同じ終わり方)。
- 2026-08-27 の対応: 上限を要求側で無条件に効かせた
  (`an_oversized_visible_width_is_capped_at_the_texture_ceiling`)。
  **パニックは消えるが、可視幅が 8192px を超える構成では波形がわずかにぼやける** (伸縮のため)。
  位置は時刻由来の UV で決まるのでずれない。
- 残っていること: 鮮鋭さを取り戻すなら**分割描画**しかない。RGBA を N 枚のテクスチャへ分け、
  `waveform_texture_slice` の UV をタイル境界で割り、N 回 `painter.image` する。
  継ぎ目と、raster revision ごとの N 枚アップロードのコストを見る必要がある。
- 到達条件: 可視ストリップ幅 (物理ピクセル) が 8192 を超える構成。4K 2 面 (7680) では届かず、
  3 面またぎや 8K で届く。**開発機の仮想デスクトップ幅は 6001px なので、ここでは再現しない。**
- 規模 / 優先度: 中 / **P3** (パニックは解消済み。残るのは限られた構成での画質)。

### 1.139 タスクバー明滅の原因 = 遷移中のフォアグラウンド往復 (2026-08-28 実測で特定)

**§1.137 の本体。** 計装 (`[presentation-window]`) とキャプチャの突き合わせで確定した。

#### 何が起きているか

fullscreen → detached の遷移中、**新しい presenter を Publish する前の約 300ms**、
フォアグラウンドが「出ていく presenter」と「入ってくるホスト」の間を **約 100ms 周期で
3 往復**する。1 周期はこの形:

```
WM_ACTIVATE    presenter   fg=presenter
WM_KILLFOCUS   presenter   fg=<host>
WM_WINDOWPOSCHANGED presenter
WM_WINDOWPOSCHANGED main
WM_ACTIVATE    presenter   fg=presenter
WM_SETFOCUS    presenter
```

実測 (transition=14、キャプチャ時刻に換算):

```
26.643 ACTIVATE / 26.661 KILLFOCUS(fg=host) / 26.708 POSCHANGED x2
26.722 ACTIVATE / 26.725 SETFOCUS / 26.738 ACTIVATE
26.758 KILLFOCUS(fg=host) / 26.807 POSCHANGED x2
26.819 ACTIVATE / 26.823 SETFOCUS / 26.841 ACTIVATE
26.865 KILLFOCUS(fg=host) / 26.899 POSCHANGED x2
26.928 ACTIVATE / 26.935 SETFOCUS
27.133 ← ここでようやく新 presenter を Publish
```

`WM_WINDOWPOSCHANGED` が presenter と main の両方へ飛ぶたびに、Windows の
「前面が全画面ウィンドウか」の判定が変わり、**タスクバーが 1 フレームごとに出入りする**。
キャプチャ下端 18px の分類で同区間に 1 フレーム刻みの反転が出るのと一致する。

#### 誰がやっているか

**mIV 自身の `SetForegroundWindow` ではない。** この区間の記録は**すべて `source=wndproc`**
(= 受信側) で、全 46 秒の実行で mIV が出した `SetFocus` は 3 件のみ。
**ホストウィンドウの生成・表示そのものがフォアグラウンドを奪い、presenter が取り返す**
という OS レベルの綱引きである。

#### 発生条件: ホスト窓が「最大化」のときだけ起きる

同じ実行の全 30 遷移を、`phase=begin` から `[detached-viewer] registered host` までの
所要時間と、区間内の `WM_KILLFOCUS` 件数で並べると、2 群にきれいに割れる。

| ホスト窓 | 生成〜登録 | Publish まで | 余分な前面奪取 |
| --- | --- | --- | --- |
| 通常サイズ (約 1600x1200) | 15-17ms | 151-158ms | **0 回** |
| 最大化 (3862x2110 @ 4K) | 162-367ms | 387-554ms | **3 回** |

奪取のオフセットは約 40 / 130 / 210ms で、最後に Publish 時の正規の受け渡しが 1 回入る。
**「必ず 3 回」ではなく「生成が長引いた 100ms ごとに 1 回」**である。通常サイズでは
生成が 15ms で終わるので往復する暇が無く、奪取は Publish 時の 1 回だけになる。

利用者が今この明滅を強く感じるのは、detached 窓を最大化して使っているため。同じ実行の
ログでも、F11 で detached をフルスクリーン化した 36.5s 以降、ホスト窓の外形が最大化サイズ
(3862x2110 @ (-11,-11)) に変わったところから 3 回の奪取が始まっている。それ以前の通常
サイズの遷移 (id=3/5/8/12) では 1 回も起きていない。

#### なぜ遷移所有者では止まらなかったか

reducer の effect (`Raise` 0 件、`Publish` -> `Visible` -> `Focus` の順序) は設計どおり
出ている。**往復はその effect が出るより前、ホスト viewport が作られる過程で起きている**。
所有権を一本化しても、egui/winit がホストを作る際の activation は reducer の外にある。

#### 原因確定 (2026-08-28 第 2 回計測): `with_maximized(true)` と `with_visible(false)` が矛盾している

`WH_CBT` + `WH_CALLWNDPROC` + `SetWinEventHook` の計装で、機構がそのまま記録された。
遷移 2 の 1 周期 (t_us 基準、`window_create` stage の内側):

```
  0.4ms  stage window_create begin
  1.0ms  HCBT_CREATEWND  ws_visible=false ws_maximized=false   <- 要求どおり非表示・非最大化で生成
 13.0ms  HCBT_MINMAX     command=SW_MAXIMIZE(3)                <- winit が最大化を適用
 15.7ms  HCBT_ACTIVATE   other=host                            <- SW_MAXIMIZE は表示 + activate を伴う
 18.3ms  host  WM_ACTIVATE WA_ACTIVE  other=main
 36.5ms  host  WM_SHOWWINDOW shown=false                       <- visible=false により再び隠される
 40.8ms  HCBT_ACTIVATE   other=main                            <- activate が main へ戻る
 60.0ms  main  WM_ACTIVATE WA_ACTIVE  other=host
 74.6ms  HCBT_ACTIVATE   other=host                            (2 周目)
 84.2ms  host  WM_SHOWWINDOW shown=false
 90.2ms  HCBT_ACTIVATE   other=main
118.9ms  HCBT_MINMAX     command=SW_MAXIMIZE(3)                (3 周目)
123.3ms  HCBT_ACTIVATE   other=host
135.8ms  host  WM_SHOWWINDOW shown=false
137.9ms  HCBT_ACTIVATE   other=main
168.8ms  stage window_create end
```

**`ShowWindow(SW_MAXIMIZE)` は「表示する」と「activate する」を必ず伴う。** 一方
builder は `with_visible(false)` を要求しているので、winit は最大化直後にその窓を隠す。
隠された窓は前面でいられないので activate が main へ戻る。これが 1 周期で、
**`create_window` の内側で 3 回繰り返される**。

全 `-> detached` 遷移で完全に同じ数が出る。

| 計数 | 値 |
| --- | --- |
| ホスト窓の生成 | 1 |
| `SW_MAXIMIZE` | **3** |
| ホストの `WA_ACTIVE` | 4-5 |
| ホストの `WM_SHOWWINDOW shown=false` | 3-4 |
| main の `WA_ACTIVE` | **2-6** |
| `HCBT_ACTIVATE` | 7 |

#### これで前回の因果の読みは逆だったと分かる

前回私は「ホスト生成に 300ms かかり、その隙に OS が前面を再決定している」と書いた。
逆である。**show/hide/activate の空転そのものが 300ms を作っている。** stage 内訳が
それを示す: `surface_create` は 0.1ms、`surface_configure` は 3.5ms、GPU 側は誤差。
`window_create` の 168-360ms がまるごとこの空転である。

「生成が長引いた 100ms ごとに 1 回」という前回の言い方も相関の言い換えでしかなかった。
実際の駆動源は最大化であり、通常サイズの detached 窓で 1 回も起きなかったのは
`SW_MAXIMIZE` が呼ばれないからである。

#### 観測されたユーザー症状との対応

- **タスクバーが出たり消えたりする**: 最大化された窓が z 順の最上位に現れては隠れるのを
  3 回繰り返すため。全画面窓の有無で shell がタスクバーの表示を切り替える
- **メインウィンドウが手前に来ることがある**: ホストが隠されるたびに activate が main へ
  戻る。1 回の F12 で main が `WA_ACTIVE` を最大 6 回受け取っている (利用者報告 2026-08-28)

#### 修正 (2026-08-28)

**非表示で作る窓に最大化を要求しない。** 生成時は非表示・非最大化のままとし、最大化は
遷移所有者が `Visible` を出すのと同じ commit で当てる。所有者は既に
`Publish -> Visible -> Focus` の順序を所有しているので、そこへ「最大化」を寄せるのは
新しい仕組みではなく、既にある所有境界へ戻すことになる。

実装では [ui_fullscreen.rs](../src/ui_fullscreen.rs) の active detached builder から
`with_maximized(placement.maximized)` を除き、hidden builder mode は visibility と geometry だけを
指定する。最大化済み host を hidden 中に restore しないため `with_maximized(false)` も送らず、
winit の新規 HWND は観測どおり非最大化で生成される。`with_visible(false)` は維持する。

最大化の owner は次のとおり。

- 動画 transition: 既存 `SetHostVisible` effect (`Publish -> Visible -> Focus` の Visible commit)
- 静止画 / 音声 detached open: content / dark-loading 描画後の既存 initial visibility release
- host-loss 後の keep-alive holdover / backstop 再生成: holdover/content 描画・HWND 登録後の release
- tray 中に hidden で生成された active / passive host: 既存 tray restore の visibility owner
- `fullscreen_viewport_recreate`: cleanup-only なので最大化せず、次の active render / transition owner
  が表示時に適用する

`Maximized(true)` は `Visible(true)` より前に同じ commit へ queue する。Windows では maximize
自身が HWND を表示するため、この順序なら content-ready 前に表示せず、通常サイズを一瞬見せてから
最大化することもない。main/fullscreen builder にはもともと maximize 指示がなく、`target=main`
で観測された 2 回の `SW_MAXIMIZE` も outgoing detached builder が最大化を再提示していたものなので
同時に消える。

回帰テストは、保存 placement が `maximized=true` でも hidden detached builder が
`visible=false / maximized=None` であることと、動画 transition の `SetHostVisible` effect が
`Maximized(true) -> Visible(true)` をこの順で発行することを固定する。新しい App state、時間窓、
guard / retry / reorder は追加していない。

#### 実機検証 (2026-08-28 17:28 capture + log)

**ログ計数** (`-> detached` で新規ホストを作る 19 遷移すべて):

| 計数 | 修正前 | 期待 | 実測 |
| --- | ---: | ---: | ---: |
| `SW_MAXIMIZE` (window_create 内) | 3 | 1 | **0** (※下記) |
| host `WA_ACTIVE` | 4-5 | 1 | **1-2** |
| host `WM_SHOWWINDOW shown=false` | 3-4 | 0 | **0** |
| main `WA_ACTIVE` | 2-6 | 0 | **0-2** |
| `HCBT_ACTIVATE` | 7 | 1 | **1** |

**`window_create` は 168-360ms から 9ms になった。** 空転が 300ms を作っていたという
診断がそのまま裏付けられた。`surface_configure` は 3.1ms で、GPU 側は元から誤差だった。

**キャプチャ計測** (下端 18px の輝度を frame ごとに分類し、状態が続いた frame 数で数える):

| capture | 長さ | 状態変化 | **短い変化 (3 frame 以下)** | 1 frame |
| --- | ---: | ---: | ---: | ---: |
| 修正前 14:10 | 46.2s | 91 | **52** | 27 |
| 修正前 15:51 | 36.2s | 52 | **30** | 15 |
| **修正後 17:28** | 42.9s | 57 | **2** | 2 |

状態変化の総数が減っていないのは、F11/F12 による意図した表示切り替えがそのまま
数えられているため。**明滅として見えるのは 3 frame 以下の変化**で、これが秒あたり
1.13 / 0.83 -> **0.047** へ約 20 分の 1 になった。

#### 最大化復元の hardware regression と追補修正 (2026-08-28)

上の実行では最大化された detached 窓を使っておらず、移設した
`Maximized(true) -> Visible(true)` commit は未検証だった。その後、利用者が detached 窓を
ダブルクリックで最大化して F12 を 2 往復すると、通常サイズで復帰する回帰を実機で確認した。
38 遷移を含むログ全体で `SIZE_MAXIMIZED` / `ws_maximized=true` / `SW_MAXIMIZE` /
`maximized=true value=true` はすべて 0 件だった。これは hidden builder から maximize を外した
ちらつき修正で導入され、未検証だった最大化経路を実機確認して発見した回帰である。

`Visible` effect に `maximized` 引数、runtime placement の一元 write 境界に old/new と caller の
計装を加え、production と同じ active-render capture -> Visible effect の seam を再現した。
保存済み `{x:100,y:120,w:1600,h:1200,maximized:true}` は、非表示で非最大化の再生成 host が
報告した `{x:200,y:220,w:1800,h:900,maximized:false}` に Visible commit より前に置換され、effect は
`Visible(true)` だけを発行した。したがって downstream command loss ではなく、render-time capture が
自分たちの hidden scaffold を利用者の placement intent として公開したことが原因。`maximized` だけでなく
`x/y/w/h` も同じ ownership violation を受ける。

placement observation authority を `UserVisibleHost / HiddenScaffold` として型にし、各 capture 時点の
実 HWND の `IsWindowVisible` から決める。hidden scaffold は placement を一切 publish せず、OS 上で
可視な host だけが利用者の geometry / maximized intent を更新できる。これにより保存済み
`maximized:true` は既存 Visible owner が読むまで保持され、同じ commit で
`Maximized(true) -> Visible(true)` が出る。新しい App state、時間窓、frame-count guard、retry は無い。
回帰テスト
`recreated_detached_host_preserves_maximized_placement_until_visible_commit` は hidden observation 後も
placement 全体が不変で、実際の video Visible effect が上記 2 command を順に出すことを固定する。
hidden observation を writable に戻す mutation は placement equality と command 列の双方で失敗し、
Visible commit から maximize を落とす mutation は command 列で失敗する。

#### 残った軽微な事象 (利用者が許容と判断)

切り替えの瞬間にメインウィンドウが一瞬見えることがある。ログ上は、出ていく presenter を
retire した時点で activation が所有者である main へ一度戻り (`fg=main`)、その直後に
ホストが受け取る、という順序になっている。ループではなく 1 回だけで、往復もしない。
今回直した churn とは別の事象。

#### 回帰修正の実機確認 (2026-08-28 18:35 前後)

- タイトルバーからの最大化は往復しても維持される (利用者確認)。
- ログ: `SIZE_MAXIMIZED` 36 件 / `SW_MAXIMIZE` 24 件 (直前の実行では両方 0 件)。
- Visible effect が実際に読んだ値を出すようになり、`maximized=true` 12 件 / `false` 7 件。
  「フラグを読み損ねたか、コマンドが失われたか」を区別できない状態は解消した。
- 非表示の足場からの観測は 365 件すべて `authority=hidden_scaffold ... outcome=ignored` で拒否。
- ちらつきの再発なし: host の `WM_SHOWWINDOW shown=false` は実行全体で 4 件、
  **`window_create` の内側は 0 件**。

#### 派生: F11 の仮想フルスクリーンは F12 往復で解除される (既存挙動、本修正の回帰ではない)

利用者報告 (2026-08-28): 別ウィンドウ化 -> F11 で仮想フルスクリーン -> F12 を 2 回で、
通常ウィンドウに戻る。

**本修正による回帰ではない。** §1.139 の修正前から、F12 OFF の通常経路が
`reason=toggle_detached_viewer_mode_disabled` で
`old_borderless=true old_restore=Some(...)` を `new_borderless=false new_restore=None` にし、
直後の `reason=f12_to_non_detached` が消えた状態を重ねてクリアしていた。always-new media 経路にも
同じ terminal 扱いがあった。全 write を ungated な `[presentation-borderless]` に集約した unit 再現で、
この順序を修正前に確認した。`active_viewport_runtime_reset_new` / `adopt_passive` は原因ではなかった。

**所有範囲を確定して修正 (2026-08-28)。** F11 borderless と
`detached_viewer_restore_placement` の対は HWND / `DetachedWindowRuntime` の状態ではなく、同じ
viewer content を main と detached の間で移送している間の **detached presentation intent** とする。
F12 は OS host を破棄するが viewer presentation 自体の終了ではないため、この対を保持する。
再生成 builder は既存どおり flag から decorations と monitor geometry を作り、F11 OFF は保持した
restore placement へ戻る。真の viewer close、F11 OFF、新しい stable detached window の生成、別の
passive window の active 採用では従来どおり対をクリアする。後二者の clear は別 window の intent を
漏らさないため正しいので削除していない。新しい bool / guard / timeout / retry は追加していない。

回帰テストは通常画像と always-new media の F12 OFF/ON、再生成 builder の
`decorations=false` / geometry、F11 OFF 後の元 placement 復帰を固定する。また source audit で両 field の
runtime write が単一 probe を迂回しないことを固定した。F12 clear の再挿入、restore だけの消去、builder
からの borderless 適用削除が killing mutation となる。

#### F11 修正の実機確認 (2026-08-28 21:30 前後)

利用者確認: 別ウィンドウ -> F11 -> F12 往復 -> borderless 維持 -> F11 解除で元配置へ復帰。

ログ (`[presentation-borderless]`):

```
f11_enter_requested   false -> false   4
f11_enter_applied     false -> true    4
f11_exit_applied      true  -> false   3
close_terminal        true  -> false   1   (borderless のまま閉じた分)
```

- **F12 起因の clear は 0 件** (`toggle_detached_viewer_mode_disabled` /
  `f12_to_non_detached` はどちらも出ない)。
- 戻り先も対で動作: enter で `new_restore=Some(x:1116, y:92, 1254x795)`、exit で `None`。
  うち 1 組は `maximized: true` を保持しており、**最大化した窓から F11 に入って解除すると
  最大化状態へ戻る**。§1.139 の 2 つの修正が正しく合成されている。

退行なし:

- 最大化: `SIZE_MAXIMIZED` 9 / `SW_MAXIMIZE` 6 / hidden scaffold の観測拒否 259。
- ちらつき: host の `WM_SHOWWINDOW shown=false` は実行全体で 2 件、
  **`window_create` の内側は 0 件**。

#### 調査計装の段階撤去 (2026-08-28、実機確認完了後)

5 分間・F12 多用時の 2.26 MB ログでは、1.139 調査計装が 65% を占めた。原因と三つの修正が
実機確認されたため、同期 `WH_CALLWNDPROC` / `WH_CBT`、UI thread message loop に callback される
`SetWinEventHook`、bounded instrumentation queue / drop accounting、vendored eframe / egui-wgpu の
stage marker と `[presentation-viewport]` sink を全廃した。vendored 4 file は marker 導入直前
`baff797b^` の blob と hash 一致・zero diff を確認した。`[presentation-window]` からは、各 event ごとに
top-level window を最大 256 件走査していた `z_rank` / `z=` だけを外した。

一方、次は計装ではなく修正または低頻度の恒久観測なので残す。

- `DetachedPlacementObservationAuthority` と `HiddenScaffold` refusal は hidden scaffold を利用者の
  placement intent として publish させない **1.139 の修正本体**。refusal log も invariant の実測用に残す。
- `[presentation-placement] event=write` は runtime placement の値が実際に変わった場合だけ出す。
  per-frame の同値 write は保存動作を変えず、ログだけ省略する。
- `write_detached_viewer_borderless_state` と `[presentation-borderless]`、単一 write-path audit、
  1.139 の全 regression test は維持する。
- `[presentation-window]` は transition の phase begin/end と、mIV が発行した
  Visible / Focus / Publish / Destroy / ShowWindow / SetWindowPos 等の effect だけを残す。
  実機での受信側 770 lines / 276 KB (`source=wndproc`) は原因確定後の恒久価値が低いため、
  `observe_window_message`、三つの wndproc call site、`wparam/lparam` payload decode と
  pending-command 相関を全廃した。`[presentation-transition]` は従来どおり残す。

**最終ログ量 (同じ 5 分 session の実測から削除分を差し引いた値):**

| prefix | lines | KB |
| --- | ---: | ---: |
| presentation-window (mIV effects 207 + phase 74) | **281** | **79** |
| presentation-placement (HiddenScaffold refusal) | **259** | **61** |
| presentation-transition | **100** | **10** |
| presentation-borderless | **16** | **5.6** |
| **合計** | **656** | **155.6** |

2.26 MB 全体に対して **約 6.7%**。前回の約 20% は
`[presentation-window] source=wndproc` 770 lines / 276 KB を残す前提の値であり、
その後の利用者承認による追加削減でこの値になった。修正本体、全 regression test、
HiddenScaffold refusal、borderless single write path は削除していない。

### 1.149 製本フォルダごとに上限ピクセルサイズを設定して自動縮小 — 見送り (2026-09-02)

- 出典: 利用者要望 (2026-08-31)。散らばった画像を製本フォルダへ集め、別ツールで一括縮小して
  から送る運用。集める時点で縮小できると手順が 1 つ減る、という話だった。
- **1.148 (複数選択の一括エクスポート) で用が足りるため見送り**。本フォルダを開いて全選択 →
  `Ctrl+E` → 長辺指定で同じ結果になる。報告者も「エクスポート機能の拡張の方が柔軟に使える
  気もする」と書いており、利用者と合意済み。
- 再判断するときの材料。縮小段は既に共有されていて、`books::write_composited_page` へ渡す
  `ExportScale::Full` を本ごとの上限へ差し替えるだけ。ただし「副産物」ではなく、次の 3 つが残る:
  1. 本ごとの上限をどこに永続化するか (本フォルダ単位の設定を新設することになる)。
  2. 無編集画像の byte-copy fast path と両立しない。上限を効かせるなら
     `page_requires_full_composite` へ上限を渡す必要があり、製本の追加規則そのものが変わる。
  3. 既にあるページへ遡って効かせるか。効かせるなら再エンコードが走るので明示操作にする。
- 規模 / 優先度: Small〜Medium / P3。

### 1.152 波形ストリップの粗トラックを永続キャッシュする — **実装済み / 未リリース (2026-09-02)**

**正本は [video-seek-strip-plan.md](video-seek-strip-plan.md) §5.5 (D32) へ移した。** 設計・容量・
行の形・覆した理由はそちらを見る。ここには経緯だけ残す。

- 出典: 利用者報告 (2026-08-31) の派生。当初の不満は本項ではなく §1.146 (同一起動・同一動画で
  HUD を出し直したときに粗トラックを捨てていた) が原因で、**それは v3.4.0 で出荷済み**。
  本項は「**起動をまたいでも 2 回目以降が速い**」という別の価値で、1.146 を入れた後に改めて
  欲しいかを判断する保留だった。**2026-09-02 に利用者が「欲しい」と判断**したので着手する。
- **当時の「案 B (開いた時点で裏で全尺を埋める)」は、実は既に動いている。** 粗い列の構築は
  波形モードを見ている間だけ進み (`wave_background_paused_for_mode`)、ストリップを閉じれば
  止まる。**したがって今回入れるのは保存だけ**で、新しい走査の規則は足さない。
  唯一まだ無いのは「既定の 180 秒でも列を作る」で、これは I/O の代償があるので
  **保存が入った後に別途決める** (プラン §5.5 の「表示範囲の閾値は動かさない」)。
- 実装は master `3f54b21c`、実機確認済み。マニュアル (video.html / tut-cache.html) と
  キャッシュ管理のラベルも更新済み。**残るのは更新履歴だけ** (Phase 0 で書く)。
  文案: 「長い動画の音声波形が、次に開いたときは待たずに表示されるようになりました。
  10 分以上の範囲で表示している間に解析した結果を保存します。」
- **本項はリリース時に削除する** (Phase 1 §6.5)。

### 1.153 ZIP / PDF のページにタグを付けられない — **低優先度 / 将来** (2026-08-31 判断)

**★はページ単位で付くのに、タグはコンテナ単位でしか付かない。** 種別ごとの現状は
[docs/item-kind-capability-matrix.md](item-kind-capability-matrix.md)、影響調査は
[docs/tag-page-support-survey.md](tag-page-support-survey.md) が正本。

**壊れてはいない。** ビューアの右パネルは以前から「タグ対象: この本 (名前)」と対象を
明示しており ([ui_metadata_panel.rs](../src/ui_metadata_panel.rs) `tag_target_note_for_item`)、
黙ってコンテナへ付け替えているわけではない。**機能が無いだけ**なので、利用者要望が
出るまで着手しない判断にした (2026-08-31)。

**ただし 1 つだけ実害がある (下の「先に塞ぐなら」参照)。**

#### 規模

段階分けは survey §7。合計 **18〜26 ファイル / 約 2,000〜3,500 行**。

| 段 | 内容 | 規模 | 単独リリース可 |
| --- | --- | --- | --- |
| 1 | タグ対象を型で分ける (ページはまだ無効) | 6〜8 / 250〜450 | ○ |
| 2 | 種別を additive に保存 (ページはまだ無効) | 8〜11 / 600〜950 | ○ |
| 3 | ローカルでページタグを end-to-end 有効化 | 10〜15 / 800〜1,250 | ○ |
| 4 | リモートでページを表示 | 5〜9 / 350〜700 | ○ |

#### 着手前に決めることが 9 件ある

survey §8 に列挙。特に **変換アーカイブ内ページの identity を source archive と cache ZIP の
どちらにするか**は、決めないと段 3 の停止境界を作れない。

#### 先に塞ぐなら: summary と一覧の母集団が食い違う

**これは仮定ではない。** メタデータ転送はページタグを export/import し、往復テストもある
([metadata_transfer.rs:7033](../src/metadata_transfer.rs:7033))。そのため出荷済みの版でも
`tags.db` にページタグ行が存在し得る。

その行が今、片側からだけ消えている。

- タグ横断一覧は `item_key` を実パス化して `Missing` として黙って落とす
  ([tag_view.rs:318](../src/tag_view.rs:318))
- 一方 summary の `COUNT(*)` は `item_tags` 全行を数える ([tags_db.rs:685](../src/tags_db.rs:685))

結果、**「タグA: 5 件」と出ているのに一覧には 3 件しか出ない**。メタデータ転送を使った
環境に限られるが、原因が利用者から見えない。

塞ぎ方は 2 通りで、どちらも段 1〜4 と独立に数十行で入る。

1. summary 側からページ行を除く (= 一覧に合わせる)
2. 一覧に「表示できない項目 N 件」を出す (= 件数の差を説明する)

段 3 まで行けばページが一覧に出るので、この対処は不要になる。**段 3 をやらないと決めて
いる間だけの措置**である点に注意。

**優先度**: 低。利用者要望が出たら段 1 から。母集団の食い違いだけは、タグまわりを
次に触るときに 1 と 2 のどちらかを入れる。

### 1.169 360 を保持したまま通常画像を経由すると、衝突モードが同時に開く (2026-09-02)

**症状**: フォルダ内で 360 画像 → 通常画像 → 360 画像とめくると、通常画像のところで
補正パネルや分析モードを開いていても、360 画像へ戻った時点でそれらが開いたまま 360 が
表示される。<kbd>V</kbd> で入り直せば衝突モードは閉じる。

**原因**: 360 の state は「保持しつつ非アクティブ化」する設計
([panorama-360-view-plan.md §6.3](panorama-360-view-plan.md))。`panorama_state` は残り、
`is_panorama_mode_active(fs_idx)` が素材ごとに有効 / 無効を切り替える。**再アクティブ化は
state の有無だけで決まり、`toggle_panorama_mode` の ON 経路 (= 衝突モードを止める後始末)
を通らない**。通常画像の側では 360 が非アクティブなので、抑止 gate も開いている。

**360 のセッション意図 (旧 §1.145、v3.5.0 で実装) より前からある挙動**で、その復帰経路
(`reconcile_panorama_session_intent`) は
V キーと同じ入口・同じ後始末を通るので、こちらとは別の話。Codex レビューが同型の経路として
指摘した (2026-09-02)。

**決めること**: 「360 が有効になる瞬間」を state 生成時ではなく**アクティブ化時**に移すか
(= 保持設計の作り直し)、通常画像側で開ける物を制限するか。前者が構造的に正しいが、
`is_panorama_mode_active` の呼び出し全域に影響する。**症状パッチ (再アクティブ化を検出して
その場でパネルを閉じる guard) は入れない** — 同じ判定が 2 か所に生まれる。

規模 / 優先度: 中 / P3。目に見えるが、V で入り直せば直る。

### 1.170 切り離した動画窓のヘルプが、メインウィンドウに出る (2026-09-02)

**症状**: F12 で切り離した動画ウィンドウで `?` (ショートカット一覧) を押すと、**メイン
ウィンドウ側にヘルプが表示される**。押した窓には出ない。利用者報告 (2026-09-02、右情報パネルのロック (旧 §1.158) の
実機確認中に発見)。

**原因は未特定**。調べる人向けの取っかかりだけ残す:

- native presenter はヘルプを自分で持っている (`shortcut_help_open`、中央モーダル)。
  [docs/video-architecture.md](video-architecture.md) の「`WM_CHAR` / Text はヘルプ開閉には
  使わず」の段に、presenter 側が effective chord へ追従する仕組みが書いてある。
- App 側は `consume_context_shortcuts_help_key` を root ([app.rs](../src/app.rs) 34109 付近) と
  フルスクリーン ([ui_fullscreen.rs](../src/ui_fullscreen.rs) 19142 / 19606 / 19917 付近) で
  消費する。**切り離した窓のキーが presenter ではなく root 側へ届いている**か、両方が
  反応して root 側だけが描いている、のどちらかが疑わしい。まず**どちらが消費したか**を
  観測してから直すこと (推測で分岐を足さない)。
- **[1.162](#1162) と同じ家系**: 切り離した窓での操作が、メインウィンドウ側の owner へ
  落ちる。あちらは owner 窓が `main_hwnd` 固定という特定済みの原因なので、同じ形か
  どうかを最初に確かめると早い。

⚠️ detached 経路なので、着手前に [detached-rework-plan.md](detached-rework-plan.md) §2
(憲法) を読むこと。症状パッチ (押した窓を推測する guard、時間窓、二重消費の抑止) を
入れない。

規模 / 優先度: 小〜中 / P3。ヘルプが出ないわけではなく、出る場所が違う。

### 1.162 右クリックでメインウィンドウが前に出る — **解決済み** (2026-09-02)

**症状**: F12 で切り離した窓や専用フルスクリーン窓で右クリックすると、メインウィンドウが
手前に出て、フルスクリーン中の右クリックが使いづらくなる。

**原因**: 無効理由のツールチップ (`TrackedMenuTooltip`) を、`WS_EX_TOPMOST` を付けたうえで
**メインウィンドウの owned window として生成していた**。owned window は常に owner の上に
置かれるため、生成した瞬間にメインが Z 順で引き上げられていた。外部ツール対応で追加した
機構による退行で、修正前の master には存在しない。

**当初この欄に書いた原因は 2 つとも誤りだった** (実測で否定):

- ❌ 「メニューのオーナー HWND が `main_hwnd` だからアクティブ化される」 →
  **フォアグラウンド窓は一度も動いていない** (`fg_changed=false`)。`TrackPopupMenuEx` は
  オーナーをアクティブ化しない。動いていたのは Z 順だけ。
- ❌ 「master にもある既存不具合」 → このブランチが持ち込んだもの。

したがって **detached 凍結の合意プロセスは適用しなかった** (自分のブランチが足したコードの
退行であり、detached 述語にも viewport 経路にも触れていない)。

**修正**: ツールチップから owner を外した。位置は自分で決め、表示 / 非表示も自分で制御し、
`Drop` で破棄するので、owner 関係を持つ必要が無い。owner を持たない popup がタスクバーへ
出ないよう `WS_EX_TOOLWINDOW` を付けた。`TTTOOLINFOW.hwnd` は従来どおりメインを指す
(こちらは「どの窓のツールか」という別の意味)。

**同じ調査で見つかった 2 件目**: そのツールチップは実装当初から**一度も表示されていなかった**。
`TTM_ADDTOOLW` が 0 を返して無言で諦めていた。原因は `cbSize` — `TTTOOLINFOW` は common
controls v6 で `lpReserved` が増えており、v6 はマニフェストで要求したプロセスにしか
読み込まれない。mIV は `set_manifest` していないので v5 が動いており、v5 は知らない大きさの
`cbSize` を拒否する。v1 の長さ (`lParam` まで = SDK の `TTTOOLINFO_V1_SIZE`) を送るよう修正した。

**進め方の記録**: 最初の 2 つの仮説はどちらも実機計測が否定した。「オーナー HWND を
差し替える」で着手していたら、症状は残ったまま detached 経路を触ることになっていた。
Z 順をメニュー構築の 7 点で採り、反転する 1 区間を名指ししてから直した。

### 1.160 縦連結で画面外のページはアニメーションしない — 仕様

- アニメの次コマ期限は `fullscreen_page_layout` (= 実際に描いたページ) からしか立てない。
  画面外のページを混ぜると、過ぎた期限で 0ms 起床を繰り返しアイドルが空転する。
- 利用者判断 (2026-09-01): 「GIF を連結で観たいケースはあまりないので仕様でよい」。
- 変えるなら「見えていないページのために起き続けない」を保ったまま行う必要がある。

### 1.156 見開きの範囲コピーが左右にまたがって選べない — 仕様未実装

- 出典: 利用者が範囲コピー (フルスクリーン上部バーのカメラアイコンを **Ctrl+クリック**
  → ドラッグで範囲選択) を試して発見 (2026-08-31)。トリムとのずれ 2 件は v3.4.0 で対応済み。
- 残っているのは **左右のページにまたがる選択ができない**こと。
  `capture_region_target_at` がポインタ位置のページを 1 つ選び、その中だけを対象にする。
- 「2 ページ分を合成して 1 枚にする」話になるので、誤動作だった 2 件とは別に判断する。
- 規模 / 優先度: 中 / P3。

### 1.163 比率固定で枠外から引いた切り取り枠が、クリックした点に固定されない — **実装済み / 未リリース (2026-09-02)**

- 出典: 利用者報告 (2026-09-02)。**比率を固定しているときだけ**、切り取り枠の外から
  ドラッグして新しい枠を引くと、押した点が枠の角に固定されず動く。既にある枠のハンドルを
  掴んだときは対角が固定されるので違和感が無い、という切り分けも報告に含まれている。
- **原因 (確定)**: 新規ドラッグは [ui_crop.rs:622](../src/ui_crop.rs:622) →
  [crop_from_points](../src/export_crop.rs:609) を通り、その中で
  [fit_to_aspect_around_center](../src/export_crop.rs:200) を呼ぶ。2 点から作った矩形を
  **その中心を保ったまま**比率へ合わせるので、押した点が動く。自由 (比率なし) では
  `aspect_ratio` が `None` で素通りするため、報告どおり比率固定時のみ出る。
- ハンドルのドラッグは [dragged_with_aspect](../src/export_crop.rs:323) →
  [anchored_aspect_rect](../src/export_crop.rs:410) で**対角を固定**している。**こちらが
  正しい挙動**で、新規ドラッグだけが別の解決をしている。1 つの操作 (枠を作る / 変える) に
  2 つの綴りがある状態。
- **対応**: 新規ドラッグ用に「始点を固定して比率へ合わせる」経路を足す。`crop_from_points` の
  呼び出しは [ui_crop.rs:622](../src/ui_crop.rs:622) の 1 か所だけなので影響は閉じている。
  `fit_to_aspect_around_center` 自体は比率モード切替 ([ui_crop.rs:323](../src/ui_crop.rs:323) /
  [:355](../src/ui_crop.rs:355)) と SNS 分割 ([ui_sns_split.rs:330](../src/ui_sns_split.rs:330))
  が使う正当な用途なので**残す**。共通化して両方を 1 つの関数に畳まない。
- **回帰確認**: 4 隅どの向きへ引いても押した点が動かない。画像端で比率を満たせないときは
  始点を動かさず、収まる範囲で止まる。自由 / 現在比率は従来どおり。X/Y/W/H の数値入力
  (`crop_from_xywh_inputs`) の挙動を変えない。
- 実装は master `846ef5dc`。`crop_from_points_with_aspect` を足し、押した点を角として
  固定する。大きさは 2 軸が要求する幅の大きい方 (= corner handle と同じ規則) を採り、
  画像端に当たったら始点を動かさずそこで止まる。`fit_to_aspect_around_center` は
  比率モード切替と SNS 分割の用途として**そのまま残した**。
  回帰テスト 5 本 (4 方向の固定 / 2 軸の大きい方 / 画像端 / 自由 / 中心固定の据え置き) は、
  それぞれ別の変異でだけ落ちることを確認済み。
- **実機確認中に別の不具合が出たので同時に直した** (master `0c3f3f34`)。**枠を引く途中で
  ポインタが切り取りパネルの上を通ると、ドラッグが破棄されて二度と再開しなかった。**
  パネルは画面左の幅 220pt の帯なので、縦長ページでは「画像の左外側」がまるごとパネル。
  `pointer_allowed` は「ここでドラッグを**始めて**よいか」の述語で、走行中のドラッグを
  続けてよいかへの答えではなかった (1 つの述語が 2 つの問いに使われていた)。
  画像の上で始まったドラッグはボタンが離れるまでポインタを持ち続ける。判断は
  `crop_overlay_keeps_the_pointer` として名前を付け、テスト 3 本 + 変異検査済み。
  **`export_crop_create_drag` の書き込み箇所を全部数えて特定した** — ドラッグ中に破棄し得るのは
  この 1 か所だけで、他は press ハンドラと切り取りモード自体を抜けるリセット 3 か所。
- **本項はリリース時に削除する** (Phase 1 §6.5)。更新履歴の文案:
  「切り取りで比率を固定しているとき、枠の外からドラッグして新しい枠を引くと、
  押した場所が枠の角に固定されずに動く問題を修正しました。」
  「切り取り枠をドラッグで引いている途中でポインタが画像の外へ出ると、画像内へ戻しても
  それ以上追従しなくなる問題を修正しました。」

### 1.164 修飾キーで、枠の内外を問わず新しい切り取り枠を引けるようにする

- 出典: 利用者提案 (2026-09-02)。初めて切り取りを設定するとき、欲しい範囲を取るには
  ハンドルまでカーソルを運ぶ必要がある。
- **前提の確認 (コードと一致)**: 切り取りモードに入った時点で枠が未設定だと、既定は
  `CropRect::full` (画像全体) ([ui_crop.rs:466](../src/ui_crop.rs:466))。その状態では
  `target_at` が画像内のどこを押しても `CropHandle::Body` を返すため、ドラッグは常に
  「枠の移動」になり、**新規作成の経路へ到達できない**。
- **対応**: 修飾キーを押している間は hit test を飛ばして create-drag に固定する。
  Space + ドラッグのパンが既に `KeyAction::CropSpacePan` として keymap 経由なので、
  **これも `KeyAction` として追加**し、`ini_name()` / `context()` / `trigger()` /
  `default_chords()` / `ALL_ACTIONS` / [docs/keymap.ini.default](keymap.ini.default) /
  context shortcuts help ([context_shortcuts.rs:189](../src/ui_dialogs/context_shortcuts.rs:189))
  を揃える。既定は Ctrl + ドラッグ。
- **決めること**: Space パンと同時に押されたときの優先順位。ドラッグ開始後に修飾キーを
  離した場合の扱い (開始時の判定を保つ)。
- 規模 / 優先度: Small / P2。

### 1.165 切り取り済みの編集プレビューを代役に使うと、寸法の説明と絵が食い違う

**利用者報告 2 件 (2026-08-31 / 2026-09-02) と、こちらで見つけた連結読みの症状は
すべてこの 1 つが原因。** 症状ごとに対症で塞がない。

#### 症状

1. **単ページ**: 切り取り済みのページをフルスクリーンで開くと、本画像が来るまでの間に
   切り取り枠と暗転が**切り取り済みのサムネイル**の上へ乗り、二重に切り取ったように見える
   (利用者報告)。
2. **連結読み**: 切り替えた直後、ページ全体フィットのはずが**少し拡大して見える**ページと、
   **切り取られた絵のまま出る**ページが混在する (こちらで確認)。
3. 本テクスチャが届いた瞬間に絵が**広がる**。

#### 原因 (確定)

**切り取りがピクセルへ適用されているのは編集プレビューキャッシュ (= 一覧のサムネイル) だけ**で、
表示用テクスチャ (raw / processed / final composite) には入っていない
([edit_preview_cache.rs:1023](../src/edit_preview_cache.rs:1023)、サムネ側は
[thumb_loader.rs:1061](../src/thumb_loader.rs:1061) が `source_dims` へ**切り取り後の寸法**を
入れる)。crop を変えても表示 cache は保持され、失効するのは編集プレビュー WebP だけ。

そのうえで、表示側は**代役としてこのサムネイルを使う**。

- 単ページ / 見開き: [fs_display_texture_choice](../src/ui_fullscreen.rs:25985) の
  `tex.or(thumb_tex)`
- 連結読み: [ui_fullscreen.rs:27873](../src/ui_fullscreen.rs:27873) の
  `fs_thumbnail_texture_for_display`

さらに連結読みでは、**ページの大きさを決める処理と、貼るテクスチャを選ぶ処理が別**である。

- 大きさ: [vertical_reading_base_size](../src/ui_fullscreen.rs:26917) →
  [choose_continuous_page_layout_size](../src/ui_fullscreen.rs:4657)。
  解決順は fs_cache → `fs_early_dims` → **サムネイルの `source_dims` (切り取り後)**
- 貼る絵: そのあとの `page_textures` ループ ([ui_fullscreen.rs:27803](../src/ui_fullscreen.rs:27803) 以降)
- 流し込みは [from_resolved_rect](../src/displayed_image_transform.rs:195)。**矩形をそのまま
  使い倍率を逆算する**ので、2 つが別の絵を指していれば差がそのまま拡大 / 変形として出る

`fs_early_dims` は**フル解像度の読み込みを開始したページにだけ**入る
([app.rs:64579](../src/app.rs:64579)、worker の `DimsOnly`)。したがって連結読みへ入った直後は
次の 2 状態が同居する。

| ページの状態 | 大きさの出どころ | 貼る絵 | 見え方 |
| --- | --- | --- | --- |
| 読み込み開始済み・未到着 | 元画像の寸法 | 切り取り済みサムネ | **拡大して見える** |
| 読み込み未開始 | 切り取り後の寸法 | 切り取り済みサムネ | 大きさは合うが**切り取られた絵** |
| 本テクスチャ到着後 | 元画像の寸法 | 元画像 | 正しい (ここで広がる) |

**普段この食い違いが出ないのは、サムネイルと元画像の縦横比が同じだから**である。代役にしても
解像度が落ちるだけで大きさの意味は変わらない。**切り取りはその前提を壊す唯一の編集**で、
だから切り取り済みページでだけ現れる。

同じ教訓が隣に既に書かれている ([ui_fullscreen.rs:27803](../src/ui_fullscreen.rs:27803)
「貼るテクスチャは描画より前に 1 回だけ決める … 最初の実装が実際にそれで、連結読みだけ
直っていなかった」)。見開きの間隔合わせでは直したが、**基準寸法の解決はまだ選んだ
テクスチャと別経路**で、同型が 1 段上に残っている。

#### 対応

**(a) 即時 — 枠と暗転を、代役を貼っている間は描かない。** overlay の条件
([ui_fullscreen.rs:15510](../src/ui_fullscreen.rs:15510)) は `!original_preview_active` は
見ているが、**本画像が来ているかを見ていない**。呼び出し地点に `state.tex` があるのでそこで塞ぐ。
利用者へ「次で直す」と回答済み (2026-09-02)。**症状 1 の枠だけが消え、絵の食い違いは残る**
ことを承知のうえで先に出す。

**(b) 構造 — 寸法の説明と貼る絵の出どころを 1 つにする。** 基準寸法を、実際に選んだ
テクスチャから引く。`ThumbnailState::Loaded` は既に **`from_edit_preview: bool`** を持つ
([grid_item.rs:487](../src/grid_item.rs:487)) のに、代役を返す
[fs_thumbnail_texture_for_display](../src/app.rs:14993) はそれを見ていない。所有を 1 つにすれば
切り取り以外の代役でも構造的に安全になる。**(a) の後に (b) を入れ、(a) の条件を見直す。**

**(c) 代替案 (採らない場合の比較用)**: 切り取り済みプレビューを代役に使わない
(`from_edit_preview` を見て断る)。実装は最小だが代役が減り「絵が出るまでが遅い」方向へ振れる。
1.166 で表示へ反映する方針を採る場合、食い違いはそちらで消えるので (b) の形が変わる。

#### 回帰確認

- 切り取り済みページを一覧から開き、本画像到着の前後で**絵の大きさと位置が動かない**。
- 連結読みへ切り替えた直後、ページ全体フィットで**拡大されて見えるページが無い**。
- 連結読みを速くスクロールして戻ったとき、ページが「広がる」瞬間が無い。
- 見開き、分割、回転、表示トリム、AI アップスケール併用でレイアウトが変わらない。
- 切り取りの無いページの表示と代役の出方が従来どおり (代役を減らしていない)。

- 規模 / 優先度: (a) Small / (b) Small〜Medium。**P1** (見えている絵が正しくない)。

### 1.169 一括書き出しの準備が、1 件でも重ければその間 UI が止まる (v3.5.0 レビュー R08)

- Ctrl+E の準備はフレーム予算 (6ms) で分割したが、**予算を見るのは 1 件を終えた後**。
  マスクの DB 読み出しと展開、補正レイヤーの読み込み、注釈フォントの準備、AI runtime の
  初期化は同期のまま UI 上にある。1 件で 100ms 以上かかればその全部の時間止まり、
  ダイアログの初回表示もキャンセルも待たされる。
- 多数件を 1 フレームで処理しなくなった効果はあるが (v3.5.0 の F04)、**UI 応答性の
  境界そのものは動いていない**。
- 直し方: UI は軽量な identity と未保存編集 override だけを固定し、重い snapshot 作成を
  worker へ出す。`materializer::load_page_edits` が既に worker 側で同じことをしているので、
  合流させるのが筋。遅い DB / 大きなマスク / 未初期化 runtime を注入して、準備中の UI tick と
  キャンセル応答を確かめる。
- 関連: §1.170 (見開き合成) と同じ「source snapshot → worker composite」境界。

### 1.170 見開きを 1 枚に合成する処理が UI 入力ハンドラに残る (v3.5.0 レビュー R09)

- `external_tool.rs::merged_spread_target` は `render_export_pixels` の crop / 回転 /
  左右コピー / 合成を入力ハンドラ内で終えてから materializer を起動する。巨大な見開きでは
  画素数に比例して待たされ、cancel は明示的に常時 false。
- **全画素 SHA-256 は v3.5.0 で削除済み** (再利用が成立しないキーを作っていただけだった)。
  残っているのは合成そのもの。
- 直し方: 左右の source と編集 snapshot を worker へ渡して合成する。§1.171 (単枚 Ctrl+E の
  ワーカー経路化) と同じ境界で、[docs/bake-stage-unification-plan.md](bake-stage-unification-plan.md)
  の段取り 6 に含まれる。

### 1.171 単枚 Ctrl+E と見開き合成が焼き込み段の設定を使わない (v3.5.0 レビュー R07)

- どちらも表示済みの最終画素を書き出すので、`bake_stage_export` /
  `bake_stage_external_tool` を読まない。「編集」まで選んでも表示用効果が残る。
- v3.5.0 では**事実に合わせて記述した**: 設定画面・マニュアル・更新履歴に「エクスポート
  (1 枚) と見開き合成は表示している結果をそのまま書き出す」と明記してある。**直したら
  この 3 か所の注記も消すこと。**
- 正本は [docs/bake-stage-unification-plan.md](bake-stage-unification-plan.md) の段取り 5・6。
  6 は今の単枚 Ctrl+E が AI 込みで書き出しているので、AI 抜きで移すと退行になる。独立して
  実機確認する (8K 超、AI 拡大あり、カラー化あり、見開き、PDF ページで出力を比べる)。

### 1.172 関連付けの cache miss で、右クリックが同期 Shell 列挙へ戻る (v3.5.0 レビュー R10)

- 先行準備 (フォルダー読み込みごと、最大 8 拡張子) が届いていない拡張子では、メニュー組み立て
  中に `enumerate_handlers` を UI から呼ぶ。先行 worker が同じ拡張子を列挙中でも待機状態を
  区別せず重複列挙する。
- v3.5.0 で「毎回の右クリックで列挙し直す」のは直った (拡張子ごとの一覧を保持)。残るのは
  **最初の 1 回**と、準備が間に合わなかった拡張子。
- 直し方: 拡張子ごとの準備状態 (未要求 / 実行中 / 準備済み) を worker が所有し、メニューへ
  非同期に候補を渡す。**項目を落として軽くする対処はしない。** メニューは同じフレームで
  組み立てるので、非同期化には「準備できるまで関連付けの項目を出さない」等の UX 判断が要る。

### 1.173 同じページを開いた別ウィンドウへ、編集の失効が届かない (v3.5.0 レビュー R11 / BA-7)

- A と B の独立 viewer で同じページを開き、A で一括貼付 / リセットをすると、DB と A は
  更新されるが B の `adjustment_page_params`・マスク・補正レイヤー・注釈の保持状態と派生
  キャッシュへ mutation が配送されない。B は古い表示を使い続ける。
- **v3.5.0 の退行ではない。** 単一ページの貼り付けにも同じ穴があり、
  [docs/detached-rework-plan.md](detached-rework-plan.md) §11 にも未実装と記録済み。
  v3.5.0 では「要求を出した context へ結果を戻す」ところまでを直した (F08)。
- 直し方: page-key 単位の mutation 通知を該当 context へ配送する。別ページ・別 context は
  失効させない。2 窓同一ページと 2 窓別ページの対になる回帰テストが要る。detached リワークの
  所有境界 (BA-7) に属するので、リワーク側と整合を取ってから着手する。
- **同じ層にある残件 (2026-09-03、v3.5.0 レビュー T01 の修正時に確認)**: 別の窓で Ctrl+Z を
  押したときの補正 Undo。復元先をページ識別子にしたので、**別ページの正本を書き換えることは
  無くなり、正本 (DB) はそのページへ正しく戻る**。ただし、そのページを開いている窓の
  `adjustment_page_params` は変更後のまま残るので、その窓が読み直すまで表示に反映されない。
  ここで場当たりの通知を足すと本項の設計と二重になるため、**本項でまとめて配送する**。

### 1.176 一括補正の適用と取り消しで、ページ数ぶんの DB 書き込みが UI を数秒止める

- **症状**: 一覧で多数チェックして <kbd>Ctrl</kbd>+<kbd>1</kbd>〜<kbd>0</kbd> のスロット一括適用、
  および その <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> で、UI が数秒固まる
  (利用者確認 2026-09-03)。
- **v3.5.0 の退行ではない。** v3.4.0 の `apply_slot_to_grid_selection` と
  `apply_adjustment_change_to_app` も同じく 1 ページずつ `set_page_params` を呼んでいる
  (`git show v3.4.0:src/app.rs` / `:src/undo_ops.rs` で確認)。v3.5.0 の U01 で直したのは
  復元先の**検索**コストで、こちらは元からある**書き込み**コスト。

**計測** (`cargo test -p mimageviewer --lib bulk_adjust_cost -- --ignored --nocapture`、
2,000 ページ、開発機 / 一時フォルダの DB):

| 区間 | 時間 |
|---|---:|
| 適用 (`capture_adjust_full` + `set_page_params` × N) | 5,584 ms |
| 取り消し (`apply_meta_undo`) | 4,139 ms |
| やり直し (`apply_meta_redo`) | 3,770 ms |
| — DB を 1 行ずつ (いまの発行のしかた) | 3,367 ms |
| — DB を 1 トランザクション (既存 `set_page_params_bulk`) | **11 ms** |
| — 復元先の解決 (U01 で直した部分) | 5 ms |

- **分かっていること**: `set_page_params` は 1 ページごとに `conn.execute` を呼ぶので、
  **ページ数ぶんの暗黙トランザクション**になる。同じ 2,000 行を 1 トランザクションで書くと
  11 ms なので、**約 300 倍**の差。U01 で直した検索はもう支配的ではない (5 ms)。
- **分かっていないこと**: 適用 5,584 ms のうち DB が 3,367 ms として、**残り約 2,200 ms の
  行き先はまだ特定していない** (キャッシュ無効化 / `effective_params` / サイドカー /
  `capture_adjust_full` の差分取り、のどれか)。**先にここを内訳へ落としてから**設計する。
  DB だけ直して「半分速くなりました」で終わらせない。

**直し方の候補** (内訳が出てから決める):

- 既に `AdjustmentDb::set_page_params_bulk` / `remove_page_params_bulk` があり、
  「全画像に適用」(`app.rs` の 1 箇所) だけが使っている。スロット一括適用と Undo / Redo の
  復元も、**同じページ集合をまとめて 1 トランザクションで書く**経路へ寄せる。
- ただし現状の書き込みは 1 ページごとに「標準と一致するなら行を消す」判定 (`matches_default`)
  と sidecar 反映を挟むので、単純に bulk API へ差し替えるだけでは意味が変わる。
  **set と remove の 2 群へ振り分けてから**それぞれ 1 トランザクションにする形になる。
- キャッシュ無効化 (`clear_caches_for_param_change` / `bump_adjustment_generation`) を
  ページごとに回している分も、まとめて 1 回にできるか見る。

- 計測用の道具は `src/app/tests.rs` の `bulk_adjust_cost_tests` に置いてある
  (`#[ignore]`、合否判定はしない)。直したら同じ道具で前後を比べる。
- 規模 / 優先度: Medium / P2。

### 1.175 `ui_snapshot` のテスト実行体が、たまにアクセス違反で落ちる / 進まなくなる

- **症状は 2 つある。同じものかは未確定。**
  - **アクセス違反**: 43 件のうち十数件を通過したところで、テスト実行体が
    `exit code 0xc0000005 (STATUS_ACCESS_VIOLATION)` でプロセスごと死ぬ。assertion 失敗では
    ないので、どのテストが原因かも harness の出力からは分からない。
    2026-09-03 の配布ビルドで発生 (`ui_snapshot-cd01fab322c84861.exe`)。
  - **停滞**: 43 件のうち 20 件を通過した後、CPU を使い続けたまま完了しなくなる。
    2026-09-03 の第 6 回レビューで発生し、15 分後に手動停止。
- **頻度**: 2026-09-03 の同一 HEAD 付近で観測した範囲では、6 回中 1 回程度。
  逐次実行 (`--test-threads=1`) と、その後の並列 2 回はいずれも 4 秒台で完走した。
  **「逐次なら出ない」とは言えない** — 逐次の試行回数が足りていない。
- **今回のリリースの変更由来ではない**。落ちた HEAD で 2 回連続 PASS し、同じ HEAD の
  別ビルドでも並列のまま 4.81 秒で PASS している。以前の版でも起きている。

**調査できる。手段は揃っている** (「再現しないから無理」ではない):

- `cdb.exe` が入っている: `C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe`。
- テスト実行体の隣に **PDB がある** (`target/debug/deps/ui_snapshot-<hash>.pdb`) ので、
  スタックはシンボル付きで解決できる。
- 実行体のパスは毎回ハッシュが変わるが、
  `cargo test -p mimageviewer --test ui_snapshot --no-run --message-format=json` の
  `executable` フィールドから取れる。
- WER の LocalDumps は **この PC で既に使っている** (`mimageviewer-core.exe` に対して
  DumpType=2 / `%LOCALAPPDATA%\CrashDumps` / 10 世代)。ただし exe 名が毎ビルド変わるので、
  同じ「実行ファイル名ごと」の設定は使いにくい。既定 (全プロセス) を有効にすると
  無関係なアプリの dump まで集まる。**cdb で回すほうが素直。**

**手順の案** (上から):

1. 実行体のパスを cargo の JSON から取り、`cdb -g -G -c "sxe -c \"!analyze -v; .dump /ma
   <out>.dmp; q\" av; g"` で包んで**繰り返し起動**する。6 回に 1 回なら、20〜30 分回せば
   1 本は捕まる見込み。落ちたところのスタックが出る。
2. スタックが `wgpu` / D3D12 / OpenGL のどれを指しているかで、次の切り分けが決まる。
3. `--test-threads=1` を **数十回**回して、並列でしか出ないのかを確かめる
   (現状は試行が足りず「並列で 1 回出た」だけ)。
4. `WGPU_BACKEND` を `dx12` / `gl` / `vulkan` で固定して、backend 依存かを見る。
5. `egui_kittest` (`snapshot` + `wgpu` feature、`Harness` ごとに device を作る) の
   upstream に同種の報告があるかを確認する。43 件が並列で device を作っては壊すので、
   **driver / backend 側の競合を疑ってはいるが、まだ何の根拠もない**。

**なぜ今すぐやらないか**: リリースを止める理由にならない (引き直せば通る、成果物には
影響しない) が、**毎回の配布ビルドで一定確率で止まる**ので、放置すると
「テストゲートが赤 = とりあえず再実行」が習慣になる。それが一番まずい。

- 規模 / 優先度: Medium / P2 (v3.6.0 で 1 度は時間を取る)。

### 1.166 切り取りを表示にも反映するか — 方針決定

- 出典: 利用者要望 (2026-09-02)。「隠蔽加工は設定画面の外でも適用済みなのに、切り取りは
  適用されない。表示トリムで設定することになるのか」。
- **現状の設計と理由**: 切り取りは最後段で、表示では範囲外を暗くするだけ。実切り出しは
  capture / export のときだけ ([app.rs:55079](../src/app.rs:55079)、
  [display-pipeline.md](display-pipeline.md) 「crop は通常表示では暗転 overlay だけなので、
  レイアウト基準も描画 UV も変えない」)。**隠蔽加工との違いは「画像の範囲が変わるか」**で、
  隠蔽は画素を書き換えるだけで寸法が変わらない。
- **表示を切り取ると困る具体**: 消しゴム / 補正レイヤー / 隠蔽加工 / 注釈は**切り取り前の
  画像へ記録する**ので、切り取りの外側を見て塗る必要がある。各ツールは自分の手前までの状態を
  表示する原則があり ([ui_fullscreen.rs:15499](../src/ui_fullscreen.rs:15499) のコメント)、
  ツール中は既に枠と暗転を出していない。
- **仕組みは既にある**: 部分矩形だけを表示する経路 = `content_bbox` (正規化 0..1)。表示トリムが
  使っており、フィット倍率と 100% 判定 ([ui_fullscreen.rs:7041](../src/ui_fullscreen.rs:7041))、
  見開き (`harmonize_spread_auto_bboxes`)、連結読み (ユニット寸法変化時の再アンカー) まで
  通っている。切り取りも矩形なので、ソース画素値を基準寸法で割れば載る。
  **各辺 2 割という上限は表示トリム側の方針**であり、仕組みの制約ではない
  ([view_trim.rs:4](../src/view_trim.rs:4) `MAX_VIEW_TRIM_MARGIN`)。
- **候補**: 左パネルで編集を開始したら切り取りを一時解除し、抜けたら戻す。ツール中に枠と暗転を
  出さない仕組みが既にあるので、その所有へ寄せられる。
- **決めること**: [fs_page_content_bbox](../src/ui_fullscreen.rs:10236) の優先順位 (現状は
  「分割が勝ち、無ければ表示トリム」。切り取りを 3 番目にどう入れるか) / 見開きで左右の
  切り取りが違うときの扱い / 反映中は枠と暗転を出さない / 常時反映か設定で選ばせるか /
  一時解除の入口をどのツールに持たせるか。
- 1.165 (b) を入れれば「場所によって切り取られたり切り取られなかったりする」食い違いは
  消えるので、**本項は「さらに表示へ反映するか」の判断**であり、1.165 の前提ではない。
- 利用者へは「検討する」と回答済み (2026-09-02)。
- 規模 / 優先度: Small〜Medium / P3 (方針決定が先)。

### 1.167 通常動画のズームとパン — V キーでモードへ入る

- 出典: 利用者要望 (2026-09-02)。360 ではドラッグとホイールで見回し / ズームできるが、
  通常動画は拡大できない。部分的に拡大して見たい。
- **採る形 (2026-09-02 合意)**: 360 と同じく **V キーでモードへ入り、モードが入力を持つ**。
- **V は通常動画では現在 no-op**: `KeyAction::FsPanorama` の処理は
  `detect_panorama(fs_idx).is_some()` で門番されており
  ([native_video.rs:9870](../src/app/native_video.rs:9870)、イベント経由も
  [:4461](../src/app/native_video.rs:4461))、動画側の
  [detect_panorama](../src/app.rs:62588) は球面メタデータか 2:1 判定に落ちなければ `None`。
  **キーの取り合いが起きない。**
- **ホイールの競合はモード化で解ける**: 通常動画のホイールは
  [immediate_native_wheel_command](../src/video/native_presenter/render_core.rs:382) で
  ファイル移動 (`NavigateItem`)。360 は
  [panorama_takes_the_wheel](../src/video/native_presenter/render_core.rs:378) で奪う。
  同じ形の兄弟を足す。
- **ポインタも同様**: [native_video_panorama_input_active](../src/app/native_video.rs:6845) が
  真のとき押下を再生クリックではなくドラッグにする
  ([native_video.rs:12629](../src/app/native_video.rs:12629))。除外条件 (音声モード中 /
  音楽 VST シェル中 / 別ウィンドウ) は**この述語からそのまま写す**。
- 動画の 360 は静止画と違い機能制限モードではない ([app.rs:63840](../src/app.rs:63840)
  「Keep playback and panels intact」)。拡大ビューも同じく再生とパネルを止めない。

#### 描画 — 表示解像度サーフェスへ載せる

- 表示は [inverse_orientation_mapping](../src/video/native_presenter/resample_pipeline.rs:903)
  が作る**アフィン逆写像** (表示画素 → ソース画素)。拡大とパンは、この写像へ倍率と原点を
  合成するだけ。
- **NIS と Anime4K の定数バッファには既にソース部分矩形がある**
  (`source_origin` / `source_extent` [:673](../src/video/native_presenter/resample_pipeline.rs:673)、
  `source_region` [:958](../src/video/native_presenter/resample_pipeline.rs:958))。今は全面が
  入っているだけ。**Lanczos にだけ同じものを足す** (cbuffer + 2 パス)。
- 定数は draw ごとに作る (`draw_lanczos` / `draw_single_pass`) ので、毎フレーム倍率が変わっても
  追加コストは無い。2 パスの中間バッファ
  ([:415](../src/video/native_presenter/resample_pipeline.rs:415)) はソース行数基準なので
  サイズ変更も不要。
- 「OS に任せる」設定でも困らない。360 中はシェーダ経路を強制する前例がある
  ([render_core.rs:3783](../src/video/native_presenter/render_core.rs:3783)
  `panorama_pose.is_some() || filter != OsDefault`)。拡大モードも同じ扱いにする。

#### レターボックスの扱い — 静止画と同じ「枠外まで広げる」を採る

**360 が既にこれをやっている。** サーフェスサイズ判定に分岐がある
([surface_policy.rs:185](../src/video/native_presenter/surface_policy.rs:185)):

> A spherical camera view owns the full video display region. Fitting the encoded 2:1
> equirectangular frame here would incorrectly retain its ordinary-playback letterbox bars.

- 拡大モードでも同じ枝に乗せ、**モードに入るとき 1 回だけ**表示領域全体のサーフェスを作る。
  以降は倍率が変わっても定数が変わるだけで、**ホイール 1 ノッチごとに ResizeBuffers が走らない**。
- サーフェス上限にも余裕がある (長辺 8192 / 総画素 4096×4096。4K 全画面で約 830 万画素)。
- 追加で要るのは 2 つだけ: **等倍のとき画像の外を黒で埋める** (背後に専用の黒背景ビジュアルが
  あり [render_core.rs:3296](../src/video/native_presenter/render_core.rs:3296)、最終ターゲットを
  黒でクリアする経路もある
  [resample_pipeline.rs:1581](../src/video/native_presenter/resample_pipeline.rs:1581)) /
  **パンのクランプ**。
- したがって**「枠外まで広げる」形は、レターボックスを保つ形と工数がほぼ変わらない**。静止画と
  挙動を揃えられるので、こちらを採る。

#### 落とし穴と未確認

- **`select_video_resample_mode` と `stretch()` に拡大後の実効範囲を渡すこと。** ソース / 表示の
  比でアルゴリズムと縮小ぼかしを決めているので、渡し忘れると**拡大しているのに縮小用フィルタで
  描く**。
- **未確認**: 画像がサーフェスを埋めないときの外側の塗りが、どの経路で決まるか (360 は常に
  埋まるので前例が無い)。着手時に実機で確認する。
- タッチのピンチは認識器自体は既にある (`TouchOwner::Pinch`,
  [native_touch.rs](../src/video/native_touch.rs)) が動画向けコマンドを出していない。初回は
  マウス / キーだけで出し、ピンチは後から足す。
- 入力は `KeyAction` 経由にし、リセット操作も用意する。
- 利用者へは「V キーで拡大モードに切り替える形で検討する」と回答済み (2026-09-02)。
- 規模 / 優先度: Medium / P3。土台が揃っているので、新規は Lanczos の部分矩形とフィルタ選択の
  見直し、モード状態と入力の配線。

### 1.168 詳細表示の列ヘッダを右クリックすると、アイテムメニューが出ることがある — 再現待ち

- 出典: 利用者報告 (2026-09-02、§1.143(b) の実機確認中)。詳細表示の列ヘッダを右クリック
  すると、列カスタマイズメニューではなく**アイテムのメニュー**が出て、そのあと左クリック
  すると列メニューが出る。スクリーンショットで「パスをコピー」「代表サムネ固定を解除」
  「ペイント編集を反映して開く」を確認済みなので、フォルダ背景メニューでも列メニューでもなく
  単一アイテムのメニューで確定。右ドラッグはマウスジェスチャに設定。
- **再現していない**。同じバイナリで再度試すと出なくなった。**間欠**である。
- **単純な当たり判定の話ではない**。`render_details_list` をそのまま描く kittest harness
  ([ui_main.rs](../src/ui_main.rs) `details_header_right_click_tests`) を作り、マウス
  ジェスチャ ON / OFF の両方でヘッダ帯を右クリックしたが、`context_menu_idx` は `None` の
  まま。行の右クリックでは従来どおり開く。ヘッダは内側縦スクロールの外に確保されるので、
  行の rect とも背景判定の `body_inner_rect` とも重ならない。
- **次に見る場所**: `context_menu_idx` に値を入れるのはコード全体で 3 か所しかない。
  [ui_main.rs](../src/ui_main.rs) の cell 経路 (`handle_cell_interaction` 内)、右ドラッグ
  短押し経路 (`open_grid_right_drag_short_tap_menu`)、フォルダ背景経路
  (`open_current_folder_context_menu_at`)。この 3 か所には**どれが発火したかを記録する
  `[ctxmenu-probe]` ログを入れてある**ので、再現したら
  `%APPDATA%\mimageviewer\logs\mimageviewer.log` を `ctxmenu-probe` で grep すれば経路と
  クリック座標が一意に決まる。**推測で直しに行かず、まずこのログを取ること。**
- 状態依存の可能性 (未検証): 直前の操作で残った `context_menu_idx` / クリックのペアリング
  状態、sticky popup の開閉、ジェスチャ短押し判定の時間しきい値。
- レーン A の右クリックメニュー刷新より前から在ったかは不明。該当 3 経路の最終更新は
  2026-07-12 / 07-24 / 08-05 でレーン A より前だが、間欠なので断定しない。
- 規模 / 優先度: 不明 (原因未特定) / P3。実害は「メニューをもう一度開き直す」程度。

### 1.174 大量ページの書庫で高速ページ送りが詰まる - 未終了ロードの上限制御と ZIP 読み出しの再利用

- 出典: [専用スレ >>340](https://egg.5ch.io/test/read.cgi/software/1782419551/340)
  (>>336 からの追加確認)、利用者の実機検証とコード調査 (2026-09-03)。
- **未着手。リリース準備中の v3.5.0 には含めず、次の次以降で改善を試す候補。**
  対応版・完了時期は確約しない。今回は調査・方針の記録だけでコードは変更していない。
- 報告: ZIP / RAR でホイールを高速に回すと「読み込み中」のまま進まなくなり、待っても
  復帰しない。ビューワモードを変えても発生。目盛りが読み込みにつれて細かくなるとの報告もある。

#### 再現範囲と証拠

- テスト素材: `testimage/zip_seek_10000/text2img_flat_10000_stored.zip`。
  静止 PNG 10,000 枚、階層なし、ZIP_STORED / ZIP64、約 42.29 GiB。元画像の再圧縮なし。
  生成条件と元ファイル対応は同ディレクトリの `generate.py` / manifest にある。
- ローカルの証拠: `testimage/zip_seek_10000/log_snapshot_20260903_130010/` の
  `perf_events.jsonl` / `mimageviewer.log` / `analyze_snapshot.py`。これらは Git 管理対象外。
  対象バイナリは v3.4.0、対象書庫の操作区間は起動後約 654.631〜692.346 秒。
- **利用者の環境では読み込み待ちと誤ったアニメーション通知を確認したが、報告者の
  「待っても復帰しない状態」および目盛りの段階的増加までは再現していない。**
  恒久的な停止を以下の原因で説明できたとは扱わない。
- 同区間の fullscreen ロードは 438 件。終了理由は `static_ok=81`、
  `cancel_after_decode=349`、`early_cancel=8`。`decode_begin` から `thread_exit` までが
  重なった数は最大 51 件、区間末尾に未終了の対応は残っていない。
  **51 件は実行途中の I/O 待ち等を含む in-flight 数で、CPU 上で 51 件が同時実行したという意味ではない。**
- `cancel_after_decode` の開始から終了までは中央値約 1.15 秒、最大約 3.03 秒。
  ログの decode 区間には ZIP 読み出しも含まれるため、ZIP と PNG デコードの寄与はまだ分離できない。

#### コード上で確認できた問題

- `app.rs::start_fs_load_with_purpose` は画像ごとに `std::thread::spawn` する。
  `update_prefetch_window` は不要になった要求を `fs_pending` から外して cancel を立てるが、
  その worker が実際に終わるまで実行枠を保持する仕組みではない。
- 通常画像の `decode_canonical_image` は書庫読み出しとデコードを行い、呼び出し元はその後に
  cancel を確認する。**先読み範囲や pending 件数の制限と、実際にまだ動いている処理数の制限が一致しない。**
  新規要求が増えると、既に不要な処理と表示対象が CPU / I/O / メモリを取り合い得る。
- `canonical_image_loader.rs::ResolvedSource::resolve` からの ZIP ページ読み出しは
  `zip_loader.rs::read_entry_from_disk` を通り、毎回 `File::open` / `ZipArchive::new` を行う。
  zip 2.4.2 の `new` は中央ディレクトリを全エントリ分解析する。
  **最初の一覧列挙に必要な仕事と、ページごとに繰り返す目次解析を分けて改善する。**
  前の画像をすべて解凍しないと目的ページへ到達できない、という問題ではない。

#### 改善方針

1. ロード要求の受付・待機・実行・取消中・終了を所有する仕組みにまとめる。
   **取消済みでも worker が終了するまで実行枠を占有させる。**
   worker 数と展開メモリの予算、開始前の待ち行列を有限にする。
   別窓を増やすだけで全体予算を超えない構成とし、cancel / result の所属 context と世代は分離する。
2. 現在ページと見開きの相方を優先し、不要な先読みは開始前に取り除く。
   同じページの要求は合流・優先度昇格し、取消と再投入を繰り返さない。
   実行中は中断可能な I/O / 展開 / デコードの段階で cancel を確認する。
   中断できない処理は終了まで枠内で待たせ、UI スレッドでは待たない。
3. **順送りと直接シークの契約を混ぜない。** 順送りは既存の「途中ページを飛ばさず表示する」
   方針を保ち、能力を超えた入力を無制限に後追いしない。直接シークは最新の指定位置を優先する。
   固定待ち時間を足してページ送りを遅くするのではなく、実際の処理能力に収まる受付にする。
   サムネイル代替、カラー化・補正、停止ページの最終画質への切り替えは維持する。
4. ZIP の解析済み目次と読み出しハンドルを有界に再利用する。既存の
   `open_archive` / `read_entry_from_archive` 等を出発点とし、並列読み出しとシーク位置の
   独立性、ファイル変更時の失効、名前解決、入れ子 ZIP、CRC / エラー処理を保つ。
   全処理を 1 本のロックで直列化して表示待ちを増やす設計にはしない。
5. 先に実処理数を安定させ、ZIP open / 目次解析 / エントリ読み出し・展開 / 画像デコード /
   GPU 反映の時間を分けて再計測する。待ち件数、取消中件数、実行数、メモリ量、取消から終了までの
   時間も記録する。高速化率や適切な並列数は実測後に決める。

#### 同じ報告から分かった表示上の問題

- **静止 PNG で「アニメーションを読み込み中…」が出る件は確認済み。**
  `item_may_expand_animation` が `.gif/.png/.webp` という拡張子だけで候補にし、
  `current_animation_expansion_started_at` が通常の Display 要求も拾うため、150ms を超えると
  `draw_animation_expansion_progress` が表示する。静止 PNG が実際に全フレーム展開されている
  証拠ではない。一般画像の読み込みと、形式確認後のアニメーション展開を区別して通知する。
- **目盛りの増加は未再現で、原因候補のまま。** 見開き・非連結読みでは
  `build_spread_display_units_for_nav` の表示単位数から目盛りを作る。未知寸法は縦長扱いなので、
  通常の表示 / 先読み / サムネイル読込で横長と判明すると後続ペアと表示単位数が変わる。
  横長 1 枚につき必ず 1 本増えるわけではなく、目盛りの間引きもある。
  単ページ・幅固定でも発生するか、横長混在かを確認する。
- 寸法キャッシュの先行取り込み・永続化・ヘッダスキャンは改善候補だが、**採用仕様は未決定**。
  Remote の `collect_zip_entry_dims` は外側 ZIP を一度開いて各エントリの先頭最大 64KiB を読む
  既存部品で、RAR / 入れ子等には別の対応が要る。背景スキャンだけでは完了前の組み替えは残る。
  全件確定待ちや元ページ番号基準のシークへ変える場合は別途 UX 判断が必要。
  Remote 側の §1.0i で「近傍だけ + 非同期補完」を不採用とした判断も参照する。

#### 回帰確認と着手時に読む文書

- 遅い読み出し / デコーダを注入し、要求を連打しても**取消中を含む実行数・待機数が上限内**である。
  cancel を出しただけでは実行枠が空かず、実終了 / エラーで必ず返却されることを状態テストする。
- 同一ページの再要求、見開き両側、ホイール / キー押しっぱなし / シーク、逆方向へ戻す操作、
  別書庫への移動、閉じる操作、別窓 2 つを確認する。古い結果が新 context へ入らず、
  片方の cancel が他方の有効な要求を消さない。通常画像 / ZIP / RAR / PDF の同等経路を監査する。
- 実 ZIP の冷 / 温キャッシュで再計測し、停止後の現在ページ完成までの時間とメモリ量を比較する。
  未キャッシュ時の待ち自体と、復帰不能な停止を区別する。静止 PNG/WebP は誤った通知を出さず、
  GIF/APNG/Animated WebP の実際の展開では通知が正常に出ることも確認する。
- [display-pipeline.md](display-pipeline.md)、[async-architecture.md](async-architecture.md)、
  [virtual-folders.md](virtual-folders.md)、[ui-responsiveness.md](ui-responsiveness.md)。
  別窓の ownership に触れる場合は [detached-rework-plan.md](detached-rework-plan.md) §2 / §11 に従う。

## 2. 一覧 / サムネイル / フォルダ走査

### 2.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

### 2.2 サムネイルバッジのレーン配置共通化

- 背景 / 現象:
  1. ZIP / PDF / RAR 等の形式バッジだけを従来比 70% に縮小したため、従来サイズを
     維持している緑色のフォルダ名バッジが相対的に大きく見える。フォルダ名は可変長で
     セル幅の 80% まで使うため、面積差も目立つ。
  2. ブックマーク一覧では、通常セルの左上バッジ列 (編集状態 / pin / タグ) と
     ブックマーク時刻を別々に同じ左上座標から描画している。後から描く時刻がタグの
     先頭を覆い、見た目では隠れた領域がタグのクリック判定として残る可能性もある。
  3. 動画の `UP` バッジも左上を独自使用しており、タグや編集状態バッジと同じ衝突を
     起こし得る。右上のチェック / スタック枚数、左下のコンテナ形式 / フォルダ名 /
     評価、右下の絞り込み件数も、現在は個別の座標計算または相互の高さ推定で配置している。
- 原則:
  - 共通化の単位は「全バッジを同じ見た目で描く巨大関数」ではなく、セル四隅の使用領域を
    割り当てる純粋なレイアウトとする。形式、フォルダ名、時刻、評価など意味の異なる
    バッジは、共通の計測 / padding / 配置部品を使いつつ個別スタイルを維持する。
  - 中央の再生アイコンや代替アイコンなど、隅のバッジではないコンテンツ overlay は
    この仕組みに含めない。
- 実装案:
  1. セルの `inner` と表示対象を受け、`TopLeft` / `TopRight` / `BottomLeft` /
     `BottomRight` の各レーンに実測済み `BadgePlacement` (矩形、文字 / style、優先度) を返す
     `ThumbnailOverlayLayout` 相当の純粋なレイアウト層を設ける。
  2. **v2.9.1 完了**: 第1段階は左上を移行し、ブックマーク時刻 → 編集状態 / pin → タグの順に幅を予約する。
     `UP` も同じレーンへ入れ、残り幅が不足する場合だけタグを実測で省略または非表示にする。
     描画と hover / click 判定は同じ `BadgePlacement` の矩形を使い、再計算をなくす。
  3. **v2.9.1 完了**: 第2段階は左下を移行し、フォルダ名 / ZIP / PDF / 変換アーカイブ、評価、ファイル名
     プレートの予約幅・縦積みを同じレイアウト結果から決める。フォルダ名バッジは形式
     バッジと機械的に同じ 70% へ揃えず、長い名前の可読性を残したコンパクトな専用
     スタイルをスナップショット比較で決める。
  4. **未着手**: 第3段階で右上 / 右下も同じ所有境界へ移し、チェックとスタック枚数の排他条件、
     絞り込み件数の配置を明示する。各段階を独立コミット可能にし、全経路を一度に
     書き換えない。
- v2.9.1 実装記録 (2026-07-31):
  - `src/thumb_overlay_layout.rs` に painter 非依存の純粋レイアウトを新設。セルごとに 1 回だけ
    構築した左上 / 左下の配置結果を、通常描画、タグ hit-test、`UP`、ブックマーク時刻へ渡す。
  - 左下は形式 / フォルダ名とファイル名プレートの実測矩形を下段へ置き、その行の上へ評価を
    4px gap で積む。旧 `lower_left_container_badge_height` の高さ推定は撤去した。
  - フォルダ名は旧フォントの 85%、8.5pt 下限・13.5pt 上限、横 padding 0.30em・縦
    padding 0.12em の専用 style とした。長い CJK 名の可読性を残しつつ、70% 形式バッジとの
    面積差を抑える値を snapshot で確認した。
  - 純関数 unit test 5 件と、フォルダ / ZIP / PDF / RAR、ブックマーク時刻 + タグの snapshot を
    追加・更新して目視確認済み。第3段階の右上 / 右下は未着手のまま残す。
  - 左下の item 種別 → コンテナバッジ / ファイル名プレートの対応は `bottom_left_content`
    として painter 非依存に切り出した。旧 `cell_has_lower_left_container_badge` の
    unit test 4 件 (フォルダは Loaded まではファイル名、アーカイブは常に形式バッジ) を
    同じ規則の検証として移設してある。ここは「フォルダ名と ★ が重なる」報告の退行ガード。
- **第3段階と一緒に片付ける残件 (v2.9.1 では未対応)**: 左上レーンは cursor を右へ送るだけで、
  **セル幅でクランプしていない**。時刻 + `UP` + 編集バッジ 6 個が同時に立つ狭いセルでは
  レーンが右へはみ出す (タグだけは実測で省略される)。第2段階以前からある挙動だが、
  時刻を同じレーンへ入れたぶん到達しやすくなった。直すには「溢れたときどのバッジを落とすか」の
  方針が要るので、右上 / 右下を同じ所有境界へ移す第3段階と合わせて決める。
- 完了条件 / 回帰テスト:
  - ブックマーク時刻と `#` から始まるタグが重ならず、表示矩形とクリック矩形が一致する。
  - `UP` + 編集状態 / pin + タグ、狭いセル、長い CJK タグでもバッジ同士が重ならない。
  - 長いフォルダ名 + 評価、ZIP / PDF / RAR + ファイル名 + 評価で重なりやセル外描画がない。
  - レイアウト純関数で矩形の非交差・優先順位・狭幅時の省略を unit test し、代表的な
    フォルダ / 形式バッジとブックマーク時刻 + タグを `docs/ui-snapshot-policy.md` に従う
    snapshot で確認する。
- 規模 / 優先度: Medium / P2 candidate。現在の重なりだけを座標ずらしで直す症状パッチは
  入れず、まず左上レーンの単一所有化から着手する。

### 2.7 RAR の header 全走査が、一覧判定とサムネイル生成で 2 回走る

- 出典: v3.1.2 で入れた RAR 判定の逐次公開 (旧 §2.3) の caller 監査 (2026-08-19)。その brief の
  §1.4 として着手したが、範囲を超えると判断して停止条件どおり切り出した。**v3.1.2 で閉じたのは
  「全件判定を待つ表示遅延」であって、判定そのものの重複ではない。**
- 機構: フォルダ一覧の eligibility 判定が `inspect_for_direct_read_cancelable` で RAR の
  header を全走査した後、thumbnail worker の代表画像選択が
  `enumerate_image_entries_detailed` で**同じ RAR の header をもう一度全部**列挙する。
  30,000 entry 級の RAR が多いフォルダでは、この 2 回目がそのまま体感時間に乗る。
- なぜ今回入れなかったか: 1 回へ統合するには `RarInspection` が entry / 代表情報を保持し、
  thumbnail と open 双方の列挙契約を変える必要がある。逐次公開とは独立した変更で、
  同じ commit に混ぜると切り分けられなくなる。
- 直す方向:
  - `rar_loader` の判定結果が、そのフォルダ generation の間だけ代表画像選択に必要な情報も
    持つようにする。`DECISION_CACHE_CAPACITY` (32、full inspection 保持) を単に増やす修正には
    しない。メモリが増えるだけで、判定回数は減らない。
  - 統合後は **同一 RAR の header 判定回数を計装または test double で 1 回に固定する**
    (逐次公開側の完了条件として書かれていた項目をここへ引き継ぐ)。
- 計測の起点: `C:\tmp\miv-rar-thumbnail-test-100` (30,000 entry の RAR を 30 個複製)。
  v3.1.2 時点の実測は visible 12 件が 0.801〜0.860 秒、候補 #130 が 4.021 秒
  (worker 累積 3,257.5ms)。2 回目の走査を消せばここがさらに下がるはず。
- 規模 / 優先度: 中 / P2。表示は既に出るようになったので体感の急所ではないが、
  同じ I/O を 2 回やっている事実は残っている。

### 2.6 ZIP / RAR のダブルクリックが時々無反応 — 原因確定・修正済み、利用者の確認待ち

- 報告条件: サムネイル表示、ZIP / RAR、Enter では開ける。最初のダブルクリックでは開かず、
  そのまま待つだけでも開かないが、しばらくして再度ダブルクリックすると開くことがある
  (専用スレ >>257 で訂正)。RAR はローカル上の直読み対象。
- **原因確定 (2026-08-19、報告環境の perf log)**: egui は
  `count = if triple_click { 3 } else if double_click { 2 } else { 1 }` で数え、`is_double()` は
  **`count == 2` の完全一致**。`triple_click` は `max_double_click_delay * 2` を**前々回のクリック**から
  測る (`egui-0.33.3/src/input_state/mod.rs:1213-1222`, `1006-1011`)。つまり **3 回目のクリックは
  triple になり `double_clicked()` が false になる**。1 回目で開かなかった利用者はもう一度
  クリックするので、その 3 回目がちょうどここに落ちていた。
  - 実測 (idx 9、同一セル、同一座標): 離す時刻 165.735 / 166.384 / 166.612。3 回目は前回から
    **228ms** で double 成立圏内だが、前々回から **877ms** で `2 x 500ms` の内側 → triple 判定。
    同 session の成功例は 400ms 間隔の 2 回目。**400ms が成立して 228ms が成立しない**という
    逆転がこれで説明できる。
  - v3.1.2 のダブルクリック時間の OS 追従はこれを**悪化させた** (triple 窓 600ms → 1000ms) が、
    原因ではない。300ms でも 600ms 以内に 3 回クリックすれば同じで、**本項は v3.1.1 以前からあった**。
- **修正 (v3.1.2)**: グリッドは egui の click count を起動判定に使わず、`response.clicked()` で
  click 成立だけを受け取る。同じ `items_generation`・同じセル idx・OS 由来のダブルクリック時間内に
  ある 2 click を自前の単一 pairing state で対にし、item activation 後・セル以外の primary click・
  一覧世代変更で対を切る。「開く → Esc → 単発クリック」で再度開いてしまう追補も同時に閉じた。
  egui 本体は変更していない (triple click は text field の行選択が使うため)。
- **状態: 利用者の再確認待ち**。再発報告が来なければ close する。再発した場合に使える観測手段:
  - `grid/cell_signal` が成功 / 失敗を問わず `time_since_last_click`、`max_double_click_delay`、
    `clicked_by_primary`、`double_clicked_by_primary` を出す。失敗例だけを見ず、同じ session の
    成功例を control として比較する。
  - `grid/activation_request accepted=true` と `grid/activation_dispatch_complete` があれば click は
    成立しているので、以降の dispatch / open 側を疑う。`double_clicked_by_primary=false` かつ
    `first_click=true` なら pointer click として成立していない側を疑う。
  - 依頼手順: 開発者で性能ログ ON → 再起動 → 症状再現 → **再起動せず**「ログを zip にする」→
    診断 ZIP を送付。ログにファイル名 / path が含まれる既存の注意書きも案内する。
- 優先度: P2。原因が確定するまで guard / retry / 閾値再調整の症状修正を入れなかった方針は、
  再発時も維持する。

### 2.10 グリッド以外の egui ダブルクリック消費箇所に場所条件が無い

- 出典: v3.1.2 のダブルクリック対応の追補として行った `Response::double_clicked()` 全箇所棚卸し
  (2026-08-19)。egui 0.33.3 は
  前回クリック位置を持たないため、同じ context 内の離れた widget / 領域のクリックも時間内なら
  ダブルクリックとして各 consumer へ届き得る。
- 今回は利用者報告のあったグリッドセルだけを修正し、本項の consumer は変更しない。現存する
  8 分岐は、詳細表示の列幅 best-fit、360° navigator の視点移動、通常 navigator の zoom 中心移動、
  分析表示の zoom reset、通常 fullscreen の transform reset、360° canvas の視点 reset、native
  presenter egui overlay の音量 reset と再生速度 reset。
- native HWND が `WM_*BUTTONDBLCLK` から作る `double_click` は Windows 自身の時間・距離判定を
  通るため本項の対象外。着手時は一律のピクセル閾値を足さず、各 consumer の意味単位 (同じ
  handle / navigator / canvas / slider など) と、別 widget クリックで pair を切る owner を決める。
- 優先度: P2。latent 誤操作の棚卸し項目で、実害報告が出た consumer から個別に再現確認する。

### 2.4 CSV / TSV からの一括タグ / レーティング付与 — 保留

- 出典: 同じメール往復。こちらから代替案として提案し利用者も歓迎したが、**利用者の実際の
  使い方 (参照は一時的で、タグを付けても後から参照しないことが多い) とは噛み合わない**ため、
  RAR フォルダのサムネイル表示遅延 (v3.1.2 で対応済み) を優先すると回答した。
- 位置づけ: 外部ツールで抽出した結果を mIV へ持ち込む導線としては筋が良い。単独では需要が
  薄いので、タグ運用側の要望が別に出たときに合わせて再判断する。
- 実装するときの前提 (利用者へ明言済み): **明示的な「取り込み」操作のときだけ動く**こと。
  パスの一覧をビューとして開く形 (実体のない仮想フォルダ) は採らない。理由は、他人由来の
  リストで任意パスを参照してしまうこと、UNC パスなら開いた瞬間に外部へ認証情報が飛び得る
  こと、実フォルダ前提の処理 (サムネイルの識別キー、移動 / 削除時の扱い、各種設定の保存先)
  への影響範囲が大きいこと。
- 優先度: P3 / 保留。

### 2.18 リモート: サムネイルカタログが開けないだけでページ表示が失敗する — 利用者報告

- 出典: 利用者報告 (2026-08-23、v3.2.0 出荷前の実機確認)。リモートで PDF を開いたとき、
  端末に「ページ表示グループの読み込みに失敗しました。」が出た。前後ページ移動で復帰。
- **v3.2.0 の退行ではない。** この経路は `732420e3` (2026-07-31) 由来で v3.1.3 以前から出荷済み。
- **サーバ側ログで確定** (`%APPDATA%\mimageviewer-remote
emote-web-log.jsonl`):

  ```
  /api/page  status=500  page.ipc_status="miv_media_internal"
  → client: spread_load_error 「ページ表示グループの読み込みに失敗しました。」
  ```

  本体ログ (`mimageviewer.log`) の同時刻:

  ```
  [216.063s] remote_ipc: container catalog open failed: disk I/O error
  [216.063s] media_operation operation=page source_kind=pdf outcome=error
  ```

- 機構: [container.rs:5222](../src/remote_ipc/container.rs:5222) が**ページ要求のたびに
  `CatalogDb::open` し、失敗するとページ全体を `MediaErrorCode::Internal` で落とす**。
  しかしこのカタログは `decode_remote_source` に渡す**キャッシュの高速化用**で、
  ページ描画には必須ではない。**「速くするための仕組みが開けなかった」だけで表示が失敗している。**
- 発生条件: 直前の 214.28〜214.34s に PDF サムネイル要求が十数件同時にカタログを読んでおり、
  その混雑中に 1 回だけ SQLite が `disk I/O error` (`SQLITE_IOERR`) を返した。
  ログ全体でこの失敗は 1 回だけ。次の要求では成功しており、利用者の「前後移動で復帰」と一致する。
- **同時刻の `/api/page` 503 は別物**。`admission_busy` = IPC 同時実行上限 (heavy 4/4、
  prefetch 3/3) による正常な背圧なので、500 と混同しない。
- 直す方向: この経路でカタログを**省略可能にする** (`Option<Arc<CatalogDb>>`)。
  開けなければログを残してキャッシュ無しで続行する。表示は遅くなるが成功する。
  - **リトライ・待ち時間・閾値調整で吸収しない。**問題は「任意の加速機構の失敗を
    致命的に扱っていること」であって、開けなかったこと自体ではない。
  - 同じ形が他の remote 経路にもないか確認する (`CatalogDb::open` の `?` 伝播)。
  - ついでに、ページ要求ごとに開き直す構造自体も見直す価値がある (§2.13 と同型)。
- 規模 / 優先度: 小 / P2 (頻度は低いが、体験は「表示できない」なので重い)。

### 2.12 サムネイルキャッシュのクリアが `-wal` / `-shm` を残す — 実測 1.4 GB

- 出典: 2026-08-20、利用者がキャッシュクリア後に計測しようとして発見。
- **実測** (`%APPDATA%\mimageviewer\cache` 配下、クリア + 再起動後):

  | 種別 | 件数 | サイズ |
  | --- | ---: | ---: |
  | `.db` (本体) | 2 | 0.2 MB |
  | `.db-wal` | 1,410 | **1,394.8 MB** |
  | `.db-shm` | 1,410 | 44.1 MB |

  サンプルした `-wal` はすべて**対応する `.db` が存在しない孤児**だった。
- 機構: カタログは `cache/<hash 先頭 2 桁>/<hash>.db` (`catalog.rs` の `db_path_for`)。
  クリアが**その `.db` だけ**を消していると思われる。SQLite は正常な close で `-wal` / `-shm` を
  自分で削除するので、**閉じずにファイルを消すと残る**。
- 実害:
  1. **容量が解放されない。**「クリアしたのにディスクが空かない」= クリアがほぼ無意味に見える。
  2. 同じフォルダを再訪すると、その path に新しい `.db` が作られる一方、古い `-wal` が残っている。
     SQLite は salt 不一致の WAL を無視するので即座の破損は起きにくいが、**消したはずの物が残った
     状態で新規 DB を作る**のは設計として健全でない。
- 直す方向: **削除前に接続を確実に閉じる**のが構造的に正しい (閉じれば SQLite が自分で消す)。
  閉じられない経路があるなら、その理由を確認してから `-wal` / `-shm` の明示削除を足す。
  **どの経路が `.db` を掴んだまま消しているのかを先に特定する** (症状として `-wal` を消して回らない)。
- 既存の 1.4 GB は、アプリ終了後に `cache` 配下の孤児 `*.db-wal` / `*.db-shm` を削除すれば安全に回収できる。
- 規模 / 優先度: 小〜中 / P2 (実害は容量。利用者が明示的に「クリア」を選んだのに効いていない)。

### 2.13 PDF は 1 ページ描くたびに文書を開き直している

- 出典: 2026-08-20、PDF ワーカーのレーン容量を直した後、並列度より効く可能性がある側として確認。
- **機構**: `core_render_with_count` ([pdf_loader.rs:1303](../src/pdf_loader.rs:1303))、
  `core_analyze_page` ([1276](../src/pdf_loader.rs:1276))、`core_get_info` ([1165](../src/pdf_loader.rs:1165))、
  `core_enumerate` ([409](../src/pdf_loader.rs:409)) が、**要求ごとに** `load_pdf_from_file` を呼ぶ。
  子ワーカーは `Pdfium` インスタンスは持ち越すが ([1000](../src/pdf_loader.rs:1000) 付近)、
  **開いた文書は持ち越さない**。
- 実害の見積り (未実測):
  1. 1 冊を 10 ページ描くと **10 回開き直す**。開くたびに xref / trailer を読み直す。
  2. ワーカーは 5 個あり、**同じ 1 冊の別ページを並列に描くと、同じ文書を同時に 5 回開く**。
     HDD では xref (ファイル末尾) と本文 (途中) の間で seek が往復する。
  3. ページ数が多い本ほど不利。フルスクリーンのページ送りは同じ文書への連続要求なので、
     いちばん効く場所で毎回捨てている。
- **ファイルハンドルは保持される**: `load_pdf_from_file` は `File::open` したハンドルを
  文書の生存期間ずっと持つ (`FPDF_LoadCustomDocument` に reader を渡す実装)。
  ただし **2026-08-20 に実測した結果、掴んだままでも `rename` / `remove_file` /
  シェル経由のゴミ箱移動はすべて成功する** (Rust の `File::open` は `FILE_SHARE_DELETE` を
  含めて開くため)。動画デコーダが共有違反を起こす件 ([app.rs:26870](../src/app.rs:26870) の
  コメント) とは事情が違う。**掴むことを理由にした解放タイマーは要らない。**
- **本当の危険は ABA**: 削除・差し替えの後もワーカーは古い文書を握り続けるので、
  同じパスに別のファイルが来たら古い中身を返す。
- 直す方向: **ワーカープロセスごとに「直前に開いた 1 冊」だけを保持する。**
  キーは path だけでなく **(path, パスワード, mtime, サイズ)** にし、**要求ごとに stat して
  確認する** (stat は開き直しに比べて無視できる。mtime / file_size は enumerate の応答が
  既に運んでいるので、mIV 内の PDF 同一性判定として一貫する)。
  判定は純関数 (前回のキー × 今回のキー → 再利用 / 開き直し) に切り出せば PDFium 無しで
  テストできる。**時間窓は使わない。**
- アイドル時のメモリ (見終わった本を各ワーカーが抱え続ける) が問題になるかは**先に実測する**。
  効くなら時計ではなく既存の決定的な信号 (フォルダ移動時の `bump_render_context_epoch`) に
  載せて解放する。
- 実装上の引っかかり: pdfium-render の `load_pdf_from_file` は `password: Option<&'a str>` と
  `&'a self` が同じ寿命なので、文書を長く持つとパスワード文字列も同じだけ生かす必要がある
  (PDFium 自身はコピーするので API 側の過剰制約)。パスワード付き PDF のために小さな回避が要る。
- 着手順: **in-process フォールバック削除の後**。`core_*` の呼び出し元が 2 系統から 1 系統に
  なり、この分割の手数が半分になる。
- **測る先**: 現在の perf イベントは main プロセス側の queue / dispatch だけで
  ([pdf_loader.rs](../src/pdf_loader.rs) の `crate::perf::event` は全て pool 側)、
  **ワーカー内部の「開く」と「描く」は分離して測れていない**。
  先に子ワーカー側へ計装を入れて、開くコストの実測を取ってから直す。
- 規模 / 優先度: 中 / P2 (ワーカー数の調整より効く可能性がある。先に実測する)。

### 2.14 PDF のフルスクリーンが PDFium 経路を 2 本持っている

- 出典: 2026-08-20、ワーカー起動失敗の調査 (フェーズ 1) で判明。
- **機構**: 同じフルスクリーン表示なのに、初回ページと再レンダリングで別の PDFium を使う。

  | | 初回ページ | 再レンダリング |
  | --- | --- | --- |
  | 入口 | `render_page_for_display` ([app.rs:50276](../src/app.rs:50276)) | `render_page_async` ([pdf_loader.rs:3502](../src/pdf_loader.rs:3502)) |
  | 実行場所 | ワーカープロセス 5 個 | **メインプロセス内の 1 スレッド** |
  | Critical 予約 | 効く | 効かない |
  | レーン分割・昇格 | 効く | 効かない |
  | context epoch の間引き | 効く | 効かない |

  再レンダリングはズームだけでなく、`ensure_pdf_display_resolution` による表示解像度合わせ
  (現ページ + 見開き相方、[ui_fullscreen.rs:13561](../src/ui_fullscreen.rs:13561)) も通る。
  つまり**通常の PDF 閲覧が日常的にこの経路を使う**。
- **`render_page_async` はフォールバックではない**。`worker_count` を見ずに常に
  `get_worker()` を使う専用経路で、呼び出し元は [app.rs:50765](../src/app.rs:50765) の 1 つだけ。
  ワーカー起動失敗の対応 ([briefs/pdf-worker-startup-failure.md](briefs/pdf-worker-startup-failure.md))
  では**意図的に残した**。
- 移すかどうかの論点 (2026-08-20 に整理):

  **ズーム連打では並列化は効かない。** 新しいズーム要求は前の要求をキャンセルする
  (`fs_pending.remove(&idx)` → `cancel.store(true)`、[app.rs:50710](../src/app.rs:50710) 付近) ので
  キューに積み上がらず、無駄になるのは実行中の 1 件だけ。その実行中の PDFium レンダリングは
  **プールでも中断できない** (`CancelWaitPolicy` は結果の扱いであって PDFium を止める話ではない)。
  要求は互いに上書きするので、並列に走らせても欲しいのは最新の 1 件だけ。

  **効く可能性があるのは 2 ケース:**
  1. **AI 先読みのレンダリング中にズームしたとき。** in-process は `priority_rx` を先に drain
     するが、それはキューの順序を変えるだけで、**既に走っている非 priority の先読みが
     終わるまで待つ** (実測で 1.5 秒級)。プールなら先読み = Normal、ズーム = Critical で、
     予約により最低 1 ワーカーが空いているので即座に始まる。
  2. **見開きの左右ページ。** 2 ページとも必要な独立した処理なのに今は 1 スレッドで直列。
     Critical は lane cap の対象外なので、プールなら同時に走れる。

  **払うコスト: ビットマップの IPC 転送。** ワーカーの応答は**無圧縮の生 RGBA**
  ([pdf_loader.rs:1219](../src/pdf_loader.rs:1219))。ズーム再レンダリングは長辺 8192px まで
  行くので、A4 比なら 8192 × 5794 × 4 ≒ **190 MB をパイプに流す**。in-process は mpsc で
  ポインタを渡すだけ。プールが `serialize_us` / `write_us` / `wire_bytes`
  ([pdf_loader.rs:494](../src/pdf_loader.rs:494)) を計測しているのは、このコストが効くから
  (初回ページも 4K ビューポートで約 33 MB 払っている)。

  | | 得るもの | 払うもの |
  | --- | --- | --- |
  | プールへ移す | 先読み実行中の待ち解消、見開きの並列化 | ズーム解像度で最大 190 MB の直列化 + パイプ転送 |

- **有力な第 3 案: in-process にいる 2 つを分ける** (2026-08-20 追記)。
  `render_page_async` を使っているのは性質が正反対の 2 つで、まとめて扱う理由が無い。

  | 用途 | 呼び出し元 | 性質 |
  | --- | --- | --- |
  | ズーム / 表示解像度合わせ | `request_pdf_rerender(.., true)` ([ui_fullscreen.rs:6960](../src/ui_fullscreen.rs:6960)) | **利用者操作に同期。低遅延が要る。最大 8192px と巨大** |
  | AI 先読み用の native 再レンダ | `prefetch_final_ai` → `request_pdf_rerender(idx, 1.0, false)` ([app.rs:52845](../src/app.rs:52845)) | **完全にバックグラウンド。遅延は問題にならない** |

  なお**通常のページ先読みは既にプール側**なので競合しない
  (`ensure_fs_page_load` → `start_fs_load` → `render_page_for_display`、
  [app.rs:56551](../src/app.rs:56551))。in-process にいるのは上の 2 つだけ。

  **AI 用 native 再レンダだけをプールへ移す**と:
  - ズームは in-process のまま = レスポンス劣化の懸念が無い
  - in-process スレッドがズーム専用になり、**先読みに塞がれなくなる** (= 上の「効く 2 ケース」の
    ①がプール移行なしで解消する)
  - IPC 転送コストは AI 側が払うが、そちらは元々バックグラウンド
  - 見開き並列 (②) は解決しない

  **①が主因だと実測で分かったら、この案を先に検討する。**

- **測るもの** (上の表がそのまま測定項目になる):
  1. **先読み実行中のズーム要求の待ち時間** — in-process (先読み完了まで待つ) vs
     プール (予約ワーカーで即開始)。
  2. **ズーム解像度での IPC 転送コスト** — 既存の `WorkerRenderMetrics` がそのまま使える。
     長辺 2048 / 4096 / 8192 で `serialize_us` + `write_us` を取る。
  3. **見開き 2 ページの完了までの時間** — 直列 vs 並列。

  1 と 3 の得が 2 の損を上回るかで決める。§2.13 の計装と同時に入れると安い。
- 規模 / 優先度: 中 / P3 (現状で壊れてはいない。実測して有意差が出たときに動かす)。

### 2.19 バックグラウンドインデクサの残り時間が「まだ出せない」と「壊れている」を区別できない — 利用者報告

- 出典: 2026-08-23。お気に入り編集ダイアログの「🔄 バックグラウンドインデクサ」に残り時間が出ておらず、
  機能を消したのか不具合かという問い合わせ。しばらく待つと `[残り 02:12 (215件/秒)]` が出た
  (= スキャンから取込フェーズへ進んだ)。**現行仕様どおりの挙動で、退行ではない。**
- 機構: 残り時間は「残件数 ÷ 直近レート」で出しているので、**全体件数 N が確定しているフェーズ**でしか
  計算できない。

  | フェーズ | 呼び出し | 残り時間 |
  | --- | --- | --- |
  | 削除 (n/N) / 取込 (n/N) | `set_msg_and_count` ([ingest_worker.rs:218](../src/ingest_worker.rs:218), [:253](../src/ingest_worker.rs:253), [name_bulk_indexer.rs:150](../src/name_bulk_indexer.rs:150)) | 出る |
  | スキャン: `<dir>` (n 件) | `set` ([search_walker.rs:246](../src/search_walker.rs:246)) | 出ない |
  | 取込待ち (writer 使用中) | `set` ([indexer_supervisor.rs:534](../src/indexer_supervisor.rs:534)) | 出ない |
  | 名前索引の更新 / 更新スキャン | `set` ([name_index_supervisor.rs:386](../src/name_index_supervisor.rs:386), [:673](../src/name_index_supervisor.rs:673)) | 出ない |

  counted フェーズに入った直後もサンプル 2 点かつ件数が進むまでは `remaining_secs=None`、
  取込完了時は `clear_count()` で消える ([indexer_progress.rs](../src/indexer_progress.rs))。
- 問題: 「まだ計算できない」「そもそも総数の概念がない」「壊れている」が**すべて同じ無表示**で、
  利用者から区別できない。大きいフォルダ / HDD・NAS / 操作中で ActivityGate が walk を止めている
  条件ではスキャンが数分続き、その間ずっと空欄のままになる。
- 直す方向: 空欄をやめ、フェーズに応じた理由を出す (例: スキャン中 = `[件数集計中…]`、
  counted 直後 = `[残り 計測中…]`、差分更新 = 表示しない)。
  - **理由は型で持たせる。** 現状 UI は `eta: Option<EtaSnapshot>` の有無しか見ておらず、None の理由
    (カウント未設定 / サンプル不足) を区別できない。`ProgressReporter` 側に理由を持たせ、UI はそれを
    表示するだけにする。**メッセージ文字列の前方一致で "スキャン:" を判定するのはやらない**
    (文言を変えた瞬間に静かに壊れる)。
  - `aggregate_total_eta` ([favorites_editor.rs:938](../src/ui_dialogs/favorites_editor.rs:938)) は
    「残り = max(各 remaining)、速度 = Σ(各 rate)」で集約している。理由付き None が混ざったときの
    集約規則 (1 つでも計測中なら全体も計測中扱いにするか) を決める。
  - タイトルバー側の ETA 表示 ([app.rs:10296](../src/app.rs:10296) のキャッシュ経由) も同じ扱いに揃える。
- 規模 / 優先度: 小 (表示のみ。索引の動作自体は変えない) / P3。

### 2.20 別ウィンドウで動画が切り替わると、静止画ウィンドウの画像が縦横比の狂った状態で固着する — 利用者報告

> ⚠ detached viewer リワーク中の領域。**症状パッチを入れない** (CLAUDE.md 凍結ルール)。
> 分類は **BA-7** (bundle 外の App-global 状態が別 context から汚染される)。
> findings-12 D3 (book open が main context 経由で `auto_aspect` を汚し、メイングリッドが
> アスペクトリフローした) と**同型**。

- 出典: 2026-08-23。静止画をフルスクリーンで見ている最中に、別ウィンドウで動画が切り替わった
  タイミングで、静止画が**圧縮されて縦横比の狂った状態**で表示された。**画像を開き直すと復帰**
  (= 1 フレームのちらつきではなく、再解決まで残る状態)。
- ログで確認できた事実 (`mimageviewer.log`、セッション経過 6284〜6296s):
  - 独立した 2 つの viewer session が同時に開いていた。
    session 31 = 静止画 (`g:\home\comfyui\...`、items_gen=24 / 300 件、表示中 idx=96 =
    `mistblossom_gpt_4_6...png` **896x1152**)、session 4 = 動画 (`c:\home\youtube\...`、
    items_gen=6 / 171 件)。
  - **動画側の切り替えが App-global の `open_fullscreen` を通っている。**
    `[6285.031s] === open_fullscreen: idx=2 ===` は利用者が静止画側で操作した記録ではなく、
    動画ウィンドウの swap ([app.rs:44508](../src/app.rs:44508))。
    9 秒間に 6 回 (6284.9 / 6285.8 / 6290.7 / 6291.7 / 6292.3 / 6293.0) 発生している。
  - つまり静止画ウィンドウの描画中に、`fullscreen_idx` を含む App-global が動画ウィンドウの
    都合で書き換えられていた。
- **コード読みで確認できた構造** (この症状の原因と断定はしていない。候補の絞り込み):
  - `DisplayedImageTransform::resolve` は `full_image_rect = display_size * total_scale` の
    一様スケールなので、**この経路では縦横比は歪められない**
    ([displayed_image_transform.rs:167](../src/displayed_image_transform.rs:167))。
  - 歪みが表現できるのは `from_resolved_rect` だけ。呼び出し側が渡した矩形に対して
    `scale_x` / `scale_y` を別々に出し、**食い違っていても平均して受け入れる**
    ([displayed_image_transform.rs:183](../src/displayed_image_transform.rs:183))。
  - この穴は認識されていて、`resolve_fs_transform_in_layout_rect` が「レイアウト矩形は枠に
    すぎないので、実テクスチャから最終矩形を出し直す」ガードとして用意されている
    ([ui_fullscreen.rs:4094](../src/ui_fullscreen.rs:4094) の doc comment が
    "no caller can publish a non-uniform pixel scale" と明記)。
  - **単ページ ([ui_fullscreen.rs:24707](../src/ui_fullscreen.rs:24707)) と連結読み
    ([:29060](../src/ui_fullscreen.rs:29060)) はこのガードを通る。通っていない呼び出しが 2 つある**:

    | 場所 | 渡している矩形 | 由来 |
    | --- | --- | --- |
    | 見開き左右 ([:10381](../src/ui_fullscreen.rs:10381) / [:10397](../src/ui_fullscreen.rs:10397)) | `rects.left_rect` / `right_rect` | `spread_layout_geometry(left_size, right_size, ...)` = **ページ寸法**。`texture_size` には実テクスチャを渡すので、両者の比が食い違えば歪む |
    | detached の凍結スナップショット ([:10172](../src/ui_fullscreen.rs:10172)、`detached_continuous_frozen_pages_for_snapshot`) | `page.rect` | レイアウトが決めた矩形をそのまま渡している。直前に一様な `logical_scale` を計算しているのに、矩形側には反映していない |

  - 後者は **detached ウィンドウが凍結描画に落ちるときの経路**で、「別ウィンドウ側の操作で
    こちらが凍結スナップショットに切り替わる」という今回の状況と形が合う。**ただし利用者が
    単ページ / 見開き / 連結読みのどれで見ていたかはログに出ていない** (`spread` / `連結` の
    ログ出力が 1 件も無い) ので、経路は特定できていない。
- **Codex 追補 (2026-08-23、ClaudeCode 分析との差分)**:
  - perf ログでは、静止画を開いた直後の `896x1152` final composite は
    `scale_x=scale_y=1.188666`、Lanczos も `1597x2054` で、画像データ・texture・通常の
    transform は縦横比を保っていた。6293.759s の開き直しも同じ cache の hit だけで直っており、
    cache 内容の破損ではない。
  - 時系列では、動画の実 source swap (6285.031s) より先に、ParkedLive 動画窓のクリック復帰が
    6283.984s に queue されている。この復帰入口は、現在の静止画を
    `park_and_close_current_active_detached_viewer_for_media_handoff` で passive snapshot にしてから
    動画 bundle を active にする ([app.rs:40136](../src/app.rs:40136) →
    [:40414](../src/app.rs:40414) → [:42678](../src/app.rs:42678))。したがって、利用者からは
    「動画切替時」に見えても、歪んだ表示矩形が作られる候補時点は source swap より約 1 秒前の
    **active still → passive snapshot handoff** である。
  - この legacy still park は `build_active_detached_image_window_snapshot(None)` を呼ぶ
    ([app.rs:42583](../src/app.rs:42583))。`ctx=None` なので `frozen_continuous_pages` は必ず空になり、
    passive 描画は mode にかかわらず `window.image_rect_norm` の単画像 fallback を使う
    ([app.rs:41752](../src/app.rs:41752)、[ui_fullscreen.rs:10641](../src/ui_fullscreen.rs:10641))。
    今回の経路については、ClaudeCode 分析が未確定としていた見開き / 連結の
    `from_resolved_rect` 候補より、この fallback の方が直接対応する。
  - 静止画 host は 6282.159s に `rect=(-11,-11 3862x2110)` で登録されており、Windows の最大化
    outer rect と整合する。一方、maximized の placement 更新は実測 client size ではなく
    restore 用 seed の `w/h` を runtime に保持する ([app.rs:43781](../src/app.rs:43781))。
    snapshot bake はその placement の `w/h` で画像矩形を fit して正規化する
    ([app.rs:41689](../src/app.rs:41689)、[ui_fullscreen.rs:10242](../src/ui_fullscreen.rs:10242)) が、
    passive draw は正規化矩形を現在の `full_rect` へ X/Y 独立で戻すだけで、保存済み
    `image_dims` による contain / 一様 scale の再解決を行わない。restore 窓と最大化 viewport の
    アスペクトが違えば、この最後の写像だけで texture が非一様に圧縮され得る。
  - したがって、App-global `open_fullscreen` の書換えは handoff の起動条件ではあるが、それ自体が
    passive 静止画の geometry を汚染した証拠はまだない。Codex の第一候補は **BA-7 の global
    state 汚染ではなく、maximized 時の restore placement と live viewport を混ぜた snapshot
    geometry ownership の不一致**。数値の最終確認には、既存案の `from_resolved_rect` 計装に加え、
    snapshot bake の `placement/maximized/image_rect_norm` と passive draw の
    `full_rect/復元後 image_rect/scale_x/scale_y` を同じ window/session id で記録する必要がある。
    `from_resolved_rect` は bake 前の一様な矩形だけを見て正常終了し、その後の
    `rect_from_normalized` で生じる歪みを捕捉できない可能性がある。
- **次にやること (直す前に)**: 無言で受け入れている所を鳴らす。
  `from_resolved_rect` で `|scale_x - scale_y|` が閾値を超えたら、`page_idx` / `texture_size` /
  渡された矩形 / session を添えてログに出す。加えて active still → passive snapshot では、bake
  時の `placement/maximized/image_rect_norm` と draw 時の `full_rect/image_rect/scale_x/scale_y` を
  同じ window/session id で記録する。**推測で直さない** — 上の候補のどれでもない可能性が残って
  いる。再現手順は「静止画ウィンドウを開いたまま、別ウィンドウで動画を数回切り替える」で、
  利用者側では再現している。
- **直す方向**: 特定できたら、`from_resolved_rect` の一様でない矩形を**受け入れずに拒否**するか、
  全呼び出しを `resolve_fs_transform_in_layout_rect` 経由に寄せる。ガードが既に存在して
  文書化までされているのに 2 経路が素通りしている状態を、分岐追加ではなく入口の一本化で閉じる。
- 規模 / 優先度: 中 / P2 (表示の破綻だが開き直せば復帰する。凍結領域なので構造修正が前提)。
- **2026-08-23 追記 — §1.115 と根が同じ。** 最大化された別ウィンドウが表示されている間、
  placement 更新が**毎フレーム走り、82/82 が中身の変わらない書き込み**であることを
  `MIV_DETACHED_WINDOW_DEBUG` のログで確認した。maximized 分岐が実測 rect を捨てて restore 用
  seed の w/h を書き戻すため ([app.rs:43782](../src/app.rs:43782))、最大化中の placement は
  実ジオメトリを表さない。**Codex がここで指摘した「bake は placement の w/h、draw は最大化
  viewport の `full_rect`」というズレの供給源がこれ。** 直し方は §1.115 の案 A
  (現在ジオメトリと restore ジオメトリを別に持つ) で、両方の入力が同時に正しくなる。
- 関連: §1.115 (**同じ根**。最大化中のちらつき)、
  [docs/detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) / BA-7、
  findings-12 D3 (同型の App-global 汚染)。

### 2.23 タグの付与時刻で並べる / 見せる — 利用者要望 (2026-08-30)

**背景**: 「間違えて付けたタグを取り消したい」ときに、付けた順で見たい。レーティングの
★設定時刻まわり (旧 §1.142 / §1.143、v3.5.0 で実装済み) の調査中に、同じ話がタグにもあると
分かった。

**データはもうある**: `tags.db` の `item_tags.applied_at` (item×tag 単位、NOT NULL) と
`tag_item_state.decided_at` (item 単位) — [tags_db.rs](../src/tags_db.rs)。スキーマ変更は
要らない。

**無いのは並べ替えと表示**: タグビューの結果は [tag_view.rs](../src/tag_view.rs) で
`entries.sort_by(|a, b| a.path.cmp(&b.path))` とパス昇順に固定されていて、付与時刻ソートも
詳細表示の列もツールチップ行も無い。

**着手前に決める論点**:

- 複数タグを持つ項目で、どの `applied_at` を出すか。**そのビューで検索対象になっている
  タグの `applied_at`** (AND / OR なら最大値) が自然。「全タグの最大値」にすると、無関係な
  タグを後から足しただけで先頭へ来てしまう。
- タグビューにビュー固有ソート (レーティングの `RatingViewSort` 相当) を持たせるかどうか。
  持たせるなら**レーティング一覧と同じ構造をなぞる**。実装は v3.5.0 で入っているので、
  設計を起こさずコードを読めばよい:
  - 時刻ソート中はカテゴリ再配置を通さない — `RatingViewSort::arranges_by_category` と
    [grid_item.rs](../src/grid_item.rs) `materialize_view_rows`
  - 詳細列と sort key はビュー限定 — `DetailsColumnId::RatedAt` /
    `DetailsSortKey::RatedAt` と `App::details_sort_key_visible`
  - ビューへ入るとき列ソートの所有権を返す — `App::reset_details_sort_to_toolbar`
    (メニュー経路と履歴で戻る経路の両方に置く)
  - **新しい enum variant を settings.db へ書かない** — `stash_details_rated_at_for_persist`
    と同じ退避が要る (released 版が読めなくなるため。[settings.rs](../src/settings.rs))
- タグビューの対象は実ファイルのみ (ZipImage / PdfPage は入らない) — 既存仕様。

**優先度**: P3 / 余裕があるとき。

## 3. 補正 / AI

### 3.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

### 3.2 補正パラメータ変更後に AI アップスケールキャッシュが優先される疑い (再現待ち)

- 背景: 5ch レス 792 の追跡項目。「画像補正パラメータを変更しても AI アップスケールキャッシュが
  優先され、ページを行き来すると変更が効いていないように見える」という報告。
- 現状 (2026-06-18): 通常環境と v1.7.0 ポータブル版の追加テストで再現せず。現在の設計では、
  色調補正や AI 設定の変更は final AI / final composite cache のキー差分または明示クリアで反映される。
  一方、最終段スマートシャープなど post-filter 系は final AI cache を再利用して final composite だけを
  作り直す。さらに AI アップスケール出力にはスマートシャープを適用しない固定仕様なので、
  操作内容によっては「変わらない」ように見える場合がある。
- 方針: 具体的な再現手順が出るまではコード修正しない。再報告時は、変更したパラメータが色調補正 /
  AI ON/OFF / デノイズ / post-filter / スマートシャープのどれかを最初に切り分ける。
- 優先度: P3 monitor / 再現待ち。

### 3.3 ページ送り中の AI アップスケールを待たない / 打ち切る

- 出典: 元画像プレビュー中の見開き 1 ページずらし停止 (2026-08-17 修正済み) の実 perf log。
  停止中に AI upscale 1.3s × 3 回が
  逐次実行され、5.22s / 4.30s の停止時間の大半を占めた。
- その修正は「元画像を描く frame が加工済み source を readiness として待つ」不整合だけを直した。
  AI コストそのものは変更しておらず、元画像表示以外でも通過先 / stale target の推論待ちが起こり得る。
- 方針: ページ送りで表示 target から外れた AI upscale を待たず、context-owned producer の cancel / 打切りと
  着地点だけの再開を ownership・generation・完了回収まで含めて設計する。時間窓や `Awaiting` の強制解除、
  AI 結果の silent fallback では直さない。
- 規模 / 優先度: 中 / P2 performance。

## 4. 入力カスタマイズ / マウス / ゲームパッド

### 4.1 Shift / Alt + ホイールのカスタマイズ再設計

- 背景: v1.7.0 のリングショートカット / マウスボタン実装中に、Shift / Alt + ホイールのペアバインドを
  追加候補にしたが、実機確認で動画まわりの退行リスクが高いと判断した。
- 方針:
  - v1.7.0 では公開 UI / 入力経路から外し、通常ホイール、Ctrl+ホイール、中ボタンドラッグの既存挙動を維持する。
  - 将来再開する場合は、グリッド / 画像フルスクリーン / 動画フルスクリーンを別々に設計する。
  - native video overlay の consumed wheel、modifier 転送、動画タイルの Ctrl+ホイール、編集パネル / スクロールパネルとの
    優先順位を先に決める。
- 実装メモ: `ring_shortcuts.shift_wheel_pair` / `alt_wheel_pair` は互換読み込み用フィールドとして残すが、
  現行 UI / 入力経路からは参照しない。
- 補足: マスク編集モードで筆系ツールを選び、キャンバス上にカーソルがある場合だけの
  `Shift+ホイール` は筆半径変更の固定入力として別途実装する。一般の入力経路や上記互換フィールドへ
  接続せず、パネル上・筆以外・動画では消費しないため、本項の横断カスタマイズ再設計を再開したものではない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

## 5. リリース前確認 / 依存更新

> 版ごとに実際に取った測定値 (perf smoke / idle health / bench / 依存確認) は
> [release-verification-records.md](release-verification-records.md) にある。ここには
> **手順そのものの未解決点**だけを置く。

### 5.0 次版の「重要な変更点」に載せるもの

[src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ書く候補。
**既定の挙動が変わったもの**は `must_read` に入れる (更新後初回起動で自動表示される。
ここに載せないと利用者に伝わらない)。載せたらこの節から消す。

| 変更 | 区分 | 入った版 |
| --- | --- | --- |
| 右クリックの「アプリケーションで開く…」に登録したアプリが、環境設定の外部ツール設定へ移った (登録は自動で引き継がれる) | must_read | v3.5.0 (予定) |
| レーティング / ブックマーク一覧で時刻順を選ぶと、フォルダや書庫を先に並べる再配置を通さなくなった (時刻どおりに一列で並ぶ) | must_read | v3.5.0 (予定) |
| 動画の `Shift+S` の巡回が 3 段階 (非表示 / サムネイル / 波形) から 5 段階へ (全体表示が加わった) | must_read | v3.5.0 (予定) |
| 右情報パネルに鍵ボタンが付き、固定するとファイルを移動しても開いたままになる (画像へは重ねず右に領域を確保する) | highlights | v3.5.0 (予定) |
| 複数選択したまま `Ctrl+E` で一括エクスポートできる (出力先 / 形式 / サイズ / ファイル名テンプレート) | highlights | v3.5.0 (予定) |
| `Ctrl+E` の隠蔽加工プリセット出力が再び選べる (v1.1.0 から選べなくなっていた) | highlights | v3.5.0 (予定) |
| 編集内容を複数の画像へまとめて貼り付けられる / まとめてリセットできる | highlights | v3.5.0 (予定) |
| 360 ビューの ON と投影方式が、次に開いた 360 素材へ引き継がれる | highlights | v3.5.0 (予定) |
| 動画のシークストリップに全体表示と高さ 3 段階が加わった | highlights | v3.5.0 (予定) |
| 詳細表示に「★設定時刻」列が加わり、レーティング一覧でその順に並べられる | highlights | v3.5.0 (予定) |

**この表は候補案 (2026-09-03 時点)。** リリース手順 Phase 1 の 5.5 で
[src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ書き写し、書いたら
この表から消す。`must_read` の 3 件は**いずれもリリース済みの挙動が変わるもの**なので、
落とすと利用者に伝わらない。

v3.2.0 ぶんは [src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ記載済み
(必読 3 件 = 代表画像の既定 / バケツの塗り 1px / 消しゴムの色調合わせ、新機能 7 件)。

### 5.11 v3.2.0 出荷前確認の記録 (2026-08-23)

**動画アップスケール (`video-upscale-shader`) をマージした後の最終ビルドに対する記録。**
マージ前のビルドに対する旧記録はこれで置き換えた。

配布ビルド: `build-dist.ps1` (全体テスト込み) → 署名の 1 本目で
SimplySign のクラウド鍵セッション切れにより失敗 → 再ログイン後
`build-dist.ps1 -SkipRustTests` で完走 (同一ソースでゲート通過済みのため但し書きに合致)。

| 項目 | 結果 |
| --- | --- |
| Rust 全体テスト | ✅ 失敗 0 件 (`lib` 単体でも 6,200 passed)。**フルゲート中に mIV を操作すると `ui_fullscreen::tests` が 5 件落ちる**ので触らないこと |
| CI (GitHub Actions) | ✅ 緑 (`f053625d`)。**`build.rs` の fxc 呼び出しが非 Windows で問題ないことを実証**。`native_presenter` が `#[cfg(windows)]` なので生成テーブルを `include!` する経路が Linux に無く、`find_fxc` も `Ok(None)` になる |
| コード署名 | ✅ 単体exe / setup.exe / portable の 3 種とも `Valid` + RFC3161。内包 vendor PE (pdfium / onnxruntime / FFmpeg 6 本) も `Valid` |
| CRT 静的リンク | ✅ `VCRUNTIME140.dll` / `MSVCP140.dll` 依存なし |
| PDFium | ✅ 最新 (chromium/8009) |
| FFmpeg | ⏸ 4 コミット新しい版あり (`n7.1.5-12-g1fdbca85aa` → `-16-g9a4bb2c579`)。**見送り** — 同じ 7.1.5 系で必要な修正が特定できておらず、更新すると LGPL 対応ソースの再掲と製品ページの手書き節の更新が伴う |
| idle health: static-foreground | ✅ PASS (完全 sleep、perf event 0 件 / CPU 1 コア比 0.0396。外部 sampler が同一 session を確認済み) |
| idle health: static-background | ✅ PASS (完全 sleep、perf event 0 件 / CPU 1 コア比 0.0167) |
| idle health: tray-residency | ⏭ 本ビルドでは未実施。マージ前ビルドでは PASS したが**軽い条件のみ** (`evidence_floor` が起動時のままで、サムネイル読込中に閉じた状態では測っていない) |
| idle health: video-pin-background | ⏭ 未実施 (waiver)。動画を代表画像に固定し、かつキャッシュから読み直される状態のフォルダが必要 |
| ポータブル版 smoke | ✅ 実機確認済み (`data\` が exe の隣、APPDATA 不変) |
| 「重要な変更点」表示 | ✅ `--whatsnew-from 3.1.3` で**必読 4 + 新機能 8** を確認 |
| 機能の実機確認 | ✅ 製本の復元抑止 (出ないこと + Explorer コピーでは出ること)、動画アップスケール、復元モーダル / ESC、動画バー固定、設定の移動先、**T キー** (別ウィンドウ動画 → メインで PDF → 動画へ戻って T)、**キー割り当て一覧のラベル 2 件** |

**見送り (実施不要と判断)**: 検索ベンチ回帰 (本版で検索未変更)、perf smoke。

**次回への申し送り**: `Assert-MivSignReady` は証明書ストアの存在確認だけなので、
SimplySign のクラウド鍵セッションが切れていても通過する (証明書の公開情報だけが残り
秘密鍵が外れるため)。**ビルド 40 分を消費してから署名で落ちる。**
使い捨ての PE を 1 本実署名してから配布ビルドに入れば数秒で分かる。
事前チェック自体をその形にする改善は §5.12。

### 5.13 v3.2.0 の残り公開作業 (2026-08-23 時点)

**出荷判断は済んでいる。出荷前確認は 2026-08-23 に全件完了した。**残りは公開手順のみ。手順の正本は CLAUDE.md「リリース手順チェックリスト」。

済んでいるもの: Phase 0 (更新履歴・利用者承認済み) / Phase 1 (版番号・installer・製品ページ・
`changelog.html` 再生成・`version_highlights`) / Phase 2 (PDFium 最新・FFmpeg 見送り判断・CI 緑) /
全体テスト / 配布ビルドと署名。記録は §5.11。

残り:

1. ✅ **配布ビルドの最終版を検証** — 3 種とも `Valid` + RFC3161、CRT 静的。
2. ✅ **実機確認 (利用者)** — アイドル健全性 2 シナリオ、T キー、キー割り当てラベル、
   製本、ポータブル版 smoke すべて完了。
3. ✅ **§5.11 の記録を最終ビルドに合わせて更新。**
4. ✅ **push** — `master` と `master:main` を `09758b90` まで同期。
5. ✅ **Vector 申請用 zip** — `dist\mImageViewer_installer_v3.2.0.zip` (setup.exe + readme.txt)。
6. ✅ **タグと Release** — `v3.2.0` を公開。body は README の v3.2.0 節 (7,111 バイト)。
   Assets 4 点とも `uploaded`。<https://github.com/MikageSawatari/mimageviewer/releases/tag/v3.2.0>
7. ✅ **リリース日** — 2026-08-23 で README 見出し・製品ページとも一致。
8. ✅ **Phase 5** — mikage.to 反映済み / Vector 申請済み (2026-08-23)。
   任意の窓の杜・MS Store は見送り (毎リリース必須ではない)。
9. **公開後の目視確認 (未実施)** — 別マシンで起動し、更新通知ダイアログの body が崩れずに出るか。

**この節はここで閉じる。** 次版のリリース記録は新しい節を起こす。

### 5.15 v3.3.1 の公開記録 (2026-08-30)

測定値は [release-verification-records.md](release-verification-records.md) の v3.3.1 節。
ここには**公開作業の到達点**だけ置く。

1. ✅ **Phase 0 / 1** — README 更新履歴 (4,359 バイト、8KB 上限内なので短縮版なし)、
   バージョン表記 5 箇所、製品ページの版・最終更新・ポータブル版リンク、
   `changelog.html` 再生成、`version_highlights.rs` に must_read 1 件
   (カーソル位置の復元 = 既定で挙動が変わるもの)。既知の問題は 2 件残置し、
   **未修正のゲームパッド十字キーによる別ウィンドウ動画シーク不可を 1 件追加**。
2. ✅ **Phase 2** — PDFium は最新。**FFmpeg を更新** (H.264 フレームスレッド同期の修正 2 件、
   詳細は verification-records)。`check-non-windows-shadow.ps1` PASS。
   `test-full.ps1` 全緑。idle health 4 シナリオ PASS。perf smoke は 2 回取得。
3. ✅ **Phase 3** — `build-dist.ps1 -SkipVst3Bridge`。4 成果物すべて署名 +
   RFC3161 タイムスタンプ、`dumpbin` で VCRUNTIME/MSVCP 不在を確認。
4. ✅ **Phase 4** — タグ `v3.3.1` 公開、Assets 4 点。
   <https://github.com/MikageSawatari/mimageviewer/releases/tag/v3.3.1>
   公開 body は 4,321 バイトで、**記法が `changelog_markdown` の対応範囲に収まることを
   機械的に確認** (13 行すべて箇条書き、`**` 28 個で釣り合い、`<kbd>` 2 組、
   他の HTML タグ・見出し・リンク・画像なし)。
5. ✅ **Phase 5** — mikage.to 反映済み。**3 つのダウンロードリンクは GitHub Releases 向けで、
   実際に辿って 200 + サイズ一致を確認。FFmpeg 対応ソースは実際に落として sha256 照合済み**
   (LGPL 義務)。Vector 申請済み。窓の杜は見送り。**MS Store は保留** — v3.3.0 がまだ審査中のため。
6. ✅ **公開後の目視確認 — 手順から外した。** 崩れの主因である記法は自動テスト
   (`changelog_markdown::tests::the_newest_changelog_entry_only_uses_markup_this_renderer_handles`)
   が公開**前**に見る。目視は体裁しか分からず、公開後でないと試せなかった。
7. ✅ **積み残しの掃除** — mikage.to の `/mimageviewer/mimageviewer.exe` (ページから
   参照されていない古い単体exe) を削除。404 を確認済み。

**この節はここで閉じる。** 次版のリリース記録は新しい節を起こす。

### 5.14 v3.3.0 の公開記録 (2026-08-29)

測定値は [release-verification-records.md](release-verification-records.md) の v3.3.0 節。
ここには**公開作業の到達点**だけ置く。

1. ✅ **Phase 0 / 1** — README 更新履歴 (6,958 バイト、8KB 上限内なので短縮版なし)、
   バージョン表記 4 箇所、製品ページの版・最終更新・ポータブル版リンク、
   `changelog.html` 再生成 (差分なし)、`version_highlights.rs`、既知の問題 2 件は残置
   (どちらも今回直していない)。
2. ✅ **Phase 2** — 依存確認・`test-full.ps1` 7,604 passed / 0 failed・idle health 4 シナリオ
   PASS・perf smoke 97.3%。
3. ✅ **Phase 3** — `build-dist.ps1 -SkipVst3Bridge`。**1 回目は起動中の mIV を検出して停止**
   (ガードが正しく働いた。core 8 = 本体 + PDF ワーカープール)。終了後に再実行して成功。
   4 成果物すべて署名済み、`dumpbin` / `signtool` の回帰チェック合格。
4. ✅ **Phase 4** — タグ `v3.3.0` 公開、Release body は README 節とバイト一致、
   Assets 4 点。<https://github.com/MikageSawatari/mimageviewer/releases/tag/v3.3.0>
5. ✅ **Phase 5** — mikage.to 反映済み / Vector 申請済み (2026-08-29)。
   窓の杜は**メジャーリリース時のみ**とする方針に変更。MS Store は更新を実施
   (版付き直リンク `https://mikage.to/mimageviewer/download/v3.3.0/mImageViewer_setup.exe`
   がリダイレクト 0・`Content-Length` 一致を確認済み)。X 告知済み。
6. **公開後の目視確認 (未実施)** — 別マシンで起動し、更新通知ダイアログの body が
   崩れずに出るか。
7. **この版のレビュー** — 26 件中 11 件修正、1 件反証で取り下げ、12 件を v3.3.1 へ。
   正本は [review-v3.3.0/README.md](review-v3.3.0/README.md)、先送りの判断根拠は §1.0。

**この節はここで閉じる。** 次版のリリース記録は新しい節を起こす。

### 5.12 署名の事前チェックが、鍵の使えなさを検出できない

- 出典: v3.2.0 の配布ビルド (2026-08-23)。`Assert-MivSignReady` を通過したのに、
  実署名の 1 本目 (`vendor/pdfium/bin/pdfium.dll`) が
  `SignTool Error: No certificates were found that met all the given criteria.` で失敗した。
- 機構: SimplySign のクラウド鍵セッションが切れると、**証明書の公開情報は
  `CurrentUser\My` に残ったまま秘密鍵だけが外れる**。`Assert-MivSignReady` は
  証明書の存在を見るだけなので通過してしまう。
- 実害: `build-dist.ps1` は clean からビルドしてから署名するので、
  **40 分ビルドしてから落ちる**。しかも中途半端な成果物は残らないので、
  丸ごとやり直しになる。
- 直す方向: 事前チェックを「証明書が見えるか」から「**実際に署名できるか**」へ変える。
  使い捨ての小さな PE を 1 本コピーして `Invoke-MivSign` し、成功したら消す。
  数秒で済み、セッション切れならビルド前に分かる。
  - 署名対象を汚さないよう、必ず一時ディレクトリのコピーに対して行う。
  - タイムスタンプ取得でネットワークへ出るので、失敗理由を
    「鍵が使えない」「TS サーバに届かない」で区別できると望ましい。
- 回避策 (現状): 配布ビルドの前に手で 1 本署名してみる。
- 規模 / 優先度: 小 / P2 (リリースのたびに 40 分を失うリスク)。

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |
| PDFium | v2.12.0 サイクルで `chromium/7988` へ更新済み (§5.0) | 更新後は通常 / パスワード付き PDF の表示とページ数を実機確認 |
| FFmpeg | v2.12.0 サイクルで `n7.1.5-12-g1fdbca85aa` へ更新済み (§5.0) | DLL・VERSION・LGPL 対応ソース・製品ページの FFmpeg 節を同じ commit に揃えて動画 / 音声を実機確認 |

### 5.2 Rust クレート

- 互換範囲の一括更新とは分け、次のメジャー更新 / rc 脱出は個別判断する:
  - `ort`
  - `pdfium-render`
  - `ffmpeg-the-third`
  - `image`
  - `zip`
  - `sevenz-rust2`
  - `delharc`
  - `unrar`
  - `turbojpeg`
- 更新後に確認するもの:
  - `cargo test`
  - 検索 bench 回帰
  - perf smoke
  - `dumpbin /dependents` で不要な VC runtime DLL が復活していないこと
- **quick-xml の非推奨 API 追随** (v3.0.0 で 0.39 → 0.41、advisory 対応)。
  `src/xmp_reader.rs` の 4 箇所が非推奨警告を出す:
  `decode_and_unescape_value` → `decoded_and_normalized_value`、
  `unescape_value` → `normalized_value`。
  **0.41 では非推奨側が新実装へ委譲済み**なので、いま呼んでいる限り挙動は
  `normalized_*` と同じ (= XML 仕様どおり属性値の改行 / タブが空白になる)。
  影響するのは**属性値だけ**で、`xtw:*` (ツイート情報の表示用テキスト) と
  `rdf:resource` / `xmp:Rating` / GPano (いずれも URI か数値) にとどまる。
  **タグ (`dc:subject`) は `Event::Text` 経由**で非推奨メソッドを通らないため無関係。
  次の更新で削除される前に呼び出しを置き換える。

### 5.9 リリース手順: 配布ビルド前に core / remote の存在が要る

- 出典: v3.0.0 リリース前確認 (2026-08-14)。`scripts/test-full.ps1` が
  **テストを 1 件も実行しないままビルドエラーで落ちた**。
- 原因: `--workspace` に launcher package が入り、その build.rs が内包対象の
  `target\release\mimageviewer-core.exe` と `mimageviewer-remote.exe` の存在を検査する。
  remote は v3.0.0 で増えたので、それ以前の成果物しか無い環境で初めて表面化した。
- `build-dist.ps1` は test-full を `cargo clean` の**前**に回すため、直前のビルドが
  残っている通常のリリース機では起きない。新しい clone や `cargo clean` 直後に起きる。
- 当座の回避と前提は CLAUDE.md「開発中のビルド・テスト選択」に記載済み。
- 直す方向: test-full が不足分を先に build するか、launcher を `--workspace` の
  テスト対象から外す (launcher 自身に unit test があるかを確認してから決める)。
- 規模 / 優先度: 小 / P3。

### 5.3 リリース / テストスクリプトの並行実行耐性とクリーン環境再現性

- 出典: v2.8.0 リリース前確認 (2026-07-26)。別セッションで
  `scripts/build-release.ps1` を実行中に通常並列の `scripts/test-full.ps1` を実行すると、
  `mimageviewer` のライブラリテストが assertion / panic を出さず `0xffffffff` で終了した。
  リリース処理の完了後、同じコミットを競合プロセスなしで再実行すると正式ゲートは
  `[test-full] PASS` で完走したため、製品テストの並列不具合ではなくリリースツール間の
  干渉と判定。v2.8.0 の出荷は止めず、次版でツールを堅牢化する。
- **P2: `build-release.ps1` の停止対象が広すぎる (v2.9.1 対応済み)**:
  - 原因はプロセス名の前方一致 `mimageviewer*`。Cargo のテストハーネスは
    `target\debug\deps\mimageviewer-<hash>.exe` で、**リポジトリ配下にあるため path 判定も
    通過**し、別セッションの `cargo test` ごと `Stop-Process -Force` していた。
  - 停止対象を `mimageviewer` / `mimageviewer-core` / `mimageviewer-vst3-host` /
    `mimageviewer-susie32` の完全名 allowlist に変更した。`build-portable.ps1` も同じ
    欠陥を持っていたので揃えた (`build-dev.ps1` は元から完全名指定で影響なし)。
  - path を読めないプロセス (昇格 / 保護) は従来どおり停止するが、allowlist を通った
    ものだけになったのでテストハーネスは対象外。
  - リポジトリ単位の test / release 相互排他ロックは**入れていない**。名前の取り違えが
    原因だったので、まず allowlist だけで様子を見る。再発したらロックを検討する。
- **P2: クリーン環境の正式テストゲートが release core に依存する**:
  - `scripts/test-full.ps1` の workspace test には launcher が含まれるが、launcher の
    build script は既存の `target\release\mimageviewer-core.exe` を要求する。このため
    release core がないクリーン checkout ではテストゲートを開始できず、既存成果物が
    ある開発環境でだけ成功する。
  - 対応案 = launcher のテストを正式ゲート内で別段に分けて必要な core を明示的に用意
    するか、launcher のテストビルドを埋め込み用 release core から分離する。
  - 完了条件 = `target\release\mimageviewer-core.exe` が存在しないクリーン環境から
    `scripts/test-full.ps1` が完走し、launcher のテストも省略されないこと。
- 規模 / 優先度: Small〜Medium / P2。いずれも製品 runtime の品質問題ではなく、
  同一 worktree で複数セッションを使うリリース運用とクリーン再現性の改善。

### 1.85 vendored egui-wgpu のテクスチャ配送に回帰テストが無い

- 出典: v3.0.0 出荷前のクラッシュ修正 (2026-08-14、`ce6616ef`)。
  `paint_and_update_textures` が surface 無し viewport で早期 return し、
  **`textures_delta.set` を丸ごと捨てていた**問題を直したが、**テストは入れていない**。
- **この境界は過去 3 回、別の症状で表面化している**:
  1. サムネイルが純黒で固着 (v1.8.0 回帰) → `poll_thumbnails` 側で resync 窓中の
     upload を先送りする回避
  2. font atlas の `Y 29..44` パニック → `set_fonts` 最大 5 世代リトライの resync
  3. font atlas の `Y 45..126` パニック (今回) → **リトライ 5 世代を回りきって落ちた**。
     リトライ回数では防げないことの実証
- 欲しいテスト (Codex 案):
  1. `Managed(0)` が初期 32px 高
  2. **surface が無い状態で** 128px 以上の full 置換を submit
  3. surface 復帰後に `y=45..126` の partial を submit
  4. full 置換が共有 renderer に届いており validation error が出ないこと
- 障害: vendored egui-wgpu に既存のテスト基盤が無く、GPU device が要る。
  `egui_kittest` の wgpu 経路 (tests/ui_snapshot.rs) が最も近い足場。
- **回避策を撤去するかの判断もここに含める**: 上記 1 の upload 先送りと 2 の resync は、
  境界が直った今は保険であって正しさの担保ではない。撤去は別レビューで
  (今回は同時に触らないと決めた)。
- **2026-08-16 の確定**: §1.31 の第 1 段として着手する (順序は §1.31 参照)。
  ブリーフ = [codex-texture-delivery-test-brief.md](briefs/codex-texture-delivery-test-brief.md)。
  - 足場は `egui_kittest` **ではなく** `vendor/egui-wgpu` の in-crate headless unit test。
    `RenderState::create` の `compatible_surface` が `Option` で全フィールドが `pub` なので
    surface 無しの device / renderer を作れる。`ui_snapshot` 実行体の既知の間欠 AV を避ける。
  - **上記の欲しいテスト 4 は exit 2 (no-surface) しか到達しない**。`surfaces` が空なら
    `get_current_texture` に届かず、`on_surface_error` はエラーを分類するだけで注入できない。
    `RecreateSurface` / `SkipFrame` の coverage は §1.86 で typed outcome の seam を作ってから。
    → 前半を **§1.85-A** として切り出した。
  - **判定は `Renderer::texture_size` の observable な値にする**。mIV の overflow guard が
    範囲外 partial を wgpu へ渡す前に skip するため、「validation error が出ない」だけでは
    full 置換が落ちていてもテストが通ってしまう。
  - `vendor/egui-wgpu` は workspace `exclude` かつ `autotests = false` なので、
    `scripts/test-full.ps1` に専用実行を足さないと**テストが存在するだけで走らない**。
- **2026-08-16: §1.85-A (前半) 完了**。
  `vendor/egui-wgpu` の DX12 headless in-crate unit test
  `paint_and_update_textures_delivers_set_and_free_without_surface` で、surface 無しのまま
  32px seed → 128px full 置換 → `y=45..126` partial を渡し、主判定の
  `Renderer::texture_size` が 128px を保持することと validation error scope が空であることを固定した。
  別 texture の no-surface `free` も `Renderer::texture()` の消失で検査する。
  vendor manifest に明示的な lib test target を設定し、`scripts/test-full.ps1` の専用
  `--manifest-path` 段から実行される。production 挙動と `src/` 配下の既存回避策は変更していない。
- **2026-08-27: caller 回避策の再監査完了 (§1.115 option (d))**。
  実機 probe で atlas delta が全件 `site=paint outcome=Submitted` と確認できたため、
  thumbnail upload 先送りと viewport lifecycle の font resync / 5 回 repeat を撤去した。
  painter が呼ばれない境界を再び開けないよう、
  `vendor/eframe/src/native/wgpu_integration.rs` の 4 early-return site がいずれも
  `apply_textures_delta` を呼ぶことを caller-level test で追加固定した。
- **残り**: exit 3/4 (`RecreateSurface` / `SkipFrame`) の `free` 配送は §1.86 で
  typed `PaintOutcome` seam を作ってから検査・修正する。本項前半からは手を伸ばさない。
- 規模 / 優先度: 中 / P2。

### 1.86 surface 取得に失敗したフレームで `textures_delta.free` が捨てられる

- 出典: v3.0.0 出荷前の font atlas 調査 (2026-08-14) で Codex が併せて指摘。
  **今回のクラッシュの原因ではない** (`set` は既に適用済みの位置にある) が、同じ関数の同型の穴。
- `paint_and_update_textures` は `set` を surface 参照より前に適用するようになったが、
  **`free` のループは描画の後ろ**にある。`get_current_texture` が失敗して
  `SurfaceErrorAction::RecreateSurface` / `SkipFrame` で戻る経路
  (`vendor/egui-wgpu/src/winit.rs`) では free が実行されない。
- 影響は**テクスチャリーク**であってクラッシュではない。egui は id を再利用しないので
  誤描画にはならず、解放されない GPU テクスチャがプロセス寿命で残るだけ。
  サムネイルを大量に流す使い方だと効いてくる可能性がある。
- 直し方: no-surface 早期 return と同じく、これらの経路でも `free` を流す。
  `set` / `free` の適用を 1 つのヘルパに寄せて、描画の成否と独立にするのが素直。
- **2026-08-16 の確定**: §1.31 の第 2 段として着手する (順序は §1.31 参照)。
  ブリーフ = [codex-delta-delivery-transaction-brief.md](briefs/codex-delta-delivery-transaction-brief.md)。
  - **障害影響は P3 のままだが、§1.31 の依存関係上は必須前提**。§1.31 は frame drop を通常経路に
    するので、捨てる経路で `free` が落ちる構造のままだと本物のリークになる。
  - **「1 つのヘルパ」を単一関数と読まない**。`set` は surface lookup の前、`free` は成功経路では
    `queue.submit` の後でなければならず、同じ時点に置けない。
    `begin_delivery` (set をちょうど一度) / inner が typed `PaintOutcome` を返す /
    `finish_delivery` (outcome ごとに free) の**二段階 transaction owner**にする。
    `RenderStateAbsent` は「配送不能」の明示的な例外 outcome として型に残す。
  - **exit 3/4 へ同じ `free` ループをコピーするだけの修正は、「構造的修正」の合意対象外**
    (2026-08-16、ClaudeCode / Codex Sol)。所有構造 (配送責任が各 return に分散している) を
    直さず症状だけ消すため。
- **2026-08-16: 完了**。
  - `begin_delivery` が render state 有りなら surface lookup 前に `set` をちょうど一度適用し、
    inner paint は `RenderStateAbsent` / `SurfaceAbsent` / `SurfaceRecreated` / `Skipped` /
    `Submitted` の typed `PaintOutcome` へ合流する。no-paint 経路も同じ owner を使う。
  - `finish_delivery` が唯一の `free` 適用箇所。非 submit outcome は labeled inner block の
    encoder / command buffer drop 後、`Submitted` は `queue.submit` 後に finalizer へ到達する。
  - 実 driver error は注入せず、小さい acquire classifier seam で `RecreateSurface` / `SkipFrame`
    を分類し、両 outcome で seed texture が消えることを `Renderer::texture()` で検査した。
    §1.85-A の no-surface test は無修正で維持する。
- 規模 / 優先度: 小 / P3 (障害影響)。**ただし §1.31 の必須前提**。

### 1.84 表示キャッシュに item-context 世代の刻印が無い (latent、ABA 危険)

- 出典: v3.0.0 出荷前の holdover 調査 (2026-08-14) で Codex が併せて指摘。
  **今回の不具合 (`docs/briefs/pdf-page-turn-and-stale-composite-plan.md`) の原因ではない**
  が、同じ調査で見つかった同型の穴。
- フルスクリーン表示チェーンの以下が **`idx` だけ**、または item 文脈を含まない世代だけで
  引いている:
  - `current_edit_result_texture` — `idx` で走査し他の `EditResultKey` 世代を無視 (src/app.rs 53781)
  - `current_comic_composite_texture` — `idx` 直引き。コメントに「世代チェックなし」と明記 (src/app.rs 56470)
  - `current_final_composite_texture` — `key.edit_key.idx` のみ (src/app.rs 55823)
  - `conceal_cache` — `idx` + global conceal 世代のみ、`items_generation` を見ない (src/ui_fullscreen.rs 4297)
  - `EditResultKey` / `FinalCompositeKey` に item-context 世代のフィールドが無い
    (src/app.rs 5550 / 6032)
- **今は `close_fullscreen` が各キャッシュをクリアするので救われているだけ**
  (src/app.rs 49757)。軽量な items 差し替え経路がそのクリアを通らないと ABA になる。
- **意図された正しい形が同じファイルにある**: `ContinuousPageTransition` は
  `items_generation` を自分で持ち、`keep_set_evict` でも照合している (src/app.rs 6434)。
- 直すときの注意 (Codex): 「current 系 helper 3 つに条件を足す」では**不十分**。
  exact-key ヒットが src/app.rs 55292 / 55640 にもあるため、
  **key / read / insert のライフサイクル全体**に item 文脈を通す必要がある。
- 規模 / 優先度: 中 / P2。リリース直前に触る範囲ではない。

### 1.87 セッション最初の拡大で UI スレッドが 0.5 秒止まる (拡大パイプラインの初回作成)

- 出典: v3.0.0 リリース前の perf smoke (2026-08-15)。
- 実測: `gpu/lanczos_regenerate` は同セッション 16 回のうち **1 回目だけ
  `encode_submit_cpu_ms=495.1`**。残り 15 回は 0.2〜7.8ms (中央値 0.5ms)。
  同フレームの `ui/fs_viewport_breakdown` は `central_ms=495.3` / `media_ms=495.2` で、
  495ms 全部がフルスクリーン中央の描画に乗っている。セッション中で 50ms を超えた
  UI スレッド作業はこの 1 件だけ。
- 読み: 初回だけという分布は wgpu の pipeline / shader を最初の 1 回で作っている形。
  利用者から見ると「その日はじめて画像を拡大表示した瞬間に 0.5 秒固まる」。
- **今回入った退行ではない**。既定の高品質拡大は v2.12.0 からで、warm-up の構造は
  そのとき入っている (§1.46)。v3.0.0 のブロッカーとしては扱わない。
- 直すなら: 起動後の暇なフレーム、または一覧描画の最初の 1 回で pipeline を先に作る。
  どのタイミングなら利用者の操作とぶつからないかは計測してから決める
  (起動直後に足すと今度は起動が遅くなる)。
- 規模 / 優先度: 小〜中 / P3。

### 1.94 音声モードの退出が期限切れで detach+attach+seek に落ち、音が途切れる

- 出典: 利用者報告 (2026-08-18)。メインウィンドウへ結合した状態で <kbd>F11</kbd> と <kbd>Z</kbd> を
  連打すると、切り替えのたびに**メインウィンドウのグリッドが一瞬見えるちらつき**と、
  **音の途切れ**が起きる。
- **同日 (2026-08-18) に入れた「別ウィンドウの動画再生中に外部アプリから戻ると Z だけ効かない」の
  修正が原因ではない**。その経路は `source=egui_key` で記録されるが、実ログ中に
  2 回しか無く (別ウィンドウでの成功例)、後述のタイムアウト 7 件のいずれとも時刻が重ならない。
  失敗時の enter はすべて既存の `source=native_key`。ちらつき自体はリリース版 v3.1.1
  (本修正を含まないプロセス) でも観測されている。
- **実測した機構** (1 セッションで 7 回発生、`mimageviewer.log`):

  ```
  180.776 exit: placement changed, re-placing via SwitchPlacement
  180.877 exit ignored reason=exit_pending  deadline_remaining_ms=1099
  181.409 exit ignored reason=exit_pending  deadline_remaining_ms=567
  181.986 exit timed out; falling back to detach+attach+seek
  181.986 fallback exit: re-attached presenter, seek=32.298
  ```

  placement 切替を伴う退出が **`VideoAudioExitPending.deadline` 内に完了せず**、
  detach → attach → **seek** のフォールバックへ落ちる。この seek が**音の途切れ**、presenter の
  付け直しが**ちらつき**。連打すると毎回この経路に入る。
- **構造上の問題**: 退出の完了判定に**時間窓 (`deadline`) を使っている**。憲法 §2 規則 5
  「races を時間窓で吸収しない」に当たる。本来は placement 切替の完了イベントで確定すべきところを
  経過時間で見切っているため、切替が遅い状況では必ずフォールバックする。
- **直す方向**: deadline を伸ばさない。`SwitchPlacement` の完了 (または presenter の再配置完了) を
  typed なイベントとして受け、それで退出を確定させる。フォールバックは「イベントが来ない」ことが
  確定した場合の最後の手段に落とす。着手前に、退出の producer / consumer と
  open / switch / close / cancel / error の各 lifecycle を列挙すること。
- detached / native video のリワーク凍結領域。症状パッチを入れず、原因に対応付けてから直す。
  着手時は [detached-rework-plan.md](detached-rework-plan.md) §2 を読み、
  ClaudeCode / Codex 双方で「症状パッチではない」ことの合意を取る。
- 再現手順: メインウィンドウのフルスクリーンで動画再生 → <kbd>F11</kbd> と <kbd>Z</kbd> を連打。
  判定はログの `timed out; falling back to detach+attach+seek` の有無で機械的にできる。
- 規模 / 優先度: 中 / P2 (音が途切れるので体感は悪いが、連打しない通常操作では出ない)。

### 5.10 入力テストの共有ロックが poison して、失敗 1 件が全滅に見える

- 出典: 動画補正スロットの入力 parity 確認 (2026-08-19) の mutation 確認中。可視性ガードを外して
  1 件だけ落とすつもりが、
  同じフィルタの 5 件すべてが FAILED になった。実際に落ちたのは 1 件で、残り 4 件は
  `fullscreen fixed-key test lock poisoned: PoisonError { .. }` だった。
- 機構: `crate::key_input::TEST_INPUT_LOCK` は入力テストを直列化するためだけの
  `Mutex<()>` だが、12 箇所すべてが `.expect(...)` / `.unwrap()` で取得している。1 件が
  panic するとロックが poison し、以降このロックを使う全テストが**本来の失敗とは無関係な
  理由で**落ちる。**最初の 1 件を読まないと原因が分からない状態**になり、CI ログでも
  mutation 確認でも切り分けの手間が増える。
- 直す方向: 取得を 1 つの helper に集約し、`unwrap_or_else(|e| e.into_inner())` で poison から
  回復する。この Mutex はデータを保護しておらず、panic で壊れる不変条件を持たないので
  回復が正しい。**ただし共有フレーム状態の後始末は別問題**なので、helper 化のときに
  「各 call site が取得直後に `clear_test_frame()` するか、guard の Drop で戻すか」を
  揃えて確認する (現在は `FullscreenFixedKeyTestGuard` だけが Drop で戻している)。
- 対象: `src/key_input.rs` (定義 + 6 箇所)、`src/app/tests.rs` (5 箇所)、`src/keymap.rs` (1 箇所)。
- 規模 / 優先度: Small / P3。製品には影響しないテスト基盤の可読性問題。

### 5.8 検索ベンチのゲートに絶対時間の下限が無い

- 出典: v3.0.0 リリース前確認 (2026-08-14)。`check_bench_regression.py` が 10 件中 7 件を
  劣化と報告したが、**製品側の劣化ではなかった**。
- 何が起きたか: 判定が `+30%` の**比率だけ**で、対象クエリの絶対時間を見ていない。
  実測はこう:

  | クエリ | baseline | 3 回の実測 | 実行間ばらつき |
  | --- | --- | --- | --- |
  | `rare_jp` | 1.03ms | 3.61 / 1.13 / 0.89 | **+304%** |
  | `super_generic` | 0.01ms | 0.02 / 0.01 / 0.01 | +184% |
  | `rare_jp_and` | 0.15ms | 0.31 / 0.42 / 0.23 | +79% |

  **同一バイナリの実行間ばらつきが閾値 (+30%) を大きく超える**。1 回目は 25 分の
  配布ビルド直後で、全クエリが一様に遅い (ディスクキャッシュと CPU の状態)。
  3 回の最良値なら 10 件中 9 件が合格し、3 件は baseline より速かった。
- 唯一残る `rare_jp_and` も 0.15ms → 0.23ms で、**差は 0.08 ミリ秒**。この規模に
  比率の閾値を当てても意味がない。
- ずれている前提: ゲートは「1 回の計測が真の性能」と仮定しているが、ミリ秒未満の
  クエリでは計測ノイズのほうが大きい。
- 直す方向 (組み合わせる):
  1. **絶対時間の下限**を入れる。`+30%` かつ `+N ms` (N=1〜2 程度) の両方を満たしたときだけ
     劣化とする。ミリ秒未満のクエリが騒がなくなる
  2. ベンチ側で **1 クエリを複数回まわして最良値**を JSON に書く (現在も batch 内では
     `best=` を出しているので、外側にも同じ考え方を広げる)
  3. baseline に**測定条件** (日付・機種・同時実行の有無) を残す。現在の
     `vendor/bench_baseline.json` は `num_docs` と `version` しか持たず、
     いつ何の上で取った値か分からない
- 当座の運用: 失敗したら**同じバイナリで 3 回まわして最良値で判断する**。
  ノイズと判断した場合に `--save` で baseline を上書きしないこと (ノイズを基準値にすると
  次から本物の劣化を見逃す)。
- 規模 / 優先度: 小 / P3 (リリースのたびに人が判断すれば回るが、毎回時間を取られる)。

## 6. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| サブフォルダ展開 / フラット仮想ビュー | `docs/subfolder-expansion-view-plan.md`, `docs/ui-responsiveness.md`, `docs/async-architecture.md`, `docs/details-view-and-filter-plan.md`, `docs/virtual-folders.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md`, `docs/operation-customize-share-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| 詳細表示 / スマートフィルタ | `docs/details-view-and-filter-plan.md`, `CLAUDE.md` の UI / スクロール節 |
| タグ / フルスクリーン右パネル / 動画 overlay | `docs/tag-catalog-redesign-plan.md`, `docs/display-pipeline.md`, `docs/video-architecture.md`, `docs/detached-viewer-implementation-plan.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |
