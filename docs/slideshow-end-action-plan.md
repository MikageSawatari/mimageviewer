# スライドショー末尾動作 + 動画スキップ + フォルダ内ナビ継続 実装プラン

## 背景

フルスクリーンのスライドショーには以下の不整合・要望があった:

1. **ナビ操作によって継続/停止が割れている**: ホイール送り・左クリック送りはスライドショーを継続するが、矢印キー / Home / End はスライドショーを停止する。どちらもフォルダ内移動なので挙動が割れているのは不自然。「一部だけスキップしてスライドショーを続けたい」用途で困る。
2. **末尾動作が固定でループのみ**: スライドショーは初版から「フォルダ内で末尾→先頭へループ」する実装で、設定できない。
3. **動画で停止する**: 送りが動画アイテムに到達すると `sync_slideshow_anchor_for_frame` がスライドショーを停止する。動画混在フォルダだと「ループしない」ように見える。

本プランは以下を実装する (ユーザー合意済み):

- **末尾動作を設定3択化**: フォルダ内ループ (既定) / 次のフォルダへ進む / 末尾で停止
- **動画はスキップして継続** (固定挙動、設定化しない)
- **フォルダ内ナビ (矢印 / Home / End) はスライドショーを止めない** (ホイール/クリックと統一)

## 確定仕様

### 末尾動作 3択 (`SlideshowEndAction`)

| モード | 末尾到達時の挙動 |
| --- | --- |
| `LoopFolder` (既定) | 現状どおりフォルダ内の先頭画像系アイテムへ折り返し |
| `NextFolder` | **手動 Ctrl+↓ と同じ skip-walk** で次フォルダへ。ただし判定述語は「静止画あり」(動画のみ・画像なしフォルダは飛ばす)。`skip_limit` 以内に静止画フォルダが見つかればそこで継続、見つからなければ停止 |
| `Stop` | 末尾で停止 (現状の「画像系が一つも無い」フォールバックと同じ停止) |

### 動画スキップ (固定)

- スライドショーの送り探索から `GridItem::Video` を除外する。
- 手動で動画に来た場合 (ホイール等) も、スライドショー実行中なら次の間隔で動画を飛ばして進む (= 動画で停止しない)。

### フォルダ内ナビ継続

- 矢印キー (↑↓←→) / Home / End がスライドショーを停止しないようにする。ホイール送り・左クリック送りは元から停止コードが無いので、これで全フォルダ内ナビが「スキップしても継続」に揃う。

## 影響ファイル

| ファイル | 変更内容 |
| --- | --- |
| `src/settings.rs` | `SlideshowEndAction` enum 追加 + `slideshow_end_action` フィールド追加 |
| `src/folder_tree.rs` | `navigate_folder_with_skip` を述語注入可能に一般化 + `folder_has_still_image` 追加 |
| `src/app.rs` | `FolderNavMode::SlideshowNext` 追加 + `spawn_folder_nav` 述語分岐 + `apply_folder_nav_result` 分岐 + `fs_nav_after_pdf_enumerate` を `DeferredFsReopen` 型に拡張 + `reopen_fullscreen_after_folder_nav_load` の resume 配線 (resume は SlideshowNext mode 由来、自由 bool は持たない) |
| `src/ui_fullscreen.rs` | 末尾3択分岐 / 動画スキップ送り / 矢印・Home・End の停止コード削除 / 動画停止コード削除 |
| `src/ui_helpers.rs` | スライドショー送り用の「動画を除外する」隣接探索 + ユニットテスト |
| `src/ui_dialogs/preferences/pages.rs` | `page_slideshow` に3択ラジオ + 注記 |
| `docs/spec.md`, `docs/keymap-spec.md` | 仕様反映 |
| `htdocs/mimageviewer/manual/settings.html`, `index.html` | マニュアル・製品ページ反映 |

## リリース状態の判断 (永続データ)

- スライドショー機能自体はリリース済み (v0.3+) だが、本プランで追加する `slideshow_end_action` は **新規フィールド**。`#[serde(default)]` で既定 `LoopFolder` (= 現状挙動) になるので **マイグレーション不要**。既存ユーザーの設定ファイルに無くても安全に読める。
- コミットメッセージに「新規フィールド・移行不要」と明記する。

## 詳細設計

### 1. `src/settings.rs` — 設定モデル

`VideoLoopMode` ([settings.rs:1332](../src/settings.rs)) と同じ enum パターンで追加:

> 行番号は本プラン作成時点の概算。実装時に最新を確認すること
> (`VideoLoopMode` は 1338 付近)。`settings.rs` は `serde::Serialize` /
> `serde::Deserialize` を **完全修飾** で使う慣習なので、derive もそれに合わせる
> (ファイル先頭で `use serde::...` していないため)。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SlideshowEndAction {
    #[default]
    LoopFolder,
    NextFolder,
    Stop,
}

impl SlideshowEndAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::LoopFolder => "フォルダ内でループ",
            Self::NextFolder => "次のフォルダへ進む",
            Self::Stop => "最後で停止",
        }
    }
}
```

`Settings` 構造体の `slideshow_interval_secs` ([settings.rs:729](../src/settings.rs)) の隣に追加:

```rust
#[serde(default)]
pub slideshow_end_action: SlideshowEndAction,
```

`Default for Settings` impl ([settings.rs:1682](../src/settings.rs) 付近) にも
`slideshow_end_action: SlideshowEndAction::default(),` を追加する。

### 2. `src/folder_tree.rs` — 述語注入 + 静止画述語

`navigate_folder_with_skip` ([folder_tree.rs:197](../src/folder_tree.rs)) は現在 `folder_should_stop` を
ハードコードしている ([folder_tree.rs:220](../src/folder_tree.rs))。これを **述語クロージャを受け取る形に一般化**する:

```rust
pub fn navigate_folder_with_skip<F, S>(
    start: &Path,
    nav_fn: F,
    should_stop: S,            // ← 追加
    skip_limit: usize,
    cancel: Option<&AtomicBool>,
) -> Option<FolderNavOutcome>
where
    F: Fn(&Path) -> Option<PathBuf>,
    S: Fn(&Path, Option<&AtomicBool>) -> bool,
{
    // ... 内部の folder_should_stop(&candidate, cancel) を should_stop(&candidate, cancel) に置換
}
```

既存呼び出し (manual Ctrl+↑↓) は `folder_should_stop` をそのまま渡す。

スライドショー用の静止画述語を追加 (`folder_should_stop` から動画 clause を外しただけ):

```rust
/// スライドショーの次フォルダ判定: 静止画系コンテンツがあるか。
/// folder_should_stop と同じだが、動画拡張子は「コンテンツあり」と数えない。
pub fn folder_has_still_image(path: &Path, cancel: Option<&AtomicBool>) -> bool {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { return false; }
    if path.is_file() {
        if !is_virtual_folder(path) { return false; }
        let ext = path.extension().and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase()).unwrap_or_default();
        return match ext.as_str() {
            "pdf" => true,
            "zip" => crate::zip_loader::first_image_entry(path, cancel).is_some(),
            _ => false,
        };
    }
    let entries = match std::fs::read_dir(path) { Ok(rd) => rd, Err(_) => return false };
    for e in entries.flatten() {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { return false; }
        let p = e.path();
        if is_apple_double(&p) { continue; }
        if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            // 動画拡張子は数えない (ここが folder_should_stop との唯一の差)
            if is_recognized_image_ext(&ext.to_lowercase()) { return true; }
        }
    }
    false
}
```

> 注: `folder_should_stop` と本関数は重複が多いので、共通部分を
> `fn folder_qualifies(path, cancel, include_video: bool)` のような内部ヘルパーに
> 切り出して 2 つの公開関数から呼ぶ形でも良い (DRY)。実装時に判断。

### 3. `src/app.rs` — フォルダ移動経路

#### 3.1 `FolderNavMode` に variant 追加

```rust
pub(crate) enum FolderNavMode {
    Grid,
    Fullscreen,
    Favsearch { root: PathBuf, fullscreen: bool },
    /// スライドショー自動送りの次フォルダ。Fullscreen と似るが
    /// (a) 述語が folder_has_still_image (動画除外)、
    /// (b) 着地後にスライドショーを再開する。
    SlideshowNext,
}
```

- `perf_tag()` ([app.rs:685](../src/app.rs)) に `SlideshowNext => "slideshow_next"` を追加。
- `folder_nav_mode_same_kind` ([app.rs:696](../src/app.rs)) には **`SlideshowNext` の同種アームを追加しない** (Codex P2)。
  `_ => false` に落として「連打累積 (`pending_folder_nav_steps`) + `chain_folder_nav_if_pending` の
  追加ホップ」を構造的に防ぐ。SlideshowNext 発火は `slideshow_playing=false` + `fs_nav_is_locked()`
  ガードで単発になるので、累積する必要が無く、累積すると意図しない多段フォルダ送りになる。

#### 3.2 `spawn_folder_nav` で述語を選択

`spawn_folder_nav` ([app.rs:12107](../src/app.rs)) の `navigate_folder_with_skip` 呼び出し
([app.rs:12143-12157](../src/app.rs)) で、mode に応じて述語を渡す:

```rust
let use_still_image_pred = matches!(mode, FolderNavMode::SlideshowNext);
// forward/back それぞれで:
navigate_folder_with_skip(
    &current,
    |p| next_folder_dfs(p, tree_opts),
    move |p, c| if use_still_image_pred {
        crate::folder_tree::folder_has_still_image(p, c)
    } else {
        crate::folder_tree::folder_should_stop(p, c)
    },
    skip_limit,
    Some(&cancel_w),
)
```

> SlideshowNext は forward 固定 (`forward=true`) で発火する想定 (スライドショーは前進のみ)。

#### 3.3 `apply_folder_nav_result` の SlideshowNext 分岐

`apply_folder_nav_result` ([app.rs:12353](../src/app.rs)) の各所で `FolderNavMode::Fullscreen`
を扱っているのと並行して `SlideshowNext` を処理する:

- **DFS 末端 (`dfs_empty`)** ([app.rs:12443](../src/app.rs)) / **`hit_image_folder=false`**
  ([app.rs:12475](../src/app.rs)): SlideshowNext のときは「次に静止画フォルダが無い」ので
  **スライドショーを停止** (`slideshow_playing=false`, `slideshow_anchor_idx=None`)。resume は
  mode 由来なので、ここで reopen しなければ自動的に再開されない。
  境界ヒントは出さない (またはスライドショー終了の控えめなトースト)。`release_fs_nav_lock()` は同様に呼ぶ。
- **`hit_image_folder=true`** ([app.rs:12494](../src/app.rs) の match): Fullscreen と同様に
  `close_fullscreen` → `load_folder_with_scan` → `reopen_fullscreen_after_folder_nav_load`。
  `SlideshowNext` も同じ処理でよいので `Fullscreen | SlideshowNext` で束ねるか、専用アームを足す。

> `close_fullscreen` ([app.rs:15872](../src/app.rs)) が `slideshow_playing=false` にするため、
> 再開は次節のフラグで行う。

#### 3.4 復帰は「適用される nav の mode」から導出する (Codex P1 反映)

⚠️ **自由な `bool` フラグは別 nav に漏れる** (Codex 指摘)。SlideshowNext 発火後、
結果適用前にユーザーが手動 Ctrl+↓ / Esc / 別ナビでキャンセル・置換すると、bool が
立ったまま残り、置換先の reopen で誤ってスライドショーを再開してしまう。

そこで **復帰は `FolderNavMode::SlideshowNext` から導出**する (フラグを mode に紐付ける):

- `reopen_fullscreen_after_folder_nav_load` ([app.rs:12298](../src/app.rs)) に
  `resume_slideshow: bool` 引数を足し、呼び出し元 (`apply_folder_nav_result` の
  SlideshowNext 分岐) が `matches!(result.mode, FolderNavMode::SlideshowNext)` を渡す。
  `open_fullscreen(new_idx)` 直後 ([app.rs:12319](../src/app.rs)) で **静止画 target が開けたときだけ**:
  ```rust
  if resume_slideshow {
      self.slideshow_playing = true;
      self.schedule_next_slideshow_from_now();
  }
  ```
- **deferred enumerate 経路の型を拡張**: ZIP/PDF enumerate 待ちの deferred reopen は現在
  `fs_nav_after_pdf_enumerate: Option<bool>` ([app.rs:6902](../src/app.rs), [app.rs:7328](../src/app.rs))
  に **forward (bool) しか持っていない**。ここに mode/resume 情報が無いと SlideshowNext の
  resume が deferred 経路で落ちる。`Option<bool>` を
  `Option<DeferredFsReopen { forward: bool, resume_slideshow: bool }>` のような型に拡張し、
  deferred reopen が最終的に `open_fullscreen` する箇所でも同じ resume 判定を通す。
- **target が無い else 分岐** ([app.rs:12338](../src/app.rs), visible 空) → 再開せず終了
  (slideshow は既に false のまま)。
- **`enumerate_defer` 早期 return** ([app.rs:12304](../src/app.rs)) → 上記の型拡張で resume を
  持ち越す。

> この方式なら自由 bool が無いので「別 nav への漏れ」が構造的に起きない。SlideshowNext
> 以外の result/deferred は resume=false で reopen する。手動 Ctrl+↓ が SlideshowNext を
> 置換した場合、その result の mode は Fullscreen なので再開しない。

#### 3.5 スライドショー再開時の最初のアイテム (Codex P2: 必須)

`find_fullscreen_nav_target` ([app.rs:12617](../src/app.rs)) は先頭の画像系を返すが **Video を含む**
([app.rs:12629](../src/app.rs))。NextFolder で着地したフォルダの先頭が動画 (例: `video.mp4` が
`image.jpg` より前にソートされる) だと、動画停止コードを撤去した後は **動画が満タンの間隔だけ
表示され、autoplay 設定次第では音声まで鳴る** (Codex P2)。

そのため **SlideshowNext の再開 (deferred reopen 含む) は静止画のみ target を必須**にする:

- `reopen_fullscreen_after_folder_nav_load` に「静止画のみ (Image/ZipImage/PdfPage、Video 除外)
  で先頭を選ぶ」経路を足す。`resume_slideshow=true` のときはこちらを使う。
- 静止画 target が見つからなければ (フィルタ後に visible 静止画ゼロ) **動画を開かず停止**して
  resume を打ち切る。
- `find_fullscreen_nav_target` 内の `is_image_like` ([app.rs:12625](../src/app.rs)) は Video 込みで
  既存挙動を変えない。SlideshowNext 専用に Video 除外版 closure を用意する
  (`find_fullscreen_nav_target` を `include_video: bool` でパラメータ化するのが簡潔)。

### 4. `src/ui_fullscreen.rs` — スライドショー本体

#### 4.1 末尾3択分岐

`handle_fs_navigation` のスライドショータイマー ([ui_fullscreen.rs:4586-4621](../src/ui_fullscreen.rs)):
末尾フォールバック ([ui_fullscreen.rs:4596-4621](../src/ui_fullscreen.rs)) を `slideshow_end_action` で分岐:

```rust
let next = adjacent_slideshow_idx(&self.items, &self.visible_indices, cur, slide_delta); // §4.2
match next {
    Some(idx) => { /* 従来どおり前進 (advance=true) */ }
    None => match self.settings.slideshow_end_action {
        SlideshowEndAction::LoopFolder => {
            // 現状の or_else: 先頭の静止画系アイテム (Video 除外) へ折り返し
        }
        SlideshowEndAction::Stop => {
            self.slideshow_playing = false;
            self.slideshow_anchor_idx = None;
        }
        SlideshowEndAction::NextFolder => {
            // Codex P1: 発火と同時に slideshow_playing=false にして timer/sync の
            // 再入を止める (anchor=None だけでは sync_slideshow_anchor_for_frame が
            // 旧フレームで再アンカーしてしまう)。resume は SlideshowNext mode 経由
            // (§3.4) で行うので、ここで false にしても復帰できる。
            // nav ロック中なら何もしない (= 二重発火 no-op)。
            if !self.fs_nav_is_locked() {
                if let Some(folder) = self.current_folder.clone() {
                    // 検索コンテキストでは「次フォルダ」概念が無い → ループにフォールバック。
                    let next_folder_ok = !self.global_search.active
                        && !self.favsearch.active
                        && !self.show_search_bar;
                    if next_folder_ok {
                        self.slideshow_playing = false;
                        self.slideshow_anchor_idx = None;
                        self.capture_fs_nav_holdover(fs_idx); // nav ロック取得 (Ctrl+↑↓ と同じ)
                        self.start_folder_nav(folder, true, crate::app::FolderNavMode::SlideshowNext);
                    } else {
                        // 検索ビュー等: ループにフォールバック (LoopFolder と同じ折り返し)
                        // → 下の LoopFolder 分岐と同じ処理を呼ぶ共通関数にする。
                    }
                } else {
                    // current_folder が無い (検索アグリゲート等) → 停止 (他の停止経路と揃えて
                    // anchor もクリア)。
                    self.slideshow_playing = false;
                    self.slideshow_anchor_idx = None;
                }
            }
        }
    }
}
```

設計上の要点 (Codex P1):

- **発火時に `slideshow_playing=false`** にする。これで `if self.slideshow_playing && !close_fs`
  のタイマー ([ui_fullscreen.rs:4578](../src/ui_fullscreen.rs)) も `sync_slideshow_anchor_for_frame`
  の `if !self.slideshow_playing { return; }` ([ui_fullscreen.rs:4329](../src/ui_fullscreen.rs)) も
  早期 return するので、in-flight 中の再アンカー・再発火が起きない。`anchor=None` 単独に頼らない。
- **`capture_fs_nav_holdover(fs_idx)`** を呼んで Ctrl+↑↓ と同じ nav ロック (`fs_nav_locked_gen`) を
  取得する。`start_folder_nav` を直接呼ぶだけでは `handle_fullscreen_ctrl_nav_context` が取る
  ロックを取らないため (Codex 指摘)。これで連打・重複発火も `fs_nav_is_locked()` で no-op になる。
- **検索コンテキスト** (`global_search.active` / `favsearch.active` / `show_search_bar`) では
  NextFolder を **LoopFolder にフォールバック** (折り返し)。`handle_fullscreen_ctrl_nav_context`
  がこれらで no-op ヒントを出す ([ui_fullscreen.rs:4435-4470](../src/ui_fullscreen.rs)) のと同様に、
  「次フォルダ」概念が無いため。LoopFolder の折り返し処理は共通ヘルパーに切り出して両分岐から呼ぶ。

#### 4.2 動画スキップ送り (`adjacent_slideshow_idx`)

`adjacent_navigable_idx` ([ui_helpers.rs:726](../src/ui_helpers.rs)) は Video を含むため、
スライドショー専用に **Video を除外する** 隣接探索を `src/ui_helpers.rs` に追加:

```rust
/// スライドショー送り用。adjacent_navigable_idx と同じだが GridItem::Video を除外する。
pub fn adjacent_slideshow_idx(
    items: &[GridItem], visible_indices: &[usize], current: usize, delta: i32,
) -> Option<usize> {
    // adjacent_navigable_idx の nav_indices フィルタから Video を抜いた版。
}
```

**LoopFolder の折り返し target**: 既存実装は先頭の `Image | ZipImage | PdfPage` を選ぶ。
loop target は画像系のみなので Video も自然に除外される。

スライドショーの自動送り ([ui_fullscreen.rs:4589-4595](../src/ui_fullscreen.rs)) は
`adjacent_slideshow_idx` (Video 除外) を使う。
**手動ナビ (矢印/ホイール/クリック) は従来どおり `adjacent_navigable_idx`** (動画にも止まれる)。

#### 4.3 動画停止コードの撤去

`sync_slideshow_anchor_for_frame` ([ui_fullscreen.rs:4332-4336](../src/ui_fullscreen.rs)) の
`if state.is_video { slideshow_playing=false; ... }` を撤去する。
代わりに「スライドショー実行中に現在が動画なら、ready 扱いにして通常間隔で次へ送る」挙動にする
(動画フレームの ready 判定は `current_slideshow_frame_ready` がサムネ/テクスチャで true を返すので、
タイマーが回り §4.2 の送りで動画を飛ばす)。

> 動画の再生 (音声含む) がスライドショー中に始まらないこと、HUD が出ないことを実機確認。

#### 4.4 フォルダ内ナビ継続

`handle_fs_key` のナビ分岐から以下の `self.slideshow_playing = false;` を **削除**:

- 矢印キー nav_next ([ui_fullscreen.rs:3655](../src/ui_fullscreen.rs))
- 矢印キー nav_prev ([ui_fullscreen.rs:3659](../src/ui_fullscreen.rs))
- Home jump_to ([ui_fullscreen.rs:3674](../src/ui_fullscreen.rs))
- End jump_to ([ui_fullscreen.rs:3689](../src/ui_fullscreen.rs))

ホイール ([ui_fullscreen.rs:4042](../src/ui_fullscreen.rs)) / 左クリック ([ui_fullscreen.rs:4239](../src/ui_fullscreen.rs))
は元から停止コードが無いので変更不要。これで全フォルダ内ナビが継続に統一される。

### 5. `src/ui_dialogs/preferences/pages.rs` — 設定 UI

`page_slideshow` ([pages.rs:210](../src/ui_dialogs/preferences/pages.rs)) の間隔スライダー下に
ラジオを追加:

```rust
ui.add_space(8.0);
ui.label("フォルダの最後まで進んだら:");
ui.radio_value(&mut s.slideshow_end_action, SlideshowEndAction::LoopFolder, "フォルダ内でループ");
ui.radio_value(&mut s.slideshow_end_action, SlideshowEndAction::NextFolder, "次のフォルダへ進む");
ui.radio_value(&mut s.slideshow_end_action, SlideshowEndAction::Stop, "最後で停止");
ui.add_space(2.0);
ui.label(RichText::new("「次のフォルダへ進む」は、移動先に画像が1枚も無ければ停止します").size(11.0).color(gray));
ui.label(RichText::new("スライドショー中、動画は自動でスキップします").size(11.0).color(gray));
```

- 設定変更後に `s.save()` 相当が呼ばれる導線を確認 (preferences は閉じる/変更時に保存しているはず)。
- グリフ規約: 使う文字は ASCII + 日本語のみ (絵文字・環境依存記号なし)。

## エッジケース

| ケース | 期待挙動 |
| --- | --- |
| フォルダに静止画1枚のみ + 動画複数 (LoopFolder) | 静止画1枚を間隔ごとに再表示 (動画は出ない) |
| フォルダ末尾が動画 (どのモードでも) | 動画で止まらず、モードに従って折り返し/次フォルダ/停止 |
| NextFolder で次が動画のみフォルダ | skip-walk が飛ばして次の静止画フォルダへ。skip_limit 内に無ければ停止 |
| NextFolder で DFS 末端 | スライドショー停止、現在画像に留まる |
| NextFolder 中に検索 (Ctrl+F/G/Fav) がアクティブ | 「次フォルダ」概念が無いので **LoopFolder にフォールバック** (折り返し、§4.1) |
| NextFolder の in-flight 中にスライドショータイマー再発火 | 発火時に `slideshow_playing=false` にしてタイマー/sync が早期 return + `fs_nav_is_locked` で再発火しない (§4.1) |
| NextFolder 着地フォルダの先頭が動画 | §3.5 で **静止画のみ target を開く** (動画は開かない)。静止画 target が無ければ停止 |
| 設定ファイルに `slideshow_end_action` が無い旧設定 | serde default で LoopFolder (移行不要) |

## テスト計画

- **ユニット (`src/ui_helpers.rs`)**: `adjacent_slideshow_idx` が Video を飛ばし、境界で None になること。`adjacent_navigable_idx` の既存テストと並べる。
- **ユニット (`src/folder_tree.rs`)**: `folder_has_still_image` が 画像あり=true / 動画のみ=false / 空=false / ZIP画像あり=true / PDF=true ([folder_tree.rs:632](../src/folder_tree.rs) 付近の既存テストに追加)。`navigate_folder_with_skip` の述語注入版が既存テストを壊さないこと。
- **ユニット (`src/settings.rs`)**: `slideshow_end_action` が serde default で LoopFolder になること。
- **実機 (`docs/e2e-smoke-test.md` 追記)**:
  - LoopFolder で末尾→先頭ループ、動画スキップ。
  - NextFolder で次フォルダへ継続 / 動画のみフォルダを飛ばす / 末端で停止。
  - 矢印/Home/End/ホイール/クリックいずれでもスライドショー継続。
  - 動画到達でスライドショーが止まらず、動画の音が鳴らない。
- **perf**: NextFolder は既存 DFS ワーカー経路なので追加計装は最小。UI スレッドで read_dir しないことを確認。

## ドキュメント更新 (実装と同時)

- `docs/spec.md` ([spec.md:154](spec.md), 835 行) — 末尾3択・動画スキップ・フォルダ内ナビ継続。
- `docs/keymap-spec.md` ([keymap-spec.md:31](keymap-spec.md), 44-45, 48) — 矢印/Home/End がスライドショー中も継続する旨。
- `htdocs/mimageviewer/manual/settings.html` / `index.html` — 設定説明。**バージョンタグ・内部用語を出さない** (CLAUDE.md 記述方針)。

## 非対象・制約

- 動画スキップは設定化しない (固定挙動)。
- NextFolder は **前進のみ** (スライドショーは前進方向)。
- フォルダ別のモード保存はしない (グローバル設定 1 個)。
- 手動 Ctrl+↑↓ の挙動 (`folder_should_stop` = 動画込み, skip-walk) は変更しない。スライドショー専用に述語を差し替えるだけ。
- UI 応答性: NextFolder の read_dir / ZIP 走査は既存の非同期ワーカー (`spawn_folder_nav`) 経由のまま。UI スレッド同期 I/O を新規に増やさない。
