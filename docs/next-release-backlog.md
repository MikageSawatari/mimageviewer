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

### 1.0g 前回終了時のカーソル位置を復元する — 利用者要望 (2026-08-30) — **v3.3.1 で対応済み**

**要望**: 「終了時に合わせてたカーソル位置の復元」。前回終了した場所は復元されるのに、
その中でどれを選んでいたかは戻らない。

**UI**: 環境設定の「前回終了した場所」に `□ 前回のカーソル位置を復元する` を追加。
既定 ON。`#[serde(default = "default_true")]` にすれば、**既存利用者は更新後も自動的に
ON** になる (フィールドが無い設定を読むと default が入る)。

**既存の仕組みにそのまま乗る。新しい選択機構を作らない**:

- [app.rs](../src/app.rs) の `select_after_load: Option<String>` と
  `try_select_after_load()` が既にある。項目名を大文字小文字無視で探して選び、
  スクロール位置の扱いまで回帰テスト付き
  (`select_after_load_overrides_folder_history` /
  `select_after_load_preserves_folder_history_scroll`)
- 保存先は [runtime_ops.rs](../src/app/runtime_ops.rs) の `on_exit_inner`。動画の再生位置と
  ウィンドウ状態を確定させている**まさにその場所**で、選択中の `item.name()` を
  `settings` へ書く。**選択のたびに保存しない** — §1.0b のとおり `Settings::save()` は
  ただではないし、終了時に 1 回で足りる
- 復元は [startup_ops.rs](../src/app/startup_ops.rs) の `open_default_startup_target`。
  `load_folder_or_convert_archive` を呼ぶ**前**に `self.select_after_load` へ入れるだけ

**注意点**:

- `select_after_load` は**パスではなく名前**で照合する。同一フォルダ内なので問題ないが、
  これは既存の意味論をそのまま使うということ (新しい照合規則を足さない)
- 復元するのは `StartupFolderMode::Previous` のときだけ。デスクトップ / 指定フォルダ /
  ドライブ一覧 / 閲覧履歴で開くときは対象外
- `last_folder` にはドライブ一覧の sentinel が入ることがある。そこは除外する
- 保存側は `last_folder` と同じガードを通す (synthetic view と detached viewer の文脈は
  メイン一覧の履歴として永続化しない)

**規模**: 設定 2 個・保存 1 箇所・復元 1 箇所・環境設定のチェックボックス 1 個 + docs。

**実装 (2026-08-30)**: 見立てどおり `select_after_load` にそのまま乗った。設計上の要点は
**保存側と復元側が同じ 1 つの述語を通ること** (`known_folders::startup_folder_is_last_folder`)。
片方だけ条件が増えると「保存はするのに復元しない」噛み合わない状態になる。

書いた当初は保存側に `is_synthetic_view_path` と detached suppression の 2 つを足していたが、
変異テストで **synthetic 側が生き残った** — `last_folder` に合成パスは書かれないので、場所の
一致判定が先に弾いていた。detached 側は `with_detached_viewer_main_history_suppressed` の中だけ
非ゼロになるスコープカウンタで、終了 / トレイ退避はそのスコープの外から来るため**常に false**。
detached の分岐は削除し、synthetic 判定は `last_folder` 側も合成パスに揃えたテストで縛って
「書き込み側の前提が崩れてもここで止まる」ことを保証した (動いているように見えるだけの
分岐を残さない)。

**スクロール位置も戻す (実機確認 2026-08-30 の追加要望)。** ただし保存するのはスクロール量 (pt)
ではなく「カーソルが画面の一番上の行から何行下にいたか」。pt をそのまま保存すると、ウィンドウ幅や
列数を変えた次の起動で同じ pt が別の行を指す。行数なら現在のレイアウトで計算し直せる。
`None` は「一番上にいた」ではなく**分からない**なので、`Some(0)` と別に扱う。

消えたフォルダから**祖先へ遡上**したときは復元しない。`resolve_startup_last_folder` が親を
返すので、素朴に「起動フォルダが決まったなら前回の続き」と扱うと、消えたフォルダの
カーソル名を親フォルダへ持ち込むことになる。

### 1.0h Z モードが中間枠と最終矩形で 2 回寄せる — Codex 指摘 (2026-08-30) — **v3.3.1 で対応済み**

**§1.0e と同種だがボケ方が違う。** Z (分析) モードは
[ui_fullscreen.rs](../src/ui_fullscreen.rs) で `ResolvedZTransform` を `Texels` で resolve し、
その `full_image_rect` を渡した先でもう一度 `Texels` で resolve している。

§1.0e のように「小さいテクスチャを大きい矩形へ貼る」ボケにはならない — 最終描画の
矩形と倍率はどちらも 2 回目の transform 由来で揃っている。残るのは 2 つ:

- 寄せが 2 回かかるので理想フィットより **1 物理ピクセルほど小さくなる**
- 呼び出し元へ返るのは **1 回目**の transform なので、返した geometry と実際に
  描いた geometry が違う (ヒット判定・ナビゲータ・UV がわずかにずれる)

**対応 (2026-08-30)**: 方向どおり直した。`ResolvedZTransform::resolve` が `pixel_fit` を
`Proportional` へ**固定する** — 呼び出し側の指定にしなかったのは、`fit_mode` /
`fit_scale_limits` / `free_rotation_rad` を同じ理由で既に固定しているから (Z の入力として
意味を持たない値)。照準枠・factor・full_image_zoom は viewport と content から決まるので
この宣言では動かないことを確認した。

`draw_fs_zoom_mode` は `draw_fs_image` の戻り値を返すようにした。通常表示の枝は元から
`single_transform = self.draw_fs_image(...)` になっており、Z だけが自分の中間枠を返して
いた。`single_transform` は当たり判定・ナビゲータ・カーソル写像の基準なので、
「画面に出ていない矩形」を配っていたことになる。

同じ調査で見つかった、直さなくてよいもの:

- 見開き焼き込みの **error fallback** の手計算 `(rect, scale)` — transform が組めなかった
  ときだけ通る。既に対で 1 か所に綴ってあるので、片側だけ transform 由来になることはない
- 見開き capture の source mapping — 寄せ済み矩形の再 resolve だが、描画ではないので
  同じボケにはならない

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

### 1.0j 貼り付けの残りが遅れて届いたときに拾えない — R-09 (2026-08-30) — **v3.3.1 で対応済み**

**R-08 (失敗した貼り付けの要求を残す) は v3.3.1 で直した。R-09 はこちら。**

`post_operation_selection` は最初の一群を適用した時点で要求を捨てる。大量コピーで
ファイルが数回に分かれて現れると、**2 回目以降が選択に入らない**。

**直すには「いつ完了か」を決める必要がある。** 今は「1 回でも結果が出たら完了」で、
それが早すぎるだけ。候補:

- **静穏時間**: 最後の追加から N 秒動きが無ければ完了。時間窓なので、遅い NAS では
  早すぎ、速いローカルでは遅すぎる。CLAUDE.md の「時間窓で競合を吸収しない」に触れる
- **Shell の完了通知**: ~~向こうに聞くのが筋~~ → **その経路は無い** (2026-08-30 に確認)。
  `IFileOperationProgressSink` は**自分で `IFileOperation` を回すとき**にしか付けられない。
  こちらがやっているのは `IContextMenu::InvokeCommand` でフォルダ背景の「貼り付け」verb を
  叩くことで、コピー操作を所有しているのは Shell 側。他人の操作に sink は挿せない。
  `SHChangeNotifyRegister` で項目が現れるのは見えるが、「終わった」は来ない (今の差分方式と
  同じ情報量)。
  つまりこの案は実質「**貼り付けを自前で実装する**」— クリップボードから `CF_HDROP` /
  `CFSTR_SHELLIDLIST` を読み、`CFSTR_PREFERREDDROPEFFECT` でコピー / 移動を決め、
  `IFileOperation` を組む。衝突時のダイアログ、エクスプローラーの「コピーの取り消し」、
  ショートカット、そして**パスを持たない仮想項目** (ZIP の中から、メールの添付から) の
  扱いまで背負う。仮想項目は今の verb 経由なら動くが、パスを前提にした `IFileOperation`
  では落ちる。**機能を減らす取引になる。**
- **同じ集合を再適用しない state**: レビューの提案。要求を保持しつつ、既に選んだ
  ものを覚えて差分だけ足す。完了判定は要らないが、要求がいつまで残るかは別途要る

**実害の大きさ**: 選択がずれるだけでデータ損失は無い。

**方針決定 (2026-08-30、利用者判断)**: 自前化はしない。**「届いた分を足す」+「利用者が
選択を変えたらそこで止める」**にする。

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

### 1.0d v3.3.0 リリースタグの再レビューが P1 5 件を指摘 (2026-08-29) — **v3.3.1 の中心**

出典は [docs/review-v3.3.0/README.md](review-v3.3.0/README.md) §9 (Codex による、公開済み
タグ `0d141615` に対する再レビュー)。§9 は静的レビューで、§9.4 自身が「成功した 41 test は
指摘した gap を覆っていない」と書いている。**そのため 5 件を独立に裏取りした (同 §10)。
結果は「5 件とも実在。ただし 2 件は新しい問題ではない」。**

| ID | 実在 | 深刻度 | v3.3.1 |
| --- | --- | --- | --- |
| R-27 (新規) | ✅ **失敗するテストで再現** | 高 — 複数ウィンドウ操作で表示と設定が食い違う | **修正済み** (`44e012a6` / `e84a8daf`) |
| R-02 | ✅ コード確認。実装が**自分のコメントの意図を裏切っている** | 中〜高 — 誤った窓を操作する | **修正済み** (`7f064a57`) |
| R-26 | ✅ コード確認。**無言のデータ損失**の連鎖を特定 | 高 — 頻度は低いが編集が黙って消える | **修正済み** (`b8cb3ce5`) |
| R-14 | ✅ (残存) doc comment に明記済みの設計上のトレードオフ | 中 | **R-07 と 1 件として扱う** |
| R-07 | ✅ 既知 | 中 | **既に予定済み** (§1.0) |

根拠と根本修正境界は §10 に書いた。要点だけ:

- **R-02**: 配り先 (`active_detached_context_is_at_rest()`、前面を見ない) と surface
  (`resolve_input_surface(.., foreground_app_hwnd(), ..)`、前面を見る) が別々の情報源を使う。
  `handle_gamepad_y_tap` のコメントは「前面がメインならメインを操作する」と書いてあるのに、
  mount 中の `self.selected` は detached bundle の field なのでそうならない。
- **R-26**: `open().ok()` → `None => Ok(())` (13 か所) → presence を立てて sidecar へ書く →
  次回起動の `import_to_dbs` は「中央が authoritative」なので**その sidecar を捨てる**。
  利用者にはエラーもトーストも出ない。
- **R-14 と R-07 は同じ構造問題** (編集文書という大きな値を UI スレッドが所有して複製する)。
  R-07 の根本修正 (worker が不変スナップショットを処理し、最新 generation だけ publish) に
  sidecar ミラーも乗せる。**別々に直さない。**

**R-27 / R-02 は detached の所有権境界に触る** ので、CLAUDE.md の凍結ルール
(症状パッチ禁止・構造的修正は ClaudeCode と Codex の合意 +
[detached-rework-plan](detached-rework-plan.md) への記録) に従うこと。

**R-27 は reducer 段を再現した**: `PresentationTransitionOwner` に
`Detached(H) -> Fullscreen` を commit させ、retire 待ちの間に `Detached` を
`ready_host = H` で要求すると、`NativeRetired` の同じ batch が
`DestroyHost { hwnd: H }` を出しながら後継を `ReadyToPrepare { host_hwnd: H }` にする。
再現テストは `src/app/presentation_transition.rs` の
`a_successor_does_not_reuse_the_host_the_same_batch_destroys` (現在 `#[ignore]`)。
production で `ready_host == H` になり得ることも確認済み — `native_video.rs` の
`ready_host_hwnd` は `detached_viewer_video_host_ready()` が真なら現在の detached host を
そのまま読み、retire 中の窓はまだ生きているため真になる。

**着地 (2026-08-30)**: 案 A′ (retiring が持つ session/window lease を後継へ移譲し、
`NativeRetired` 後に現在の host claim を採り直してから Prepare する) を実装した。
下は着手前に迷っていた記録として残す。

~~期待している `AwaitingHost` への差し替えが正しい着地か未確定。~~
`AwaitingHost` は毎フレームの `poll_video_presentation_transition` が
`detached_viewer_video_host_ready()` を見て解決するが、host を作るのは要求時の
`ensure_detached_viewer_window_id()` で、**`DestroyHost` はその後に走る**。作り直す側に
倒すなら「破棄後に誰が host を用意するか」を決める必要があり、所有権を移譲する側に
倒すなら `RetireOutgoing` の Close/Destroy を出さない判断が要る。どちらも detached の
所有権境界の設計判断で、CLAUDE.md の凍結ルール (症状パッチ禁止 / 構造的修正は
ClaudeCode と Codex の合意 + プランへの記録) の対象。

### 1.0e 縮小表示が Lanczos の結果を GPU で貼り直してボケる — 専用スレ >>311-312 (2026-08-29) — **v3.3.1 で対応済み**

**修正済み**: 原因 1 と 2 の両方を、リサンプラと描画矩形が**同じ 1 つの関数**
(`displayed_image_transform::physical_pixel_extent`) を使う形で直した。

- 丸め: 整数から 1e-3 以内なら丸め、それ以外は切り捨てる。`floor` だけだと f32 の
  `1440/1600` = 0.899999976 で**両軸とも 1px 小さくなる**。丸めだけにすると端数のある
  倍率で 1px はみ出す。両方を独立したテストで縛った。
- 矩形: 縮小方向でも原点とサイズを物理ピクセルへ寄せる (`snap_rect_to_physical_pixels`)。
  サイズは上の関数から取るので、2 か所で丸めが割れることがない。
- 寄せてよい矩形かを型で区別した (`RectPixelFit`)。ナビゲータの縮図は
  `Proportional` — 主表示を比例縮小した位置関係そのものが情報で、貼るのも
  リサンプラの出力ではないため。ここを寄せると可視範囲を示す枠が主表示とずれる。

回帰テストは「ある倍率でリサンプラの出力サイズと描画矩形の物理ピクセルサイズが
一致する」ことを、利用者の 2 枚 + DPI 125% + 縦長 + 回転 90/180/270 度で縛る
(`gpu_lanczos::tests::the_resampled_texture_is_exactly_the_size_it_is_drawn_at` ほか)。
`floor` へ戻す / 常に丸める / 矩形サイズを寄せるのをやめる、の 3 つの変異をすべて拒否する。

**副作用として PDF の下敷き差し替えが最大 0.5 物理ピクセル動く。** 低解像度の下敷きは
拡大側なので寄らず、最終ラスタは縮小側なので寄るため。該当テスト 2 本は「完全一致」から
「1 物理ピクセル未満」へ言い直した (削除も無効化もしていない)。全面のボケと引き換えに
見合う差と判断した。

**実機確認の手順**: `python scripts/make_downscale_check_chart.py` でチャートを作り
(既定の出力先 `C:\tmp\miv-lanczos-check`)、フルスクリーンのページフィットで開く。

- **1612x2418 を使う理由**: 4K のページフィットで、表示倍率 100% / 125% / 150% の
  どれでも旧コードが両軸とも 1px 短いテクスチャ (1439x2159) を作る。理論上の描画サイズは
  ちょうど 1440x2160 なので、修正後は 1 テクセル = 1 物理ピクセル。
- **整数倍は使えない**。倍率がちょうど 1/2 や 1/3 だと f32 に誤差が乗らず、旧コードでも
  正しいサイズになる。**整数倍はこのバグが出ない条件**。
- **判定**: 旧コードは 1439px を 1440px へ引き伸ばすので、再サンプルの位相が左端 0 →
  中央 0.5 → 右端 1 と流れる。位相 0.5 が 2 タップ平均になるので、**細かい縞のコントラスト
  が中央で落ちる**。修正後は全面で均一。オフライン再現での実測 (8 分割した区画の標準偏差):

  | | 左端 | | | 中央 | | | 右端 |
  | --- | --- | --- | --- | --- | --- | --- | --- |
  | 縦縞 2.5px 修正後 | 69.8 | 69.7 | 69.8 | 69.7 | 69.8 | 69.8 | 69.8 |
  | 縦縞 2.5px v3.3.0 | 62.1 | 47.0 | 33.3 | **23.5** | 33.2 | 46.9 | 62.1 |

  上下方向は画像の縦中央が最悪点で、横縞の帯は**帯ごと** 69.8 → 22 まで落ちる。

  利用者の 2 枚で他ビューアと並べる比較も併せて行う。

---

以下は報告時の分析 (そのまま残す)。

利用者が 2560x1440 / 100% / フルスクリーン単ページで、マンガミーヤ 7.4 / Leeyes 2.6.1 /
MassiGra 0.45 / ZipPla 1.7.2.9 / NeeView 46.3 と mIV 3.2.0 を同一画像で比較したスクリーン
ショットを提供した (低解像度 1120x1600 PNG = 画像A、高解像度 4248x6048 JPEG = 画像B、
いずれも 8bit グレースケール、全ビューアを Lanczos3 相当に設定)。**mIV だけ明らかにボケる。**

**カーネルは正しい。サイズと矩形が食い違っている。** `gpu_lanczos_spike.wgsl` の Lanczos3 を
そのまま再現すると他ビューアと一致する (下表)。問題は Lanczos で作った整数サイズの
テクスチャを、それとは違うサイズ・小数位置の矩形へ貼り、egui/wgpu が **もう一度バイリニアで
貼り直している** こと。

#### 測定 (ラプラシアン標準偏差 = シャープさ)

| | マンガミーヤ | Leeyes | MassiGra | ZipPla | NeeView | **mIV 標準** | シェーダ式を再現 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A (1120x1600 -> 1008x1440) | 104.6 | 104.6 | 104.6 | 105.1 | 116.9 | **61.5** | 105.3 |
| B (4248x6048 -> 1011x1440) | 75.4 | 75.5 | 75.4 | 75.5 | 80.0 | **55.5** | 75.8 |

他4本は PIL の Lanczos3 と RMSE 0.048 でほぼ完全一致 (= 教科書どおりの実装)。

#### 原因は独立して 2 つある。片方だけ直しても残る

1. **出力サイズの丸めが `floor` + f32** — [gpu_lanczos.rs:1096](../src/gpu_lanczos.rs) の
   `quantized_target_size()` が `floor(source * physical_scale)` を使う。`physical_scale` は
   f32 なので `1440/1600` は 0.899999976... になり、`floor(1600*s)=1439` /
   `floor(1120*s)=1007` と **両軸とも 1px 小さくなる**。1007x1439 を 1008x1440 へ
   引き伸ばすので全面がボケ、ずれは相殺されて 0 になる (画像A がこれ)。
2. **描画矩形が小数サイズのまま** — [displayed_image_transform.rs:167](../src/displayed_image_transform.rs)
   の `full_image_rect` は `display_size * total_scale` の小数サイズで、
   [同:735](../src/displayed_image_transform.rs) `snap_rect_origin_to_physical_pixel` は
   `physical_scale_is_near_integer` (= 倍率 1 以上で整数近傍) のときしか働かず、**原点しか
   スナップせず、サイズは一度もスナップしない**。画像B は縦が `floor(6048*s)=1440` で
   ぴったり合う一方、横は texture 1011px に対し矩形幅 1011.4286px なので横だけ再サンプル
   される。これは浮動小数の誤差ではなく、縮小では普通に毎回起きる。

#### 裏取り

- 画像B の周波数応答 (mIV / マンガミーヤ) は **横 0.16 / 縦 0.88** (0.95 ナイキスト)。
  位相相関で **横 -0.471px ずれ、縦 0.001px**。「縦は素通り、横だけ位相ほぼ一定の 2 タップ
  平均」= 追加バイリニアそのものの指紋。伝達率 |1-2φ| は φ=0.47 で 0.06 まで落ちる。
- 画像A を「Lanczos -> 1007x1439 -> バイリニアで 1008x1440」でモデル化すると
  **RMSE 0.366 / lapstd 61.50 (実測 61.51)**。ほぼ完全再現。
- 画像B のスクショが他より 1px 広い (1012 vs 1011) のも、1011.4286px 幅の矩形が
  1012 列にまたがった結果として説明できる。

#### 修正方針 (症状ではなく所有権を直す)

1. `floor` を許容誤差つき `round` にする。さらに f32 の倍率から再計算せず、**実際に描く
   物理ピクセル矩形から target を決める**。
2. **縮小方向でも矩形を物理ピクセルへスナップする。原点だけでなくサイズも。**
   1011.4286 -> 1011 に丸めても縦横比誤差は 0.5px 未満で目視不能。他ビューアはどれも
   「整数サイズのビットマップを作って 1:1 転送」しているので、そこで初めて同じ土俵になる。
3. target テクスチャサイズと描画矩形を**同じ関数から返す**。今は 2 か所で別々に綴られて
   いるので、丸めを直しても再びずれる (CLAUDE.md の「1 つの意味を 2 か所に書かない」)。

回帰テストは「ある倍率で target サイズと描画矩形の物理ピクセルサイズが一致する」ことを
`0.9` / `1440/6048` / DPI 125% など複数の倍率で縛る。既存の `quantized_target_size` の
unit test は整数倍率しか見ていないので通ってしまう。

#### 同時に見るべき副次の食い違い

- **ニアレストが縮小方向で効いていない。** 利用者のスクショは標準とバイナリ一致。NEAREST
  サンプラーは *ソース* テクスチャへ設定される ([app.rs:58279](../src/app.rs) ほか) が、
  縮小時に実際に貼られるのは Lanczos 出力テクスチャなので設定が届かない。仕様として
  「縮小ではニアレストは効かない」で通すなら UI で明示する。
- **「シャープ化」(`PostFilter::Sharpen`) と「シャープ拡大」(`PostFilter::UpscaleSharp`) が
  紛らわしい。** 前者は CPU で画素を書き換えるので縮小でも効き、後者は描画時の拡大器選択
  なので縮小では何もしない。利用者は前者を試して「ボケは減るがトーンにモアレ」と報告した。
- **シャープ化がトーンの濃度を上げる。** 縮小後のトーンはナイキスト近傍のリップルになり、
  シャープ化がそれを増幅する。明部のオーバーシュートは 255 で切られる (画像A は 61% が
  既に純白) 一方、暗部には余裕があるため**非対称にクリップされ、トーン領域だけ平均が
  下がる**。実測: 平坦な白 254.93 / 平坦な黒 12.6 は全変種で不変なのに、トーン領域は
  206.0 (標準) -> 195.9 (シャープ化) / 192.2 (AIシャープ化100)。全体の平均が下がって見える
  のはこれだけが理由。**ボケの代替にはならない**ので、上記 1-2 を直すのが本筋。
- なお標準でも、追加バイリニアがドットを紙に滲ませるため純白画素が 61.5% -> 55.5% に減る。
  「mIV はトーンが少し濁る」という印象はこれで説明できる。

### 1.0b Settings::save が実環境で 43ms かかる (2026-08-29 の perf smoke で判明) — **v3.3.1 で対応済み**

v3.3.0 のリリース前 perf smoke (実機・実設定) で確認した。**退行ではない** (v3.2.0 に同じ
コードがある) が、R-06 で使った合成データの実測 1.3ms は**実環境を 30 倍過小評価**していた。

**観測**: フォルダ移動 1 回ごとに `Settings::save()` が走り (`app.rs` の `sli_settings_save`)、
**1 回 43〜61ms**。38 回の移動で計 1.8 秒。`Ctrl+↓` 連打はこれを 1 フォルダごとに払う。
プロセス最初の 1 回は世代ローテを伴い 129ms。

**原因**: `%APPDATA%\mimageviewer\settings.db` が 36.7 MB あり、その 98% が VST3 の状態 BLOB。

| テーブル | 行数 | サイズ |
| --- | --- | --- |
| `vst3_plugins` | 7 | 32.04 MB |
| `vst3_chain_slots` | 2 | 4.16 MB |
| 他すべて | — | 0.2 MB |

`save_full` はハッシュ比較で**この 2 テーブルの行を書き直していない**。それでもここへ至るまでに

1. `save_internal` の `self.clone()` (36 MB)
2. `save_full` の `settings.clone()` (36 MB)
3. `serde_json::to_value(&persisted)` — 32 MB の base64 を含む Value を構築してから
   `extract_complex_fields` で捨てる
4. `hash_vst3_plugins` / `hash_vst3_chain_slots` — 36 MB をハッシュ

が走る。つまり **「書かないと決めるための準備」に 43ms** を払っている。

**修正済み** (`f5b9a50d`): `Vst3PluginEntry::state` を `Arc<str>` にして 2 回の clone を
参照カウント加算にし、dirty 判定を「直近保存した中身と共有 `Arc` が同じか」の O(1) 比較に
置き換え、`extract_complex_fields` がどうせ捨てる 2 フィールドを `Value` 構築前に外した。
実機と同じ形 (7 行 32MB + 2 スロット 4.16MB) の release 実測で **41ms → 0.13ms**。

| 段 | 修正前 | 修正後 |
| --- | --- | --- |
| clone (save_internal) | 10.4ms | 4.5µs |
| dirty 判定 | 6.3ms | 0.2µs |
| clone (save_full) | 10.2ms | 3.7µs |
| to_value | 10.5ms | 94µs |
| drop | 3.5ms | 25µs |

**DB のサイズ自体は変わらない** — VST3 の行は今も `settings.db` に入る。バックアップの
積み上がりは §1.0c で別に扱う。

R-06 (シークストリップのホイール) はこの実測を知る前に直したが、**判断は変わらない** —
1 ノッチごとに 43ms は論外なので、先送りにしたのは正しかった。

### 1.0c 設定バックアップが約 3 GB 残っている (2026-08-29 に観測) — **v3.3.1 で対応済み**

**修正済み**: `preupgrade-v*` と隔離 `.corrupted-*` に保持世代 3 の上限を付けた
(`db_backup::RETAINED_UNROTATED_BACKUPS`)。**作る側で上限を持たせる**形で、
新しいものが手に入ってから古い世代を落とす (先に消すと、失敗時に安全網が減った状態が残る)。

- 隔離は main / -wal / -shm の 3 ファイルで 1 世代。**セット単位で消す** — 1 ファイルだけ
  残すと WAL の無い main が残り、quarantine の rollback が防いでいる事故と同じ形になる。
- 更新時刻が読めなかった世代は**消さない**。消すのは取り返しがつかないので、上限を
  一時的に超えるほうを選ぶ。
- 判断は純関数 `backup_groups_to_drop` に分離してあり、時計を触らずにテストできる。
  ファイルを触る側 (`collect_backup_groups` / `remove_backup_groups`) は tempdir で検証する。

`bak1..10` は従来どおり (spec §6.1 のローテーションが最古を 1 個落とす)。
**1.0b で DB 自体は小さくならない** (VST3 の行は今も settings.db に入る) ので、
この上限が実サイズの効く対策になる。

---

以下は観測時の記録 (そのまま残す)。

同じ実機で `settings.db*` が **124 ファイル・2,980 MB**。内訳は `preupgrade-v*` 36 個
(1,053 MB)、隔離 `corrupted-*` 22 個 (775 MB)、その他 55 個 (748 MB)、`bak1..10` 10 個
(367 MB)、main 1 個 (37 MB)。

`bak1..10` は spec どおり。問題は **`preupgrade` と隔離ファイルに掃除の仕組みが無い**こと。
36 MB × N が黙って積み上がる。2026-07-19 の誤判定隔離 (古い検証バイナリが
`DetailsColumnId::PageCount` を `Corrupted` と誤分類) の残骸も含む。

**修正方向**: `preupgrade` は最新 N 世代だけ残す、隔離ファイルは復旧に成功した時点で
掃除するか、設定 UI から一覧・削除できるようにする (復元 UI が既に bak と preupgrade を
一覧表示しているので、そこへ削除を足すのが自然)。

**生成規則** ([settings.rs:7074](../src/settings.rs))**: 利用者が起動した版ごとに 1 個**、
`settings.db` の完全コピー。`if !pre_path.exists()` があるので同じ版で何度起動しても 1 個だが、
版が変わるたびに 1 個増える。**削除経路は存在しない** (`bak1..10` の世代ローテが最古を 1 個
消すだけで、`preupgrade` と隔離ファイルには何も無い)。mIV は現在 46 版あるので、初期から
使い続けている利用者は最大 46 個持つ。

**普通の利用者への実害は小さい**: VST3 を除いた設定の中身は実機でも 0.2 MB (設定 351 項目・
動画再生位置 1,322 件・お気に入り 34 件・タグ 4 件を**全部含めて**)。SQLite のページ確保を
足しても 1 版 1 MB 未満で、46 版ぶんでも数十 MB。**量ではなく「上限が無い」ことが問題**。

**1.0b と同時に直すのが効率的**: 根は同じ「設定 DB が VST3 で肥大する」なので、1.0b で
VST3 状態を `Settings` 本体から外せば DB が 0.2 MB 級に戻り、1.0c の実害も自動的に小さくなる。

### 1.0 v3.3.0 レビューからの持ち越し (2026-08-29 に判断)

正本は [docs/review-v3.3.0/README.md](review-v3.3.0/README.md)。v3.3.0 出荷時に
「複数ウィンドウの不具合解消」を優先し、以下を次版へ回した。**いずれも実測または
コード確認のうえで判断しており、推測で落としたものは無い。**

| ID | 内容 | 退行か | 実測 / 根拠 |
| --- | --- | --- | --- |
| R-07 (d) | 補正マスクの圧縮・DB 保存が UI スレッド | v3.2.0 以前から。**v3.3.0 が 5.8 倍速く**した後 | 24MP で 70.6ms/ストローク (v3.2.0 形式は 410ms) |
| (b) | 図形のキー移動 / 回転がキーリピートごとに全文書を保存 | v3.2.0 以前から | リピート 1 回ごとに 70ms + undo 2 文書 |
| (c) | 塗りの毎フレームに文書 1 複製が残る | v3.2.0 以前から | 24MP で 27.7ms/フレーム (もう 1 枚は `3315fd05` で削除済み) |
| R-23 | Remote の catalog 無し寸法 probe が直列・無制限 | **退行** (v3.2.0 は probe せず即返した) | ローカル SSD 初回 454µs/件 = 1,000 件で 0.45 秒。NAS なら 1〜10 秒。10,000 件で 4.5 秒。**v3.3.1 で一括・並列化 (答え不変)。件数を減らすかは §1.0i** |
| R-18 | Remote の ZIP 寸法 probe が 64KiB で打ち切る | いいえ (経路ごと v3.3.0 の新規) | v3.2.0 は寸法を一切持たず見開きが効かなかった。**見送っても v3.2.0 より悪くならない** |
| R-21 | 波形→サムネイル切替後も不可視の全尺解析が続く | いいえ (シークストリップが新機能) | 今回の目玉機能が裏で CPU/IO を食う。**v3.3.1 で対応済み** (破棄ではなく一時停止。サムネイル側は要求駆動なので対処不要と両 loop を読んで確認) |
| R-12 | detached parked fallback が UI で `parent().is_dir()` | いいえ | クリック 1 回につき 1 syscall。コード側にも「恒常経路ではない」と明記。**v3.3.1 では見送り** — レビューの修正方向 (parked snapshot を async build/commit/abort まで保持) は detached の lifecycle 変更で凍結ルールの対象。1 syscall のために所有権境界を動かす取引が見合わない (切断された共有だけが遅い) |
| R-08 / R-09 | paste 後の選択 request の lifecycle | いいえ | 選択がずれる / 不完全。データ損失なし。**R-08 は v3.3.1 で対応済み** (Shell 呼び出しが即失敗したら要求を取り下げる)。**R-09 も v3.3.1 で対応済み** (完了判定は持たず、届いた分を足す。利用者が選択を変えたら手を引く) |
| R-11 | トレイ退避が sidecar 書き込み完了を UI で待つ | いいえ | `df245720` で writer 側の複製 1 回分は減った。**対応済み** (2026-08-30) — `PersistScope` で「待つ」を終了経路だけの選択にした。待っていた理由はプロセスが止まることだけで、トレイ退避では writer も読み手も生きたまま (読み手は pending を先に見る) |
| R-19 | 360 動画の roll が描画へ渡らない | いいえ | 該当素材のみ。shader 配線 + 実機検証が要る |
| R-25 | 補正図形の Delete が `KeyAction` を迂回 | いいえ | 再割当不可・conflict 検出漏れ。**v3.3.1 で対応済み** (`LaDeleteShape`) |
| R-10 / R-13 / R-16 / R-22 | P3 4 件 | いいえ | R-10 は §1.141 で既に P3 裁定済み |

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

### 1.95 OS 側でコピー・移動したファイルへ編集内容を引き継ぐ (内容ハッシュ照合) — 利用者要望 ✅ Phase 1 実装済み (2026-08-22)

- 出典: 利用者要望 2026-08-20。エクスプローラー等でファイルを移動・コピーすると、
  補正 / 消しゴム / モザイク / 注釈 / トリミング / ★ / タグが引き継がれない。
- **正本: [edit-content-identity-plan.md](edit-content-identity-plan.md)。Phase 1 (A1-A6) 実装済み・実機確認済み。**
  複数コピー元の選択 (A6) まで完了。残りは同計画の「未着手」節 (アプリ内コピー / ページ移動の
  同じ失効漏れ、復元を通っていないファイルの注釈サムネイル) を参照。
  着手前に必ず読む。以下は要旨だけ。
- 現状の穴: 中央 DB は絶対パスキー。フォルダ側サイドカーは**フォルダごと**の移動しか救えず、
  リネーム移行は**アプリ内**リネームだけ。さらにサイドカーがミラーするのは
  `adjust / mask / conceal / local_adjust / export_crop / comic / tags` のみで、
  **★ / 回転 / 見開き / トリム / 本の続きはフォルダごと移動でも失われる**。
- 方式: 編集の確定点で物理ファイルのハッシュを台帳 (`content_identity.db`) に記録し、
  フォルダ読み込み時に **size → 先頭 64KB → 全体** の 3 段で照合。size はフォルダ走査で
  既に取得済み (`image_metas`) なので、候補が無いフォルダでは **I/O ゼロ**。
  照合と復元は worker (`GlobalIoSemaphore` Low、フォルダ切替でキャンセル)。
- コピー実処理は `rename_key_migration::STORES` 駆動の `copy_store` を新設して共有する
  (`App::copy_book_page_edit_key` は UI スレッド前提で worker から呼べない)。
- **変換アーカイブ (RAR/7z/LZH) を最初から対象にする**。キャッシュ ZIP のパスは元パスの
  純関数なので、再変換を待たずキーを付け替えられる。Source / ConvertedCache の 2 基底 ×
  exact / prefix の **4 面**をコピーする。
- 決定済み: 移動もコピーも既定で確認する / 復元範囲は★・タグ込みで一括 /
  「すべて選ぶ・すべて解除」ボタンを置く / 「このフォルダ以下では聞かない」は作らず
  全体 OFF 設定 1 個 (既定 ON、OFF で照合を完全停止) / リモートは対象外 /
  既存編集の遡りは一括スキャンせず訪問時に少しずつ。
- 規模 / 優先度: Medium / P2 (データ損失ではないが、編集した資産が黙って失われる体験は重い)。

#### 追加項目: 取り消した編集が復元元として残り続ける ✅ 修正済み (2026-08-25)

- 出典: Susie クラッシュ検証中に、編集していないはずのテストファイルで復元ダイアログが出た。
- **索引側は正しい。** `ContentIdentityIndex::upsert` は `has_restorable_content == false` の
  エントリを索引に入れない ([content_identity.rs:221](../src/content_identity.rs:221))。
  「中身が同じだけ」で候補になることは設計上ない。
- **記録側が実データと同期していない。** 台帳では 5 ファイルが
  `has_restorable_content = 1` かつ `last_edit_at` が全て同一だったが、**編集データは
  どのデータベースにも無かった** (`content_identity.db` 以外の全 DB を走査して 0 件)。
- **経路**: 利用者が `Ctrl+Alt+1` (`FsAdjustSlotDefault1` = 補正プリセットスロットを標準設定へ
  読み込む) を実行 → 編集として記録 → 対象が 8x8 単色画像で結果が標準値と変わらず、
  保存時に破棄 (`edit_preview_close: outcome=delete_no_edits`) → **台帳の記録だけが残る**。
- **構造**: `has_restorable_content` は 0 → 1 の単調遷移として実装されている
  ([content_identity.rs:219](../src/content_identity.rs:219))。遅れて届いた detection cache が
  先の復元元更新を消さないための設計だが、その結果**「編集を入れてすぐ取り消した」ファイルが
  永久に復元元として残る**。テストデータが特殊だったから目立っただけで、通常の画像でも
  「補正をかけて元に戻した」後に同内容のコピーを開けば同じことが起きる。
- **修正 (2026-08-25)**: 記録側で flag を下げる経路は作らなかった。編集を取り消す経路は
  `delete_mask_with_sidecar` / `remove_view_trim_page_override` / `remove_folder_thumb_pin` など
  複数あり、そのどれもが「編集した」として同じ record を通る。全部を数え上げて `Absent` を
  渡す形にすると、1 つ漏らしただけで**本物の復元元を消す**側に倒れる。
  - 代わりに**読む側で確かめる**。復元が運ぶのは `copy_stores_at` が `STORES` の unique 行を
    写すことが全てなので、そこに 1 行も無い復元元は選ばれても no-op にしかならない。
    **候補から外しても失われるものは無い**、が根拠。
    `rename_key_migration::ledger_keys_with_restorable_rows_at` が同じ表・同じ正規化で引く。
  - 台帳自身 (`content_identity.db` / `edit_origin`) は `STORES` に載っているが**除外する**
    (`content_identity::LEDGER_DB_FILE`)。台帳の行はこの問いの対象であって答えではない。
    最初これを数えてしまい、常に「行がある」になって probe が効かなかった。
  - 仮想ページの `<container>::<entry>` も見る。`LIKE` はパスに普通に現れる `_` / `%` を
    ワイルドカードとして拾うので、半開区間 (`::` 〜 `:;`) で引く。
  - **読めない store があった key は「行がある」側へ倒す**。余計な確認が 1 回出る方が、
    復元元を黙って消すよりはるかに軽い。
  - ついでに flag も下ろす (`clear_restorable_if_unchanged`)。単調遷移の例外はここだけで、
    根拠は「遅れて届いた 0」ではなく「実データがもう無い」。probe 中に UI スレッドが
    新しい編集を記録していたら `last_edit_at` の一致条件で弾かれる (compare-and-swap)。
    既に台帳へ残ってしまった stale 行も、次に該当コピーを開いた時点で自動的に消える。
  - probe は候補が出たときだけ走り (= full hash 一致が必要なので稀)、背景 I/O の枠内で行う。
- **既存テストの fixture を直した**: `folder_backfill_then_copy_...` と
  `book_byte_copy_is_declined_...` は、実データを 1 行も持たないファイルを backfill して
  候補を期待していた。本番の backfill は編集済みファイルにしか走らないので、fixture 側に
  実際の ★ 行を置いて production と同じ形にした (`rated-only.jpg` という名前が既に
  そう主張していた)。
- 再現データ: `C:\tmp\miv-susie-crash-test` (中身が同一の `MIVOK` ファイル 5 個)。
- 規模 / 優先度: 小〜中 / P3 (実害は余計な確認ダイアログ。データは失われない)。

### 1.99 複数ウィンドウモードで RAR を開くとメイングリッドまで書庫一覧へ切り替わる — 専用スレ >>270 ✅ 実装済み (2026-08-22)

> 実装は detached-rework の `21c3dc0d` / `d7e139d0` に入っており、2026-08-22 の master マージで
> 取り込まれた。型付き宛先意図 `OpenRequestOwner::DetachedGridArchive` →
> `ArchiveConvertCompletionPolicy::DetachedGridArchive` を、ダブルクリック / Enter /
> 直読み完了 / 変換完了 / cache hit のすべてが引き継ぐ。visibility 述語・`show_viewport_*`・
> R2e は不変。**実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>270 (2026-08-20)。利用者環境・手元とも再現済み。
- 症状: 複数ウィンドウモードで RAR を開くと独立した画像ウィンドウだけでなく、
  メインウィンドウのグリッドも RAR 内の一覧へ切り替わる。ZIP では発生しない。
- 期待する不変条件: 複数ウィンドウモードから画像 / 本を開く操作は、表示先の独立ウィンドウだけを
  更新し、メイングリッドの場所・一覧 generation・検索 / 絞り込み・選択状態を変更しない。
- 修正方針:
  - RAR の open request が、どの入口からメイングリッドの一覧遷移へ流れているかを確認する。
    RAR だけを後段で戻す guard は入れず、表示要求の所有 context と dispatch 先を正す。
  - 直読み RAR / CBR と変換対象 RAR の両方を確認し、ZIP / PDF を対照経路として open request の
    差を棚卸しする。
  - detached 述語 / viewport 経路へ触れる場合は [detached-rework-plan.md](detached-rework-plan.md)
    §2 に従い、症状パッチでないことを ClaudeCode / Codex の双方で確認し、同書 §11 に記録する。
- 回帰確認: 複数ウィンドウモードで RAR / CBR / ZIP / PDF を順に開いてもメイングリッドが不変で、
  各独立ウィンドウには正しいページが表示される。フル機能ウィンドウの従来の一覧遷移は壊さない。
- **根本原因 (2026-08-21 特定、v3.1.3 では未修正)**: ZIP / PDF はダブルクリック・Enter・
  ゲームパッドのどの入口でも共通の detached 振り分け ([ui_main.rs:13017](../src/ui_main.rs:13017)、
  [app.rs:34249](../src/app.rs:34249)) を先に通るが、**typed open plan が本コンテナとして認識するのは
  PDF / ZIP だけで、`ConvertibleArchive` である RAR / CBR は対象外**
  ([app.rs:37317](../src/app.rs:37317))。RAR は振り分けを素通りして通常ナビゲーションへ落ち、
  所有者が `OpenRequestOwner::Navigation` に固定される ([app.rs:18298](../src/app.rs:18298))。
  RAR は同期的に ZIP として開けず、直読み可否を App-global で probe した**後**に
  メイン一覧へ遷移する ([archive_convert.rs:675](../src/ui_dialogs/archive_convert.rs:675) /
  [854](../src/ui_dialogs/archive_convert.rs:854))。
- **「RAR を ZIP と同じ descriptor に足す」では直らない。** RAR は非ソリッド・入れ子なしだけが
  直読みで、それ以外は変換対象という分岐がある ([virtual-folders.md:116](virtual-folders.md))。
- **正しい修正 (2026-08-21 に具体化)**: 完了時点では detached 窓も bundle もまだ存在しないので、
  「既存 context の所有者」ではなく **「この要求のために新しい detached context を作る」という
  型付きの宛先意図**を非同期完了へ持たせる。**同型の実装が既にある**:
  ブックマーク経路は `OpenRequestOwner::Bookmark(owner)` →
  `ArchiveConvertCompletionPolicy::Bookmark(owner)` ([archive_convert.rs:47](../src/ui_dialogs/archive_convert.rs:47))
  を持ち、直読み完了と変換完了の両方から
  `open_converted_bookmark_in_detached_context` ([app.rs:39993](../src/app.rs:39993)) へ着地して
  `ViewerContextDescriptor::Zip { path: 実体, archive_source_override: 元アーカイブ }` で
  **新しい detached context を作る**。グリッドからの `ConvertibleArchive` 開きにも、
  `DetachedGridItemOpenPlan::FolderCandidate` ([app.rs:1857](../src/app.rs:1857)) と同じ形で
  candidate variant と着地関数を用意する。
  入口は既に共通化されているので、ダブルクリック ([ui_main.rs:13017](../src/ui_main.rs:13017)) と
  Enter ([app.rs:34249](../src/app.rs:34249)) の両方が同じ gate を通る。
  **visibility 述語や `show_viewport_*` builder の変更は不要。**
  変換キャッシュ命中時の同期経路 (`open_archive_via_cache_owned`) も同じ宛先意図を通す。
- **R2e (viewer context registry) には依存しない** (正本:
  [detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §5)。
- 規模 / 優先度: Medium / P1。次版修正候補。

### 1.100 複数の画像ウィンドウを開くと、先に開いたウィンドウでマウスジェスチャが効かなくなる ✅ 解消済み
- **状態: 2026-08-27 実機で解消を確認 (利用者)。** 非アクティブなウィンドウでも、リング
  ショートカット・マウスジェスチャの両方が発火する。
- **いつ直ったか**: `d69fed5b` (2026-08-22、左クリックで右ドラッグを取り消す) の本文が経緯を
  書いている。右ドラッグを開始できる面は 3 つではなく 4 つで、4 つ目の
  **passive detached window は「この項目が書かれた後に detached リワークで増えた」**。
  入口は [`apply_passive_detached_right_drag_event`](../src/ui_fullscreen.rs) で、
  `RightDragOwner::DetachedWindow(window_id)` を持つ。`MouseGestureState` も
  表示種別だけでなく `owner` を持つようになった ([ring_shortcut.rs](../src/ring_shortcut.rs))。
- ⚠ **下の「根本原因」は 2026-08-21 時点の記述で、現在のコードには当てはまらない。**
  OS watcher が `VK_LBUTTON` しか見ないのは今もそのままだが、passive window が独自に
  pointer 入力を報告するようになったため、アクティブ化を待つ必要が無くなった。
  **項目が名指ししたコードが変わっていないことは、不具合が残っている証拠にならない**
  (2026-08-27、ClaudeCode がこの誤りを踏んだ)。

- 出典: 専用スレ >>270 (2026-08-20)。利用者環境・手元とも再現済み。
- 症状: 複数ウィンドウモードで画像ウィンドウを複数開くと、後から開いたウィンドウでは
  マウスジェスチャを使えるが、先に開いたウィンドウへ戻っても認識されない。
- 期待する不変条件: 右ドラッグの入力状態と発火先は、入力を開始した viewer window / viewport が
  所有する。アクティブウィンドウを切り替えても別ウィンドウの状態に上書きされず、閉じた
  ウィンドウの状態も残らない。
- 修正方針:
  - pointer event の source、ジェスチャ状態機械、実行 context の各所有先を追い、グローバルな
    active target や最後に開いた viewport へ誤って寄っている境界を正す。
  - open / activate / deactivate / close / cancel の lifecycle と、リングショートカット・短い
    右クリックの兄弟経路も同じ owner 規則になっているか確認する。
  - detached 述語 / viewport 経路へ触れる場合は §1.99 と同じく
    [detached-rework-plan.md](detached-rework-plan.md) §2 / §11 の規約に従う。
- 回帰確認: 3 枚以上の独立ウィンドウを開き、前後のウィンドウを交互にアクティブ化して同じ
  ジェスチャが発火する。最新ウィンドウを閉じても先行ウィンドウで継続し、メイングリッドと
  native 動画のジェスチャを壊さない。
- **根本原因 (2026-08-21 特定・訂正、v3.1.3 では未修正)。これは R2 そのもの**:
  - **非アクティブな静止画ウィンドウ上の右ドラッグは、アクティブ化もせずジェスチャ状態機械にも
    届かない完全な no-op である。** 非アクティブな静止画窓は `show_viewport_deferred` で描かれ、
    コールバックが root pass へ報告するのは描画 / focus / close 要求 / placement だけで、
    ポインタ入力を一切拾っていない ([ui_fullscreen.rs:11622](../src/ui_fullscreen.rs:11622)、
    [ui_fullscreen.rs:11175](../src/ui_fullscreen.rs:11175))。アクティブ化を担う OS watcher は
    **`VK_LBUTTON` しかサンプルしない** ([detached_window_manager.rs:326](../src/app/detached_window_manager.rs:326))。
    左クリックでアクティブ化した後ならジェスチャは効く (§4.5 の「アクティブにすれば動く」と一致)
  - `MouseGestureState` が持つ識別情報は表示種別だけで、**window ID / viewport ID / session ID を
    持たない** ([ring_shortcut.rs:53](../src/ring_shortcut.rs:53))。状態は
    `ViewerContextBundle` ではなく **App に 1 個だけ**ある ([app.rs:10005](../src/app.rs:10005))。
    発火時に owner を再解決せず、その時点で mount されている context へ適用する。
    複数の画像ウィンドウはどれも `RightDragContext::ImageFullscreen` なので区別できない
  - `last_input_surface` は `MainWindow` / `Viewer` の二択で全 viewer window を区別しない
    ([detached_window_manager.rs:452](../src/app/detached_window_manager.rs:452))
  - **訂正**: 当初「非アクティブな窓が集める pointer 情報は `any_pressed` / `any_released` だけ」
    「アクティブ化は press ではなく release で起きるので最初の右ドラッグが失われる」と記録したが、
    これが当てはまるのは `show_viewport_immediate` で描く `ParkedLive` の**動画 / 音声窓だけ**
    ([ui_fullscreen.rs:11785](../src/ui_fullscreen.rs:11785)、[app.rs:39217](../src/app.rs:39217))。
    静止画窓はそこへも到達しない
- **利用者決定 (2026-08-21)**: 「ジェスチャを認識し、ジェスチャされた場合は自動でアクティブ化した
  上で、ジェスチャコマンドを実行する」。**両方のウィンドウが見えている以上、ジェスチャが受理される
  のが期待動作**という判断。右押下だけでアクティブ化する案 (1 回目のドラッグを消費する / 単なる
  右クリックで窓が前面化する) は採らない。
- 必要なもの (3 つ、いずれも R2 の守備範囲):
  1. 非アクティブ窓のポインタ列を、その窓の identity 付きで root pass へ届ける
     (`DeferredDetachedImageWindowEvent` は既に window id を持つ)
  2. ジェスチャ状態を **window 所有**にする (= R2b 残件の「散在 state の typed 集約」)
  3. 成立時に「アクティブ化 → コマンド実行」を**型付きの順序**として表す (guard / 遅延ではなく)
- **R2 (`DetachedWindowRuntime` + 状態の集約) の一部として実施する。** ジェスチャ状態を window
  所有にするのは R2 の守備範囲であり、App 側に識別子を足す小修正は憲法 3 に抵触する。
  一方、コマンドはアクティブ化**後**に実行するので、マウントされていない context へ適用する必要は
  無く、**R2e (viewer context registry) の完成には依存しない**
  (正本: [detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §5)。
- **状態 (2026-08-21)**: 実装済み・検収合格・**実機確認済み**。静止画 (`8db282e3` + `5b83df3f`)、
  ガイド表示 (`d2c19796`)、動画 ParkedLive (`c71d8c08` + `4c5260a2` + `12f60c97`) のすべてで動作確認済み。
  指示書は [detached-rework-stage-passive-gesture.md](detached-rework-stage-passive-gesture.md)、
  進捗記録は [detached-rework-plan.md](detached-rework-plan.md) §9.5。
  **実機で確認が取れて出荷するまで本項は消さない** (根本原因・利用者決定・訂正履歴の正本)。
- 規模 / 優先度: Medium / P1。

### 1.101 動画の上部 HUD / 下部シークバーを個別に固定表示できるようにする — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> 設定は動画専用 bool `video_top_bar_locked` / `video_seek_bar_locked` の 2 つ。既定は両方 `false`。
> 静止画の `fullscreen_*_locked` は共有せず、余白だけ `fullscreen_fixed_bar_gap_px` を再利用する。
> 固定状態は可視性純関数への**入力**として渡し、描画と HUD HWND の hit-test region は
> `render_once` が算出した `top_bar_drawn_visible` / `bottom_hud_visible` の**同じ snapshot** を読む。
> 上部の固定は下部へ漏らさない。tile grid / navigation preview 中は上下とも抑止する。
> 固定バーは映像へ重ねず、`compute_video_visual_transform` の target からバー領域と余白を除外する。
> compact は除外後の残り領域の右上 1/4 へ合成する。鍵アイコンは静止画のベクター描画を共有する。
> 固定表示は external drag 中も維持し、touch latch 自体は変えない。
> 音声専用設定は増やさず、通常の音楽ビューは従来どおり常時表示。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。動画シークバーを常時表示したい要望。
- 方針: 静止画と同じ考え方で、動画の上部 HUD と下部シークバーをそれぞれ独立して
  固定 / 自動表示へ切り替えられるようにする。動画専用設定とし、既定は現在の自動表示を維持する。
- 実装先は native presenter の HUD overlay
  (`src/video/native_presenter/{render_core.rs,overlay_draw.rs}`) とし、旧 egui 動画 UI へだけ
  設定を足さない。通常の音楽ビューは対象外とし、VST 用 audio-only native shell だけ動画設定を共有する。
- 固定中は映像 target からバー領域を除外し、VST editor、タッチ操作、HUD HWND の hit-test region /
  z-order を確認する。固定解除後は既存の自動表示へ戻ること。
- 規模 / 優先度: Medium / P2。

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

### 1.103 ごみ箱へ移す場合に mIV の削除確認を省略できる設定 — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> 省略の述語は `設定 ON && DeleteConfirmKind::RecycleBin`。
> **フォルダと ZIP / PDF / 対応アーカイブも省略対象に含む。** ごみ箱から復元できることを前提に、
> タイル 1 つが一覧に見えていない多数のファイルを持ち、そのすべてを確認なしでごみ箱へ移すことは
> 設定画面の説明で明示する。
> `MayPermanent` は従来どおりキャンセルが初期選択、`FOF_WANTNUKEWARNING` も不変。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。削除確認を省略したい要望。
- 仕様:
  - 設定は既定 OFF。ON の場合も、mIV の事前判定が `DeleteConfirmKind::RecycleBin` のときだけ
    mIV の確認ダイアログを省略する。
  - リムーバブル / ネットワークドライブ、ボリューム設定、容量等から
    `DeleteConfirmKind::MayPermanent` と判定した場合は、従来どおり確認を表示し、初期選択を
    キャンセルにする。
  - Shell 側の最終判断は mIV から確定できないため、`IFileOperation` の
    `FOF_WANTNUKEWARNING` は維持する。Windows が出す完全削除警告までは抑止しない。
  - 複数選択時は 1 件でも `MayPermanent` があれば省略しない。全件 `RecycleBin` なら、フォルダや
    一覧に見えていない内容を持つアーカイブも確認を省略する。
- 回帰確認: 通常のローカルファイル、フォルダ、ZIP / PDF / 対応アーカイブをごみ箱へ移す場合は
  確認が省略され、完全削除候補、混在選択、Shell がごみ箱へ移せないケースでは警告導線が残る。
  設定 OFF は従来動作と同じ。
- 規模 / 優先度: Small / P2。

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

### 1.109 複数ウィンドウ・見開き表示でページ戻りを長押しすると操作不能になる — 専用スレ >>276 ✅ v3.1.3 で出荷済み

> 修正は 2 段。`9b84c265` が「rendition sequence が active というだけで全 upload を止める」decision を
> 廃し、塗り元 (paint source) で 3 状態に分けた (`defer_ui_uploads` は後方互換の計測フィールドへ降格、
> 実判定は `ui_work_admission`)。`c3a65a6b` が、その時「別原因」として残した「detached context が
> 自前の thumbnail 結果を捨てる」を直した。**本項の登録はこの 2 コミットより後** (08-21 00:00 対
> 08-20 22:24 / 23:24) なので、調査の記録として書かれたまま完了印が付いていなかった。
> 実機確認済みで、**v3.1.3 (2026-08-21) の更新履歴に記載して出荷済み**。v3.2.0 の対象ではない。
> 以下は着手前の記録。

- 出典: 専用スレ >>276 (2026-08-20)。v3.1.2 の複数ウィンドウモードで報告。
- 報告条件:
  - 見開き表示でカーソルキーを長押ししてページを戻すと、その後操作不能になる。
  - JPEG ではかなり多く戻ったとき、WebP では数ページで発生しやすい。
  - **手元では、複数の静止画ウィンドウを開いた状態で、静止 WebP 600 枚の見開きを
    長押しで往復すると再現した**。テストデータは
    `C:\tmp\miv-spread-webp-portrait-600-20260820` (1200x1800、連番入り)。
- 状態: **再現・perf log で原因確認済み**。WebP のデコード負荷や UI thread の停止ではなく、
  通過表示用 navigation sequence と full-size upload pacing の循環待ち。
- 実ログの時系列 (`perf_events.jsonl` / `mimageviewer.log`、2026-08-20):
  1. 見開き `[415, 416]` の full-size load を開始し、静止 WebP の decode は両方とも
     約 20〜21ms で完了した。
  2. `idx=416` は `fs/ready` まで進んだが、`idx=415` は `fs_upload_backlog` に残ったまま、
     次のページ戻り入力で `[415, 416]` が navigation target になった。
  3. `idx=415` の thumbnail は `thumbnail_not_loaded` のため pass-through rendition を作れず、
     `materialized_ready=false` / `rendition_ready=false` / `still_awaiting` になった。
  4. 一方、`page_turn_decision_for_inputs` は rendition 対応 sequence が active というだけで
     `defer_ui_uploads=true` にする (`src/ui_fullscreen.rs:8832-8845`)。
     `poll_prefetch` はこの値で早期 return するため (`src/app.rs:63201-63207`)、すでに decode
     済みの `idx=415` を backlog から GPU へ載せる経路まで止まった。
  5. target が完成しないので sequence は退役できず、直前の `[417, 418]` を
     `nav_holdover` で表示したまま循環した。ログでは `UI uploads deferred` が 32768 frames、
     `no stand-in` が 131072 frames まで継続した。
- **根本原因**: rendition が未準備の navigation sequence が、target を materialize するために
  必要な upload 自体を抑止できる状態モデルになっている。見開きの片側だけ upload backlog に
  残るタイミングで自己解除不能になる。複数ウィンドウはこの順序を再現しやすくする明確な条件だが、
  共有 navigation / upload 経路の問題なので単一ウィンドウで絶対に起きないとは仮定しない。
- 期待する不変条件: 長押し中に受理したページ移動が順に処理され、キーを離した後は最後に
  表示したページで必ず settled になる。前方向 / 後方向、素材形式、先読み完了順にかかわらず、
  次の入力を塞ぐ pending / transition 状態が残らない。
- 修正方針:
  - timeout、強制 reset、repaint 追加では直さない。
  - pass-through rendition が未準備でも、対象見開きの完成済み full-size result を
    `fs_upload_backlog` から反映して materialized target を settle できる状態遷移にする。
    `rendition_sequence_active` だけで全 upload を止める現在の decision を見直し、
    rendition の待機と target materialization の所有関係を typed phase で明確にする。
  - 1 フレームあたりの upload pacing と「現在ページ + 見開き相方」の優先順位を確認し、
    片側だけ反映された直後の入力でも完成経路を失わないようにする。
  - navigation sequence、`fs_pending`、`fs_upload_backlog`、viewer context の mount / park / activate
    を同じ context owner と generation で棚卸しし、前後移動・open / switch / close / cancel / error
    の兄弟経路にも同じ循環がないか確認する。
- 回帰確認:
  - JPEG / PNG / 静止 WebP / Animated WebP、同一枚数・同一寸法で比較する。
  - 左綴じ / 右綴じ、左右キー、前進 / 後退、先読み枚数、サムネイルキャッシュ有無を振る。
  - 複数ウィンドウモードを主対象にし、フル機能ウィンドウと F12 detached を対照にする。
  - 少なくとも「target の片側に thumbnail が無い」「もう片側だけ full-size ready」「残り片側の
    full-size result は upload backlog」という実ログの順序を固定し、target が materialized で
    settle して backlog が drain されることを検査する。
  - page-turn request / accepted / materialized / settled、表示 generation、cache source、
    key level / release、active viewer context を同じ perf log で追い、同一 target の
    `still_awaiting` が無期限に継続しないことを確認する。
- 修正時の注意: 複数ウィンドウの入力 / 表示状態所有に触れるため、
  [detached-rework-plan.md](detached-rework-plan.md) §2 の禁止事項に従う。再現した症状だけに
  reset を足さず、前後移動と全素材で共有する state transition の破れを所有境界で直す。
  detached predicate / viewport 経路へ変更が及ぶ場合は ClaudeCode / Codex 双方で構造的修正と
  合意し、`detached-rework-plan.md` §11 に変更範囲と理由を記録する。
- 規模 / 優先度: Medium / P1。

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
- **現状の整理**: 右クリックの「アプリケーションで開く…」から Windows 関連付けアプリと任意 exe を
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

### 1.120 Susie 32bit ワーカー異常終了後の自己回復 — 外部SNSでの不具合言及 ✅ 実装済み (2026-08-25、実機確認済み)

- 出典: 2026-08-24。mIV の Susie 中継 exe が繰り返しエラーで落ちるとの外部SNS投稿。
  プラグイン名、対象画像、ログ、並列設定は不明で再現未確認。直接の報告ではないため、まず条件採取が必要。
- **コード上確認できた問題**:
  - Susie プラグインを32bit子プロセスへ隔離しているため、プラグインの access violation 等で本体まで
    落ちない設計は維持されている。
  - 起動成功後に worker が想定外終了しても自動 respawn しない
    ([async-architecture.md:757](async-architecture.md#54-pdf-ワーカー--susie-ワーカーの想定外終了))。
  - `run_dispatcher` は `send_recv` の transport error 後も loop を抜けないため、死んだ pipe の dispatcher が
    共有 queue から後続 job を取り、エラーを返し続ける可能性がある
    ([susie_loader.rs:794](../src/susie_loader.rs:794))。
  - README の「プラグインクラッシュ時は自動再起動」という説明は現行実装と食い違う。実装を合わせるまで
    説明を訂正するか、既知の問題として明示する。
- 修正方向:
  - worker slot ごとの generation と状態 (`Starting / Ready / Failed / Backoff / Shutdown`) を持つ
    `SusieWorkerSupervisor` が child / dispatcher の lifetime と実効 worker 数を所有する。
  - EOF / BrokenPipe 等の fatal transport error ではその dispatcher を即座に退役させ、残り queue を
    奪わせない。回数上限と backoff を付けて同じ slot を再 handshake する。
  - クラッシュを起こした in-flight request は自動再送しない。同じ不正画像 / プラグインで crash loop に
    なるため、その item はエラーとして返し、後続 request のためだけに worker を補充する。
  - 通常のプラグイン decode error と worker transport failure を型で分け、診断画面へ plugin / extension、
    worker id、再起動回数、最終失敗理由を出す。並列実行 OFF で回避できる plugin-global race も案内する。
- 回帰: handshake 後に意図的に exit するダミーワーカーで、in-flight 1件だけが失敗し、後続は再生成された
  worker で成功すること、shutdown / cancel と respawn が競合しないこと、再起動上限後は queue を
  無限消費せず persistent notice になることを統合テストする。
- 規模 / 優先度: Medium (3〜7日) / P2。利用者のプラグイン名・画像・ログが得られれば優先度を再判断する。

#### 実機で再現させた結果 (2026-08-25)

検証手段が無かったので、**落ちるプラグインを自作した** ([susie-crash-plugin.md](susie-crash-plugin.md))。
`C:\tmp\miv-susie-crash-test` の 8 ファイル (正常 5 / クラッシュ 3) を一覧で開いた結果:

- **ワーカーはクラッシュのたびに 1 つずつ減り、再生成されない。** 起動時 3 本
  (`susie: 3 workers ready`) が、クラッシュ 3 回で **0 本**になった。この間 mIV 自身は動作を続けている。
- 枯渇後は Susie 形式が**一切読めない**。利用者からは「しばらく使っていたら Susie が全部読めなく
  なった」に見える。**アプリを再起動するまで戻らない。**
- 一覧の初回表示では 8 枚中 6 枚が成功した。生き残っているワーカーが処理できたためで、
  **1 回のクラッシュで全滅するわけではない**。枯渇するまでは部分的にしか見えない。
- 表示が残っていた 1 枚はサムネイルキャッシュ由来 (`state=DisplayReady(LiveCache)`) で、
  キャッシュが切れると失敗に変わった。

**この結果を受けて段階分けを変更する。** 当初案の「段階 1 = dispatcher の退役だけ、respawn は後」
では**直らない**。dispatcher を退役させてもワーカーは減り続け、枯渇後の結末は同じである。
respawn までを 1 つの修正として扱う:

1. transport error でその dispatcher を退役させ、共有キューを吸わせない
2. **backoff 付きで同じスロットを再起動する** (本命。これが無いと枯渇は止まらない)
3. クラッシュを起こした要求は**再送しない** (同じ不正画像で crash loop になる)。その 1 件は
   エラーで返し、後続のためだけにワーカーを補充する
4. 再起動上限に達したら無言で諦めず、利用者へ通知する

**副産物: フルスクリーンが Susie 拡張子を認識しない** (§2.21 として分離)。

#### 実装 (2026-08-25、実機確認済み)

`af6882e5` + `9ecbbec7`。4 つの欠陥を閉じた。**順に実機で確かめ、直すたびに次が現れた**ので、
その経緯も残す。

1. **dispatcher が終了理由を返す** (`DispatcherExit::{Shutdown, WorkerLost}`)。以前は
   `shutdown` でしか抜けず、死んだ pipe を持つ dispatcher が共有キューから後続を取り続けた。
   しかも失敗は即座に返るので、生きているワーカーより速く吸っていた。
   落ちたと扱うのは transport が切れた場合だけで、`InvalidData` 等は含めない。
2. **スロットが backoff 付きで作り直す** (`run_worker_slot`)。上限 5 回、200ms × 試行回数。
   待つのは競合を時間で隠すためではなく、crash loop が CPU を焼き切らないため。
3. **再起動回数は連続失敗で数える** (`restart_count_after_loss`)。累積で数えていたため、
   実機では 3 スロットとも上限に達して Susie が丸ごと死んだ。1 件でも応答を返せた
   ワーカーは働けていたので数え直す。上限が意味を持つのは「起動しても何も返せない」場合だけ。
4. **最後のスロットが閉じたらキューを排出する** (`fail_pending_jobs_no_workers`)。
   これが無いと積まれた要求に誰も答えず、UI が「読込中」で固着した (実機で確認)。
   投入の可否は `is_ready()` の外側チェックではなく**積むのと同じロックの中**で見る。
5. **ワーカーを落とした対象は二度と投げない** (`crashers`)。失敗したデコードはサムネイルに
   残らないので、記録しないと**同じフォルダを開くたびに同じ画像でワーカーが死に続ける**。
   これが 1〜4 だけでは止まらなかった殺し合いの原因。記憶はプールの生存期間だけで、
   起動し直せば忘れる。実ファイルは正規化パス、書庫内は「エントリ名#バイト長」で識別する
   (名前だけだと別書庫の同名エントリを巻き添えにする)。
- `is_ready()` は起動時の本数ではなく**生存スロット数**を見る。全滅しても真を返していた。
- 実機結果: 5 回死んで 5 回とも復帰、スロット喪失 0、上限到達 0。クラッシュする 3 ファイルは
  各 1 回だけ記録され、残る 5 ファイルは何度開き直しても読める。
- README とマニュアルの「自動再起動されます」は**実装が追いついた**形。マニュアルには
  利用者から見える 2 点 (クラッシュした画像は再試行しない / 何度も落ちる場合は打ち切る) を追記。
#### 通知と診断表示 (2026-08-25、実機確認済み)

残っていた 2 点を閉じた。

- **枠が尽きたことを利用者へ伝える**。`SusieWorkerNotice` を one-shot で発行し、
  PDF の `PdfWorkerNotice` と同じ形 (毎 update poll → 閉じるまで残る Window) で出す。
  - **発行条件を 1 か所に閉じた** (`should_notify_workers_gone`)。枠が 1 つ減っただけでは
    出さない (残りが処理を続けられる間、利用者にできることが無い)。終了・再読み込みで
    畳んだ場合も出さない。**最後の枠が諦めたときだけ**。
  - そのために `run_worker_slot` は `SlotExit::{Shutdown, GaveUp}` を**抜けた場所で確定**
    させる。閉じてからフラグを読み直すと、その間に終了要求が来た場合に「自分から畳んだ」と
    誤って読める。
  - 文面が案内するのは**アプリの再起動ではなく「⟳ プラグインを再読み込み」**。`reload()` は
    プールを作り直すので再起動は要らず、案内すると開いている一覧と表示位置を捨てさせる。
    再読み込みボタンを notice 側には置いていない (`reload` は同期でプロセスを 3 つ起こすため、
    新しい UI スレッド同期経路を増やさない)。
- **診断パネルに実績を出す** (`SusieWorkerHealth`)。起動できた枠数 / 生存枠数 / 作り直し回数 /
  打ち切り数 / 落とした対象数 / 最後の失敗理由。0 の項目は出さない (実際に起きた項目が埋もれる)。
  - **副産物の修正**: 枠を使い切った状態が `WorkerSpawnFailed` として出ていた。「起動または
    ハンドシェイクに失敗しました」と書かれるが実際には起動しており、利用者が確認すべきものが
    違う。`WorkersExhausted` を分けた。判定は純関数 `pool_status_from` に出してテストした
    (`a_pool_that_ran_and_then_died_is_not_reported_as_a_failed_start`)。
  - 正常時は何も出さない。復帰の履歴は「読めない画像があった」ときに辿るためのもので、
    常時出す情報ではない。
- **ついでに直した race**: `live_workers` はスロットスレッドを起こした**後**に
  `store(worker_count)` していた。起動直後に落ちたワーカーが先に減算すると 0 を下回って
  戻れない。スレッドを起こす前に `fetch_add` する形にした。
- 変更ファイル: [susie_loader.rs](../src/susie_loader.rs) /
  [ui_susie_diagnostic.rs](../src/ui_susie_diagnostic.rs) /
  [ui_dialogs/susie_worker_notice.rs](../src/ui_dialogs/susie_worker_notice.rs) /
  `app.rs` (4 行) / マニュアル (設定・FAQ) / [async-architecture.md](async-architecture.md) §5.4。
  §5.4 は「Susie も respawn しない」と書いたままだったので、実装に合わせた。

### 2.21 フルスクリーンが Susie プラグインの拡張子を画像として扱わない ✅ 見立ての誤りと判明 (2026-08-25)

- 出典: 2026-08-25。クラッシュプラグインの検証中、サムネイルは Susie 経路を通るのに、
  同じファイルをフルスクリーンで開くと
  `fs load FAIL: The file extension `."miv-crashtest"` was not recognized as an image format`
  で落ちた。ここから「フルスクリーンは Susie に到達していない」と記録した。
- **この見立ては誤り。フルスクリーンは Susie に到達している。**
  - 復号は `decode_canonical_image` の 1 本で、fallback は image → WIC → Susie
    ([canonical_image_loader.rs](../src/canonical_image_loader.rs))。フルスクリーンの
    呼び出し ([app.rs:51540](../src/app.rs:51540)) は `CanonicalDecodeOptions::fullscreen`
    (= `susie_priority: true`) を渡していて、Susie 段を飛ばす分岐は無い。
  - 実プラグイン (`ifpi.spi`) と実ファイル (`C165.PI`) で、製品と同じ入口を通して
    640×400 を復号できることを統合テストで確認した
    (`fullscreen_decode_entry_reaches_susie_for_a_plugin_only_extension`)。
    既存の Susie 統合テストは `susie_loader::decode_file` を直接叩いていて
    **この入口を一度も通っていなかった**ため、疑いを晴らせなかった。
- **本当の問題は診断文だった**。連鎖が全部失敗したとき、返していたのは image クレートの
  エラーだけ。未対応拡張子ではそれが「拡張子が画像として認識されない」になるため、
  **WIC も Susie も試したあとの失敗なのに Susie を試していないように読める**。
  Susie 側のエラーは `.map_err(|_| primary_error)` で捨てられていた。
  - 修正: `CanonicalDecodeError::Decode` を `DecodeChainFailure { primary, susie_error }` に変え、
    `Display` が「WIC も読めず、Susie は〈理由〉で失敗した」まで出すようにした。
    「試したか」の bool は**持たない** (この型が作られるのは連鎖の最後だけで、常に true にしかならない)。
  - 観測された失敗そのもの (クラッシュプラグイン検証中の decode 失敗) は §1.120 の
    ワーカー枯渇と、修正で入れた「一度落とした対象は投げ直さない」記憶で説明が付く。
    どちらも設計どおりで、新しい欠陥ではない。
- 教訓: **エラー文の出所を確かめずに、そこから経路の有無を推定した**。最初の decoder の
  エラーは連鎖について何も語らない。捨てられている失敗理由を先に拾うべきだった
  (§1.121 と同じ教訓)。

### 1.121 RAR を代表サムネにしても親フォルダのタイルに出ない — 専用スレ >>302 ✅ 修正済み (2026-08-24、実機確認済み)

- 出典: 専用スレ >>302。RAR があるフォルダで RAR を `P` で代表サムネに指定し、親フォルダへ移動すると
  そのサムネイルが出ない。利用者の切り分けでは v3.1.0 は正常、v3.1.1 で発生。
- **原因 (計装を入れて 1 回の再現で確定)**。`archive cache unavailable → base_req` と
  `thumb_not_eligible failed` が同じ 1 秒の中に並んで記録された:
  - pin の解決自体は成功していた (cascade → `ArchiveFirstImage` → `dummy-book-002.rar`、pinned_key も正しい)。
    捨てていたのは `converted_archive_cache_paths` に対象 RAR が載っていなかったため。
  - 載らなかったのは、pin-root の収集 ([app.rs:25141](../src/app.rs:25141)) が
    `candidate && !requested && (Pending | Evicted)` を要求していたから。
    **`Loaded` と `Failed` は終端状態で、そこへ落ちたコンテナの pin は二度と解決されない。**
  - **中身がアーカイブだけのフォルダは自動選定が代表画像を選べず (アーカイブを展開しない仕様)、
    必ず `Failed` になる。**そのため出口のないループになっていた:
    ピンが捨てられる → 自動選定が走る → 失敗する → `Failed` → 発見されない → ピンが捨てられる。
  - `Loaded` は反対側から同じ穴に落ちる (画像が 1 枚混ざったフォルダはその画像を描いたまま固定)。
  - 初めて成立させたのは `bc4adbdd` (v3.1.2、prefetch gate と admission 制限の追加)。
    `0f346e20` は下流を整備しただけで、見つからない root は救えない。
- **v3.1.1 の境界は未確定のまま**。v3.1.0..v3.1.1 で `folder_thumb_pins.rs` / `thumb_loader.rs` /
  `rar_loader.rs` / `zip_loader.rs` は blob が同一、`app.rs` の 3 コミットもこの経路に差分が無い
  (ClaudeCode と Codex が独立に確認)。現行の原因は上記で閉じているので、修正には支障しない。
- **修正**: pin-root の発見条件から**サムネイルの状態と進行中要求を外した**。ピンの有無は、
  そのタイルが今どう描かれているかとは無関係な事実である。コストを抑える役目は可視範囲の
  絞り込み (`candidate_indices`) と root ごとの解決結果の記憶
  (`converted_archive_pin_root_states`) が引き継ぐ。解決済みの root は DB も cascade も引き直さない。
- **回帰テスト**: タイルを終端状態にしてから collector を回す 2 本
  (`converted_archive_pin_root_is_found_when_the_folder_tile_already_failed` /
  `..._while_the_folder_tile_is_requested`)。既存の archive-through-folder-pin テストは
  `start_converted_archive_cache_paths_refresh` を直接呼ぶため、描画済みのタイルを一度も見ておらず
  この退行を捕まえられなかった。
- 教訓: **無言の早期 return に理由を型で残してから直した**。事前の推定は
  ClaudeCode / Codex とも `already_requested` か `Loaded` で、**どちらも外れていた** (実際は `Failed`)。
### 1.122 F12 で動画をメイン ⇄ 別ウィンドウへ往復させると重い ✅ 主因は修正済み (2026-08-25、実機確認済み)

- 出典: R2e ②-d の実機 smoke 中の指摘 (2026-08-25)。動作はするが体感でかなり遅い。
- **既存の問題であることは確認済み**: 利用者が **v3.2.0 と v3.0.0 ポータブル**でも同じだと
  確認した。R2e ブランチ由来ではない。**presenter 本体
  ([src/video/native_presenter/](../src/video/native_presenter/)) は R2e で 1 行も触っていない。**

#### ⚠ 最初にここへ書いたログ根拠は誤りだった (2026-08-25 訂正)

当初この項目には `shared output pool exhausted` / `late_drop=88` / `[demux] audio packet
send waited` / `[SLOW FRAME]` を根拠として並べていたが、**保存ログを読み直したところ
別セッションのものだった**。

- 引用元は `mimageviewer.log.bak1` で、**video-strip worktree の dev ビルド**
  (`ffmpeg DLLs expected at C:\home\mimageviewer-video-strip\target\dev-runtime`) のログ。
  **F12 の placement 切替は 1 度も含まれていない** (`switch placement request=` が 0 件)。
- `shared output pool exhausted` は保存ログ全体で **1 回だけ**で、しかも
  **動画の切替 (idx 41 → 42) の瞬間**。直後に旧 decode スレッドが exit しており、
  source 切替時の一過性。F12 とは無関係。
- `[demux] audio packet send waited 20-41ms queue_len_before=64/64` は、同じ区間の
  presenter summary が `fps=59.1 late_drop=0` の**正常再生中に連続して出ている**。
  音声キューが先読みで満杯なだけの通常の backpressure で、症状ではない。
- **F12 往復を含むログは 1 つも残っていない** (該当セッションのログはローテーション済み)。

つまり **§1.122 には現時点で計測根拠が無い**。プールを疑う理由も無くなった。

#### 構造から分かっていること (コードを読んだ結果)

placement 切替は [src/video/mod.rs](../src/video/mod.rs) の `SwitchPlacement` 分岐で処理され、
**placement が変わる場合は presenter を丸ごと作り直す**。`NativeRenderCore::new`
([render_core.rs](../src/video/native_presenter/render_core.rs)) が 1 往復ごとに作るもの:

| 段 | 内容 |
| --- | --- |
| `d3d11_device` | `D3D11CreateDevice` で**新しい D3D11 デバイス**を作る |
| `shader_pipelines` | `VideoGradePipeline::new` + `VideoResamplePipeline::new` (Anime4K 含む全 PS を生成) |
| `swapchain_dcomp` | swapchain + `DCompositionCreateDevice` + visual ツリー + 背景 |
| `egui_overlay` | `NativeEguiOverlay::new` — **wgpu instance / adapter / device (D3D12) を新規作成**し、 |
| | `egui::Context` を作って `configure_fonts_with_settings` で**フォントを構成し直す** |

再生スレッドはこの間フレームを出せないので、**体感遅延 = この区間の実時間**。
「新しいサーフェスを確保してから古いスロットを返す」といった順序の問題ではなく、
**デバイス級のリソースを往復ごとに作り直していること**が疑わしい。

#### 原因 — 実行時の HLSL コンパイル (確定、修正済み)

保存ログに、presenter を作るたびに同じ区間で **毎回 1.8〜2.5 秒**消えている証拠が 15 回分残っていた
(`presenter D3D11 device created` → `Anime4K bytecode loaded` の差)。この間にあるのは
COM cast と `VideoGradePipeline::new` / `VideoResamplePipeline::new` だけ。

単体テストで 7 本の `D3DCompile` を個別に計った結果 (2026-08-25 実測):

| shader | compile |
| --- | --- |
| grade `vs_main` | 2.1 ms |
| grade `ps_main` | 8.0 ms |
| resample `vs_main` | 1.6 ms |
| resample `ps_horizontal` | 6.8 ms |
| resample `ps_vertical` | 6.6 ms |
| resample `ps_nearest` | 2.6 ms |
| **NIS `fs_nis`** | **2138.1 ms** |
| **合計** | **2155.8 ms** |

**NIS の pixel shader 1 本だけで 2.1 秒**。これを `NativeRenderCore::new` の中で毎回
コンパイルしていたので、**F12 片道 ≈ 2.2 秒 / 往復 ≈ 4.5 秒**の固定費用になっていた。
同じコストは **動画を最初に開くときにも**払っている (ウィンドウが出るまでの待ち)。

※ 当初疑ったプール枯渇は無関係だった。

#### 修正

**シェーダを全部ビルド時に FXC で DXBC 化する**。Anime4K は最初からこの経路
(`build.rs` の `find_fxc` / `compile_hlsl_with_fxc`) なので、残りを揃えただけ。

- 手書き HLSL を Rust の文字列リテラルから [src/video/native_presenter/shaders/](../src/video/native_presenter/shaders/)
  の `.hlsl` へ出し、`build.rs` が FXC で `.cso` にする
- NIS はすでに `build.rs` が WGSL → HLSL にしていたので、FXC 工程を足すだけ
- 実行時は `CreatePixelShader` に `include_bytes!` した DXBC を渡すのみ。
  production 経路から `D3DCompile` が消える (残るのは `#[cfg(test)]` の Anime4K GPU 実測テストのみ)
- 「この HLSL はコンパイルできる」テストはビルドが保証するので削除し、
  「実行時に渡す DXBC が実在するか」を見るテストに差し替えた
  (副次効果: lib テストからも NIS の 2.1 秒が消える)

検証は `nis_draw_writes_target_with_default_d3d11_rasterizer` と
`anime4k_chain_writes_target_with_native_fullscreen_vertex_shader` (実 GPU で描画して出力を見る) が兼ねる。

#### 実機確認済み (2026-08-25)。残りの内訳

`shader_pipelines` は **2156ms → 2.8〜5.7ms**。往復 **≈ 5.2 秒 → ≈ 0.95 秒**。
利用者確認でも「速くなりました」。以下は実機ログの実測値:

| 段 | main → 別ウィンドウ | 別ウィンドウ → main |
| --- | --- | --- |
| **total** | **234〜252ms** | **590〜722ms** |
| `host_attach` | 2〜10ms | 45〜48ms |
| `render_core_new` | 166〜176ms | 184〜187ms |
| `prepare` | 17〜26ms | 18〜19ms |
| **`publish`** | 7〜10ms | **289〜424ms** |
| `detach_old` | ≈ 5ms | ≈ 5ms |

`render_core_new` の内訳: `d3d11_device` 27〜31ms / `shader_pipelines` 2.8〜5.7ms /
`swapchain_dcomp` 4〜5ms / **`egui_overlay` 116〜139ms**。
`egui_overlay` の内訳: `wgpu_instance` 0.3〜0.7ms / `adapter` 1.7〜2.2ms /
**`device` 64〜72ms** (wgpu `request_device`) / `renderer` 9.5〜11ms / `egui_ctx` 0.0ms。

⚠ **フォントは無関係だった** (`of_which_fonts=0.0`)。egui の font atlas は初回描画時に
遅延構築されるので、`configure_fonts_with_settings` 自体は払っていない。
事前に疑っていたので記録する — 測らなければここを直しに行っていた。

#### `publish` の非対称は `SetFocus` だった (2026-08-26 実測、確定)

probe を render / pump 両スレッドに入れて F12 を 5 回切替えた。
`publish` の内訳は `hud=0.0 retire=0.0` で、**全部が pump 待ち**。pump 側も
`queue=0.4〜3.7ms` (拾うのは遅くない) / `topology=0.0` / `intents=0.0` で、
残りはすべて `publish_host` → `show_for_placement` の中にあった:

| 呼び出し | main へ戻る | 別ウィンドウへ |
| --- | --- | --- |
| `ShowWindow` | 1.8〜2.1ms | 1.2〜1.7ms |
| `SetWindowPos` | 0.1〜0.2ms | 0.0〜0.1ms |
| **`SetFocus`** | **259.9 / 331.2ms** | **3.8 / 4.4ms** |

**待ち時間は main UI スレッドのフレーム境界の整数倍**で、cross-thread の同期
メッセージが 3〜4 往復していることを示す:

- `18.176s` 開始 → `18.436s` 完了 (260ms)。間の UI フレーム = 18.176 / 18.260 / 18.343 / 18.432 — **3 間隔**
- `10.958s` 開始 → `11.290s` 完了 (332ms)。間の UI フレーム = 11.026 / 11.112 / 11.197 / 11.285 — **4 間隔**

#### なぜ main へ戻る向きだけ遅いのか — placement ではなかった

`SetFocus` は両方向とも同じように呼ばれている (どちらも child window + activate=true)。
違うのは、**その時点で main UI スレッドが遅いかどうか**だけ:

> **動画が detached にある間だけ、main UI スレッドが 1 フレーム 43ms (`pre_grid` 41ms) ・周期 85ms になる。**

- 遅いフレームの区間は `8.456→11.367s` と `14.864→18.432s`。**どちらも動画が
  detached にある区間と一致**し、main へ戻すと止まる (18.432s 以降、ログ終端 25.1s まで 0 件)。
- 同じ理由で `host_attach` も main 方向だけ 40〜45ms (`CreateWindowEx` が親へ
  `WM_PARENTNOTIFY` を送る)。**最初の 1 回だけ 4.2ms** なのは、その時点では
  まだ UI スレッドが速かったから。

つまり **残りは「F12 の問題」ではなく「detached 再生中に main UI スレッドが
1 フレーム 41ms かかる問題」**。これを直せば `SetFocus` と `CreateWindowEx` の待ちが
同時に消え、**detached 再生中のアプリ全体のもたつきも同時に消える**。

⚠ `SetFocus` を遅延・省略・別スレッド化して待ちを隠すのは対策にしない。

#### 41ms の正体は fullscreen viewport 区間 (2026-08-26、`--perf-log` 実測)

`ui/slow_frame_breakdown` を 33 フレーム分読んだ。**全フレームで同じ形**:

| 区間 | 実測 |
| --- | --- |
| `total_ms` | 40.6〜53.9 |
| `pre_grid_ms` | 38.8〜51.6 |
| **`render_fullscreen_viewport_ms`** | **35.9〜48.3** |
| `keep_fullscreen_viewport_ms` | 0.0 |
| `ensure_native_video_front_ms` | 0.0 |
| `background_polls_ms` / `bars_ms` / `grid_ms` / `post_grid_ms` | 0.1〜1.4 |

発生区間は `11.2〜13.6s` と `15.6〜16.3s` — **動画が別ウィンドウにある区間だけ**。

`keep_fullscreen_viewport_ms = 0.0` なので、**本来の描画経路
(`update_active_viewer_context` 内) はこの間一度も描いていない**。
区間 ([app.rs:66084](../src/app.rs:66084)-) の内訳は次の 4 つ:

1. `render_fullscreen_viewport` (今回はウィンドウ内動画モードなので走らないはず)
2. `render_detached_image_windows` (受動的な静止画別窓。今回は無いはず)
3. **`render_active_detached_viewport_backstop`** — keep-alive 経路
   ([ui_fullscreen.rs:13484](../src/ui_fullscreen.rs:13484))。他の経路が描かなかった
   フレームで 1 回描いて OS ウィンドウを破棄させないための backstop
4. `flush_pending_detached_cleanup_font_atlas_resync`

見立ては 3 だが**未確認**。次の測定で 4 分割する。

#### 根本原因 — 16ms repaint pump が「隠れた viewport」前提のまま残っている (2026-08-26、確定)

41ms を最後まで割った結果 (median、144 サンプル):

| 区間 | 値 |
| --- | --- |
| `render_fullscreen_viewport` 全体 | 39.5ms |
| 　`prep_ms` (描画前の処理) | **0.0ms** |
| 　`closure_ms` (自前の描画コード) | **0.3ms** |
| 　**`eframe_show_ms`** (`show_viewport_immediate` の内部) | **38.6ms** |
| `detached_backstop_ms` / `detached_image_windows_ms` / `font_atlas_resync_ms` | 0.0ms |

→ **keep-alive backstop だという見立ては誤り**。描画コードも無関係で、
実体は **子 viewport の render + present そのもの**。

そして **それを毎フレーム走らせているのが 16ms pump**:

```rust
// src/app.rs:65106
if native_owner_hwnd != 0 {
    let pump_interval = std::time::Duration::from_millis(16);
    ...
}
```

perf の `prev_frame_causes` で裏取り済み — detached 再生中の repaint の
**123/135 件**、main ウィンドウ再生中は **97/97 件** が
[app.rs:65123](../src/app.rs:65123) 発。**両モードで同じ pump が回っており、
違うのは 1 回あたりの値段だけ**。

##### なぜこれが入ったのか (推測ではなく当時の記録)

`5d9bc31d` (2026-05-05, `fix(video): keep UI pumping for native fullscreen input`) が
[docs/dcomp-native-presenter-integration-plan.md](dcomp-native-presenter-integration-plan.md)
に残している:

> while a native presenter HWND is active, the UI thread keeps a **lightweight** 16ms
> repaint pump alive ... This prevents native-window shortcut events such as Escape from
> sitting in the UI event queue **when the hidden egui fullscreen viewport has no other
> reason to repaint**.

→ 当時 fullscreen viewport は **hidden** だったので repaint は実際に安かった。
detached viewer の導入でこの viewport が **表示中の 1864x1132 ウィンドウ**になり、
**前提だけが失効した**。pump はそのまま。

##### 因果の連鎖 (全部実測済み)

```
native presenter が生きている間、UI スレッドは 16ms ごとに repaint を要求 (app.rs:65106)
        ↓  detached だと 1 回の repaint が show_viewport_immediate で 38.6ms
UI スレッドが ~12fps (周期 85ms) に落ちる
        ↓  pump スレッドからの cross-thread 同期メッセージは 1 往復 = UI 1 フレーム
SetFocus 260〜354ms (3〜4 往復) / CreateWindowEx 40ms
        ↓
F12 往復 ~950ms のうち約 350ms
```

**アプリ全体のもたつきでもある** — 別ウィンドウで動画を見ている間、
メインウィンドウの一覧操作はすべて 12fps で動いている。

##### 単価側 (38.6ms) は §1.129 で消えた — pump だけが残っている (2026-08-26)

上の連鎖は **2 つの掛け算**だった。そのうち **単価側 = §1.129 は修正済み**で、
12fps の症状は消えている (実機確認済み)。**頻度側 = 16ms pump は未着手**。

修正後の新規セッションを 1 本測った結果 (`perf_events.jsonl`、5166 フレーム):

| | 修正前 | 修正後 |
| --- | --- | --- |
| `slow_frame_breakdown` (30ms 超フレーム) | 33〜140 件 | **0 件** |
| viewport paint > 8ms | 210 件 | **18 件** |
| `vp_max_tex` / `main_max_tex` | 2048 / 8192 | **8192 / 8192** |

→ **F12 往復の遅さも、別ウィンドウ再生中のもたつきも、この時点で実用域に入っている。**

##### それでも pump は回っている (修正後の実測)

`prev_frame_causes` を 3787 フレーム分集計した。最大の要求元は状態で分かれる:

| 状態 | 最大の repaint 要因 | 該当フレーム |
| --- | --- | --- |
| フルスクリーン (動画を開いている) | **`app.rs` の pump** | **1905 / 1930 (98.7%)** |
| 一覧 (動画なし) | egui 自身の `context.rs:529` | 1479 / 1857 (79.6%) |

一方で、実測のフレーム間隔は **フルスクリーン有無にかかわらず median 6.0ms**で、
pump が要求する 16ms より短い。**pump だけ消しても周期は 16ms にならない**。
フルスクリーン **1930 フレーム**の repaint 要求元を分けると
(「フレーム」= その要求元を含むフレーム数、「延べ」= 発行回数):

| フレーム | 延べ | 要求元 | 間隔 |
| --- | --- | --- | --- |
| **1905** (98.7%) | 1905 | `poll_video` 末尾の pump | **16ms 固定** |
| 431 | 431 | 削除 purge の input idle guard | 残り時間 (入力後 1 秒だけ) |
| **343** (17.8%) | **672** | **`maybe_defer_for_main_font_atlas_resync` の `!safety.is_settled()`** | **16ms 固定** |
| 182 | 182 | tail の `reasons` 非空 | 即時 |
| 148 | 148 | `auto_aspect` streak / input idle guard | 残り時間 |
| 174 | 174 | egui 自身 (scroll_area / tooltip / area) | — |
| 13 | 13 | font atlas の full upload 再発行 | 即時 |

→ **pump はフルスクリーンフレームの 98.7%** に乗っている。
スパンで見ても **動画を開いている区間を連続で埋めている** (3.0s / 2.7s / 2.7s / 4.9s / 2.4s)。
**これが主因**であることは変わらない。

⚠ **ただし 16ms 固定のスピンはもう 1 つある**。
[`maybe_defer_for_main_font_atlas_resync`](../src/app.rs) は main font atlas resync が
pending の間、「フレームが settled になるまで 16ms ごとに見に行く」形。
pump と違って**連続ではなく 3 回のバースト** (0.3s / 0.3s / 3.8s、計 ≈ 4.4s) だが、
1 フレームに **2 回**発行している (672 延べ / 343 フレーム)。
形は同じ (= 条件を poll するための固定間隔 repaint) だが、**owner も原因も違う**
(こちらは動画ではなく「フレームが settled になるの」を待っている) ので、
憲法 §2 規則 7 に従って **§1.130 として別項目にした**。同じ変更でまとめて直さない。

⚠ この 343 フレームは §1.129 の修正**後**のログ。atlas rebuild 自体は止まったが、
   **resync 待ちのスピンは別経路として残っている**。

##### 修正の方向 (未着手、Codex と合意済み)

pump の目的は「native window の shortcut event を UI キューに滞留させない」こと。
現状 **video スレッドは `egui::Context` を持っておらず UI スレッドを起こせない**
(`src/video/` 内に UI 向け `request_repaint` は無い) ので、定期 poll で代用している。

**Direction A = イベント駆動 + 既存の意味的期限への one-shot wake**。
Codex の判断は「**完全な形にしたときだけ**構造的修正」で、イベントだけの狭い版は却下。

- **イベント側の入り口は 1 つしかない**: [`NativeOutputEventSender::send`](../src/video/mod.rs)。
  native window から UI へ向かうイベントは全部ここを通るので、
  `wake: Arc<dyn Fn() + Send + Sync>` を 1 つ持たせればよい。
  **同形の既存例**: [test_script.rs:1023](../src/test_script.rs:1023) の `RunnerBridge.wake`
  (`ctx.clone()` を持ち `request_repaint_of(ViewportId::ROOT)` を呼ぶ)。
- **それだけでは足りない**。`poll_video` にはイベントではなく
  **時間 / 位置のしきい値**で発火する処理が乗っている。全部に one-shot wake が要る:

| 乗っている処理 | 期限 | 場所 |
| --- | --- | --- |
| EOF drain の quiet ticks | 3 tick 連続 (≈ 48ms) | `video/mod.rs` `EOF_DRAIN_QUIET_TICKS` |
| stuck seek の強制解除 | 1200ms | `video/mod.rs` `SEEK_STUCK_EOF_TIMEOUT` |
| placement 切替 pending の timeout | `native_video_mode_switch.deadline` | `app.rs` |
| 再生位置の resume 保存 | 5 秒 | `app.rs` `video_resume_last_save` |
| preparing HUD の数値更新 | 50ms | `video/mod.rs` `tick` |
| **CH/BM ループ境界** | **位置の跨ぎ検出 (prev→cur)** | [native_video.rs:4758](../src/app/native_video.rs:4758) |
| normalize scan の完了 poll | ワーカー完了 | `app.rs` `poll_normalize_scan` |
| detached host の再親付け | host HWND 変化 | `app.rs` `poll_detached_video_host_resync` |

  ↑ **CH/BM ループ境界だけはイベント化できない** (位置を sample して跨ぎを検出する形)。
  ただし **次の境界秒数は既知**なので、そこへ one-shot wake を張る方が現行より**精度が上がる**
  (現行は 16ms 分オーバーランする)。

- **Direction B (pump は残して viewport だけ描かない) は不可能**。
  egui 0.33.3 は親パスで提示されなかった child viewport を除去するので、
  `show_viewport_deferred` 化 (= R4 の所有権再構成) が先に要る。

⚠ pump 間隔を伸ばす / detached のときだけ間引く、は症状パッチなので採用しない。
⚠ detached 述語に触れるので [detached-rework-plan.md](detached-rework-plan.md) §2 の手続きが必要
   (合意は取済み。実装後に §11 への記録が要る)。
⚠ トレイ非表示時の Win32 50ms pump は別物なので維持。keep-alive backstop も消さない。
⚠ ワーカーからは `request_repaint()` ではなく **`request_repaint_of(ViewportId::ROOT)`**。

↑ **16ms を注入しているのは 2 か所あり、片方だけ消してもループは残る**:

1. [app.rs](../src/app.rs) `poll_video` 末尾 — `if native_owner_hwnd != 0 { 16ms }` (= 本体)
2. [video/mod.rs](../src/video/mod.rs) `VideoPlayer::tick` の native 経路 —
   再生中 / seek 中は `Some(16ms)`、preparing は `Some(50ms)` を返す

#### Direction A 実装 ✅ (2026-08-26、実機確認済み)

2 か所の固定16ms注入を両方削除し、`VideoPlayer` が所有する `VideoUiWake` を
worker / native event funnel と共有した。worker からの wake はすべて
`request_repaint_of(ViewportId::ROOT)`。新しい detached bool / Option、猶予時間、
debounce、fallback cadence は追加していない。

| 旧 pump に乗っていた処理 | 置換 |
| --- | --- |
| native window の全イベント | 唯一の funnel `NativeOutputEventSender::send` が publish 後に ROOT を即時 wake |
| video info / engine event / decoder EOF・fatal | 各 worker の publish 成功 / clock state 変更時に ROOT を即時 wake |
| EOF drain | 旧「3 UI tick」を実時間 **48ms quiet** に変更。quiet 前は既知の media end と観測済み audio/video 残量から消費完了時刻を one-shot、quiet 開始後は残り48msを one-shot |
| stuck seek | `seek_eof_stuck_since + 1200ms` の残りを one-shot |
| placement switch timeout | `native_video_mode_switch.deadline` の残りを one-shot |
| resume 保存 | 実再生中だけ `video_resume_last_save + 5s` を one-shot。paused / idle は予約しない |
| preparing HUD | Loading / Buffering / 初回 frame 前だけ50ms。Seeking 完了は event、stuck は1200ms deadline |
| CH/BM loop | 現区間の次境界と playback speed から境界時刻を one-shot。wake 後も既存 prev→cur crossing 判定を通す |
| normalize scan | provisional / terminal message の mpsc publish 成功時に ROOT を即時 wake |
| detached host 再親付け | HWND registration / adoption / watcher repair で host generation が変わった事実に対して ROOT を1回 wake |
| seek-bar hover thumbnail | overlay request 自体は既存 event funnel を通る。App が mutex へ書いた直後と thumbnail worker の terminal publish で ROOT を wake |

監査で表外に見つかった依存も同じ所有境界へ移した: user seek の250ms coalesce、
既知 duration の再生末尾、video info 到着、engine/audio readiness、decoder EOF / fatal、
native presenter init fault、thumbnail cache publish。marker thumbnail decode / media navigation /
native open・source・tile pending 等は、もともと各 owner が独自の completion wake または
固有 deadline を予約しており、今回削除した2本の pump には依存しないため変更していない。

`maybe_defer_for_main_font_atlas_resync` の16ms spin、その他の `app.rs` 16ms site、tray 50ms、
detached backstop、native window の focus / placement / epoch 処理は scope 外のまま維持。

##### 実機確認結果 (2026-08-26)

利用者の動作確認: Escape / F12、末尾停止、全体ループ、CH/BM ループ、
末尾超えシーク、2x / 0.5x、音量ノーマライズ、ホバーサムネイル、preparing HUD は全て正常。

`--perf-log` の実測比較 (どちらも §1.129 修正後。フルスクリーンが開いている区間だけを集計):

| | 修正前 (15.9s / 5 区間) | 修正後 (120.3s / 17 区間) |
| --- | --- | --- |
| **動画を開いている間の UI フレーム** | **121.7 / 秒** | **48.0 / 秒 (−61%)** |
| pump / deadline だけが理由のフレーム | 1089 (56.4%) | 924 (15.6%) |
| 30ms 超フレーム | 0 | 23 (うち 17 件は `background_polls`、viewport は 1 件) |
| `vp_max_tex` / `main_max_tex` | 8192 / 8192 | 8192 / 8192 (§1.129 維持) |

**最も直接的な証拠 — フルスクリーンを開いたまま UI が審るようになった**。
0.5 秒を超える無描画区間が 65 件、**うち 62 件がフルスクリーン中**で、
最長 **17.3 秒** (他に 5.0 / 5.0 / 4.4 / 3.7 / 3.2 秒…)。
修正前は pump が 16ms ごとに repaint を要求していたので、**これは構造上不可能だった**。

残る 924 フレーム (再生中の 4〜11/秒) は `poll_video` 末尾の one-shot deadline 発。
一定間隔のスピンではなく、一時停止中は 0 件 (上の 17.3 秒の無描画区間がそれ)。

⚠ タスクマネージャーの CPU% では判別できない (どちらも 0% 表示)。
   判定には perf log のフレーム間隔を見ること。

#### process-lifetime overlay GPU device epoch 共有 ✅ (2026-08-26、実機確認済み)

残り優先 2 として、[src/video/native_presenter/overlay_gpu.rs](../src/video/native_presenter/overlay_gpu.rs)
に process-owned `OverlayGpuService` を追加した。`OnceLock` は device 直置きではなく、共有
`wgpu::Instance` と `Mutex<Vec<Arc<DeviceEpoch>>>` を持つ replaceable cache manager を保持する。
overlay は DComp visual から Surface を先に作り、loss 未確定かつ compatible な epoch を再利用する。
Surface / `egui_wgpu::Renderer` / `egui::Context` は従来どおり窓ごとで、D3D11 present device も
presenter ごとのまま。

⚠ gate の理由を 2026-08-26 に訂正した。当初 Codex は「複数の動画窓が並行し得る」とし、
ClaudeCode は裏を取らずにそのまま伝えたが、**この前提は誤り**。生きているメディア session は常に 1 つで
([app.rs](../src/app.rs) `close_parked_live_media_windows_for_new_media` + 回帰 test `new_media_open_closes_existing_parked_live_window`)、
動画→動画の移動は `SwitchSource` で presenter を作り直さないし、F12 切替は新旧とも
同じ render thread 上で直列。**正しい理由は teardown** — [video/mod.rs](../src/video/mod.rs) の
`Drop for NativeVideoOutput` は render thread の join を待たず別 thread へ逃すので、
cancel 後の旧 thread の submit と新 thread の configure が重なり得る。overlay ごとに
Device を持っていた頃は無害で、**共有したことで初めて生じる**。
実機ログでは旧 thread 停止 (49.516s) → 新 thread 開始 (50.163s) と 0.65 秒空いており、
「起こり得るが未観測」が正確な状態。実際の競合は `gate_wait_ms` で見る。

epoch の RwLock は `Surface::configure` を
write lock、texture update / buffer update / acquire / submit / present の連続区間を read lock にした。
`Context::run` と tessellation は lock 外。device-lost callback は generation 付き一方向 latch だけを
立て、新規 overlay は lost epoch を skip する。既存 overlay は次 draw で terminal error を返す。
Surface Lost / Outdated / Timeout は device loss と扱わない。

次の実機ログ向けに span も分割した。

- `render core created`: 旧 `egui_overlay` total を維持しつつ `overlay_new` /
  `overlay_first_render` を追加。
- `egui overlay created`: service/Instance、Surface、Adapter、Device、reuse / generation、Renderer、
  Context/font configure、`Surface::configure` を分離。
- `egui_overlay_present`: `first_render`、`egui_run_ms`、`tessellate_ms`、font atlas を含む
  `texture_update_ms`、`surface_acquire_ms`、buffer update/encode、submit/present、gate wait / GPU span。
  atlas の CPU 構築だけは `Context::run` 内の通常 UI layout と分離できないため、
  `egui_run_ms` は両者を含む。texture upload と acquire 以降は独立して読める。
- F12 `detach_old`: `NativeEguiOverlay` drop と残りの `NativeRenderCore` drop を分離。

実装直前の最新値は switch total **214.8〜235.0ms** / `render_core_new` **144.1〜161.3ms** /
`egui_overlay` **104.2〜113.0ms**。Instance + Adapter + Device の予測削減 **54.5〜68.2ms** から、
次の switch は teardown 改善を数えず **約147〜181ms** を期待する。`overlay_new` の reuse 時
Adapter / Device は 0ms 近傍、`overlay_first_render` は atlas / per-context Renderer 分だけ残るはず。
`detach_old.egui_overlay` は service の強参照により Device drop を含まなくなるが、削減幅は未測定。

pure logic test は lost epoch 非再利用 + healthy compatible epoch 再利用、old generation callback の
successor 非干渉、configure / submission gate の排他を検証済み。実 Surface を 2 個作るには
Win32 / DComp window が必要なため、2 overlay の同一 `wgpu::Device` identity は unit test 化していない。
実機 F12 往復で `device_reused=true` と同一 generation を確認する。

##### 実機測定結果 (2026-08-26)

同じ解析ツールで 2 セッションを比べた (before = pump 修正後 / device 共有前)。
**27 回の overlay 作成のうち 26 回が `reused=true`、全部 `generation=1`** —
セッション中すっと device 1 つで回った。

| span | before (6 switch) | after (8 switch) | |
| --- | --- | --- | --- |
| **switch total** | 231.8ms | **198.1ms** | −34 |
| `render_core_new` | 159.1 | **105.0** | **−54** |
| 　`egui_overlay` | 104〜113 | **42.0** | |
| 　　`overlay_new` | (未分割) | **5.0** | |
| 　　`overlay_first_render` | (未分割) | **37.2** | ← 今後の最大 |
| 　`d3d11_device` | 21〜25 | 26.1 | 共有していない |
| `detach_old` | 38.3 | **29.2** | −9 (`egui_overlay` 部分は 6.4) |
| `publish` | 13.4 | 30.3 | **+17** |

overlay 内訳は予測どおり: `wgpu_instance` / `surface` / `adapter` / `device` がすべて **0.0ms**。
予測外の収穫として **`egui_wgpu::Renderer::new` も 8.4〜9.7ms → 1.1ms**
(warm device で pipeline 作成が安い)。

⚠ **`publish` が上がって利得の 1/3 を食っている**。方向別に分けると:

| | before | after |
| --- | --- | --- |
| → detached | 7.7 / 11.7 / 13.4 | 12.8 / 13.5 / 10.4 (変化なし) |
| → main | 40.5 / 18.6 / 13.3 | 57.3 / 43.6 / 48.4 / 51.4 / 17.0 |

`publish` は全部 `wait` (= pump の `SetFocus` を main UI thread が処理するのを待つ)。
→ main だけ遅い非対称は **元々あった** (18.6 vs 11.7) が、今回拡大したように見える。

**ただし before n=3 / after n=5 で、before にも 40.5 の外れ値がある。
この差を今回の変更に帰属させるにはサンプルが足りない**。仮説は 2 つあるがどちらも未検証:

- `render_core_new` が 54ms 早くなった分、`SetFocus` が main UI thread の sleep/wake 位相の
  別のところへ落ちるようになった (= 生産側を速くして消費側の遅延が露出した)
- 今回のセッションは F12 往復だけでなく「動画を連続で開き直す」操作を含むので、
  foreground / focus の状態が before と違う

実機確認: F12 往復、連続での動画切替、再生中のリサイズ、別 DPI モニターへの移動、
音声モード、VST GUI 開閉、初回 (`reused=false`) → F12 (`reused=true`) の HUD 表示を利用者が確認済み。

#### 残りの優先順 (2026-08-26 時点)

1. ~~**16ms pump のイベント駆動化 (Direction A)**~~ ✅ **実装・実機確認済み (2026-08-26)**。
   上の「Direction A 実装」節を参照。動画を開いている間の UI フレームが 121.7/秒 → 48.0/秒。
   残りの固定スピンは §1.130。
2. ~~**`egui_overlay` の wgpu device 共有**~~ ✅ **実装・実機確認済み (2026-08-26)**。
   上の節を参照。switch total 231.8 → 198.1ms。
3. ~~`d3d11_device` の共有~~ **見送り**。1 device に immediate context は 1 本しか作れず、
   `Present` まで同じ submission owner に寄せる必要がある。この repo は共有 `GpuVideoDevice` で
   immediate context 競合による hard-stuck を既に経験済みなので、26ms のために戻らない。
4. **残りの最大は `overlay_first_render` (37.2ms)** — overlay ごとに新しい `egui::Context` を
   作るので font atlas を毎回建て直している。次に見るならここか `publish` (`SetFocus`)。

- ⚠ **guard / delay / retry で待ち時間を隠さない。**
- 規模 \\ 優先度: Medium / P3 (主因は 2 段階とも取れた。残りは体感上実用域)。

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

### 1.124 F12 を連打すると実際の押下が破棄される ✅ 修正済み (2026-08-26、実機確認済み)

- 出典: 2026-08-25、§1.122 修正後の実機確認。別ウィンドウから F12 で main へ戻るとき、
  **連打すると最初の何回かが無反応**。**§1.122 とは別の既存不具合**。
- ログにそのまま残っている (`mimageviewer.log`):

  ```
  30.274  PlacementSwitched request=12 applied DetachedWindow generation=9   ← 切替完了
  30.663  ignore stale native F12 toggle: os_down=false presentation=DetachedWindow
  30.906  ignore stale native F12 toggle: os_down=false presentation=DetachedWindow
  31.203  ignore stale native F12 toggle: os_down=false presentation=DetachedWindow
  32.093  switch placement request=13 -> target=MainWindow                   ← 4 回目でやっと反応
  ```

  切替完了の **390ms 後から** 240〜300ms 間隔で 3 回。rebuild 直後に 1 回だけ来る
  stale 再配送ではなく、**人間の連打そのもの**。1 セッションで 7 件、
  うち 6 件が `presentation=DetachedWindow` (= 利用者の報告と一致)。
- 原因: [native_video.rs](../src/app/native_video.rs) の F12 arm が
  `native_video_key_physically_down()` で **「今この瞬間キーが物理的に押されているか」**を
  `GetAsyncKeyState` に聞いている。これは「この event は rebuild の再配送か」の
  **代用判定**でしかない。presenter の wndproc が WM_KEYDOWN を拾ってから App が
  event を引くまでの遅延よりも押下時間が短いと、**本物の押下も同じく false になる**。
  早押しと stale 再配送をこの検査では区別できない。
- §1.122 で切替が速くなった分、連打しやすくなって顔を出しやすくなっている
  (利用者も「前からかもしれない」と報告)。

#### 修正 — 廃れた proxy を削除した (追加ではなく削除)

当初は「`KeyDown` に presenter generation を乗せる」を提案したが、**Codex の指摘で撤回した**。
rebuild 後に**新しい HWND が decode した**再配送は**現世代**で stamp されるので、generation では
早押しと区別できない。generation は stale key の identity ではない。

実際に入れたのは削除である。F12 arm に届く `repeat=false` は
**現 HWND・現 epoch が生成した first-key-down** であり、probe に仕事が残っていない:

| 軸 | 何が落とすか | 入った時期 |
| --- | --- | --- |
| 旧 presenter 由来 | `window_event_belongs_to_generation` (host ごとの epoch) | **2026-07-30** (`0cf4b6a9` pump 分離) |
| hold による auto-repeat | `WM_KEYDOWN` の previous-key-state (lParam bit 30) → 既存の `!key.repeat` | 当初から |
| 切替中の押下 | `switch_native_video_viewer_presentation` の pending guard | 当初から |

物理 probe は **2026-07-01** 。つまり 1 行目の識別子が入る **1 ヶ月前**の代用判定で、
その後撤去されずに残っていた。

**削除したもの**: `native_video_key_physically_down` / その呼び出し /
`NativeVideoKeyBlockReason::StaleDetachedToggle` / formatter の対応 case。
基盤の `key_input::physical_key_down` は他に 3 箱所の本番利用があるので残す。

**回帰テスト**:

- `window_event_is_accepted_only_for_the_current_presenter_generation` — 述語を切り出して表で 3 通り。
  述語を `true` に変異させて落ちることを確認済み (空振りでない)
- `native_video_f12_does_not_toggle_while_a_placement_switch_is_pending` — キー経路を通して pending guard を固定
- 既存の `native_video_f12_toggles_detached_viewer_mode` は **probe 削除後に初めて意味を持つ**。
  旧コードは `#[cfg(test)] { true }` で probe を迷回しており、**この退行を 2 ヶ月近く隠していた**。
  旧コードで落ちるテストは原理的に書けない (テストビルドでは常に true を返していたため)

憲法 §2 規則 5 はこの probe を好例として名指ししていたので、実機ログで反証されたとして
該当部分を撤回した (一般原則は維持)。触った範囲と判断理由は
[detached-rework-plan.md](detached-rework-plan.md) §11 に記録。

- ✅ **実機確認済み (2026-08-26)**: F12 押下 15 回に対し切替 13 回・意図的な無視 2 回で、
  取りこぼしゼロ・二重トグルゼロ。無視された 2 件はどちらも切替飛行中で、
  `ignore F12 toggle while video placement switch is pending` として記録されていた
  (= 意図した pending guard)。`ignore stale native F12 toggle` は 0 件。

### 1.125 ★固定を押すと、開いている fullscreen が古い idx を指したままになる ✅ 修正済み (2026-08-26、実機確認済み)

- ✅ **実機確認済み (2026-08-26)**: 9 回の判断がすべて説明可能だった。
  追従 7 回 / 閉鎖 1 回 / 対象なし 1 回。閉じたのは `target_len=6` かつ再生中ファイルが
  その 6 件に含まれないときだけ。**往復の対称性も確認** (`57 → 2 → 57`、`5 → 0 → 5`) で、
  固定中に別ファイルへ移動していても解除時に正しく戻る。`no snapshot key` の異常は 0 件。
- ✅ **実装 (2026-08-26)**: 案 C を採用。ただし identity は
  `snapshot_owner_entry` の prefix owner 解決ではなく、`selected` と同じ
  `snapshot_key_from_grid_item` の完全一致とした。hit は generation bump 前に live
  `FsCacheEntry` を退避し、items 交換 / bump / invalidate 後に新 idx へ再挿入して、
  fullscreen と音声 / VST / normalize / loop / EOF / marker / native pending を同じ owner の
  index-space へ追従させる。miss は交換前に正規 `close_fullscreen()` を通す。
- 解除側の at-origin 直接復元にも同じ調停を適用し、activation 時ではなく解除直前に実際に
  開いている item を解決する。media-navigation と generation-stamped marker worker は cancel +
  新 index-space で再開、folder-nav / holdover は解放する。別 context の pending は owner stamp が
  一致しない限り変更しない。
- 回帰テスト 12 件を追加し、activate hit / miss、解除前の viewer 内移動、ZipImage と ZipFile の
  prefix 誤一致、130 item の大規模並べ替え、player Box identity の往復維持、複合 media session、
  late worker result、mounted main / detached / ParkedLive / promoted active / sibling parked の native
  owner 行列、folder-nav / 派生 index lifecycle を固定した。detached 憲法 §11 に判断理由を記録。

- 出典: 2026-08-26、別件 (per-context snapshot) の実機確認中に利用者が発見。
  **動画を再生しながら ★固定 を押すと再生が止まり、
  「読込中...」のまま固着する**。画面には `c:\home\youtube\movie\youtube` のような
  **フォルダに見える path** が出ていた。
- **既存の不具合**。`activate_snapshot` の最終変更は **2026-06-04** (`969ba68f`、★固定 UI 導入時)。
  発見時のブランチは [snapshot_ops.rs](../src/app/snapshot_ops.rs) と
  [ui_fullscreen.rs](../src/ui_fullscreen.rs) を 1 行も触っていない。

#### 原因 — `activate_snapshot` が fullscreen を一切見ていない

[snapshot_ops.rs](../src/app/snapshot_ops.rs) の `activate_snapshot` (165〜330 行) には
`fullscreen_idx` / `fs_cache` / `close_fullscreen` への参照が **1 つも無い**。やっているのは:

1. `items` を snapshot の部分集合へ**丸ごと差し替える**
2. `bump_items_generation()` + `invalidate_idx_state_and_queues()`

しかし **fullscreen ビューアは古い `fs_idx` を指したまま**なので:

- `fs_idx` の先が**別のアイテム**になる。snapshot に含まれない / フォルダだった場合、
  fullscreen はそれを読めず **「読込中...」で永久に止まる**
- `fs_cache` は `ItemsGenerationMap<FsCacheEntry>`
  ([viewer_context_registry.rs:845](../src/app/viewer_context_registry.rs:845)) なので、
  generation を進めた時点で **開いていた動画エントリごと無効化**される → 再生が止まる

**設計の穴はここだけ**: snapshot 側は fullscreen を知らないわけではなく、
`snapshot_current_fullscreen_path()` ([snapshot_ops.rs:711](../src/app/snapshot_ops.rs:711)) を持ち、
**snapshot が既に active な状態での fullscreen ナビ**はちゃんと扱っている。
抜けているのは **「fullscreen を開いている最中に snapshot を activate した」場合の整合**だけ。

`deactivate_snapshot` も同じ形で items を戻すので、**解除側にも同型の穴があるはず**。
修正時は両方を列挙すること。

#### 直し方は仕様判断 — 決めてから着手する

| 案 | 挙動 | 考えどころ |
| --- | --- | --- |
| A | ★固定時に fullscreen を閉じる | 単純で確実。ただし見ていたものが消える |
| B | 同じアイテムを新 idx へ追従させる | 一番自然。`snapshot_owner_entry` が既に path → entry 解決を持っている |
| C | snapshot に含まれるなら B、含まれなければ閉じる | B の自然さを保ちつつ範囲外を定義できる |

⚠ **「読込中のまま固着するなら timeout で閉じる」は症状パッチ**。idx が意味を失ったこと自体を扱う。
⚠ 動画の再生停止だけを見て `fs_cache` を generation 無効化から外すのも不可。
   無効化は「idx の意味が変わった」という正しい事実を表している。直すべきは **見ている側の追従**。

#### 再現手順

1. 動画を含むフォルダを開く
2. 動画を fullscreen で再生する
3. (一覧側に戻らずに) **★固定** を押す
4. → 再生が止まり、「読込中...」のまま戻らない

- 規模 \\ 優先度: Small〜Medium / **P2** (通常操作で到達し、固着する)。

### 1.126 ★固定の items 交換で `image_metas` だけ取り残される — 添字空間の交換漏れ ✅ 修正済み (2026-08-26)

- ✅ `SnapshotState` に `saved_image_metas` / `list_view_image_metas` を追加し、visible subset の
  capture、activate の swap、snapshot list 復帰、at-origin deactivate の restore を
  `items` / `thumbnails` と同じ位置対応で行う。`visible_indices = [4, 9]` の非連続 subset と
  元一覧への往復を回帰テストで固定した。

- 出典: 2026-08-26、§1.125 の設計相談中に Codex が発見。
  **§1.125 とは別の症状**なので別項目にした (憲法 §2 規則 7: ついでに直さない)。
- `image_metas` は `items` と **同じ位置の `Vec`** なのに、`activate_snapshot` が
  subset 化していない。`SnapshotState` にも `saved_image_metas` /
  `list_view_image_metas` が無い。
- 具体例: 元の `visible_indices = [4, 9]` なら、新しい `items[0]` は旧 4 の項目だが、
  `image_metas[0]` は **旧 0 のまま**。
- `invalidate_idx_state_and_queues()` はこれを消さない。同関数の責務コメントが
  **「caller 責任」**と明記している ([app.rs:25872](../src/app.rs:25872))。
- 規模 \\ 優先度: Small / P3 (表示されるメタ情報がずれる。固着はしない)。

### 1.127 ★固定の items 交換後、Details 表示の index state が再構築されない ✅ 修正済み (2026-08-26)

- ✅ items swap 後の generation bump / index queue invalidation に続けて、旧
  `details_meta_pending` を cancel + receiver drop、`details_tag_prewarm_indices` を clear し、
  color filter の有無に依存せず最終 `visible_indices` から `details_order` を再構築する。
  activate / at-origin deactivate の両方向を、color filter OFF と late result rejection を含む
  回帰テストで固定した。

- 出典: §1.126 と同じ、Codex の指摘 (2026-08-26)。
- `details_order` / `details_tag_prewarm_indices` / `details_meta_pending` は、
  items 交換後に**無条件では再構築・cancel されない**。
- color filter が有効な場合だけ後段の `rebuild_visible_indices()` が偶然更新するので、
  **通常の Details 表示では旧 order が残り得る**。
- 規模 \\ 優先度: Small / P3。

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

### 1.129 別ウィンドウの viewport を 1 枚描くのに CPU 9500 万サイクル使っている ✅ 修正済み (2026-08-26、実機確認済み)

- 出典: 2026-08-26、§1.122 の分解。§1.122 の**頻度**側 (16ms pump) とは別の
  **単価**側の問題なので別項目にした (憲法 §2 規則 7)。
- 動画を別ウィンドウへ出している間、main UI スレッドの
  `show_viewport_immediate` が **1 回 38.6ms** かかる。描いているのは実質黒地 1 枚
  (native presenter が子 HWND に映像を出している)。

#### 実測 (2026-08-26、223 サンプルの median)

| | 値 |
| --- | --- |
| `body_ms` (`show_viewport_immediate` 全体) | 38.9ms |
| `closure_ms` (自前の描画コード) | 0.3ms |
| `prep_ms` (描画前の処理) | 0.0ms |
| eframe 部分のみ (`body - closure`) | **38.58ms / 95,224,648 cycles** |

**待ちではなく CPU 実行**であることを `QueryThreadCycleTime` で確定させた。
校正値として、確実に CPU を焼いている closure (0.3ms) のサイクルレートを同時に取った:

| | cycles/ms |
| --- | --- |
| `closure_cycles_per_ms` (校正値) | 2,487,568 |
| `body_cycles_per_ms` | 2,478,017 |
| eframe 部分のみ | 2,478,149 |

差 0.4%。**vsync / GPU / lock 待ちではない**。
(測定は `dev-runtime` = release 継承の opt-level 2。release でも同じ桁)

#### 影響

§1.122 の pump を直しても**この単価は残る**。別ウィンドウでマウスを動かす・
パネルを出すなどで repaint が起きるたび 38.6ms かかるのなら、それ自体が体感不具合。

#### 原因 — viewport ごとに `max_texture_side` が違う (確定)

`egui_ctx.run` の中であることを vendored eframe に probe を入れて確定させた:

```
total=40.2ms input=0.0 run=39.4 set_window=0.0 tessellate=0.0 paint=0.7
             prims=1 delta_set=1 delta_bytes=262144 max_tex=None
```

`set_window` / `tessellate` / `paint` は全部 1ms 未満。**39.4ms は `run`**で、
自前の描画は 0.3ms なので egui 自身。そして**毎フレーム atlas 全体の delta**が出ている。

| viewport | `max_texture_side` | atlas | delta |
| --- | --- | --- | --- |
| detached (遅い側) | **None** → egui 既定 2048 | 2048x32 | 262,144 = 2048x32x4 |
| root / 他 | **Some(8192)** | 8192x32 | 1,048,576 = 8192x32x4 |

perf 側でも遅いフレーム **73 件すべて** `vp_max_tex=2048` / `main_max_tex=8192`。

egui の font atlas は **`Context` に 1 つ**しか無く、`Fonts::begin_pass` は
`max_texture_side` が変わると atlas を作り直す。`FontsImpl::new` は
**登録済み全フォントを ab_glyph で再パース**するので、
2 つの viewport が 2048 と 8192 を交互に報告すると**毎パス rebuild** になる。
これがサイズにも内容にも依存しない一定 38.6ms の正体。

#### なぜ食い違うのか — eframe wgpu backend の非対称

`RawInput::max_texture_side` は `Option<usize>` で、**None なら egui が既定 2048** を使う。
wgpu backend はこの値を **`State::new` で 1 度スナップショットするだけ**で以後同期しないので、
render state が無い時点で作られた viewport は `None` のまま固定される。

**glow backend は毎フレーム全 viewport へ配っている**
([glow_integration.rs:221](../vendor/eframe/src/native/glow_integration.rs:221))。wgpu 側だけこれが無い。
mIV 由来ではなく上流の非対称。

#### 修正

`vendor/eframe` の wgpu backend で、**root と immediate viewport の両方**が
`take_egui_input` の直前に painter の値へ同期するようにした。glow と同じ契約に揃えただけで、
値をスナップショットせず painter に追従させる構造の修正。

⚠ 上流 eframe への還元候補。vendored crate を更新するときはこの差分を持ち越すこと。

#### 旧仮説 (いずれも実測で否定済み、同じ道を辿らないために残す)

- **keep-alive backstop が描いている** → `detached_backstop_ms = 0.0`。外れ
- **font atlas の ppp スラッシング** → `vp_ppp == main_ppp == 1.5`、`fill = 0.009`。外れ
  (rebuild していたのは当たりだが、引き金は ppp ではなく `max_texture_side` だった)
- **per-pixel 処理** → ウィンドウを 1242x720 → 348x260 (面積 1/10) にしても 38.3〜39.5ms。外れ
- **vsync / GPU / lock 待ち** → cycles が校正値と 0.4% 差。外れ

#### 仮説 (否定済み、参考)

egui の font atlas は **`Context` に 1 つ**しか無いが、main と子 viewport が
異なる `pixels_per_point` を要求しうる。epaint 0.33 の `Fonts::begin_pass` は
`fill_ratio() > 0.8` で atlas を**作り直す**ので、両方の ppp 分の glyph が交互に
積むと毎フレーム rebuild + 全 atlas の delta アップロードになり得る。
mIV は fallback font を 6 系統登録しているので atlas は大きい。

検証中: `vp_ppp` / `main_ppp` / `vp_atlas_w,h` / `vp_atlas_fill` / `vp_galleys` /
`main_atlas_fill` / `main_galleys` と viewport サイズを perf event へ追加した。
サイズに比例するなら per-pixel 処理、atlas fill が鋸歯状なら rebuild のスラッシング。

- 規模 \ 優先度: 未知 / P2 (原因が分かるまでは見積もらない)。

### 1.131 複数ウィンドウで動画再生中、他の窓をアクティブにすると 13ms で動画へ奪い返される ✅ 修正済み

- 出典: 2026-08-27、利用者報告。**複数ウィンドウモードで動画を再生中に PDF を開こうとすると
  「ウィンドウがちらつくだけで開けない」。動画の前に開いていた PDF ウィンドウも、
  アクティブにしようとすると動画のウィンドウが手前に来る。**動画が無ければ開ける。
- 状態: **2026-08-27 修正・実機確認済み**。複数ウィンドウで動画再生中に PDF が開けること、
  先に開いていた窓が前面に留まること、parked 中の HUD クリックが従来どおり 1 回目で前面に
  来ること、シークストリップを開いた状態でも同じことを利用者が確認。

#### 原因 — 実ログで確定 (推測ではない)

`MIV_DETACHED_WINDOW_DEBUG=1` で再現。同じ 4 行が 5 回繰り返された:

```
31.172  passive_activate_begin id=4                                  ← 利用者が PDF 窓をクリック
31.185  parked-live hud command converted to activation: window_id=1
        event=RequestSeekStripWindow { center: Thumbnails { .. }, .. }   ← 動画側が活性化を要求
31.185  session_closing window_id=4 reason=pause_active_context      ← PDF が 13ms で降ろされる
31.185  session_begin window_id=1 source=Video                       ← 動画が奪い返す
```

**`RequestSeekStripWindow` は利用者のクリックではない。**
[render_core.rs](../src/video/native_presenter/render_core.rs) が**描画中に**、strip の
layout key (overlay サイズ / DPI / 可視セル数 / center) が変わるたびに push する
layout / resource 要求。それが「HUD がクリックされた」と分類されていた。

[native_video.rs](../src/app/native_video.rs) の
`native_video_output_event_is_parked_live_hud_click_activation` (旧):

```rust
Ev::Window(_) | Ev::PlacementSwitched { .. } | Ev::PlacementSwitchFailed { .. }
| Ev::RequestSeekThumbnail { .. } | Ev::ClearSeekThumbnail
| Ev::TileColumnsDelta { .. } => false,          // 維持イベント (活性化しない)
Ev::NavigateItem { via_wheel, .. } => !*via_wheel,
_ => true,                                        // ⚠ それ以外は全部「HUD クリック」
```

**catch-all `_ => true` が構造的な欠陥。**`NativeVideoOutputEvent` は 77 variant あり、
後から足したイベントは、何もしなくても「利用者がクリックした → 前面へ奪う」に既定で入る。

- `RequestSeekStripWindow` の追加: **2026-08-23** (`2edc070c` シークストリップ)
- この分類器の最終更新: **2026-08-21** (`c71d8c08` detached-rework R2)

シークストリップは 10 個近くイベントを足したが、維持側に登録されたのは 3 つだけ。
利用者の見立て (「ストリップのマージで壊れた可能性」) がそのまま当たっていた。

#### 修正 (2026-08-27)

`_ => true` を撤去し、**77 variant を網羅 match で分類**した。新しいイベントを足すと
コンパイラが分類を要求する。「未知のイベント = 利用者のクリック = 前面を奪う」という
既定をやめるのが本体で、`RequestSeekStripWindow` を維持側へ足すだけの症状パッチにはしない。

Codex は方針に同意したうえで、一次分類案に **5 件の反例**を出した。全件を emit 元で裏取りし、
うち 2 件は**同じバグの別インスタンス**だった:

| 指摘 | 裏取り結果 | 対応 |
| --- | --- | --- |
| `CloseSeekStrip` は cause 依存 | ✓ **`HudHidden` は描画の else 枝から出る** (2 例目) | 既存の `SeekStripCloseCause::is_user_dismissal()` を使う |
| `SetVst3PanelPos` は自動 clamp でも出る | ✓ `saved_pos_was_clamped` で発火 (3 例目)。利用者のドラッグは左ボタン経路が先に活性化するので失われない | false |
| `TouchChromeLearned` は利用者入力 | ✓ `ToggleChrome` / `PageSide` のタップにのみ応答 | true |
| `SetVst3PanelVisible` に producer が無い | ✓ presenter に push 箇所ゼロ | default-deny で false |
| `TileColumnsDelta` は入力源が 2 つ | ✓ ただし**両方とも利用者入力**。今回の欠陥ではない | 現状の false を維持 (下記) |

自分で追加確認した 1 件: `VideoScaleSettingsCommitted` は presenter が適用完了後に送る通知
なので false。

回帰テスト: `parked_live_renderer_emitted_events_do_not_request_activation`
([src/app/tests.rs](../src/app/tests.rs))。`RequestSeekStripWindow` を活性化側へ戻す変異と
`CloseSeekStrip` の cause を無視する変異の両方で落ちることを確認済み。

凍結ルール下の合意は [detached-rework-plan.md](detached-rework-plan.md) §11 に記録した。

#### 残した 2 件 (憲法 §2 規則 7 によりスコープ外)

1. **`TileColumnsDelta` の provenance 分離** — Ctrl+ホイールと HUD の列数 ± ボタンが同じ
   variant を共有する。現状はどちらも parked 中に活性化しない (R2 以降の挙動)。
   分けるには payload に typed origin が要り、今回の欠陥とは別。
2. **活性化要求の寿命・順序** — [ui_fullscreen.rs](../src/ui_fullscreen.rs) の
   `take_parked_live_activation_requests_after_passive_render` の消費側は
   `detached_window_state_is_parked_live(id)` だけを見る。これは「この窓は parked-live か」
   であって「**この活性化要求はまだ望まれているか**」ではない。要求は生成理由も順序も
   持たない `Vec<u64>` で、同一 batch は時系列でなく ID 順に並ぶ。
   今回の症状の原因ではないが、同じ「1 つの述語で 2 つの問いに答えている」形。
   扱うなら既存の `DetachedActivationIntent` を含む reducer へ統合し、
   **入力順序か request identity** で最新を定義する (フレーム数や数 ms の猶予で棄却しない)。

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

### 1.132 v3.3.0 出荷前レビュー (Codex) の指摘と対応 ✅ 全 7 ラウンド完了 (2026-08-27〜29)

- 出典: 2026-08-27、Codex による `v3.2.0..HEAD` の出荷前レビュー。
  実装は Codex Sol、検収は ClaudeCode (3 本の brief に分けて実施)。

#### P1 — 製品版に `Drop for ViewerContextBundle` が存在しなかった ✅

`#[cfg(all(test, windows))]` と `#[cfg(windows)]` が重なっており、複数の
`#[cfg]` は AND されるので実効条件が `test && windows` になっていた。
`2918e639` (read view 導入) で事故的に追加されたもの。

worker pool (5〜14 スレッド) の condvar を起こす唯一の経路なので、
**窓の開閉ごとに 1 プールずつ永久残留**していた。実機で 400 スレッドまで
増えるのを確認し、修正後は繰り返しても増えないことを確認済み。

再発防止は **製品ビルドで落ちる形** にした (テストでは検出できない穴だったため):

- `#[cfg(all(windows, not(test)))]` の const item が `ViewerContextBundle: Drop` を要求 → E0277
- `tools/viewer_context_audit` に正規形を登録

両方ともバグを再導入して実際に落ちることを確認済み。

⚠ **検証中に踏んだ罠**: worktree が 2 つあると `target\dev-runtime\mimageviewer-core.exe` も
2 つできる。相対パスで起動手順を渡したため、利用者が**修正の無い別 worktree の
ビルド**を測っていた。どのビルドが書いたログかは `ffmpeg DLLs expected at ...` 行で判別できる。
今後は検証バイナリを**絶対パス**で渡す。

#### P2-2 / P2-3 — App-global slot の context 所有化 ✅ (ただし到達性に訂正あり)

`rating_filter_suppressed_at` / `favsearch_subfolder_restore` / `global_search_subfolder_restore`
を `ViewerContextBundle` へ移し、既存の `swap_field!` 契約に乗せた。

⚠ **レビューの前提は実機と違った**。「A 窓の一時解除を B 窓の snapshot 解除が消す」
とされていたが、**今日の mIV では踏めない**:

- ★固定 (snapshot) は **メインウィンドウのツールバー専用**で、メインは 1 つしかない
- suppression を立てる経路は、直前の
  `if self.open_grid_container_in_detached_book_context(..) { return None; }` で
  **別ウィンドウ側が早期 return する**

つまりこの修正は「今起きている不具合の修正」ではなく、
**fork が増えたときに踏む穴を先に塞いだもの**。単体テストが検証手段で、
**手動確認の手順は作れない** (次に同じ指摘が出たときに再調査しないこと)。

ただし P2-3 のうち **「canonical な `return_to` があるときでも fallback slot を
消費していた」半分は単一窓でも成立する**ので、こちらは実害の修正。

#### P2-2 re-review follow-up — detached descriptor open が main filter suppression を変更 ✅

`open_grid_item_in_detached_book_context_with_auto_fullscreen` の Descriptor arm は、
fresh detached bundle を作る前の mounted main projection 上で rating / facet suppression を
立てていた。★付き ZIP / PDF を別窓へ開くと main の
`rating_filter_suppressed_at`、facet filter、suppression stack が変わる実到達バグだった。

2026-08-27 修正。Descriptor helper から抑制を外し、detached start 不成立後に main navigation を
採用する通常到達3入口 (Enter / double-click / gamepad accept) と、明示 container mode の
static site 1箇所の計4箇所にある既存抑制だけを正本にした。FolderCandidate も分類後の main
fallback だけが抑制し、ConvertibleArchiveCandidate の detached completion は抑制しない。実 producer で★付き
ZIP/PDFを開くテストを追加し、main state 不変と detached の全ページ表示を固定した。

#### follow-up — reading-history owner と convertible 非遷移 lifecycle ✅

2026-08-27 修正。Enter、double-click、gamepad accept、明示 container mode の4 static site は、
detached arbitration が main fallback を採用した後だけ `reading_history_return_from` を更新する。
このうち通常到達するのは前3入口で、明示 mode は multi-window 時に副作用なしで早期 return する。

残っていた反例は `GridItem::ConvertibleArchive` だった。main fallback 採用後でも、Ignore 判定や
非同期変換 request より前に reading-history 予約、rating / facet suppression stack、smart-folder
position を更新していたため、Ignore と dialog cancel で画面が遷移しないのに main state だけが
変わっていた。`MainGridArchiveTransitionIntent` を既存の open / convert request owner に保持し、
cache / RAR direct / 変換完了の実 load 成功時だけ4状態を commit する。Ignore、request start failure、
dialog cancel は intent を捨てるだけで main items と4状態を変えない。これは d7645901 の regression
ではない。同 commit より前から `note_reading_history_open` は入口先頭にあり、d7645901 は呼出しを
convertible arm 内へ移したものの Ignore 判定より前という誤った側を維持し、両 suppression も元から
同じ側にあった。

#### final P2 — ★固定中の変換 cache alias と拒否前 mutation ✅

2026-08-27 修正。★固定 snapshot の member は source `book.7z` だが、cache hit と非同期変換完了は
実体の cache ZIP を `load_folder_with_scan_claimed` へ渡すため、実 path だけを見ていた snapshot guard が
正当な open を範囲外として拒否していた。`OpenRequestOwner::MainGridArchive` の
`MainGridArchiveTransitionIntent::source_path` を snapshot scope identity とし、既存の
`snapshot_owner_entry` (完全一致 + separator-aware prefix 一致) へ渡す。cache path との OR にはせず、
source 自体が snapshot member / member container 配下でない要求は従来どおり拒否する。

同 guard は smart-folder session の認可 consume / surface clear、folder-pane cancel、synthetic restore、
smart scope reconcile、reading-history reservation clear、bookmark return reconcile より後ろにあった。
これらは guard が読む snapshot / navigation scope / typed owner identity の成立には不要なので、guard を
`load_folder_with_scan_claimed` の先頭 precondition へ移した。拒否時に残す effect は stale な
`pending_auto_fs_open` の破棄と feedback toast だけで、main の表示・復元 ownership は変更しない。

実 gamepad producer からの同期 cache hit、同 producerが作った request の `ConvertDone` completion、
範囲外の common load 拒否を回帰テストにした。前2本は source alias 解決を cache ZIP 判定へ戻す mutation、
拒否テストは smart session consume / clear または reading-history reconcile を guard より上へ戻す mutation で
失敗する。拒否テストは smart session の `Option` だけでなく、拒否後も one-shot source 認可を実
`preserve_smart_folder_session_for_load` 境界で consume できること、main items と snapshot count が同一なことを
確認する。Folder / ZIP / PDF の3通常入口 + 明示 mode static site が `AddressBarNav::Direct` の common guard
より前に caller-side effect を commit する既知の同型は、本 defect の funnel 内修正には含めていない。

#### final P2 re-review — scope preflight と RAR generation swap ✅

2026-08-27 修正。`claim_open_request_owner` は snapshot scope guard より前で、拒否される
navigation でも進行中の archive conversion、未解決 startup open、競合 bookmark open を
終了していた。navigation scope、snapshot active、`snapshot_internal_nav`、typed owner の
source identity だけを読む純粋な `snapshot_scope_allows_open` へ判定を集約し、container
dispatcher と pre-scan 対応 folder load の2入口では claim より前に実行する。common load の
既存 guard も同じ述語を使う。cache path と source path は OR にせず、
`MainGridArchiveTransitionIntent::source_path` が実 path を置き換える。ユーザー操作の拒否 effect は
共通 helper 1箇所に置き、stale `pending_auto_fs_open` clear と既存 toast を1回だけ行う。

dialog を隠して走る RAR direct-read 完了も、実 load / auto-fullscreen 予約 / smart-folder 認可より
前に現在 snapshot で同じ preflight を再実行する。generation N の pin 中に始めた RAR が、解除と
filter 変更を経た generation N+1 に含まれなければ旧完了を捨てる。これは現在のユーザー操作では
ないため toast は出さず、通常ログだけを残す。`ArchiveConvertState` の Drop による cancel token、
deferred fullscreen の `release_fs_nav_lock`、bookmark request cleanup は拒否 branch でも完了する。

PDF の guard 非経由 route は password dialog の OK retry と detached descriptor だけだった。
前者は `show_pdf_password_dialog` が common modal input blocker に入り、背面で snapshot の解除・
filter 変更・再固定を行えない。後者は `DetachedPhysical` context である。現行 UI から同じ
generation swap は到達不能なので、規則7に従い変更していない。

回帰テストは両 claim 入口で archive request / cancel token を保持し、未解決 startup owner と
競合 bookmark owner も保持すること、RAR generation swap が load を拒否しつつ state Drop と nav
lock cleanup を終えること、current generation 内の RAR は通常どおり開くことを固定した。
mutation は `move_scope_preflight_below_claim`、
`omit_rar_direct_completion_scope_preflight`、
`reject_all_rar_direct_completions_in_snapshot`。

#### §1.126 / §1.127 — 添字空間の追従 ✅

`SnapshotState` に `saved_image_metas` / `list_view_image_metas` を追加し、
`items` / `thumbnails` と同じ `mem::replace` の並びへ入れた。Details は専用の
invalidate / rebuild 境界を作り、**color filter OFF でも**最終 `visible_indices` から order を
再構築する (ON だと後段の `rebuild_visible_indices` が偶然直していた)。実機確認済み。

#### P2-4 — `cancel_all_context_work` へ集約 ✅

既存 8 件 + 新規 4 件 (`fs_pending` / `details_meta_pending` / `comic_bake_pending` /
`erase_inpaint_pending`) を 1 つの関数へ。`Drop` がそれだけを呼ぶので、
`close_fullscreen` を経ない bulk retire 経路も**構造的に**カバーされる。

**重大度は P1 と別物** — こちらは一発型 worker で自然終了し、**蓄積しない**。
コストは孤児化したジョブ 1 件分の CPU / GPU / AI 時間 (最悪で MI-GAN 完走)。
この区別を曖昧にすると watchdog 等の過剰な機構を呼び込むので明記する。

✅ `facet_name_cache_pending` は有限 worker を持つが cancel ハンドルが無く、
**あえて新設しない**判断 (得られるのは有限ジョブ 1 本の末尾だけで、
代わりに worker 側へ新しい監視点が増える)。
`pdf_enumerate_pending` は `PdfEnumerateHandle::Drop` が既に自己処理していた。

#### 検収で使った手法

全 12 件の mutation (各 cancel / 各 guard を 1 つずつ壊し、名指しされたテストが
落ちるか確認) を実施。すべて期待どおり落ちた。

⚠ 検収側の事故 2 件 (記録): ① フィールド名を変える mutation は参照側が
全壊れてコンパイル不能になり不定になる (→ `swap_field!` の 1 行だけ消す形へ)。
② 置換先を空文字列にすると逆変換が全箇所にマッチして**復元できず、
Codex の修正の半分が消えたままになった** (→ マーカーを残す形へ、
かつ `git diff --numstat` を Codex 申告値と突き合わせる)。

- 規模 \\ 優先度: — (完了)。

#### 第 2 ラウンド (2026-08-27、HEAD `52620c96`) — P1 ×2 / P2 ×2 + 小 4 件 ✅ 実機確認済み

レーン A / B / C を master へ統合した後の全体レビュー。**4 件とも自分で emit 元まで
裏取りし、全件に変異テスト**(ガードを外すと落ちること)を通した。

| | 指摘 | 直し方 | commit |
| --- | --- | --- | --- |
| P1 | 圧縮マスクが最大 20 GB のピークを作れる | 上限を 4 GiB → **128 Mi エントリ** (134 MP、編集できる最大の画像より上)。`Vec::with_capacity(宣言値)` をやめ、**実際に届いたバイトへ伸ばす** | `4ff168d2` |
| P1 | duration 由来で波形が無制限確保 | **32 MiB (約 5.5 日) を超えたら coarse 列を作らない**。⚠ 初回の修正は routing を直しておらず、30 分より広い段が全滅した (第 3 ラウンド P1-2 で修正) | `4ff168d2` → `10e9bf82` |
| P2 | 8192px 超のテクスチャでパニック | 上限を**無条件**に適用 (下記の設計反転あり) | `2295f2c9` |
| P2 | 最大化状態の解釈が二系統 | `read_window_state` が **1 つの解決済み状態から両方を答える** | `94671438` |
| 小 | 既定 180 秒が ladder に無く戻せない | ladder に追加 + 「既定は必ず段である」を規則としてテスト化 | `4ab98d3f` |
| 小 | 360 設計書の冒頭が「360 動画は未実装」 | 訂正 (同じ文書の §13.8 は実装済みと書いていた) | `4ab98d3f` |
| 小 | テスト 2 件が `#[test]` を失って未登録 | 復帰。**2 件とも通る** = テストは正しく、番人がいなかっただけ | `38e5e230` |
| 小 | `git diff --check` が空白 2 件で失敗 | 修正 | `38e5e230` |

**設計判断の反転が 1 件**: `WaveSpanRequest::pixel_width` は「可視幅が上限を超えても丸めない、
**テクスチャ生成側の責務として残す**」と明示し、テストもそれを守っていた。しかし生成側は
分割も縮小もせず `load_texture` を呼ぶだけで、**責務は実装されないまま**だった。要求した幅が
そのまま焼かれる以上、上限を知っているのは要求側しかない。テストは削除せず理由付きで
書き換えた。鮮鋭さを取り戻す分割案は §1.140。

**出荷前に入れなかった 1 件**: Susie のクラッシュ対象 ID (§1.141)。失敗は限定的で
自己修復可能、修正は decode のホットパスに触れ、**実際のクラッシュを再現する手段が無い**。

**実機確認済み (2026-08-27)**: 最大化終了→再起動→最大化解除で通常サイズが戻る / 通常サイズの
位置とサイズが戻る / 波形ストリップと 3 分の段 / 補正レイヤーのマスク読み書き。

**残る既知項目**: F12 の白いホストとタスクバー明滅 (§1.137〜§1.139、マージ・実機確認済み) /
動画レンダースレッドの復旧不在 (§1.135) / スレッド終了時の native heap 例外 (§1.123) /
detached placement テストの実モニター依存 (§1.136)。

#### 第 3 ラウンド (2026-08-27、HEAD `dd39d1f6`) — 第 2 ラウンドの修正が作った退行 2 件 ✅

**2 件とも、私が入れた修正の退行だった。**新しい状態と新しい上限を入れて、
**それを読む側を数えなかった**のが共通の原因 (memory: enumerate-same-shaped-paths)。

| | 指摘 | 直し方 | commit |
| --- | --- | --- | --- |
| P1-1 | 上限が 1 マスク単位で、文書全体の累積を縛らない | `DocumentBudget` (既定 1 GiB) を導入。**ヘッダの count で、展開前に課金**。sidecar / local_adjust 行 / bundle 行ごとに scope を開く。旧 number-array 形式も同じ予算で課金 | `056d456f` |
| P1-2 | `OverBudget` が window 経路へ落ちず、30 分より広い段が全滅 | `CoarseAvailability` に **`Never`** を足した。`CoarseProgressive` は「列が埋まるのを待つ」経路なので、来ない列を待つと `process_coarse_request` が毎回 `Failed` を返す | `10e9bf82` |
| P2-1 | 8192px に丸めた raster が「表示済み」と認識されない | 要求側と判定側が `effective_visible_pixel_width` を共有。再生中は中心が 100ms ごとに動くので、認識されないと**毎フレーム要求し直して自分の復号をキャンセル**していた | `10e9bf82` |
| P3 | 重複 `#[test]` 2 件 | doc の上にあった余分な属性を除去 (前回は不足分を足しただけだった) | `10e9bf82` |
| P3 | 360 設計書 Phase 3 のチェックが未了 | 実装済みへ更新 | — |

**教訓**: P1-2 は「列を作らない」と決めたときに、その状態を読む `decide_wave_render_route` /
`process_coarse_request` を辿らなかった。P2-1 は「幅に上限を付けた」ときに、その幅と比較する
`displayed` 判定を辿らなかった。**どちらも 1 つの述語を 2 か所が別々に答えていた形**で、
今回はどちらも「1 つの答えを共有する」形に直した。

#### 第 4・5 ラウンド (2026-08-28〜29) — 前ラウンドの修正が作った退行と、機能の取り戻し ✅

| | 指摘 | 直し方 | commit |
| --- | --- | --- | --- |
| P1 | 予算が 1 マスク単位で、文書全体の累積を縛らない | `DocumentBudget` (既定 1 GiB) をヘッダの count で**展開前に**課金。旧 number-array 形式も同じ予算 | `056d456f` |
| P1 | `run_local_materialize` が予算を迂回 (**content restore も**。指摘は 1 か所だったが実際は 2 か所) | `parse_layers_json` を唯一の入口にし、**src 全体を歩く監査テスト**で入口の増加を止める | `2dc3deda` |
| P1 | 列を作らない判断が 3 時間の一括 PCM (3.86 GiB×2) を呼ぶ | `WINDOW_DECODE_MAX_SPAN_SECS` は好みでなくメモリ境界。route 列挙でなく**確保量**を縛るテスト | `2dc3deda` |
| P1 | 長時間波形の機能が「安全なエラー」に置き換わった | **列を duration でなく bin 数で縛る**。上限以下は不変、超えたら粗くなるだけ。上限 64 MiB = 11 日 | `abae17fe` |
| P2 | 一時展開バッファがピークに入らない | 保持分は差し引き、**一時分は差し引かずに検査** | `2dc3deda` |
| P2 | 8192px raster が「表示済み」と認識されない | 要求側と判定側が `effective_visible_pixel_width` を共有 | `10e9bf82` |
| P2 | 非 Windows 確認スクリプトが後片付けで exit 1 | 入れ子 `.git` を名前で除外 + 削除前に属性を正規化 | `a7f2505c` |
| P3 | 重複 `#[test]` / 360 文書 / 記録の誤り | 除去・更新・訂正 | `10e9bf82` `30432543` |

**私が作った退行が 4 件あった** (P1-2 / P2-1 / 3 時間 PCM / 長時間機能の喪失)。共通するのは
**新しい状態や上限を入れて、それを読む側を数えなかった**こと。`Never` を足したら routing と
`process_coarse_request` を、幅に上限を付けたら `displayed` 判定を辿るべきだった。

**bin 幅を可変にするときにも同じ罠があった**: chunk は 600 bin なので、bin を粗くすると
1 chunk の復号範囲が伸びて PCM 問題が戻る。bin 幅の上限を `WINDOW_DECODE_MAX_SPAN_SECS / 600`
= 3 秒に置いて塞いだ。

**テスト名が assert と逆になっていた**件も指摘された
(`..._still_routes_its_widest_spans_to_a_window_decode` が「1 時間以上は通さない」を assert)。
挙動を変えたときに名前を直さなかったもので、実際に縛っている内容へ改名した。

**レビュー運用の注意**: 第 5 ラウンドの P0 (「未コミットの構文エラー」) は、レビュアーが
**編集中の作業ツリーを読んだ**ことによる誤検出だった。以後は**コミット済みの HEAD を指定**して
依頼する。

#### 第 6・7 ラウンド (2026-08-29、HEAD `5f92a2cb` / `8a80c336`) — 可変 bin 幅の後始末 ✅

| | 指摘 | 直し方 | commit |
| --- | --- | --- | --- |
| P1 | 可変 bin 幅が任意の f64 で、解析器・位置合わせ・照合の格子が合わない | 幅を **100ms の整数倍**に限定。12 日のファイルは 2 個目の bin で 1µs 許容差を超え、チャンクごと捨てられて二度と再解析されていなかった | `5f92a2cb` |
| P2 | `WaveRaster.bin_secs` が 0.1 秒固定で、実際の描画粒度と食い違う | `coarse.scale.bin_secs` を渡す (native overlay / テクスチャ鍵 / 計測ログが揃う) | `5f92a2cb` |
| P2 | 格子の回帰テストが全長で実 PCM を確保し、300 日で 615MiB / 26 秒かかる | **数式の検査と解析器の裏取りを分ける**。純粋テストが実在し得る全長を掃き、実 PCM は粗くなり始める最小の 12 日だけ。1.1 秒 / 44MiB へ | `215464e2` |

**P1 は私が作った 5 件目の同型**だった (第 4・5 ラウンドの 4 件と同じく、新しい状態を入れて
読む側を数えていない)。今回は `waveform_analysis_range` の doc comment が
「bin_secs はミリ秒に丸められている」と**前提を明記していた**のに、幅を可変にしたとき
その前提を読みに行かなかった。

**テストを分けるときの判断**: 幅ごとの違いは「100ms の整数倍か」「整数フレームに乗るか」
という**数式**なので、PCM を作らずに全長を掃ける。解析器を通す裏取りは 1 ケースあれば
足りる — そして選ぶべきは**実際に不具合を再現した長さ** (12 日) であって、いちばん長い
ケースではない。分割後も、格子を作る `ceil` を外すと**両方とも落ちる**ことを確認した。

**ログ文字列に空白の塊が焼き付いていた**件もこのラウンドで見つけた。行継続が壊れた跡で、
`perf log` に出る 2 件を含む 5 か所。同じ形を他の編集ファイルまで数えて潰した。

### 1.133 外部 Activation が scope 判定より前に変換をキャンセルする — 解消済み

- 出典: 2026-08-27、§1.132 (v3.3.0 出荷前レビュー) の追加確認で Codex が発見。
  **v3.3.0 出荷前に閉じる対象**。

#### 解消前の原因記録


[startup_ops.rs](../src/app/startup_ops.rs) の Activation 分岐は、**パス解決と
snapshot scope 判定より前に無条件で** `cancel_archive_convert_for_navigation` を呼ぶ:

```rust
if matches!(source, StartupOpenPathSource::Activation) {
    self.cancel_archive_convert_for_navigation("activation_navigation");
```

したがって「**範囲外 Activation → 変換が先に死ぬ → その後 navigation は拒否**」
が成立する。SendTo / 二重起動で踏める。`3aaa4659` で入れた preflight は
UI producer の claim だけを対象としており、**この経路は対象外だった**。

#### ⚠ 単に削除してはいけない

この早期キャンセルは「Activation の解決中に、古い bookmark / archive completion が
表示へ着地する race」を防ぐために入っている (コメントに明記あり、
既存テストも「Activation は解決前にキャンセルする」を固定している)。

正しい形は **Activation に request ownership / 世代を持たせ、
scope admission の後で supersede を確定させる**こと。そうしないと、早期
キャンセル導入前の race が戻る。

#### 解消内容

snapshot 非 active では拒否 gate がないため従来の到着時 cancel を維持する。snapshot
active のときだけ supersede を scope admission 後へ defer し、既存の typed
StartupOpenPathResolvePending::owner = Activation を未 admission owner として使う。
先行 bookmark resolver は同 pending 内へ所有移動し、archive の pending navigation と
bookmark の media / page landing は Activation 解決中も破棄せず各 typed state 内に保持する。

範囲外なら toast だけを出して resolver / conversion / bookmark と timeout clock を復元し、
範囲内なら既存 claim_open_request_owner が初めて旧 owner を cancel する。拒否前には
folder history、fs navigation lock、pending_auto_fs_open、bookmark view state を動かさない。
bookmark-owned cache ZIP は BookmarkOpenRequestOwner::target の source identity で scope
admission し、cache path との OR で snapshot 範囲を拡張しない。

変換 race を固定していた旧テストは
admitted_snapshot_activation_holds_stale_archive_bookmark_completion_until_supersede
へ変形し、未解決中の stale completion 非着地と admission 後の cancel の両方を維持した。
拒否側、held resolver、media / book landing も独立テストで固定した。

#### 手動再現 (Codex 提示)

1. キャッシュ未作成で変換に時間のかかる大きめの 7z をフォルダ A に置く
2. 範囲外のフォルダ B を用意する
3. A の一覧を ★固定 (7z が member、B が範囲外)
4. 「確認せず変換」で 7z を開き、進捗ウィンドウが出るまで待つ
5. **その最中に SendTo / 二重起動で B を既存インスタンスへ転送**
6. 修正後: B は範囲外 toast で拒否されるが、進捗ウィンドウと bookmark lifecycle は
   生存する。Activation 解決中に変換が完了しても表示へ先着せず、拒否確定後に元の
   7z / bookmark open が通常どおり続行して cache ZIP / page wait へ進む
7. 対照確認として B を snapshot member にして同じ操作を行うと、scope admission 後に
   Activation が勝ち、変換 / bookmark は cancel されて B へ移動する

- 規模 \\ 優先度: — (**P2 解消、v3.3.0 出荷前 gate 完了**)。

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

### 1.142 ★時刻ソートを選んでもカテゴリ再配置が並びを組み替える — 利用者報告 (2026-08-30)

**症状**: レーティング一覧で「★設定時刻（新しい順）」を選んでも、いま★を付けたばかりの
ファイルが先頭に来ない。利用者環境では 10 番目に出た。

**ソート自体は正しい。壊しているのはその後の再配置。** `rating.db` を直接読むと対象行の
`rated_at_ms` は 2026-08-30 17:15:25 で ★2 の最新、[rating_view.rs](../src/rating_view.rs) の
`sort_rows` の結果でも 1 位になる。ところが [app.rs](../src/app.rs) の
`install_rating_view_rows` は、並べ替えた直後に `grid_item::arrange_grid_items` を
`settings.grid_display_order` (既定 = 1 行目「フォルダ + アーカイブ類」/ 2 行目「画像 +
動画・音声」) で通す。★2 に実在するフォルダ / ZIP / PDF / 7z が **9 件**あり、それが時刻に
関係なく先頭へ出るため対象が 10 番目になる。報告の順位と一致した。9 件のうち 1 件は
`rated_at_ms` が NULL のアーカイブで、**時刻なしが時刻ありより前**という自己矛盾した並びに
なっている。

**同型の経路を先に数える**:

- ブックマークビュー ([app.rs](../src/app.rs) `install_bookmark_view_rows`) も「登録日時↓↑」で
  並べた直後に同じ再配置を通す。**同じ修正が要る**。
- 閲覧履歴は再配置していない (`install_reading_history_entries` は `start_loading_items` を
  直接呼ぶ)。時刻順のフラット横断ビューとしては**閲覧履歴の方が期待どおり**で、
  レーティング / ブックマークだけが例外になっている。
- mIV Remote のレーティング一覧 ([collections.rs](../src/remote_ipc/collections.rs)) も
  再配置しない。**同じ DB を本体とリモートで見ると並びが違う**。

**方針 (利用者と合意済み)**: 少なくとも時刻ソート (`RatingViewSort::RatedAtDesc/Asc`・
`BookmarkViewSort::CreatedAtDesc/Asc`) を選んでいる間はカテゴリ再配置を通さない。
「時刻で一列に並べる」という要求に、カテゴリのグルーピングは含まれていない。
`Normal(SortOrder)` のときは従来どおり (フォルダを先に見せる期待が残るため) 据え置く。

**やらないこと**: 再配置の後ろに「フォルダを後方へ動かす」ような並べ直しを足さない。
再配置を通すか通さないかの分岐 1 つで足りる。`grid_display_order` の意味も、通常フォルダの
挙動も変えない。

**テスト**: rating / bookmark それぞれで「時刻ソート時の行順が `sort_rows` の結果と一致する」
「`Normal` ソート時は従来どおりカテゴリ順になる」を純関数レベルで固定する。フォルダ /
アーカイブ / 画像を混ぜ、時刻 NULL の行を 1 件入れた入力で確認する。

**当面の回避策 (利用者向け)**: 環境設定 → サムネイル → 「グリッドのカテゴリ表示順」で
4 カテゴリすべてを 1 行目に置くと混在ソートになり、★時刻順が保たれる。ただし通常フォルダの
表示にも効く設定なので、恒久策ではない。

**優先度**: 早め (この一連 3 件の中で最優先)。

### 1.143 レーティング一覧の詳細表示に★設定時刻が無く、列ソートが★時刻順を黙って上書きする (2026-08-30)

§1.142 と同じ報告から出た別原因。**こちらは詳細表示 (Ctrl+-) 固有**で、再配置とは独立に
★時刻順が消える。

**(a) 全フォルダ共通の列ソートが、ビュー固有ソートに勝つ**

`rebuild_details_order` ([app.rs](../src/app.rs)) は、列ソートの対象外を「本として表示中」と
「閲覧履歴」だけにしている。レーティング / ブックマークのように**ビュー固有ソートを持つ
ビューが入っていない**。`settings.details_sort_key` は全フォルダ共通の永続設定なので、
別のフォルダで列ヘッダを押した状態がそのまま持ち込まれる。利用者環境の実値は
`details_sort_key = "VideoDimensions"` / `details_sort_ascending = true` で、この状態で
Ctrl+- すると★時刻順は完全に消える (動画サイズは大半が空 → 実質ファイル名順)。

このとき `details_header_sort_active` が真になるので、ツールバーの「★時刻↓↑」は無効化され
理由もツールチップに出る。ところが**メニューの「ソート順」側は無効化されず、選べてしまう**
([ui_main.rs](../src/ui_main.rs) のメニュー経路は `details_header_sort_active` を見ていない)。
選んでも見た目が変わらないので、まずこの 2 つの入口を同じ述語に揃える。

候補: (i) `rebuild_details_order` の除外条件へレーティング / ブックマークを足す、
(ii) これらのビューへ入るとき `details_sort_key` を `Toolbar` へ戻す。(i) だけだと
「詳細表示で名前順にも並べたい」を潰すので、(ii) + 下の★時刻列の組み合わせが素直。

**(b) ★設定時刻の列とツールチップ行が無い**

`DetailsColumnId` ([settings.rs](../src/settings.rs)) に★時刻が無い。
[rating-list-view-plan.md §5.2](rating-list-view-plan.md) は MVP を「ツールバー順に委ねる」と
決め、`DetailsSortKey::RatedAt` (表示名「★設定時刻」) を **Phase 2 の未実装項目**として
残している。(a) のせいでその委譲が成立していないので、両方まとめて扱う。

ツールチップ / 選択情報バーにも★時刻が出ない。`selection_info_content`
([ui_main.rs](../src/ui_main.rs)) にはブックマークの「登録日時」・閲覧履歴の「最終閲覧」という
前例があり、同じ枠へ 1 行足すだけで済む。

**plan の条件を守る**: 列も sort key もレーティング一覧の中だけに出し、ビューを抜けるとき
選択中なら `Toolbar` へ戻す。通常フォルダの詳細設定に「常に空の列」を残さない。

**データの現実 (表示規約を決める材料)**: 実環境の `rating.db` では ★2 が 7017 行中 6087 行
(87%) で `rated_at_ms` NULL (v2.1.0 以前 / 外部取り込み由来)。★1 は 15728 行中 15727 行に
時刻がある。列を出すと空欄が大量に出る前提で、空欄表示と「NULL は末尾」規約を明示する。

**優先度**: §1.142 の後。

### 1.144 Ctrl+E の隠蔽プリセット出力が v1.1.0 から常に選べない — 退行、原因確定

- 出典: 利用者報告 (2026-08-31)。プリセットを 1〜4 に保存しても、Ctrl+E ダイアログの
  プリセットが全部グレーアウトして選べない。開発者環境でも再現。
- **原因 (確定)**: `export_page_pixels_for_idx` が `conceal_mask: None` を固定で渡している
  ([ui_fullscreen.rs:34233](../src/ui_fullscreen.rs:34233))。その結果
  `has_conceal_mask` が常に false になり、チェックボックスの有効条件
  `has_conceal_mask && slot.is_some()` ([ui_fullscreen.rs:34410](../src/ui_fullscreen.rs:34410))
  が成立しない。**マスクを描いていても、どの画像でも必ず無効**。
  プリセットの保存自体は正常。
- **混入**: `be05cfef [Pipeline P1] Reorder edit and final display pipeline` (2026-06-03)。
  出荷は **v1.1.0 から**。以後ずっと死んでいた。
- **なぜ切られたか (直前のコメントが書いている)**: 「フルスクリーン Ctrl+E は
  conceal_mask=None なので、ここで焼いた注釈が worker の conceal preset 合成に潰されない (Inc 7)」。
  表示パイプラインは `raw → erase → local_adjust → conceal` の順で
  ([display-pipeline.md](display-pipeline.md) §875 付近)、`ensure_final_composite_pixels` が
  返す時点で隠蔽が焼き込み済み。マスクも渡すと worker が二重適用する。
  それを避けて `None` にした結果、**プリセット出力ごと機能停止した**。
- **したがって「マスクを渡す」だけでは直らない**。隠蔽適用前の段
  (`raw → erase → local_adjust`) を base として渡し、そこへプリセットごとに隠蔽をかける。
- **同じ構造が製本側に既にある**。`BakedEditSnapshot` はマスクとプリセットを別々に持ち、
  [books.rs:1085](../src/books.rs:1085) が「global AI upscale / denoise だけを除外し、
  それ以外は Ctrl+E と同じ順で適用する」と書いている。ここを参照する。
- 回帰確認: マスクを描いた画像でプリセット 1〜4 が有効になること、`_0`〜`_4` が同時に出ること、
  注釈のあるページで二重適用にならないこと。**「マスクが無い画像では無効」という本来の
  条件は残す** (今回の修正でそこまで外さない)。
- **1.148 の前提ではない**。1.148 (複数選択の一括エクスポート) はプリセット選択を持たない
  設計なので、本項と独立に進められる。
- 規模 / 優先度: Medium / **P1** (機能が丸ごと死んでいる)。

### 1.145 360 ビューの ON 意図と投影方式が、一覧へ戻ると失われる — 利用者要望

- 出典: 利用者要望 (2026-08-31)。
  - 360 を解除して再度 ON にすると、投影方式が既定へ戻る。
  - `Ctrl+↑↓` でフォルダを移動すると 360 ビューが解除される。対象の素材なら継続してほしい。
  - 同一フォルダ内の 360→通常→360 では維持されている (自動 ON が効くため)。
- 原因: リセットは 1 か所。`close_fullscreen` の `panorama_state = None`
  ([app.rs:52013](../src/app.rs:52013))。一覧へ戻る / フォルダ移動はどの表示モードでも
  close を通るので、**モードによらず起きる**。
- **サムネイル / 波形ストリップと同じ仕組みで解決できる**。違いは置き場所だけ:

  | | 置き場所 | 結果 |
  | --- | --- | --- |
  | シークストリップ | `Settings::video_seek_strip_state` | 何をしても残る |
  | 360 | `App::panorama_state` (セッション状態) | close で消える |

- ただし 360 は対象素材でしか成立しないので、単純な ON/OFF ではなく
  **「ON の意図を覚えて、次が対象なら復帰」**にする。XMP による自動 ON の判定が既にある
  ([app.rs:61875](../src/app.rs:61875)) ので、そこへ「セッションの意図」を足す。
- 投影方式も同じ場所に置けば 1 件で片付く。ただし現在の実装は
  **「既定を書き換えない」を明示的に選んでいる** ([app.rs:63387](../src/app.rs:63387):
  「今見ているこの 1 枚を別の写り方で見る操作であって、既定の変更ではない」)。
  `Settings::panorama_projection` は既定のまま据え置き、**セッション内で引き継ぐ値を別に持つ**。
- ⚠️ この状態をどこに持つかは**複数ウィンドウの所有権整理と干渉する**。App グローバルに置くと
  窓ごとに 360 の状態が混ざる。1.111 / detached リワークの後か、少なくとも置き場所を
  そちらと合わせてから入れる。
- 規模 / 優先度: Small〜Medium / P2。

### 1.146 HUD が自動で隠れるだけで、ストリップの選択と波形の解析結果が捨てられる — **対応済み** (2026-08-31)

- 出典: 利用者報告 (2026-08-31)、追加説明 (2026-08-31)。**シークバーを固定していない既定状態**で:
  1. 下端へカーソルを寄せてシークバーを出す
  2. `Shift+S` かアイコンでストリップを表示する
  3. 画面中央へカーソルを戻す → シークバーが自動で隠れる
  4. 再び下端へ寄せる → **ストリップが「なし」に戻っている**。表示し直すと
     **波形の再解析が入り、30 分の表示範囲で 2〜3 秒待たされる**
- 報告者の要点: 「初回は仕方ない。**2 回目以降**がキャッシュで速くなってほしい」。
  ここでいう 2 回目は**次回起動時ではなく、同一起動・同一動画で HUD を出し直したとき**。

#### 原因 (確定) — 2 つが同時に起きている

HUD が自動で隠れると `SeekStripCloseCause::HudHidden` でセッションが閉じる
([render_core.rs:10040](../src/video/native_presenter/render_core.rs:10040))。

1. **選択状態が消える。**
   `clears_persisted_state` ([seek_strip.rs:846](../src/video/seek_strip.rs:846)) が
   `!strip_locked && matches!(self, HudHidden | TileModeOpened | Unavailable)` で true を返す。
   **固定していない限り、HUD が隠れるだけで選択が消える。** ステップ 4 の正体。
2. **解析結果が消える。**
   `stop_video_seek_strip_session` が `wave_worker` を drop する
   ([native_video.rs:7423](../src/app/native_video.rs:7423)。コメント「セッションが死ぬ経路は
   この関数だけ」)。粗トラックは `WaveWorkerRuntime.coarse` にあり、**ワーカースレッドと一緒に
   死ぬ**。こちらは **固定していても、閉じた原因が何でも、同じ動画のままでも失われる**。
   2〜3 秒の再解析はこれ。

#### 対応 (2026-08-31 決定) — a と b を両方入れる。優先。

- **a: `HudHidden` では選択をクリアしない。** 固定の有無に関わらず、HUD の自動非表示は
  「利用者がストリップを閉じた」ではない。`is_user_dismissal`
  (`Toggle` / `DownwardDrag` / `Escape`) が既に「利用者の意思」を表しているので、
  **`HudHidden` をそちら側から外す**。`TileModeOpened` / `Unavailable` は別画面 / 素材都合
  なので現状のままでよいか、あわせて判断する。
- **b: 動画が変わっていなければ粗トラックを保持する。** セッションを畳んでもワーカー
  (少なくとも粗トラック) を捨てない。**同じ動画のあいだだけ保持し、動画が変わったら捨てる**
  ので、保持量は 1 本ぶんに収まる (`MAX_COARSE_WAVEFORM_BYTES = 64 MiB` が上限、
  通常の 1〜2 時間なら 250〜500 KB)。
  **前例がある**: `wave_holdover` ([native_video.rs:34](../src/app/native_video.rs:34)) が
  モード切替のあいだ描画済みラスタを保持している。同じ考え方を粗トラックへ広げる。
- **判断の根拠 (利用者確定 2026-08-31)**: 「HUD を閉じてもメモリを少し消費するのは問題ない。
  **頻繁に開閉してキャッシュが消える方が体験を損なう**」。`HudHidden` でセッションごと畳んで
  いたのは資源を握りっぱなしにしないためだが、**その判断をここで覆す**。覆した理由を
  `seek_strip.rs` / `native_video.rs` の該当コメントにも残す。

#### 回帰確認

- 固定していない状態で、HUD の自動非表示 → 再表示を繰り返してもストリップが残り、
  **再解析が走らない** (2 回目以降が即座に出る)。
- 動画を切り替えたら粗トラックが捨てられ、メモリが増え続けない。
- 利用者が明示的に閉じた (`Shift+S` / 下方向ドラッグ / Esc) ときは従来どおり選択が消える。
- タイル一覧へ入る / 素材が使えないときの挙動が変わっていない。
- フルスクリーン退出・別ウィンドウ間の移動でも、保持しているワーカーが漏れない。

- 規模 / 優先度: a = Small / b = Small〜Medium。**P1** (頻繁に踏む体験劣化)。
- 起動をまたぐ永続化は 1.152。**本項とは別件**で、本項を直せば報告の不満は解消する。

#### 実装 (2026-08-31)

- **a**: `clears_persisted_state` から `HudHidden` を外した。あわせて **`Unavailable` も
  外した** — 固定中にそれを禁じている既存コメントの理由 (「素材の無い 1 本が、そのあとの
  動画すべてから選択を奪う」) が、固定していなくてもそのまま成り立つため。
  **`TileModeOpened` は残した**: タイル一覧は利用者がもう一方の面を明示的に開いたもので、
  戻ったときの復帰は `video_tile_reopen_pending` が別に持っている。一緒に変えると二重になる。
- **b**: `SeekStripCloseCause::keeps_viewing_the_same_video` を足し、真の境界では
  `SeekStripWaveWorker` を **cancel せずに** `App::video_seek_strip_wave_holdover` へ
  動画パスと一緒に預ける。拾うのは `take_or_spawn_seek_strip_wave_worker` 1 か所で、
  パスを照合し、**新しく spawn したものと見分けが付かない状態** (背景段は再開済み) で返す。
  手放すのは動画が変わる / フルスクリーンを出るときで、`sync_native_video_seek_strip` が
  毎フレーム現在のパスと照合して取りこぼしを拾う (ストリップを閉じたまま次の動画へ
  移る経路があるので、close だけに任せない)。
- **cancel は不可逆** (立てた時点でスレッドが終わる)。預ける経路で呼ぶと、次のセッションが
  死んだ worker を拾って全尺解析が二度と進まない。テストは同一性・停止状態・生存の 3 つを
  見ており、`cancel` を足す変異で落ちることを確認した。
- 実機確認 (回帰確認リストの 5 項目) は未実施。

### 1.147 360 ビュー中、ストリップ上でもホイールが 360 に取られる — 利用者報告

- 出典: 利用者報告 (2026-08-31)。360 ビューが ON のとき、音声波形ストリップにマウスを
  乗せてもホイールが 360 の視野角操作へ行き、ストリップの時間範囲を変えられない。
  サムネイル列も同じ扱いにしてほしい。
- ホイールの宛先をポインタ位置で決める。ストリップ (波形 / サムネイル列の両方) の上なら
  ストリップへ、それ以外は 360 へ。
- 既存の同型として、フルスクリーンのホイール分岐に
  `should_handle_fullscreen_wheel` / `fullscreen_cursor_in_panel_for_wheel`
  ([ui_fullscreen.rs](../src/ui_fullscreen.rs)) がある。**判定を新規に足さず、
  同じ「ポインタがどの領域にいるか」で決める形に寄せる**。
- 規模 / 優先度: Small / P2。

### 1.148 複数選択したまま Ctrl+E で一括エクスポート — 利用者要望

- 出典: 利用者要望 (2026-08-31)。モザイクを付けた画像を、縮小しつつ特定フォルダへ
  まとめて出したい。メール添付用に「送信用」フォルダを作って縮小版を置く運用をしている。
  ファイル名は `<filename>_edited` のような展開マクロで指定したい (`<filename>` / `<dirname>`)。
- **仕様 (2026-08-31 決定)**: グリッドで複数選択 → `Ctrl+E` → 一括用ダイアログ。
  出力先フォルダ / 画像形式 / 出力サイズ / ファイル名テンプレートを選ぶ。
  **隠蔽加工のプリセット選択は持たない** (各画像は自分の最終合成をそのまま出す)。
  これにより 1.144 の「隠蔽前 base が要る」問題を**踏まずに済む**ので、1.144 と独立に進められる。
- **⚠️ 製本の合成ワーカーを共有する。新しいパイプラインを書かない。** 必要なものは既にある:

  | | 製本 | 一括エクスポート |
  | --- | --- | --- |
  | UI スレッドでの edit snapshot | `book_baked_edit_snapshot` | そのまま |
  | グリッド選択から N 件を組み立て | `add_grid_selection_to_named_book` ([ui_fullscreen.rs:33447](../src/ui_fullscreen.rs:33447))。スタック展開込み | そのまま |
  | ワーカーでの N 件ループ | `append_pages_at` → `start_book_op` | そのまま |
  | デコード → 合成 → エンコード | `write_source` の `Composited` 分岐 ([books.rs:932](../src/books.rs:932)) / `write_composited_color_image` ([books.rs:1263](../src/books.rs:1263)) | そのまま |
  | 出力先 | 本フォルダ固定 | フォルダ選択 |
  | ファイル名 | ゼロ埋め連番 | テンプレート |
  | 縮小 | なし | `ExportScale::scaled_size` を書き出し直前に 1 段挟む |

- **`append_pages_at` を直接呼ばない**。あの関数は本固有の事情を抱えている
  (ページ番号の採番、`MAX_BOOK_PAGES` の上限、無編集時の byte-copy fast path、
  `restore_declines` の記録、`edit_copies` / `semantic_copies` の集計)。エクスポートから呼ぶと
  意味を持たない処理が付いてくる。**1 件ぶんの「デコード → 合成 → エンコード → 書き出し」を
  関数として切り出し、製本とエクスポートの両方がそれを使う**。切り出す単位は既に
  `write_source` の `Composited` 分岐がほぼその形をしている。
- 縮小段を共通化すると **1.149 (製本フォルダごとの上限サイズ) がほぼ副産物になる**。
  製本側が同じ縮小段へ上限を渡すだけになる。
- 決めること:
  - テンプレートに使える置換子 (`<filename>` / `<dirname>` / 連番)。同名衝突時の扱い。
  - 対象外 (動画 / 音声 / フォルダ) を選択に含んでいたときの扱い。黙って飛ばすか件数に出すか。
  - 進捗表示とキャンセル。既存の `ExportPending` (`total` / `done` / `successes` / `errors`) が
    そのまま使える形か確認する。
- 規模 / 優先度: Medium / P2。

### 1.149 製本フォルダごとに上限ピクセルサイズを設定して自動縮小 — 利用者要望

- 出典: 利用者要望 (2026-08-31)。散らばった画像を一旦製本フォルダへ集め、別ツールで
  一括縮小してから送る運用をしている。集める時点で縮小できると手順が 1 つ減る。
- **1.148 の後に判断する**。1.148 で縮小段を共通化すれば、製本側は同じ段へ上限を渡すだけに
  なるのでほぼ副産物。逆に 1.148 だけで用が足りる可能性もある
  (集めた後に本フォルダから一括エクスポートすればよい)。報告者も「エクスポート機能の拡張の方が
  柔軟に使える気もする」と書いている。
- 決めること: **既にある本のページには遡って効かない**設計にするか。効かせるなら再エンコードが
  走るので、明示操作にする。
- 規模 / 優先度: Small (1.148 の後) / P3。

### 1.150 編集内容を複数画像へまとめて貼り付ける — 利用者要望

- 出典: 利用者要望 (2026-08-31)。画面コピーを複数枚撮り、同じ位置を切り抜くために同じ
  トリミングを複数画像へ適用したい。現状は 1 枚ずつ貼り付けるしかない。
- 現状: 「編集内容をコピー / 貼り付け」はどの経路も**単一対象のみ**。3 か所ある
  ([context_menu.rs:1078](../src/ui_dialogs/context_menu.rs:1078) / :1476 / :1940)。
  対象は `GridItem::Image` / `ZipImage` / `PdfPage` に限定。
  **同型の入口をすべて塞ぐ** (片方だけ直すと経路によって挙動が違う)。
- 決めること:
  - **途中失敗時に成功分を残すか、全部戻すか**。トーストで「成功 N 件 / 失敗 M 件」を出して
    残す形が自然 (報告者もトースト報告でよいとしている)。
  - 対象外 (動画 / 音声 / フォルダ) を選択に含んでいたときの扱い。
  - Undo の対象にするか。現在の Undo はレーティング / タグ用 ([undo_stack.rs](../src/undo_stack.rs))。
- 規模 / 優先度: Medium / P2。

### 1.151 編集内容をリセットする機能 — 利用者要望

- 出典: 利用者要望 (2026-08-31)。編集内容をクリアする手段が見つからない。単体と複数の
  両方でできるとよい。マニュアルを検索しても出てこなかったとのこと。
- 現状: それらしい機能が見当たらない。**単体すら無い**と思われる。
- 仕様 (方針):
  - 対象はカーソル位置のアイテム、またはチェックした複数アイテム。
  - **モーダルダイアログでの確認を必須**にする。誤操作の影響が大きいため。
  - 確認ダイアログに「何件の、どの種類を消すか」を出す。選択を間違えたときに気づける。
- **決めること: 何を消すか**。編集は 7 種類ある (補正 / 消しゴム / モザイク / 補正レイヤー /
  注釈 / 切り取り / 回転)。**★ とタグは含めない** — 「編集をリセット」で評価まで消えると事故になる。
  種類を選べるようにするか、まとめて消すかも決める。
- 決めること: Undo の対象にするか (1.150 と同じ論点)。
- 規模 / 優先度: Medium / P2。
### 1.152 波形ストリップの粗トラックを永続キャッシュし、開いた時点で全尺を裏で埋める — 未決 (案 B)

- 出典: 利用者報告 (2026-08-31) の派生。新しい動画を開いた**初回**の波形表示が遅い。
  開発機の実測で、**HDD 上の大きな動画・表示範囲 30 分で数秒**。
  30 分は `WINDOW_DECODE_MAX_SPAN_SECS = 1800` = 1 回のデコード範囲の上限なので、
  この測定は最悪ケースにあたる。
- **⚠️ 報告者の不満は本項ではなく 1.146 で解消する (2026-08-31 訂正)。** 報告者の言う「2 回目」は
  次回起動時ではなく、**同一起動・同一動画で HUD を出し直したとき**だった。それは
  メモリ上の粗トラックを捨てているのが原因で、ディスクへの永続化は要らない。
  本項は「**起動をまたいでも初回から速い**」という別の価値についての検討であり、
  1.146 を入れた後に、まだ欲しいかを改めて判断する。

#### 容量 — 十分小さい (実測ではなくコード上の定数からの計算)

粗トラックは既に量子化・固定長・チャンク分割済み。

| 定数 | 値 | 出典 |
| --- | --- | --- |
| 1 bin のバイト数 | **7** (peak L/R, rms L/R, 帯域エネルギー×3) | `QuantizedWaveformBin` ([seek_strip_wave.rs:420](../src/video/seek_strip_wave.rs:420)) |
| bin 幅 | **0.1 秒** | `COARSE_BIN_SECS` ([seek_strip_wave.rs:39](../src/video/seek_strip_wave.rs:39)) |
| チャンク | **600 bin = 60 秒 = 4,200 バイト** | `COARSE_BINS_PER_CHUNK` |

= **70 バイト/秒**。1 時間 252 KB / 2 時間 500 KB / 1000 本 (平均 1 時間) で約 250 MB。
超長尺は `CoarseScale` が bin 幅を広げるので、**1 本あたりの上限は設計上すでに効いている**
(`MAX_COARSE_WAVEFORM_BYTES = 64 MiB`)。

peak だけに削れば 2 バイト/bin (1 時間 72 KB) にできるが、帯域エネルギーは色付き波形が
使うので **7 バイトのままにする**。

#### 工数 — Small〜Medium。必要なものはほぼ揃っている

| 要素 | 状態 |
| --- | --- |
| キャッシュキー | **既にある**。`WaveFileIdentity { normalized_path, size, mtime }` ([seek_strip_wave.rs:45](../src/video/seek_strip_wave.rs:45))。mIV の他 DB と同じ形なので invalidation もこれで足りる |
| 量子化・固定長 | **既にある** |
| チャンク分割 | **既にある** (60 秒)。部分保存が自然にできる |
| 完了結果と identity の対応 | **既にある**。`CompletedTimelineAnalysis` |
| 保存先 | 新規。SQLite 1 テーブル (identity + chunk index → blob) |
| 読み書きの差し込み | 2 か所 (チャンク完成時に書く / 開いたときに読む) |
| 破棄 | 新規。容量上限 + LRU (サムネイルキャッシュと同じ考え方)。設定と「消す」導線も要る |

⚠️ モジュール冒頭に **「All caches are process-memory-only by design」** と明記されている
([seek_strip_wave.rs:10](../src/video/seek_strip_wave.rs:10))。**この方針を覆す判断が先**で、
実装はその後。覆すなら同コメントを書き換えて理由を残す。

#### 採る方式: 案 B (開いたら裏で全尺を埋める)

案 A (計算したチャンクだけ機会的に保存) は余計なデコードをせず安全だが、**初回は今と同じ速さ
のまま**で、報告の不満に効かない。したがって **案 B を採る**。

- **ストリップを開いている間だけ**、低優先度で先を埋める。利用者が波形を見ている最中に限る
  ので、見ない動画を勝手に全尺デコードしない。
- 表示中の範囲を最優先し、埋め作業がそれを遅らせないこと。**前面 (利用者が待っている範囲) を
  必ず先に通す**。
- ストリップを閉じたら埋め作業を止める。途中まで貯めたチャンクは残す (チャンク単位なので
  部分的でも有効)。
- 次に同じ動画を開いたときは、埋まっているチャンクを読むだけで表示が出る。

#### 決めること

- 上限容量の既定値と、超えたときの捨て方 (LRU)。既存のキャッシュ管理ダイアログ
  ([cache_manager.rs](../src/ui_dialogs/cache_manager.rs) / archive_cache_manager) に並べるか、
  新しい導線にするか。
- 埋め作業の優先度を、既存のサムネイル / PDF の優先度レーンとどう共存させるか。
  **前面優先の原則を崩さない**。
- ポータブル版の保存先 (`<exe_dir>\data`) での容量方針。

- 規模 / 優先度: Small〜Medium / P3。**1.146 の回答待ち**。

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

### 2.11 動画サムネイル中央の再生アイコンを目立ちにくくする — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> `video_thumbnail_indicator` の 3 択 (`PlayIcon` 既定 / `BottomLeftBadge` / `Hidden`)。
> **既定は現在の中央アイコンのまま** (利用者確定 2026-08-22: 「既定は現在の動画再生アイコンで OK。
> 設定で変更できれば良い」)。対象確認の回答待ちは解消。
> 中央アイコンと左下バッジは `video_thumbnail_indicator_parts` が両方を返す形で**構造的に排他**。
> **音声は対象外** — サムネイルを生成せず音楽アイコンがセルの中身そのものなので、消すと空の箱になる
> (`bottom_left_content` で Video と Audio のアームを分離済み)。
> バッジの色は `FormatBadgeKind` で決める。**ラベル文字列の一致で決めない** (旧実装は未知ラベルが
> すべてアーカイブの橙に落ちるため、動画も新形式も静かに橙になる)。ラベルは `動画` — 生成中の中央
> プレースホルダと同じ語にし、1 セルに `動画` と `VIDEO` が並ぶのを避けた。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。動画サムネイル上のアイコンを小さくするか、
  非表示にしたい要望。
- まず、対象が動画セル中央の `▶` 再生アイコンで合っているか回答を待つ。
- 対象が合っていれば、中央の代表画像を隠しにくい角の小型表示を既定候補とし、非表示設定も
  用意する方向で検討する。既存 §2.2 の隅バッジ配置とは意味が異なる overlay なので、単に
  同じレーンへ入れず、再生可能であることの視認性と hover / 選択表示との重なりを確認する。
- スナップショットは通常動画、代表画像あり / なし、選択 / hover、狭いセル、形式 / 評価 / タグ
  バッジとの組み合わせを含める。
- 規模 / 優先度: Small / P2 (対象確認後に仕様確定)。

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

### 2.15 PDF: 利用者が待っている open を、背景の一斉 open に埋もれさせない ✅ 実装済み (2026-08-21)

> §2.16 と同じ 1 つの機構で実装した。設計と経緯は
> [briefs/pdf-open-admission.md](briefs/pdf-open-admission.md)。
> **実機確認済み (2026-08-21)**: 「サムネイルがまだ出ていない状態でも PDF ページを開ける」。
> 同じ本のページ送りの速度に変化なし。以下は着手前の記録。

- 出典: 利用者報告 (2026-08-20)。「複数ウィンドウを開こうとしているとき、ダブルクリックで
  画像がなかなか開かない」。3 回クリックし、待ちが 1.5 → 2.2 → 3.7 秒と積み上がった。
- **実測** (利用者実機の perf log、`worker_open_ms` を分離したので分かった):

  | 全 render の `worker_open_ms` | p50 | p90 | p99 | 最大 |
  | --- | ---: | ---: | ---: | ---: |
  | | **0.3 ms** | 763 ms | 5,730 ms | **10,803 ms** |

  1 秒超の open が 75 件あり、**固まって発生**している:

  ```
  t=11.9 open=5744ms  20230103_008.pdf     t=13.8 open= 7311ms 20230103_010.pdf
  t=12.4 open=6252ms  20230103_001.pdf     t=16.5 open=10314ms 20230103_009.pdf
  t=13.1 open=6955ms  20230103_005.pdf     t=17.0 open=10803ms 20230103_006.pdf
  ```

  **別々の PDF を一斉に開いている区間** (フォルダのカバーサムネイル生成)。対象ファイルは
  HDD 上にあり、5 ワーカーが同じディスクを取り合う。
- **重要**: 遅いのは**ファイルではなく状況**。同じ `20221030_001.pdf` は他の時点で
  **0.4 ms** で開いている (数十サンプル)。当初「病的なファイル」と誤診したが、
  利用者の指摘で確認して訂正した。
- **Critical レーンの予約はワーカーを 1 つ確保するだけで、ディスクは確保しない。**
  他の 4 ワーカーが大きな PDF を読んでいる間、予約されたワーカーも同じディスクを待つ。
- 直す方向: **前面の PDF open が待っている間、背景 (カバーサムネイル生成・先読み) の open を
  走らせない。** スクロール中の先読み抑制 (`decide_prefetch_allowed`、
  [prefetch-suppression-during-scroll-plan.md](prefetch-suppression-during-scroll-plan.md)) と同じ形。
  **時間窓ではなく「前面が待っているか」で決める。**
- **設計原則 (利用者、2026-08-21)**: **利用者の直接の操作に応答することが最優先**。
  資源が競合したら、背景仕事より前面を通す。
- 規模 / 優先度: 中 / P2。
- 関連: §2.16 (同時 open 数の上限)。両者は補完関係で、こちらは**優先度**、あちらは**総量**。

### 2.16 PDF: open と render を別の資源として扱う (同時 open 数の上限) ✅ 実装済み (2026-08-21)

> 実装は `MSG_OPEN` を新設して open を独立した IPC 要求にする形になった。当初案の
> 「job 単位で許可枠を数える」は、枠が render まで覆って SSD の throughput を落とすため
> 採らなかった。詳細は [briefs/pdf-open-admission.md](briefs/pdf-open-admission.md) §3.1。
> 以下は着手前の記録。

- 出典: 2026-08-21、§2.15 の議論から。利用者提案「PDFium 並列度 10、同時 open は 3 まで」。
- **実測が支持している**: 7.4 秒かかった要求の内訳は

  ```
  worker_open_ms   = 7371.8   ← 競合を全部吸収している
  worker_page_ms   =   11.5
  worker_render_ms =  154.9   ← 平常時 (100〜170ms) と変わらない
  ```

  **競合の最中でも描画時間は動かない。** つまり:
  - **open はディスク律速** — HDD で同時に走らせるとシークが往復して待ちが跳ねる
  - **render は CPU 律速** — 並列度を上げた分だけ効く

  **性質が正反対のものを、同じ並列度で縛っているのが現状の構造。**
- 直す方向: **render の並列度は保ったまま、同時 open 数だけを上限で絞る。**
  - **親側の dispatch で絞る** (ワーカー内部で待たせない)。親はどのワーカーへどの文書を
    送ったかを知っているので、**「そのワーカーが保持している文書と違う要求 = open が要る」と
    予測できる**。§2.13 で入れた文書キャッシュがこの予測を成立させている。
    ワーカー間の名前付きセマフォのような仕掛けは要らない
  - **ストレージ種別を自動判定しない。** HDD / SSD を実行時に見分けて挙動を変えるのは
    「実行時状態で挙動を変えない」方針に反する。**固定値か設定値**にする
- 規模 / 優先度: 中 / P2 (dispatcher の資源管理に手を入れるので、出荷直前には入れない)。
- 関連: §2.15 (前面優先)、§2.13 (文書キャッシュ)。

### 2.17 フォルダ代表サムネの既定ソートが一覧の既定と食い違う (番号順 vs ファイル名順) — 利用者報告 ✅ 実装済み (2026-08-22)

> `default_folder_thumb_sort` を `FileName` にし、`schema_meta.folder_thumb_sort_default_v2` marker による
> 一度きりの移行を追加。**値と marker は同じ transaction で commit する** (marker だけ残ると次回起動が
> 「移行済み」と判断して番号順のまま取り残されるため)。**移行の失敗は致命的にしない** — ログを残して
> 保存済み設定のままロードし、marker が立たないので次回再試行する (load 全体を失敗させると
> `FailedFallbackDefault` でそのセッションが既定設定 + save 抑止になり、全利用者が 1 回だけ通る移行の
> 代償として重すぎる)。`natural_sort_key` は変更していない。3.2.0 の `version_highlights` で戻し方を告知。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 2026-08-22。`00表紙.jpg` / `00表紙2.jpg` の 2 枚だけが入ったフォルダで、一覧の先頭は
  `00表紙.jpg` なのに、フォルダタイルの代表サムネだけ `00表紙2.jpg` になるという報告。
- **どちらも既定値のまま**で起きる (利用者が設定を変えたせいではない)。既定が 2 つに分かれている:

  | 設定 | 既定 | 定義 |
  | --- | --- | --- |
  | 一覧の並び順 `sort_order` | ファイル名順 | [settings.rs:5388](../src/settings.rs:5388) (`SortOrder::default()` = `FileName`)、固定テスト [settings.rs:9564](../src/settings.rs:9564) |
  | 代表画像の選択基準 `folder_thumb_sort` | 番号順（区切り無視） | [settings.rs:5126](../src/settings.rs:5126)。導入コミット `e6793dbb` (2026-04-12) から不変 |

- 機構: 代表画像の自動選定は [thumb_loader.rs:2349](../src/thumb_loader.rs:2349)
  `resolve_folder_thumb_image_inner`。**一覧の `sort_order` は見ず** `folder_thumb_sort` で
  並べて先頭 1 枚を採る。実ログでも確認済み:
  `resolve_folder_thumb_image: ... sort=Numeric depth=3 -> ...\00表紙2.jpg` /
  保存キー `folderthumb:auto-v2:numeric:d3:...`。ピン (`folder_thumb_pins.db`) は無関係。
- 番号順で反転する理由: `natural_sort_key` ([ui_helpers.rs:979](../src/ui_helpers.rs:979)) は
  記号・空白を捨てて数字塊 / 文字塊に割るが、**拡張子も直前の文字塊に溶ける**。
  - `00表紙.jpg` → `[数字0, "表紙jpg"]`
  - `00表紙2.jpg` → `[数字0, "表紙", 数字2, "jpg"]`

  2 要素目が `"表紙" < "表紙jpg"` (前方一致で短い方が小) なので、**数字が付いている方が勝つ**。
  `cover.jpg` vs `cover2.jpg`、`a.jpg` vs `a10.jpg` も同型 (連番同士 `001` / `002` は期待どおり)。
  ファイル名順 (Windows ソートキー) なら `00表紙.jpg` が先頭になる (`LCMapStringEx` の
  ソートキー比較・`StrCmpLogicalW` の双方で確認)。
- **直す方向 (利用者判断済み)**: 既定をファイル名順に合わせる。ただし `default_folder_thumb_sort`
  を変えるだけでは**新規インストールにしか効かない**。[settings_db.rs:1550](../src/settings_db.rs:1550)
  `write_settings_kv` が Settings の全フィールドを毎回書くので、既存利用者の `settings_kv` には
  全員 `folder_thumb_sort = "Numeric"` が入っており、**「保存されている」= 「利用者が選んだ」とは
  判別できない**。そこで更新時に一度だけ移行する:
  1. `schema_meta` に一度きりのフラグ (例 `folder_thumb_sort_default_v2`) を置く。既存の
     `bootstrap_complete` / `migrated_from_json_at` と同じ `INSERT OR IGNORE` の形で足せる。
  2. フラグが無い既存 DB では、起動時に一度だけ `folder_thumb_sort` を `FileName` へ書き換えて
     フラグを立てる。クリーンインストールは bootstrap 時にフラグだけ立て、値は触らない。
  3. 副作用: 意図して「番号順」を選んでいた利用者も 1 回だけ戻る (上記のとおり区別できない)。
     §5.0 経由で `version_highlights` の `must_read` に載せ、戻し方 (環境設定 → フォルダ・ファイル →
     「代表画像の選択基準」) を明示する。
  4. 代表サムネのキャッシュキーにソート順が入る (`...:numeric:...` → `...:filename:...`) ので
     選び直しは自動で走る。旧キーのエントリが残って容量だけ増えないかは実装時に確認する。
  5. マニュアルの既定表記 ([settings.html:470](../htdocs/mimageviewer/manual/settings.html:470)) を
     同時に更新する。
- 実装前に決める枝: 「番号順の natural key から拡張子を除く」案でも
  `[0,"表紙"] < [0,"表紙",2]` となり番号順のまま期待どおりになる。ただし `natural_sort_key` は
  一覧ソート ([app/folder_scan.rs:132](../src/app/folder_scan.rs:132))、ファイル名スタック、
  ZIP ツリー、スマートフォルダが共有しており、**番号順を選んでいる全一覧の並びが変わる**。
  既定の食い違いを消すだけなら上の移行案の方が影響範囲が小さい。
- 規模 / 優先度: 小 (既定値 + 一度きりの移行) / P2。

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

### 2.22 貼り付け / 新しいフォルダー作成後に、追加項目へカーソル・選択を移す — 専用スレ >>305 ✅ 実装済み (2026-08-26、実機確認済み)

- 出典: 専用スレ >>305 (2026-08-26)。グリッドへ貼り付けたファイルが現在のソート順で
  離れた位置へ入ると、どれが追加されたものか分からなくなるため、エクスプローラーと同様に
  貼り付けた項目を選択状態にしてほしいという要望。新しいフォルダーを作成した場合も、
  作成後のフォルダーへカーソルが移らず不便なことがあるため、同じ完了規則へ揃える。
- 期待する動作 (貼り付け / 新しいフォルダー作成で共通):
  - 現在表示中の実フォルダへ追加された項目が **1 件**なら、既存のチェック選択を解除し、
    追加項目へカーソルと Shift 選択の anchor だけを移す。チェックマークは付けない。
  - 追加項目が **複数件**なら、既存のチェック選択を解除し、一覧へ現れた追加項目をすべて
    チェック選択する。現在の表示順で先頭の追加項目へカーソル / anchor を置き、見える位置へ
    スクロールする。
  - サムネイル表示 / 詳細表示、チェック方式 / エクスプローラー方式のどちらでも、上の
    単一 / 複数規則を同じにする。現在のソートと絞り込みは変更しない。絞り込みによって
    表示されない追加項目を無理に選択したり、絞り込みを自動解除したりしない。
- 現状と修正境界:
  - 新しいフォルダー作成は既に `select_after_load` へ作成名を渡しているが、利用者の実操作では
    カーソルが移らないことがある。非同期 reload のどこで selection hint が失われるかを確認し、
    表示へ install した後の共通選択処理まで届くことを回帰テストにする。
  - Ctrl+V / 背景メニューの貼り付けは、現在はフォルダ背景の Shell `paste` verb を呼び、
    notify-rs のフォルダ変更通知で一覧を再読込するだけなので、mIV は実際に作成された出力パスを
    所有していない。元の basename だけで推測せず、名前衝突時に Shell が改名した結果も含めて
    **成功した操作の実出力パス**を取得できる完了通知を設計する。必要なら
    `IFileOperationProgressSink` を持つ操作経路への移行、または operation token 付きの限定的な
    前後 snapshot を検討する。外部アプリが同時に追加したファイルを誤選択しないこと。
  - `select_after_load: Option<String>` へ複数名や追加 bool を足して分岐を増やさず、宛先フォルダの
    identity / operation token / 実出力パス群 / 適用世代を持つ typed な post-operation selection
    request を 1 つの owner にする。reload 完了時に path key から新しい index へ解決し、cursor /
    anchor / checked / scroll を一度だけ更新する。
- 完了条件:
  1. 名前順 / 日付順で追加項目が離れて配置される 1 件・複数件の貼り付けで、上の選択規則になる。
  2. コピー貼り付け / 切り取り貼り付け / 名前衝突による改名、作成直後のフォルダーを確認する。
  3. 操作完了前に別フォルダへ移動した場合、その一覧の選択を汚さない。外部変更との同時発生でも
     貼り付け操作と無関係な項目を選択しない。
  4. フィルタで一部または全部が非表示の場合、フィルタを変えず、表示中の実出力だけへ適用する。
- 関連: [shell-file-operations-context-menu-plan.md](shell-file-operations-context-menu-plan.md)
  §7〜§8、`src/ui_dialogs/new_folder.rs`、`App::try_select_after_load`。
- 規模 / 優先度: 新しいフォルダー側は Small、貼り付け完了結果の取得を含めると Medium / P2。

#### 実装 (2026-08-26)

正本は [post_operation_selection.rs](../src/post_operation_selection.rs) の module doc。

- **新しいフォルダーでカーソルが移らなかった原因**: `reload_current_folder_preserving_override`
  が `select_after_load` を**無条件で現在の選択に上書き**していた。作成側は先に名前を
  置いてから再読込を頼むので、使われる前に潰されていた。**名前変更も同じ経路で同じように
  壊れていた** (報告は無かった)。`preserve_cursor_hint_for_reload` が「既にヒントがあれば
  触らない」を持つ 1 か所になった。
- **貼り付けの出力パス**: Shell の背景 `paste` verb に委ねているので mIV は何が作られたか
  知らない。クリップボードの元名から推測すると、**まさに拾いたい衝突時の改名を取り逃がす**。
  操作の**直前**に一覧にあったパスを控え、差分を追加項目とみなす方式にした。
- **型付き要求 1 つ**にまとめた (`select_after_load` へ bool や複数名を足さない):

  | 持ち物 | 効く完了条件 |
  | --- | --- |
  | 適用先フォルダ | 3 (操作完了前に別フォルダへ移っても、その一覧を汚さない) |
  | `ExpectedOutputs::{Known, AddedSince}` | 2 (mIV が作った物と Shell が作った物を同じ扱いにする) |
  | 表示中の項目だけを渡す入口 | 4 (フィルタを変えず、隠れている出力は待つだけ) |
  | 前回適用した集合 | 大きい貼り付けで届くたびに選び足し、増えなくなったら手を引く |

  判断は `post_operation_selection::decide` の純関数で、状態遷移をテストで固定した。
- **選択規則**: 1 件はチェックを付けずカーソルだけ。複数件は全部チェックして表示順の
  先頭へカーソルと Shift 起点。表示形式・選択方式では変えない。
- **自動再読込との調停**: `check_external_folder_changes` は要求が生きている間、自前の
  「元の選択へ戻す」を止める (貼り付けの完了を拾うのはこの再読込なので、放っておくと
  打ち消し合う)。

#### 残っている穴

**貼り付け中に外部アプリが同じフォルダへ足したファイルと区別できない。**差分方式の
原理的な限界。消すには貼り付け自体を `IFileOperation` + `IFileOperationProgressSink` へ
移して実出力を受け取るしかなく、それは
[shell-file-operations-context-menu-plan.md](shell-file-operations-context-menu-plan.md)
§7 の残作業 (drop-to-folder の PowerShell 置き換え) と同じ移行になる。

**絞り込みで貼り付け結果が 1 件も見えない場合、何も起きない。**実機確認 (2026-08-26) で
「分かりやすくはないが、一旦この動きで良い」と判断。案内を出すなら「貼り付けたファイルが
絞り込みで隠れています」のようなトーストになる。

#### 実機確認 (2026-08-26)

新しいフォルダー / 名前変更 / 1 件貼り付け / 複数件貼り付け / 切り取り貼り付け /
操作直後の別フォルダ移動 / 絞り込み中、すべて期待どおり。名前衝突は利用者の環境の OS
ダイアログに「両方保持」が無く (無視 / スキップのみ) 未確認。改名経路自体は
`AddedSince` の差分で拾うので、選択規則としては同じ扱いになる。

### 2.23 タグの付与時刻で並べる / 見せる — 利用者要望 (2026-08-30)

**背景**: 「間違えて付けたタグを取り消したい」ときに、付けた順で見たい。§1.142 / §1.143 の
調査中に、レーティングの★時刻と同じ話がタグにもあると分かった。

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
  持たせるなら §1.142 / §1.143 と同じ形 (時刻ソート中はカテゴリ再配置を通さない / 詳細列と
  sort key はビュー限定 / 抜けるとき `Toolbar` へ戻す) に揃える。**先に §1.142・§1.143 を
  済ませ、同じ構造をなぞる。**
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

### 4.5 非アクティブなウィンドウでは右クリックのリングが効かない — 利用者報告 ✅ 解決済み (§1.100 の修正による)

> 下記のとおり §1.100 と同一原因。§1.100 は実機確認済み (`8db282e3` + `5b83df3f` / `d2c19796` /
> `c71d8c08` + `4c5260a2` + `12f60c97`) なので本項も解決済み。経緯の記録として残す。

- ⚠ **§1.100 と同一症状であることが 2026-08-21 に確定した** (同じ 1 つの原因)。
  本項が持っていた追加事実 2 点 (「**アクティブにすれば動く**」= 非アクティブ窓でのみ効かない /
  「App グローバル状態が塞いでいる」仮説の**反証**) は §1.100 へ移した。
  **修正・仕様判断はすべて §1.100 側で行う。本項は経緯の記録として残す。**

- 出典: 2026-08-20 の利用者報告。複数ウィンドウ表示中、**新しいウィンドウを開くと前のウィンドウで
  リングが効かない**。追加確認で「**アクティブにすれば動く**」= 非アクティブ窓では効かない、と判明。
- **調査で否定された仮説**: `ring_picker` / `mouse_ring_flick` / `mouse_gesture` は App グローバルで
  bundle swap 対象外なので、片方の窓が置いた状態がもう片方を塞ぐ、と考えた。**塞がっているなら
  アクティブにしても効かないはずなので、この説明は成り立たない。**
- 実際の挙動: 非アクティブ窓では、**最初のクリックがウィンドウのアクティブ化に使われている**と
  見られる。mIV は detached ウィンドウに active / passive の概念を持ち、passive 側は egui の
  キーイベントからアクティブ化を判断する作りになっている。
- **これは不具合というより仕様の選択**。利用者の期待は「見えている別ウィンドウに直接ジェスチャしたい」で、
  現行仕様と食い違っている。
- 扱い: detached リワークの **R2 (状態の集約)** が扱う領域。BA-7 (App グローバル状態の per-window 化) に
  隣接する。**症状パッチ (非アクティブ窓でも右クリックを通す guard) を入れない。**
- 併せて記録: 上記のとおり `ring_picker` / `mouse_ring_flick` / `mouse_gesture` /
  `mouse_ring_grid_target_idx` / `mouse_ring_nav` は**現在 App グローバルで per-window 化されていない**
  (`analysis_mode` など 223 個は swap 対象)。今回の症状の原因ではないが、**複数ウィンドウでの
  リング状態の所有者は未整理**であり、R2 で扱う対象。
- 規模 / 優先度: 仕様判断が先 / P3。

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
| (なし) | | |

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

### 1.125 見開き表示のまま Ctrl+G で補正レイヤーへ入ると、左右の切り替えもマスク編集も効かない — 実機報告 ✅ 修正済み (2026-08-26、実機確認済み)

- 出典: 2026-08-26、利用者報告。見開き表示のページで <kbd>Ctrl</kbd>+<kbd>G</kbd> を押して
  補正レイヤー画面へ入ると、左右ページの切り替えが動かず、マスク編集も含めて操作がまったく
  効かなくなる。<kbd>E</kbd> (消しゴム) と <kbd>Ctrl</kbd>+<kbd>M</kbd> (隠蔽加工) は正常だった。

- **原因**: 編集ツールの canvas 用 transform は**単ページ経路でしか作られない**
  (`draw_fs_spread` は返さない)。そのため編集ツールは「編集する 1 ページ」を単独で表示して
  から始める必要があり、消しゴム / 隠蔽 / 注釈 / 切り取りの 4 つは見開きから入ると単ページへ
  倒す退避を持っていた。**補正レイヤーだけこの退避を持っていなかった。**同じ 10 行が 4 か所に
  写されていて、5 つ目に書き忘れられた形。

- **修正**: 手順を `App::plan_page_edit_pivot` / `enter_page_edit_single_view` /
  `leave_page_edit_single_view` の 1 か所へ集約し、5 つのツールすべてがそこを通す。型名も
  `EraseSpreadCtx` → `PageEditSpreadPivot` (4 ツールで使うのに消しゴムの名前だった)。
  解除の経路はツールごとに複数あり (専用 reset / フルスクリーンを閉じる / ページ送り /
  コンテキスト切替)、局所補正だけで 6 か所ある。復元を各経路へ書くと必ず漏れるので、
  **`reconcile_page_edit_spread_pivots` が毎フレーム「モードが落ちていたら戻す」を 1 か所で
  見る**形にした。

- **左右の切り替え** (2026-08-26 の実機報告で追加): 倒した後は `resolve_spread_pair` が
  Single を返すのでパネルの L/R セレクタが消えていた。`App::page_edit_spread_pair` が退避から
  ペアを返すようにし、補正レイヤーパネルのボタンは押されたページへ表示を移すようにした。
  選択状態は `adjust_spread_target` ではなく**表示中のページ**で決める (倒す前の値が残るため)。

- **テキスト注釈** (2026-08-26 に追加): パネルへ L/R セレクタを足した。注釈はページごとの
  作業セットなので、切り替え時は現在ページを確定してから入り直す。

  ⚠ **最初の実装は「確定」に `reset_text_mode` を使っていて、退避を使い切っていた** (実機報告:
  右ページを選んで抜けると単ページのままになる)。**退場処理は左右切り替えには使えない** ——
  退場は退避を消費して見開きへ戻してしまうので、後で本当に抜けたときに戻る先が無い。

  消しゴムはこれを正しくやっていた。`switch_erase_target_in_spread` は
  `apply_inpaint_only` (「`erase_spread_ctx` を含む消しゴム状態は壊さない (spread 切替を
  意識した版)」とコメントまで付いている) で確定する。**動いている実装が隣にあったのに、
  読まずに書いた。**テキスト側も同じ対に揃えた:

  | | 役割 |
  | --- | --- |
  | `commit_text_page` | このページは確定した (モードも退避も触らない) |
  | `reset_text_mode` | ツールを終わる (退避を使い切って見開きへ戻す) |
  | `switch_text_target_in_spread` | 左右切り替え。パネルは手順を埋め込まずこれを呼ぶ |

  併せて、**入場時**にプレビュー下地 / しっぽ stash / スタンプ埋め込み worker を捨てるようにした。
  どれもページ固有なのに退場時にしか消しておらず、左右切り替えは退場を通らないので残っていた。

  **補正レイヤー側に同じ不具合は無い** (確認済み): 切り替えがピボットするだけで退避に触らず、
  選択レイヤーも `HashMap<page_idx, _>` でページごとに持っている。テキスト側だけが
  フラットなフィールドで持っていたのが差。

### 1.126 フォルダ切替でサイドカーの読み書きが UI スレッドを 8 秒止める — perf-log で実測 ✅ 実装済み (2026-08-26、実機確認済み)

- 出典: 2026-08-26、利用者報告「操作していると何度か応答なしになって非常に重くなる」。
  perf-log (`nav` seq=12) で内訳が取れた。**推測ではなく実測**。

  | 区間 | 時間 | スレッド |
  | --- | --- | --- |
  | `load_folder_end` (`d:\home\scan\comic`, 177 items) | **8334.5ms** | main |
  | └ `sli_sidecar_flush` (dirty サイドカーの書き出し) | **6058.9ms** | main |
  | └ `sli_sidecar_import` (読み直し) | **2180.2ms** | main |
  | └ 残りすべて (scan / sort / DB / catalog / workers) | 合計 90ms 未満 | main |

  [app.rs](../src/app.rs) の `flush_all_sidecars()` と、その後の import。どちらも
  [ui-responsiveness.md](ui-responsiveness.md) §4 が禁じている **UI スレッドからの同期 I/O**。

- **原因は実測できた** (2026-08-26、サイドカー実物を計測)。**遅いのは書き方ではなく、
  書いている中身**:

  | | |
  | --- | --- |
  | `d:\home\scan\comic\mimageviewer.dat` | **699.9 MB** (415 items) |
  | 最大の 1 item | **69 MB** (`20230103_009.pdf::page_9`) |
  | その 69 MB の中身 | `local_adjust_layers[0].mask` |
  | mask の実体 | `{"RasterVector": {"width": 3082, "height": 4486, "alpha": [0,0,0,…]}}` |

  **補正レイヤーのラスターマスクを、1 画素 1 数値の JSON 配列で書いている。**
  3082×4486 = 1382 万要素。全 415 item の内訳は local_adjust_layers 164.6 MB に対し
  adjust 0.3 MB / mask 0.2 MB / comic・export_crop はほぼ 0。

  さらに `serde_json::to_string_pretty` なので、**1382 万要素の配列が 1 行 1 数値**に
  展開される。compact なら 165 MB 相当のものが 700 MB になっているのはこれ。

  **消しゴム / 隠蔽のマスクは同じ問題を持っていない** (`mask` は全体で 178 KB)。あちらは
  `mask_db` へ raw で書いており、サイドカーには参照しか置いていない。**補正レイヤーだけが
  ラスターを JSON へ直接入れている。**

- **3 つ別々の問題が重なっている。**分けて考えること:
  1. ラスターマスクの格納形式 (JSON 配列 → 消しゴムと同じ `mask_db` の raw、または圧縮)
  2. `to_string_pretty` (マスクを外に出せば残りは小さいので、これ単体は些細)
  3. flush / import が UI スレッド同期

  **1 を直せば 2 と 3 の実害はほぼ消える** (165 MB → 数百 KB)。3 の worker 化だけでは
  700 MB の I/O が背景へ移るだけで、メモリと保存時間は残る。

- ⚠ **補正レイヤーはリリース済み** (v1.1.0)。格納形式を変えるなら**移行が必須**
  ([CLAUDE.md](../CLAUDE.md)「永続データ・スキーマ変更時の判断」)。既存の巨大サイドカーを
  読んで新形式へ移す経路が要る。

- **紛らわしい観測**: 同じセッションに 6.5 秒の無反応区間がもう 1 つあるが、そちらは
  **どのスレッドにもイベントが 1 つも無く**、直前の `ui/tail_repaint` が `action=none`。
  = 意図した就寝であって不具合ではない。100〜600ms のヒッチ 28 件は 27 件までが AI
  アップスケールの 1.5 秒以内で、これは別の話 (体感の重さには効くが「応答なし」ではない)。

- **直し方の方針**: [ui-responsiveness.md](ui-responsiveness.md) §2 の worker 化テンプレ
  (`XxxPending { cancel, rx }` + `start_xxx` / `poll_xxx`)。フォルダ切替の完了を待たせずに
  書き出す場合、**切替先で同じサイドカーを読む競合**をどう扱うかが設計の核。

- 規模 / 優先度: 未見積 / P2 (実害が「数秒の無反応」なので体感は大きい)。

#### 方針は決まっている (2026-08-26、利用者判断)

**サイドカー機能は維持し、格納形式だけ変える。**削除は検討したうえで却下した。

検討時に一度「サイドカーにはマスクが入っていないのでは」「フォーマットが用途に合って
いないのでは」という見立てが出たが、**どちらもコードで否定された**。次に読む人が同じ道を
辿らないよう、根拠を残す:

| 種類 | サイドカー内の形式 | 415 item の合計 |
| --- | --- | --- |
| 消しゴム `mask` | **1bit/pixel + deflate + base64** (`SidecarMask`) | 0.2 MB |
| 隠蔽 `conceal` | 同上 | 同程度 |
| 補正レイヤー `local_adjust_layers` | `RasterVectorMask.alpha: Vec<f32>` を **JSON 数値配列のまま** | **164.6 MB** |

3 種ともマスクは保存されており、「別プロファイルでも復元できる」という当初の意図は実装
されている。**フォーマットの問題ではなく、補正レイヤーだけがメモリ上の構造体の serde を
そのまま使っている**のが欠陥。消しゴム側が同じファイルで成立していることが、形式が用途に
適している証拠になっている。

削除しない理由: サイドカーが守っているのは**アプリ外でフォルダごと移動された**場合。
[rename_key_migration.rs](../src/rename_key_migration.rs) は「対象外」節のとおり**アプリ内
リネーム専用**で、そこは埋まらない。[metadata_transfer.rs](../src/metadata_transfer.rs) は
別環境への復元を単体で担えるが**明示操作**なので、エクスポートしていない利用者は救えない。
黙って重くなるのは悪いが、**黙ってデータが消えるほうがさらに悪い**。

`tag_sidecar_backup_enabled` は既定 OFF で、タグは文字列なので容量問題とは無関係。今回を
理由に一緒に削る根拠は無い。

#### 着手順

1. ~~**`RasterVectorMask` の格納形式を圧縮形式にする。**~~ → **完了 (2026-08-26)**
2. ~~flush の worker 化~~ / **import の worker 化は残り** (下記)
3. サイドカーが肥大したときに気づける仕組み (サイズをログか設定画面へ) — **未着手**

#### 1 と 2 の結果 (2026-08-26)

正本は [preset-and-adjustment.md](preset-and-adjustment.md) §9.3 / §9.3.2。

- **格納形式**: 画素ごとの値を `"<tag>:<count>:<base64(deflate(bytes))>"` へ詰める
  ([crates/local-adjust-core/src/mask_codec.rs](../crates/local-adjust-core/src/mask_codec.rs))。
  `serde` のフィールド属性なので、レイヤー配列を持つ**全ストアが同時に**新形式になる
  (`local_adjust.db` / `mimageviewer.dat` / 編集 bundle / メタデータ移送)。
- **量子化は 8bit**。実データの alpha は 0.0 / 1.0 だけ (31.5M 値中 99.95% が 0) だったが、
  `SubjectMask` はマッティングモデルの連続値なので 1bit にはしない。2 値データなら
  deflate が潰すので容量差も出ない。
- **`RasterVectorMask` だけではなかった**。同じ形の per-pixel 配列が `RasterMask.alpha` /
  `SubjectMask.alpha` / `SubjectMask.source_alpha` / `RegionMask.labels` にもある。5 か所まとめて直した。
- **`local_adjust.db` も同じ欠陥で、しかもサイドカーより大きかった** (959 MB / 82 行 /
  最大 1 行 93 MB)。そのページを開くたびに UI スレッドで 93 MB の数値配列をパースしていた。
  `local_adjust_db::repack_legacy_masks` が起動時 worker で移行 + VACUUM する。
- **移行の実測** (利用者の実データのコピー):

  | | 前 | 後 | 時間 |
  | --- | --- | --- | --- |
  | `local_adjust.db` | 959.0 MB | 2.7 MB | 5.2 s (worker、82 行走査 / 69 行書換 / 0 skip) |
  | `mimageviewer.dat` (415 items) | 733.9 MB | 0.6 MB | 読み 0.8s / 書き 0.3s |

- **flush は非同期化**。`SidecarWriter` (専用スレッド 1 本)。まだ届いていない内容は
  pending に残り `SidecarFile::load` がディスクより先に見るので、フォルダ切替が
  書き込み完了を待つ ack は不要になった。
- **定期フラッシュは 5 秒 → 10 分**、判定は「dirty になった時刻」から。通常の保存契機は
  「編集ツールを抜けた時 / フォルダ切替 / アプリ終了」で、定期はクラッシュ・電源断用の保険。
  「編集ツールを抜けた」は `page_edit_tool_owns_canvas` の true → false から導くので、
  ツールが増えても呼び出し側を足さなくてよい。

#### 実機確認 (2026-08-26)

利用者の実環境で移行が走った。ログ:

```
[6.301s] local_adjust: repacked 69 of 82 rows in 5.6s
         (JSON 954.5MB -> 2.6MB, file 959.0MB -> 2.7MB, 0 skipped as changed)
[20.537s] sidecar: rewriting legacy mask arrays (733870900 bytes): d:\home\scan\comic\mimageviewer.dat
```

現物も `local_adjust.db` = 2.7 MB、`d:\home\scan\comic\mimageviewer.dat` = 0.65 MB。
**編集中のひっかかりは解消**、補正レイヤーのマスクも表示を確認。

移行後の DB は全 83 行が読め (**読めなかった行 0**)、マスクの階調は
2 が 32 件 / 190〜242 が 12 件 / **256 が 22 件**。**実データに連続階調のマスクが 22 件ある**ので、
サイドカーのサンプル (0.0 / 1.0 だけ) を見て 1bit を選んでいたら潰していた。

#### 残り

- **import (`SidecarFile::load` + `import_to_dbs`) は UI スレッドのまま。**移行後は
  0.6 MB / 数 ms なので実害は消えたが、§4 チェックリスト違反ではある。ネットワーク
  ドライブや巨大フォルダでは効く。
- **移行の 1 回目だけ UI が止まる** (実測 0.8 秒 / フォルダ)。旧形式を読む経路が
  同期なので。2 回目以降は無い。
- **mtime fast-path に当たり続けるフォルダは移行されない**。実害は disk 容量だけ
  (開かないので遅くもならない)。開けば slow-path に入って移行される。
- `metadata_transfer` の `FORMAT_VERSION` は **上げていない**。上げると `!=` 比較なので
  新版が既存の v7 エクスポートを一切読めなくなる。上げない場合の副作用は「新版で
  書き出した bundle を旧版で import すると layers のパースで失敗する」だけで、
  データが黙って壊れる経路は無い。
- **VACUUM は `local_adjust.db` だけ**。他の DB (tags.db 213MB / fts_meta.db 365MB /
  audio_analysis.db 1.5GB) の肥大は別問題で、未調査。
