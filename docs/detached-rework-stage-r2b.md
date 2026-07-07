# Stage R2b 指示書: 状態遷移の reducer 化 + メディア live-park (ParkedLive)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) と §6-3 (live-park 仕様) を読むこと。**

- 前提: R2a (807dbbf7) の runtime shadow state が入っていること。
- 実装: Codex / 検収: Fable / 実機 smoke: ユーザー (R2b 完了後、R2 のメイン gate)
- **Part 1 と Part 2 は必ず別コミットにする** (回帰の切り分けのため)。

## Part 1: state を挙動の SSoT に昇格し、重複フラグを削除する

R2a で記録専用だった `runtime.state / pinned / linked` を判定の正とし、
同じ情報を重複表現している散在フィールドを削除する。

### 進め方

1. 対象候補を列挙する。少なくとも以下を検討する (実際の削除可否はコードで確認):
   - `detached_viewer_pin_active` → `runtime.pinned`
   - `detached_viewer_independent_active` → `runtime.linked == false`
   - `ActiveDetachedSession.closing` → `state == Closing`
   - `detached_image_window_close_pending` → `state == Closing` の窓の集合
   - `DetachedImageWindowSnapshot` 内の pin/focus 系の重複フィールド
2. 各フィールドについて **読者を全部 runtime 参照へ移行してからフィールドを削除**。
   1 フィールドずつ機械的に。
3. runtime に還元できない意味を持つフィールド (例: bundle 側の pin は独自バンドル
   属性として意味が別) は**無理に消さず**、完了報告の対応表に「残す + 理由」を書く。
4. 完了報告に対応表: `旧フィールド → 新しい参照元 / 削除 or 残置 (理由)`。

### 不変条件

- state の書き込みは引き続き `transition_detached_window_state` 1 本のみ。
- 遷移の副作用 (focus 要求・bundle swap・close コマンド送信) は既存関数のままで
  よいが、**判定**は state を読む形へ寄せる。
- `state_transition_unexpected` 警告が既存テストで増えないこと (増える場合は遷移の
  張り忘れなので直す)。

## Part 2: メディア live-park (`ParkedLive`)

仕様の正はプラン §6-3。要約: 再生中の動画 / 音声 (音声モード含む) の detached 窓は、
別の窓のアクティブ化やメインからの別画像 open で**閉じず、再生を継続したまま
非アクティブ化**する。

### 2.1 実装の前提 (Fable 調査済みの構造)

- `VideoPlayer` は `fs_cache` (`FsCacheEntry::Video`) 内にあり、**bundle と一緒に
  pause で退避できる**。デコード worker・音声 (cpal)・native presenter の present
  loop は独立スレッドなので、egui 側の文脈を止めても再生自体は続く。
- 現行の park は「表示テクスチャを凍結して snapshot 化」であり、動画はテクスチャが
  無く失敗 → `close_legacy_detached` で閉じている ([app.rs:24210](../src/app.rs) 付近)。

### 2.2 実装内容

1. **設計ノート先行**: コードを書く前に、完了報告の下書きとして
   「ParkedLive 中に誰が何を駆動するか」(present loop / EOF 処理 / HUD /
   egui host viewport の毎フレーム描画 / player の poll・tick 経路) を短く整理し、
   コミットに含める (docs 追記 or 報告本文)。ここが曖昧なまま書き始めない。
2. **park 分岐**: park 対象の active 窓が再生中メディア (動画・音声・動画の音声
   モード) の場合、snapshot 凍結の代わりに:
   - bundle を paused_bundle として退避 (通常の pause と同じ)
   - `VideoPlayer` / native presenter / 音声出力を**停止しない**
   - `transition_detached_window_state(id, ParkedLive, ...)`
   - passive 描画リストに「live メディア窓」として登録し、egui host viewport を
     毎フレーム描画して窓を生かす (BA-5 対策。中身は presenter child が覆うので
     backdrop のみでよい)
3. **非アクティブ中の入力**: ParkedLive 窓へのクリックは**復帰のみ** (シーク等の
   HUD 操作は復帰後に有効)。native HUD は ParkedLive 中は非表示または不活性にする
   (どちらにしたかを報告に書く)。キーボードは復帰前の窓には効かせない。
4. **復帰**: クリック → `Resuming` → paused_bundle を swap-in → `Active`。
   復帰で映像・音声が途切れないこと (7-eof / keep_audio_mode の source-swap と
   同系の要件)。
5. **新メディアの再生開始**: 別の動画 / 音声の再生を開始する全経路 (grid からの
   open / fullscreen 内ナビ / 音楽ビュー) で、ParkedLive 窓が存在すれば **close する**
   (`Closing` 遷移 + 再生停止 + 窓 close)。ParkedLive は常に最大 1 本。
6. **EOF / 連続再生**: ParkedLive 中に再生末尾へ達した場合、**フォルダ内の自動
   進行はしない** (paused bundle の文脈を非アクティブのまま動かさない)。ループ設定
   が有効ならループ、それ以外は末尾で停止。この扱いを報告に明記 (ゲートで
   ユーザー確認する仕様事項)。
7. **仕様書更新**: [detached-viewer-implementation-plan.md](detached-viewer-implementation-plan.md)
   §3 の状態表に ParkedLive (メディア live-park) を追記。

### 2.3 テスト要件

- Part 1: 削除フィールドごとの読者移行テスト (既存テストの緑維持が主)。
  reducer 遷移の代表列 (Open→Active→Parked→Resuming→Active→Closing→Removed) を
  1 本のテストで固定。
- Part 2 (cfg(test) shim で):
  - 再生中メディアの park → state が ParkedLive、player 停止 API が呼ばれない
  - ParkedLive 窓のクリック → Resuming → Active、player 継続
  - 新メディア open → ParkedLive 窓が Closing に遷移し停止
  - 非メディアの park は従来どおり Parked (凍結)
  - EOF: ParkedLive では自動進行しない

## 3. 完了条件

- [ ] Part 1 の対応表 (削除 / 残置 + 理由) が完了報告にある
- [ ] 削除したフィールドの grep 0 件リスト
- [ ] Part 2 の設計ノート (駆動の整理) がある
- [ ] §2.3 のテストが存在して緑、既存 detached テスト全緑
- [ ] `cargo fmt --check` / `cargo test --bin mimageviewer-core` / `cargo test` 緑
- [ ] `.\scripts\build-release.ps1` で実機検証バイナリ

## 4. 実機 smoke (R2 メイン gate、ユーザー)

1. 動画を detached で再生 → 別のピン留め窓をクリックしてアクティブ化 →
   **動画窓が残り、映像と音声が継続する** (今回の直接目標)
2. その動画窓をクリック → 復帰し、シーク・音量等が操作できる。映像・音声が
   途切れない
3. 動画 live-park 中に別の動画を開く → 古い動画窓が閉じ、新しい動画が開く
4. 音声モード (Z) の窓でも 1〜2 を確認
5. 従来分の回帰: 静止画 park / 復帰、見開き passive、F12 往復、Ctrl+↑↓、
   左右振動・窓消えの再発なし
