# ローカル文字起こし T2: 検索

実装元: `e28309180`、作業ツリー `G:\home\mimageviewer-asr-t1`。コミット・製品バイナリの起動・lib全件テストは行っていない。

## 着手時の前提確認

1. `fts_index` の各ソースは bigram の候補収集と STORED 原文の最終照合で共用される。`fts_meta::INDEX_VERSION=11` と既存の再構築予約・走査印の初期化経路を確認した。12へ上げ、その経路を使う。
2. `IngestSession` は既存の索引workerが持ち、動画・音声の文書を作る。`transcript.db` は非作成のread-only open、`status`・区間・rev/indexed・削除triggerを既に持つ。UIへDB読取を追加する必要はない。
3. 説明文には共通 `AtomicU64`、supervisorごとの既読番号、受け持ち範囲の反映待ち、commit/reload後のrev一致確認がある。`STORES` の表示キャッシュ指定と完了採用点も既存である。
4. Ctrl+F は `app/metadata_ops.rs` の既存workerで説明文を読み、通常の検索語・AND/OR/NOT照合へ渡している。動画はPass 2、音声はPass 1の経路を保つ。
5. MCPは同じ `SearchScope` の対象指定を候補と最終照合の両方へ渡す。ただし許可の対象指定を組む `agent_ipc/mod.rs::caption_search_target` の変更が必要だったため、実装前に停止して報告した。追加の許可を受け、関数名・呼び出しは変えず、その本体にTranscript除外の1行だけを加えた。握手・IPC版・pipe・応答型は変更していない。
6. Ctrl+G/Ctrl+Fは `global_search_ui::TARGET_CHOICES` を共有する。「AI 説明」の次へ「文字起こし」を追加する。Appに状態項目を足さない。

上記の調整以外に計画の前提との食い違いはない。

## 変更と一般化

- `SourceKind::Transcript`、`transcript_text`（bigram + STORED）、`PerSourceText.transcript`を追加し「すべての対象」に含めた。動画・音声だけで成功した区間の本文を空白で結合・正規化する。署名やconfigが古い行も対象。failed・行なしは本文を入れない。
- 新しい `derived_text` は `STORES` の `derived_text` 指定を正本とする。説明文／文字起こしのread-only reader、反映待ちSQL、rev一致の確認、削除記録の範囲外整理を共用する。各DBのschema初期化は既存の持ち主に残し、索引側は既存DBだけを開く。
- `caption_epoch` / `notify_caption_changed` を `derived_text_epoch` / `notify_derived_text_changed` へ一般化。番号は1つ、既読番号は従来どおりsupervisorごと。起動時・全件走査後・通知後の同じ反映処理が、登録ストアを順に拾う。読み取れたストアだけを処理し、commit/reload完了後に同じrevだけを確認する。処理中に増えた番号は次の巡回へ残す。
- `StoreMutationEffects` はdescriptorから索引通知と各表示の失効を決める。作成・削除・取り込み・改名・復元の既存の完了採用点を使う。pinだけの変更は表示を失効させ、本文の索引通知を出さないという従来の扱いを維持する。
- Ctrl+Fも同じストア一覧・正規化をworker上で使う。検索対象を外したストアは開かない。MCPは受付時の同じ許可snapshotで説明文／文字起こしを独立に除外し、本文や一致箇所を応答に足さない。
- `ingest/derived_text_read` にストア名・成功・正規化後のbyte数・読取時間を記録する。説明文も同じ計装を通す。
- `INDEX_VERSION=12` の既存再構築経路を使い、元の保存DBは移行しない。更新履歴の未公開節の先頭で初回の索引作り直しを告知した。

表示・ジャンプ・音声抽出・worker推論・保存時刻の契約は変更していない。T3の抜粋・時刻ジャンプは作っていない。

## 検証

実行時の資源を抑えるためCargoビルドとnextestは1並列。全件は実行していない。すべてのコンパイルを伴うコマンドは次の入口で実行した。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t2 -WorkDir G:\home\mimageviewer-asr-t1 -Command '<下記のコマンド>'
```

このsandboxでは共有ロックディレクトリと共有TEMPへの書込が拒否された。scriptの警告どおりBelowNormalで続行し、子コマンドでは `CARGO_BUILD_JOBS=1`・`MSBUILDDISABLENODEREUSE=1`・`CMAKE_BUILD_PARALLEL_LEVEL=1` と、作業ツリー内 `target/asr-t2/tmp` のTEMP/TMPを設定した。PATHの先頭には作業ツリーの `vendor/ffmpeg/bin` と `vendor/vcrt` を追加した。ログは `target/asr-t2/*.log`（追跡外）。

最初の焦点テストは798件中796件成功。残りは、T1では文字起こしを索引通知しなかったことを期待する既存の汎用コピー／改名／削除・孤児整理テスト2件だった。実際に行を変更した場合だけ通知するT2の期待値へ直した。変更なし・本棚除外・pinだけの場合の「通知しない」検査は残している。

`history_transition_storage_keeps_app_stack_footprint_bounded` は成功し、`size_of::<App>() = 109,896`（上限110,000、余裕104byte）。Appのフィールドを追加せず、番号も既存の1つを改名して使った。

MCPの焦点11件は成功（7.274秒）。コマンドは以下（共通wrapper経由）。

```text
cargo nextest run -j 1 -p mimageviewer-mcp --lib -E 'test(/client_text_limits|stage2_complete_tool_discovery|transcript|cursor_tools|tools_list|search_items_description/)' --success-output final
```

- 最大tools/list: **43,915文字**（全権限・protocol・IDの組合せを既存テストで検査）。512文字を足して44,427、48,000以内。
- 最長description: **list_folder、667文字／667 UTF-16 units**。目標2,048と上限4,096の両方以内。
- server instructions: **852文字／852 UTF-16 units**。上限4,096以内。
- search_items自身の説明に説明文・文字起こしと各許可の条件が残る検査も追加した。
- `python scripts/check_ui_glyphs.py`: 危険な文字0件。`python scripts/check_site_cards.py`: 53枚、上限超過0件。

### UI

本体と共通の対象選択widgetを抽出し、headlessの入力で「文字起こし」を選ぶと `Only(Transcript)` になることを検査する。既存の選択肢・ID・幅・利用可能なpopup高さは変更していない。PNG生成の1件は成功した。新規PNGは次の2枚（既存PNGは更新しない）。

| PNG | 寸法 | byte |
| --- | --- | ---: |
| `tests/snapshots/transcript_search_target_dark.png` | 320×420 | 22,536 |
| `tests/snapshots/transcript_search_target_light.png` | 320×420 | 22,517 |

両方を目視し、既存の順番・文字起こしの追加位置・選択中表示・文字の欠けと重なりがないことを確認した。App・DB・製品アプリは起動しない純描画fixtureである。生成時だけ `UPDATE_SNAPSHOTS=1` を使い、最終の焦点テストでは指定せず通常比較する。

### UI応答性の確認

`docs/ui-responsiveness.md` §4に沿って確認した。UIに加えた処理は対象enumの選択と既存の非同期要求だけで、DB open・SELECT・stat・待機は追加していない。Ctrl+Fは既存の現在地検索worker、ingest・反映待ちの読取は既存のsupervisor/ingest worker、MCPは既存handlerのworkerで行う。通知はAtomicの加算だけで、追加のchannel・UI上のjoin・リトライ・周期処理は作っていない。

### 最終の本体焦点テスト

**800件成功、失敗0、376.077秒**。次のコマンドを共通wrapper経由で実行した（`focused-final.log`）。

```text
cargo nextest run -j 1 -p mimageviewer --lib -E 'test(/derived_text|transcript|caption|fts_index|fts_meta|ingest|indexer|metadata_reconfiguration|rename_key_migration|metadata_transfer|ctrl_f|search_items|history_transition_storage_keeps_app_stack_footprint_bounded/)' --no-fail-fast --success-output final
```

動画／音声のok・stale・failed・行なしのingest、ソースのSTORED往復と除外、旧v11 schemaと走査印の再構築、作成／削除／取り込み／改名後の再ingest、rev違いのack、受け持ち範囲外の保留、descriptorによる索引／表示の失効、Ctrl+FのAND/NOT・対象指定、MCPの説明文／文字起こし許可4通りの候補／最終照合、Appサイズ、新規PNGの通常比較が成功した。既存の説明文のreader公開前・処理中の通知・範囲別反映・本候補・pin・取り込み・汎用操作の回帰もこの実行に含む。

### 1,000時間相当の合成データ計測

明示実行用のignored test `derived_text::tests::transcript_synthetic_ten_million_chars_ingest_perf` を単独で実行し成功した（テスト全体35.556秒、`perf.log`）。通常の焦点実行では走らない。

```text
cargo nextest run --config-file target/asr-t2/perf-nextest.toml -j 1 -p mimageviewer --lib -E 'test(/transcript_synthetic_ten_million_chars_ingest_perf/)' --run-ignored only --success-output final
```

開発用の一時nextest設定は、この計測1件だけに最大20分の実行枠を与える。リポジトリの通常のテスト設定・timeoutは変更していない。再実行時は `--config-file target/asr-t2/perf-nextest.toml` を省略すれば通常の300秒枠で実行できる（今回の全体実行時間は約36秒）。

| 項目 | 結果 |
| --- | ---: |
| コーパス | 音声1,000本、各1時間相当・10,000文字（合計1,000万文字） |
| 本文UTF-8 | 30,000,000 byte（28.61MiB） |
| ingest | **24.1667395秒**（約41.4万文字/秒） |
| Tantivy索引 | **73,808,008 byte（70.39MiB）** |
| transcript.db | **30,867,456 byte（29.44MiB）** |
| 結果 | 1,000件成功、派生テキスト読取エラー0 |

seed固定の512種類の漢字を疑似乱数で並べた合成本文で、各行は1区間。生成・DBへの事前保存・索引の初期作成は時間から除き、既存の `IngestSession::apply` のDB読取、JSON復元、本文結合・正規化、bigram化、投入、各commit/reload、管理メタ更新を計測した。索引容量はdispatcher/readerを閉じた後の索引ディレクトリのファイル合計。元音声はダミーで、音声の復号・ASR推論・動画メタのprobeは含まない。

通常のtest profile（unoptimized + debuginfo）での値で、製品の最適化ビルドでは測っていない。本文の語彙や繰り返し、ファイル本数、mergeの状態で容量と時間は変わるため、実動画に対する保証値ではない。他セッションの負荷を排除した計測でもない。作業ツリー内の一時領域だけを使い、実データ・評価正本へ書き込んでいない。

### 既存UIの通常比較

以下を共通wrapper経由で実行し、**5件成功、14.839秒**（`ui-existing.log`）。短い語の拒否表示dark/lightと、検索バーを固定したtoolbarのlight・履歴popup light・narrow darkを比較し、既存PNGの変更はなかった。

```text
cargo nextest run -j 1 -p mimageviewer --test ui_snapshot -E 'test(/global_search|toolbar_pinned_search/)' --success-output final
```

`cargo check -p mimageviewer --lib --tests` は共通wrapper経由で成功（3分05秒、`check-final.log`）。既存の非推奨API・未使用項目等の警告はあるが、コンパイルエラーはない。

### 確認用ビルドと最終チェック

`cargo fmt --check` 成功後、共通wrapper経由で `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-dev.ps1` を実行し、**成功**した（09:11:58〜09:24:22、`build-dev.log`）。core・remote・EPUB PDF worker・MCP adapter・music-video editorを含む `target/dev-runtime/` が揃い、VCRT/PE検査も `runtime=4 pe=5` で成功した。通常機能のまま、portable featureは使っていない。製品は起動していない。

UIグリフ・サイトカード・差分の空白検査も成功し、最後に引数なしの `cargo fmt` を実行した。コミット・ステージングは行わず、変更を作業ツリーに残した。

### 利用者による確認の例

以下の実機操作は実行していない。確認用ビルドは通常の `%APPDATA%\mimageviewer` を使い、実際の設定・データを更新し得る。単一起動mutexを共有するため、インストール版とトレイ常駐のmImageViewerを先に終了してから、作業ツリーを現在地として起動する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

1. 文字起こしが保存済みの動画／音声があるお気に入りで、初回の索引作り直しを待ち、Ctrl+Gの「文字起こし」で本文の語を探す。「すべての対象」でも同じ項目が出る。
2. 同じフォルダでCtrl+Fの「文字起こし」を選び、本文の語と除外語で絞り込める。
3. 作成・削除・取り込み・改名の完了後、索引が追いつくと検索結果も変わる。
4. MCPの文字起こし許可OFFでは本文だけにある語で検索されず、ONにすると同じ接続で検索に使われる。返るのは従来どおり項目で、本文・抜粋・時刻は増えない。

## 計画との差異・残した契約

仕様の変更はない。`caption_search_target` は調整の指示に従い改名せず、その本体に許可フィルターを1行だけ追加した。既存の任意パスを使う説明文テスト・呼び出しのため、`new_with_caption_db_path` 等の入口は共通の `DerivedTextPaths` へ委譲する薄い互換入口として残した。説明文用と文字起こし用の反映ループ・通知番号を別々には持たない。

禁止された `crates/agent-ipc/src/lib.rs`、`crates/mcp-server/src/pipe.rs`、`src/agent_ipc/pipe.rs` は変更していない。保存DBの移行・IPCの版上げ・T3の抜粋や時刻指定は行っていない。

## 変更ファイル

- 索引・共通部品: `src/fts_index.rs`、`src/fts_meta.rs`、`src/ingest_text.rs`、`src/ingest_worker.rs`、`src/derived_text.rs`、`src/derived_text/{tests,test_support}.rs`、`src/lib.rs`。
- 反映・DB・汎用操作: `src/indexer_manager.rs`、`src/indexer_supervisor.rs`、`src/metadata_reconfiguration.rs`、`src/ai_caption_db.rs`、`src/transcript_db.rs`、`src/rename_key_migration.rs`、`src/metadata_transfer.rs`。
- 通知の受け渡し: `src/app.rs`、`src/app/{book_append_undo,caption_display_tests,content_identity_detection,content_identity_restore,image_folder_zip_operation}.rs`、`src/ui_dialogs/{caption_job,caption_delete,transcript_job}.rs`。既存の呼び出しとテストを共通通知名へ合わせた。
- Ctrl+F／対象UI／MCP: `src/app/metadata_ops.rs`、`src/global_search_ui.rs`、`src/agent_ipc/{mod,tests}.rs`、`crates/mcp-server/src/schema.rs`。`mod.rs`は許可済み関数内の1行のみ。
- 文書: 本報告、`docs/{search-architecture,async-architecture,architecture-overview,mcp-integration-plan}.md`、`CHANGELOG.md`、`htdocs/mimageviewer/{index,privacy}.html`、`htdocs/mimageviewer/manual/{search,tut-ai-transcript,ai-integration}.html`。MCP計画とAI連携マニュアルはsearch_itemsの説明だけを変更した。
