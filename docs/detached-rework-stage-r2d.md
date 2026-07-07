# Stage R2d 指示書: passive / ParkedLive 窓の deferred viewport 化 (BA-5 の構造的根治)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) を読むこと。**

- 位置付け: BA-5 (「immediate viewport は親が毎フレーム描かないと死ぬ」) の実害が
  3 件に達したため、R1c の記録事項どおり R4 から前倒しした独立ステージ。
  **対象は非 active 窓 (Parked / ParkedLive) のみ**。active 窓は immediate のまま
  (フル機能の live 描画は R4 で判断)。
- 実装: Codex / 検収: Fable / 実機: 帰宅後バッチに含める
- **Phase 0 (調査) → 設計ノート → 実装** の順。Phase 0 の結果次第でスコープを
  縮める判断があるため、**Phase 0 の報告を先に出して Fable の確認を待つこと**。

## 1. 何が根治されるか

`show_viewport_deferred` の窓は egui がライフタイムを管理し、「親が 1 フレーム
描かなかったら OS 窓破棄」が起きない。これにより:

- resync の discard パス (R1c で gate 済み) が passive 窓を殺せなくなる
- park/resume 遷移の描画順序ミス (R2b F7) で窓が消えることがなくなる
- 「遷移フレームの描画責務」を人間が管理する必要がなくなる

⚠ ただし R1c の resync gate・R2b fix3 の遷移順序・既定サイズ拒否などの既存ガードは
**このステージでは削除しない** (載せ替え完了と実機安定の確認後、ゲート C で判断)。

## 2. Phase 0: 調査 (コードを書く前に報告)

egui 0.33 / eframe 0.33 のソース (~/.cargo/registry) で以下を確定し、設計ノートとして
報告する:

1. **callback の所有制約**: `show_viewport_deferred` の UI callback は
   `Arc<dyn Fn(&Context, ViewportClass) + Send + Sync>` 相当で **App を借用できない**。
   passive 窓の描画に必要なデータ (凍結テクスチャ handle・見開き frozen pages・
   タイトル・close ボタン状態) を `Arc<Mutex<...>>` 等の共有構造に切り出す設計を
   具体化する。App への入力伝達 (クリック復帰要求・close 要求・placement 実測) は
   共有キュー → `App::update` で poll、の一方向にする。
2. **OS 窓生成タイミング**: deferred では OS 窓の生成が `show_viewport_deferred`
   呼び出しと同期しない可能性が高い。**R1 の HWND before/after 差分法が成立するか**
   を確認し、成立しない場合は R1b の未請求窓採用 (消去法) を「deferred callback の
   初回実行時に走らせる」等の代替を設計する (geometry 推定への逆戻りは禁止 = 憲法 1)。
3. **wgpu backend の deferred 対応**: eframe wgpu integration が deferred viewport を
   完全サポートしているか (immediate 専用の制約・既知 issue の有無)。
4. **ParkedLive (native presenter child) との相性**: presenter child HWND の親付けが
   deferred 窓でも同じに扱えるか。懸念があれば **ParkedLive は immediate のまま
   Parked (凍結静止画) だけ deferred 化**するスコープ縮小を提案してよい。
5. **repaint 駆動**: 凍結内容の更新時 (`ctx.request_repaint_of(viewport_id)`) と
   ParkedLive backdrop の要否。
6. **IME**: passive 窓に TextEdit は無いため `update_ime_state` 不要の見込みだが、
   CLAUDE.md の「新しいビューポートでは IME 状態更新」ルールに対する例外理由を
   コード コメントに残す。

## 3. 実装 (Phase 0 承認後)

- `render_detached_image_windows` の immediate 呼び出しを deferred 登録 + 共有状態
  更新に置き換える。ViewportId は現行の `detached_image_window_viewport_id(id)` を
  維持 (identity 不変 = 実窓の作り直しなし)。
- 入力: クリック復帰・close・placement 実測はキュー経由で App に届き、既存の
  遷移 (`transition_detached_window_state`) と runtime.placement 更新に接続する。
  既定サイズ拒否ガードはキュー適用側で従来どおり効かせる。
- hwnd 登録は Phase 0 で確定した方式で runtime に一本化を維持。
- ログ: 窓生成 / 破棄 / callback 初回実行を既存の `state_transition` /
  `registered host` 体系に合わせて出す (実機バッチで検証可能に)。

## 4. テスト

- 共有状態とキューの plumbing 単体テスト (クリック要求→ activation queue、
  placement 実測→ runtime 反映、close 要求→ Closing 遷移)
- 「passive 窓が 1 フレーム描かれなくても runtime から消えない」ことを表す
  ロジックテスト (egui 実窓は headless 不可のため、キュー / 状態層で固定)
- 既存 detached / parked_live / placement テスト全緑
- 実機バッチ項目 (次回): passive 窓存在中の F12 往復・resync 発火・park/resume
  連打で窓が一度も消えない / `registered host` が生成時以外に増えない

## 5. 完了条件

- [ ] Phase 0 設計ノートが報告され、Fable 承認済み
- [ ] passive (および可能なら ParkedLive) 窓が deferred viewport で表示される
- [ ] ViewportId 不変・hwnd 登録が runtime 一本のまま
- [ ] §4 のテスト緑 + full test 緑 + `cargo fmt --check`
- [ ] コミットに `(detached-rework R2d)` を含める
- [ ] `.\scripts\build-release.ps1` で実機バイナリ準備
