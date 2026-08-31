# R-07 + R-14: 補正レイヤーの保存を UI スレッドから外す — 並行セッション用ブリーフ

作成 2026-08-30。**別 worktree の新規セッションがこの文書だけを読んで着手できる**ように書いてある。
master 側の主セッションは細かいバグ修正、`external-tool-launch` worktree は外部プログラム起動を
並行して進めている。ファイルの重なりは §7 を見ること。

## 0. まず作業場所を確認する — master では作業しない

**この作業は専用の worktree で行う。** `C:/home/mimageviewer` (master) には別セッションが
同時に居るので、そこで `src/` を編集すると index と HEAD を取り合うことになる
(実際に 2026-08-30、bare commit が別セッションの編集を巻き込む事故が起きている)。

`git rev-parse --show-toplevel` が `C:/home/mimageviewer` を返したら、**まだ作業を始めず**、
利用者へ worktree の作成を依頼すること。作成コマンドは以下 (利用者が PowerShell で実行する):

```powershell
git worktree add ..\mimageviewer-localadjust -b local-adjust-ownership
robocopy .\vendor ..\mimageviewer-localadjust\vendor /E /XD target vst3sdk /NFL /NDL /NJH /NJS /NP
```

`src/` を触る = 本体パッケージをビルドするので `vendor/` が要る (`build.rs` が必須ファイルを
検査する)。**junction や symlink で共有しない** — worktree 撤収時の再帰削除で main 側の
`vendor/` を消す事故が複数回起きている。robocopy は `target` を除いてビルド成果物 4.2 GB を
落とし、`vst3sdk` は C++ ブリッジを再ビルドしない限り不要。コピーは 800 MB 程度。
robocopy はコピーがあると終了コード 1 を返すが、**これは成功**である。

すでに master 側で編集を始めてしまっていた場合は、**その変更を消さずに** worktree へ移す。
`git checkout --` や `git restore` で master 側を巻き戻すのは最後にすること (未コミットの
変更は git では復元できない)。

worktree の撤収は必ず `.\scripts\safe-worktree-remove.ps1 <path>` を通す。


## 1. 何を直すのか

補正レイヤー (ローカル調整) の編集で、**大きな編集文書を UI スレッドが所有して、複製して、
その場で直列化・圧縮・DB 保存している**。24MP の画像で 1 ストロークあたり 70.6ms、
塗りの毎フレームにさらに 27.7ms かかる (実測値は v3.3.0 レビュー由来)。

レビューでは R-07 と R-14 の 2 件に分かれていたが、**発生源は同じ 1 つの関数**なので
1 件として扱う (根拠: [docs/review-v3.3.0/README.md](../review-v3.3.0/README.md) §10.4)。

**これはテキスト注釈 (comic overlay) の重さとは無関係**である。あちらは別サブシステムで、
別項目 ([next-release-backlog.md](../next-release-backlog.md) §1.0k) として立ててある。
共有コードは無いので、混ぜないこと。

## 2. 着手前に読むもの

必読:

- [CLAUDE.md](../../CLAUDE.md) — 応答は日本語。コミット規律、`cargo fmt`、pre-commit フック
- [docs/README.md](../README.md) と [docs/architecture-overview.md](../architecture-overview.md)
- [docs/async-architecture.md](../async-architecture.md) — worker 追加とキャンセル規約
- [docs/ui-responsiveness.md](../ui-responsiveness.md) — §2 に worker 化の実装テンプレ、§4 にチェックリスト
- [docs/preset-and-adjustment.md](../preset-and-adjustment.md) — 補正の適用順序と保存先
- [docs/next-release-backlog.md](../next-release-backlog.md) §1.0 の R-07 行と、その下の (b) (c) の注記
- [docs/review-v3.3.0/README.md](../review-v3.3.0/README.md) §10.4

**バグ修正の一般原則** (CLAUDE.md) がそのまま効く。症状を消す guard / delay / retry /
silent fallback を根本原因の代わりに入れない。相互排他な状態を bool や `Option` の
有無で表しているなら、分岐を足さずに単一の typed request / state owner へ集約する。

## 3. 発生源 — 1 つの関数に 3 つの複製が集まっている

`App::set_local_adjust_layers_for_idx` ([src/app.rs](../../src/app.rs)、`54485` 付近)。
UI スレッドで順に走る:

1. **直列化 + 圧縮 + DB 書き込み** — `db.set_layers(&key, &layers)`
   ([src/local_adjust_db.rs](../../src/local_adjust_db.rs) `103` 付近) が
   `serde_json::to_string` を呼ぶ。マスクの serde impl が
   [crates/local-adjust-core/src/mask_codec.rs](../../crates/local-adjust-core/src/mask_codec.rs) で
   q8 量子化 → deflate → base64 を行い、その後 SQLite `execute`。**これが R-07 (d)**
2. **サイドカー用の 2 枚目** — `let sidecar_layers = layers.clone();`。全ラスターマスクの deep clone
3. **サイドカー内の 3 枚目** — `with_sidecar_mut` → `SidecarFile::items_mut()`
   ([src/sidecar.rs](../../src/sidecar.rs) `198`) の `Arc::make_mut`。
   writer が前の snapshot を持っている間だけ 1 回複製する。**これが R-14 の残り**

`items` は既に `Arc` 共有になっている (`df245720`) ので、`queue_flush`
([src/sidecar.rs](../../src/sidecar.rs) `536`) 自体は `Arc` を 1 つ渡すだけで複製しない。
残っているのは上の 2 と 3 だけである。**そこは直っている前提で読むこと。**

### 目指す形

レビューが書いた方向は「`(key, 不変スナップショット, generation)` を worker が処理し、
**最新 generation の応答だけを publish する**」。UI は pointer handoff だけを行う。
サイドカーのミラーも同じ所有権に乗せれば、2 と 3 の複製が両方消える。

## 4. 絶対に開け直してはいけない境界 — R-26

`b8cb3ce5` で入った `EditStoreOutcome` / `App::edit_store_write` /
`edit_store_write_succeeded` は、**「DB が開けなかった」を「成功」と区別する**ためのもの。
これが無かったとき、`open().ok()` → `None => Ok(())` が成功に化け、presence を立てて
サイドカーへ書き、次回起動の `import_to_dbs` が「中央が authoritative」としてその
サイドカーを捨てていた。**利用者にはエラーもトーストも出ずに編集が消えていた。**

保存を worker へ出すと、**成否が後から返る**。つまり:

- 「durable な保存が成功したときだけ presence を立ててサイドカーを書く」という現在の
  順序が、そのままでは成り立たなくなる
- 失敗したときの `set_local_adjust_layers_for_idx_memory_only` へのフォールバック
  (画面には残すが durable mirror は書かない) も、非同期では別の形になる

**この再設計が本作業の中心**である。「UI から重い処理を追い出す」だけの付け替えでは
R-26 を開け直す。backlog にも「別々に考えない」と明記してある。

先に、失敗経路を含めた**状態遷移テスト**を書いてから実装に入ること。

## 5. 同じ作業に含める 2 件

### (b) 図形のキー移動 / 回転がキーリピートごとに全文書を保存する

リピート 1 回ごとに 70ms + undo 文書 2 つ。**見積もりを 1 度外している**ので注意:

- キーを離したときだけではなく、**ページ移動・モード終了・フルスクリーン終了など
  編集状態を畳む全箇所**で確定が要る
- ブラシは破棄でよいが、キー移動は完了した編集なので破棄できない
- 取りこぼし防止の**監査テスト**を必ず付ける

### (c) 塗りの毎フレームに文書 1 複製が残る — 契約変更が要る

**この複製は無駄ではない。正しさを支えている。** 16 個の closure が通る
`mutate_local_adjust_layer_from_canvas_impl`
([src/ui_adjustment_panel.rs](../../src/ui_adjustment_panel.rs) `9566`) は、文書を丸ごと
複製してから closure に渡し、closure が `false` を返したら複製ごと捨てる。つまり
**「`false` を返した = 文書は変わっていない」という不変条件を、複製の破棄そのものが
作っている**。その場で書き換える形にすると、`false` を返した編集の書き込みが残る。

書き込みは散らばっておらず、`local_adjust_target_raster_vector_mask_mut`
([src/ui_adjustment_panel.rs](../../src/ui_adjustment_panel.rs) `1939`) 1 か所に集まっている。
`false` の前に文書を書く経路は 3 つあり、**`create` 引数で止まるのは 1 つ目だけ** (確認済み):

1. `create = true` のときスロットへ空マスクを作る (`*slot = Some(..empty..)`)
2. **`create` に関係なく** `Raster` を `RasterVector` へ昇格する (`mem::replace` の分岐)
3. **`create` に関係なく** `resize_to(width, height)` を呼ぶ (Base 経路と Override 経路の両方)

その後で呼び出し側は `source.size != [mask.width, mask.height]` や「塗ったが 1 画素も
変わらなかった」で `false` を返せる。**`create=false` で呼んでいる経路も安全ではない。**

集約されているのは朗報 (直す先が 1 つ) だが、必要なのは**契約の変更**であって複製の削除
ではない。方向: 材質化・昇格・リサイズを「それ自体が変更である」ものとして closure の外へ
出し、`mutate` は変更の有無だけを返す純粋な編集にする。そのうえで seam は文書を
**move** して (複製せずに) 渡す。

## 6. 進め方

1. **測る**。直す前に、24MP 画像で 1 ストローク / 1 フレームのどこに時間が行っているかを
   `--perf-log` + `python scripts/analyze_perf.py` で確認する。backlog の 70.6ms / 27.7ms は
   v3.3.0 時点の数字なので、現在値を自分で取り直すこと
2. **(c) の契約変更を先にやる**。ここが一番設計的で、他の 3 件はこれが済むと素直になる
3. **(d) + R-14 の worker 化**。R-26 の境界を先に設計し、失敗経路のテストを書いてから
4. **(b) の確定タイミング**。畳む全箇所を列挙してから

各段でコミットする。**次の段へ進む前に必ずコミットする** (混ざると切り分け不能になる)。

## 7. 他セッションとの分担 — ファイルの重なり

| | 対象ファイル |
| --- | --- |
| **この作業** | `src/ui_adjustment_panel.rs` / `src/local_adjust_db.rs` / `src/sidecar.rs` / `src/edit_bundle.rs` / `crates/local-adjust-core/` / `src/app.rs` の 54485 付近 |
| `external-tool-launch` worktree | `src/context_menu_model.rs` / `src/external_tool.rs` / `src/open_with.rs` / `src/native_context_menu.rs` / `src/settings*.rs` / `src/ui_dialogs/` / `src/ui_main.rs` / `src/ui_fullscreen.rs` / `src/app.rs` の 10701-15786・26212・60220-60549・66930-67594 |
| master (主セッション) | 細かいバグ修正。着手前に確認すること |

**重なるのは `src/app.rs` だけ**で、行域は離れている。worktree を分けるので index と HEAD は
独立しており、CLAUDE.md の「1 つの作業ツリーを共有する場合の規律」(pathspec commit 等) は
不要。**merge のときだけ** `src/app.rs` で conflict しうる。

`src/app.rs` の `App` 構造体へフィールドを足す場合は、末尾ではなく**関連する既存
フィールドの隣**へ置くこと (両ブランチが末尾へ足すと必ず conflict する)。

## 8. detached の凍結ルール

この作業は detached viewer の述語や viewport 経路に**触らない見込み**だが、もし触ることに
なったら CLAUDE.md「Detached viewer リワーク中のルール」に従う (症状パッチ禁止、構造的修正は
ClaudeCode と Codex の双方が「これは症状パッチではない」ことに合意、
[docs/detached-rework-plan.md](../detached-rework-plan.md) §11 へ記録)。

## 9. テスト

- 状態遷移テスト (保存の成功 / DB が開けない / 書き込み失敗 / 世代が追い越された)
- (c) は「`false` を返す編集が文書を変えていないこと」を**新しい契約の下で**検証する。
  挙動不変のリファクタなので、旧コードで落ちるテストを求めない。**変異させた新コードで
  落ちること**を確かめる
- (b) は編集状態を畳む全経路の監査テスト
- 実行は `cargo test -p mimageviewer --lib` (`--bin mimageviewer-core` では 0 tests)。
  仕上げに `.\scripts\test-full.ps1`

## 10. 完了の定義

- 24MP でストローク中に UI スレッドが直列化・圧縮・DB 書き込みを行わない (perf log で確認)
- 塗りの毎フレーム複製が無い
- キーリピート中に全文書保存が走らない
- R-26 の保証が保たれている (durable に書けていないのに presence を立てない)
- `docs/next-release-backlog.md` の R-07 / R-14 行を更新し、
  `docs/preset-and-adjustment.md` と `docs/async-architecture.md` に設計変更を反映
- ネイティブ挙動の実機確認が要るなら `.\scripts\build-dev.ps1` まで用意して利用者へ渡す
  (**エージェント自身は起動しない**。CLAUDE.md「検証起動時の設定データ保護」)
