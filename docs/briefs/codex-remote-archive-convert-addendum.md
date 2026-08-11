# 追補: リモートのアーカイブ変換 — 調査結果を受けた訂正と段階分け

`codex-remote-archive-convert-brief.md` の追補 (2026-08-09)。
調査報告の内容を確認した。**ブリーフ側の記述に誤りが 2 箇所あった**ので訂正する。

## 1. ブリーフの訂正

### 1.1 `begin_convert()` は「プロセス全体」ではない (§2.7 の誤り)

`convert_lock: Mutex<()>` は `ArchiveCacheDb` の**フィールド**なので、
`ArchiveCacheDb::open()` を別に呼べばロックも別になる。ブリーフの
「プロセス全体で変換を直列化する `MutexGuard`」は誤り。

**正しい指示**: 報告のとおり、`App` が既に持っている実体をセッションへ渡し、リモートの
ジョブも同じ実体を使う。リモート側で `open()` し直さないこと。渡す境界は既存の AI 実行
ブリッジ登録箇所でよい。

### 1.2 物理フォルダ一覧にアーカイブは「出ていない」(§1 の誤り)

ブリーフは「一覧に出るのに開けない」と書いたが、Web 側が
[app.js](../../crates/remote-web/web/app.js) で `archive` を無条件に除外している。
出ていたのは**集約系ビュー**のほう。ブックマークが
`BookContainerKind::OtherArchive → RemoteEntryKind::Archive` を `Ignore` 判定なしで
生成しているのを確認した ([collections.rs:541](../../src/remote_ipc/collections.rs))。

つまり現状は **物理フォルダでは見えず、集約系では見えるが開けない**。逆になっている。

## 2. 追加で確認した事実

`ContainerEngine::settings_for_listing` は起動時スナップショットを clone し、
**`sort_order` だけ**をライブ読みする ([container.rs:631](../../src/remote_ipc/container.rs))。
報告のとおり `archive_file_handling` は停止するまで反映されない。

これは**アーカイブに限らない**。直前の増分で使った `skip_image_if_video_exists` /
`video_thumb_use_sidecar_image` も同じく固まる。設定同期は archive 固有の作業ではなく、
リモート全体の前提の問題として扱う。

## 3. 段階分け

報告の提案どおり段階を分ける。**1 段階ごとに止めて報告すること。**

### C-0: 設定同期 (先に単独で入れる)

- `settings_for_listing` の「`sort_order` だけライブ」を、リモートの判断に使う設定へ広げる
- **範囲は 2 案から選んで理由を報告すること**
  - (a) 必要な項目だけライブ読みに足す — 影響が小さいが、次に必要になるたび足すことになる
  - (b) Settings 全体をライブ読みにする — 単一の出所になるが、1 listing あたりの読み取りが増える
  - 判断基準は **1 listing あたりの費用**。`sort_settings.load()` の実測と比べて決める
- archive とは独立に入る変更なので、**この段階だけで一度止めて報告する**

### C-1: ジョブ基盤 + キャッシュ所有権

- AI 名義の長時間ジョブ判定を「リモート長時間ジョブ」へ一般化する
  ([session.rs:1326](../../src/remote_ipc/session.rs))
- `ArchiveCacheDb` の実体をセッションへ注入する (§1.1)
- アーカイブジョブの型を追加する。AI 固有型を流用しない
  - start / state / cancel / recoverable / result
  - `Ask` は `AwaitingConfirmation` 状態と確認 API
  - 進捗は `files_done / files_total / bytes_written`
- **公開上の元ソースと、読み込み用の実体 (直接読み RAR またはキャッシュ ZIP) を分ける。**
  報告の指摘どおり、キャッシュパスが表示・履歴の識別子になってはいけない

### C-2: 変換と Web UI の接続

- `Ask` の確認、進捗、中止
- 直接読める RAR は変換せずに開く
- 変換済みは確認なしで開く
- Web の `archive` 除外を外す (§1.2)
- 集約系の `Ignore` 不整合を直す (§1.2)

## 4. 判断が要る点 (C-1 の設計時に報告すること)

- パスワード付きアーカイブ (元ブリーフ §2.8)。この増分で対応するか
- 変換中に session owner が別端末へ移ったときの扱い
- 変換中に本体が操作権を取り戻したときの扱い。排他があるので、リモートが操作権を失う
  瞬間は必ずある

## 5. ドキュメント

決定は `docs/web-remote-plan.md` へ書き戻すこと。設定同期 (C-0) は archive を超えて
リモート全体に効くので、§12 の該当箇所へ「何がライブで何がスナップショットか」を
明記する。ブリーフは git 管理外なので、そこにしか無い決定は次のセッションが読まない。
