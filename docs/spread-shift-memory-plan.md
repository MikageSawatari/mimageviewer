# §1.364 見開きの一ページずらしの本別記憶 — 設計 R3

更新: 2026-10-11（最新決定は§11.6）。対象: `mimageviewer-nextview` / `next-viewer`、R3照合基点
`ec4a43b38da862f577b2872ed7c7d626f996be5d`（R2 `1837ded3f` と master の新しいまれな失敗方針を統合済み。製品コードは §1.342 統合済みの `fa280d0d0` と同じ）。
状態: **Q1〜Q5 はすべて決定済み。利用者への未決質問なし。独立再レビュー・構造合意は別工程で、実装の許可・検収ではない。**
今回の変更は本書と `target/D-1364-design-r3-msg.txt` だけ。コード変更、テスト実行、ビルド、
製品起動、コミットは行わない。以下の型・メソッド・テーブル名は、現行と明記したもの以外は提案。

上記はR3文書作成時の作業範囲の記録。後続D2/D2bの実装・確認履歴は§11.4–11.5に保持する。
root改名のopen-context/shift scope追従と同本pending-open直列化の記述は、
2026-10-11の利用者決定§11.6で上書きする。Q1〜Q5と他のR3契約は変更しない。
実装記述だけで自動検証・独立完了レビュー・利用者確認の完了を表さない。

## 1. 正本・確定要件

正本は [next-release-backlog.md §1.364](next-release-backlog.md)。全節と指定された
CLAUDE.md のバグ修正、状態削減、まれな失敗、通常利用の退行相談、永続データ、detached の規則、
AGENTS.md の役割を読んだ。関連資料は docs/README.md、architecture-overview.md、
display-pipeline.md、async-architecture.md、virtual-folders.md、keymap-spec.md、
key-customization-impl-plan.md、ui-responsiveness.md §4、detached-rework-plan.md §2、
web-remote-plan.md（特に §2 / §12.7–12.8 / §12.16 / §14）。

G の参照正本は **`C:/home/mimageviewer-nextbook/docs/book-composition-plan.md` を読み取り専用で参照**。
R1では独立 ACCEPT 済み R6 を参照し、R2では E 統合後 G0（`76488f0ec`）の §6.1 / §7 / §9.3 / §14 と
`C:/home/mimageviewer-nextbook/target/G-1242-g0-msg.txt:22`–31 の8契約を再照合した。
G の文書・worktreeは変更しない。特に G Q1/Q6 等の利用者決定を再質問しない。
利用者の決定と日付は §12、G0との対応は §10 に記録する。
R3では2026-10-10の全回答を反映し、CLAUDE.md「まれな失敗は既定で割り切る」と
AGENTS.mdの更新済み方針を §4.4 に適用した。R2の通常再入場・正常終了・G契約は維持する。

R1レビューからの修正:

| 指摘 | R2の対応 |
| --- | --- |
| P2-1 先行保存中の再入場 | §6.1: admitted cutoffと同writer読取、shift/resume両完了を既存open continuationへ。正常exitもdrain |
| P2-2 既存setterの連続入力 | §4.1/§11.1: 専用DB採択で既存setter非同期化を除去。旧 §4.2 と未決質問は削除 |
| P2-3 nested probe不足 | §9.1/C10: descriptor限定slash-familyをcopy/probe共有。nestedのみ候補/flag/復元fixture |
| P2-4 importの内容ID観測 | §9.1/C10: commit済追加/置換の捕捉logical root集合をrecorderへ。cancel適用済batchも対象 |
| P2-5 covered未記載scope | §9.2/C10: 非空でもcoverage内未記載を解除、外と旧字段不在は保持 |

確定している仕様:

- 一ページずらしを**本ごとに記憶する全体設定**を追加し、既定 ON。
  閲覧履歴の記録設定と独立する。「本ごと」は保存単位であり、本別の ON/OFF 行を増やす意味ではない。
- 再入場で読書位置とずらしを解決し、復元した組で表示する。解除はその本の基本の自動見開きへ戻す。
- 上 HUD「…」から調整の有無、実際の内部アンカーへの移動、解除、記憶設定を操作できる。
  調整中は見開き操作の近くに控えめな印。保存・解除の結果を通知する。
- OFF は従来のセッション内ずらし。通常ページ送り、シーク、単ページ時の Ctrl+左右による一ページ移動を維持。
- **Composed では一時ずらし・保存・解除を共通 action owner で拒否**し、構成 editor を案内する。
  G の作成時には現在の組を取り込み、保存済みずらしは削除せず休眠保持（G Q1）。
- G の `BookLayoutSource::Automatic(resolved_rules, optional_shift)` / `Composed` をそのまま接点とする。
  Composed と shift の重ね掛け、editor からの休眠行削除、G 独自のずらし復元を作らない。
- 本機能の保存データは未リリース。**shift データの移行は不要**。
  既存 spread.db の行・スキーマ、既存 resume、既存メタ情報形式まで未リリース扱いしない。

## 2. 現行コードで確認した前提と原因

実機観測はしていない。下表の根拠はすべて現在の worktree の source inspection。

| 確認事項 | 現行根拠（file:line） | 設計上の意味 |
| --- | --- | --- |
| shift は一個のセッション index | `src/app.rs:16553`、`src/ui_fullscreen.rs:28405` | 一ページずつの変更履歴や全ペアの保存ではなく、最後の内部 anchor 一個を記憶する |
| 本再入場で消す | `src/app.rs:25821`、`src/app.rs:25885` | 本別設定読込が None を生成することが組の再現失敗の原因。描画への guard 追加で直さない |
| 移動先と anchor が異なる | `src/ui_fullscreen.rs:15407`、`src/ui_fullscreen.rs:15427` | `(new_idx, anchor_idx)` の後者を保存。押したページ・new_idx・画面左ページを保存しない |
| 実例: index 2 からの nudge が `(2,0)` | `src/ui_fullscreen.rs:74925`、`src/ui_fullscreen.rs:74933` | HUD は現在 3 ページ目でも「基点 1 ページ目」を示し得る。RTL でも画面左から推測しない |
| 四つの key action とゲームパッドの入口 | `src/keymap.rs:1692`、`src/keymap.rs:6574`、`src/ui_fullscreen.rs:31297`、`src/app/gamepad_input.rs:6550`、`src/app/gamepad_input.rs:6964` | UI 無効化だけでなく queue の共通受付で資格を判定 |
| non-spread は従来のページ移動 | `src/ui_fullscreen.rs:28411` | Single/Split の移動を構成あり拒否に巻き込まない |
| mode/flow/direction と import にも reset | `src/ui_fullscreen.rs:25434`、`src/ui_fullscreen.rs:37805`、`src/ui_fullscreen.rs:37818`、`src/ui_fullscreen.rs:37854`、`src/ui_fullscreen.rs:37878`、`src/app.rs:44563` | None 代入の意味を分類し、明示変更と hydration を分ける |
| resume と履歴は別 | `src/book_resume_db.rs:35`、`src/book_resume_db.rs:78`、`src/app.rs:56814`、`src/app.rs:56855` | `reading_history_enabled` を shift 読み書きの gate に使わない |
| 論理 identity 対応表がある | `src/app/file_type_resume.rs:15`、`src/app/file_type_resume.rs:33`、`src/app/file_type_resume.rs:135` | 行生成 worker の最終順と identity を一緒に採用する。raw index fallback は shift にはない |
| 深い ZIP 階層の resume は未記録 | `src/app.rs:56824` | shift は階層別保存できるが、未存在の読書位置を復元したと称しない（§6） |
| 変換書庫の spread は物理 cache root | `src/app.rs:25715`、`src/app.rs:25719` | 新 shift は論理 source、既存 base pairing は現行 cache 側。両者を別の typed key で扱う |
| anchor を unit cache token に含む | `src/ui_fullscreen.rs:14619`、`src/ui_fullscreen.rs:14628` | 復元した anchor の採用と cache 失効を同じ context の一回の遷移にする |
| F12 bundle が shift と cache を運ぶ | `src/app/viewer_context_registry.rs:1079`、`src/app/viewer_context_registry.rs:2711`、`src/app/viewer_context_registry.rs:3466` | 保存行の global map と、表示中 context の調整を混同しない |
| App の spread 接続が同期 writer | `src/app.rs:18962`、`src/ui_fullscreen.rs:37392`、`src/remote_ipc/ui.rs:2552` | 既存 setter の同期契約は維持。新 shift は専用 DB / worker に限定する |
| Remote の共通 builder は shift=None | `src/ui_fullscreen.rs:15237`、`src/ui_fullscreen.rs:15270`、`src/remote_ipc/container.rs:6100` | 保存を PC のみに足すと Remote の組が変わる。optional_shift を共通 builder へ渡す |
| metadata は本別表示設定を搬送 | `src/metadata_transfer.rs:453`、`src/metadata_transfer.rs:2436`、`src/metadata_transfer.rs:4645` | shift も利用者の本別表示設定として明示転送に含める。origin は別解決が必要 |

守る不変条件は「**受入済みの同一本 scope / 同じページ identity の調整だけが、
その context の Automatic unit を作る**」。保存位置の数字が同じだから別ページへ適用する、
mount 中の App.current_folder を保存 reply の宛先にする、兄弟窓の close で最新保存値を上書きする、
という経路をなくす。今回の未記憶という症状は新機能の対象だが、既存 reset / 削除 / 非同期 adoption を
横断してこの不変条件を設計する。

## 3. 本とページのキー契約

### 3.1 本の scope と論理・物理キー

G と共通の `BookScopeKey { logical_root, level_prefix }` を使う。prefix は raw tree で同定した、
wrapper collapse 後の実効階層。種類 policy / facet で表示行が減っても別の本に付け替えない。
永続 `path` は `spread_db::container_key(logical_root, level_prefix)` と同じ合成を
`path_key::normalize_keep_drive` で正規化する（現行 helper: `src/spread_db.rs:30`、`src/path_key.rs:34`）。
G の KeepDrive scope と同一。shift には旧 root fallback 継承を**設けない**。
子章へ親章のページ基点を継承できないためで、既存 spread 設定の fallback はそのまま維持する。

| 対象 | shift の logical_root / PageRef | base pairing / 読取側 |
| --- | --- | --- |
| 画像本フォルダ・製本本 | 本フォルダ / 本からの相対画像 path | 既存 folder spread key / 実画像 |
| ZIP/CBZ、直接 RAR/CBR | outer 本体 / outer からの完全 entry_name | 現行 spread exact/fallback / 実書庫 |
| ZIP 内 directory・nested ZIP | outer 本体 + 実効 prefix / nested chain を含む完全 entry_name | 現行 tree 階層 / 既存 bytes 読取 |
| RAR/7z/LZH 等の変換閲覧 | **元書庫** + 同じ実効 prefix / cache にある完全 entry_name | **既存 spread は cache ZIP 側**。デコード・既存ページ編集も cache 側を維持 |
| PDF | 元 PDF / 既存 document page identity（§3.2） | 元 PDF の既存設定 / PDF read target |
| EPUB | 元 EPUB / 既存 document page identity（G Q6決定、§3.2） | 元 EPUB の既存設定 / 世代 PDF read target |

変換元の alias はその context が受入済みの physical cache と archive_source_override の対応からだけ得る。
G §11.2 と同じく、別 cache や兄弟 context へ global alias を当てない。元書庫が見えなくても、
受入済み cache の F5 閲覧を維持し、論理元パスの存在確認を新たな必須 gate にしない。
再変換で cache path が変わっても同じ entry_name は同じ shift。entry/階層自体が変われば未解決として保持する。
KeepDrive のため別 drive の同名本へ自動適用しない。既存 spread/resume の DriveStripped 規則
（`src/path_key.rs:20`）は変更しない。drive 変更は既存内容 ID または明示転送の範囲で追随する。

保存対象は一つの本として確定したページ列。通常の非本 folder、mixed media、stack、抽出一覧へ
新しい本別 shift を作らない。G の OrdinaryList は従来の一時 shift を維持し、元 scope に構成が
ある場合は共通資格で拒否する。G の ExtractedResult は A2（初期 Single、一時 Spread、shift 禁止）を使う。
A2 は G の承認済み変更であり、1.364 が独自に未統合の抽出操作を先に削ることはしない。
本機能先行中のこれらの一覧は現行 session 操作を保ち、保存対象にだけしない。

### 3.2 anchor の identity と番号禁止の境界

`SpreadShiftPageRef` はしおり / G の相対参照と同じ意味の型:
`RelativePath` / `ArchiveEntry` / `DocumentPageIdentity`。
G とは pure adapter を共有し、`(scope, ref) ↔ book_resume_db::PageIdentity ↔ GridItem / RemoteAddress`
を変換する。既存しおり型は serde 型ではない（`src/book_bookmarks.rs:99`）、resume 型の Image は
絶対 path（`src/book_resume_db.rs:36`）。そのまま JSON を流用せず、root を重複保存しない相対 wire 型を使う。

- 保存するのは `compute_spread_offset_nudge` の**内部 anchor** の実ページ参照。
  `items` index、nav position、HUD の n/N、押したページの数字、raw resume page、±1 の offset を保存しない。
- RelativePath は root 内の検証済み相対 path。ArchiveEntry は outer からの chain 全体。
  照合はしおりの slash/case 規則（`src/book_bookmarks.rs:1832`）と揃える。実 bytes 読取には元表記を残す。
  絶対 path、drive 指定、空要素、`.`、`..`、NUL、scope 外参照、過大値を拒否する。
  既存 relative 検証の根拠は `src/book_bookmarks.rs:1796`。保存値からファイルを勝手に開かず、
  安全性確認済み manifest へ照合する。
- **PDF/EPUB には番号以外の安定ページ ID が現行にはない**。
  resume は `Document { container, page_num }`（`src/book_resume_db.rs:45`）、しおり/G は
  `PdfPage(u32)`（`src/book_bookmarks.rs:102`）。G Q6 と coordinator の R2 指示に従い、
  **既存個別編集と同じ 0-origin 物理ページ identity に限る例外**を採る（§12 の短い確認）。
  表示順番号とは異なるが、整数を文字列や hash へ包んで「番号を保存していない」とは主張しない。
- 同じ ZIP member / folder file の追加・削除・並べ替えでは参照を再解決する。
  基点以降は**現在の自動規則と現在順**で組む。全ペアを固定する機能ではない。
  PDF の内部挿入・再編、EPUB の reflow 再変換で同番号が別内容になることは既存方式でも追跡できず、
  G Q6 で受容された制限としてマニュアルへ明記する。全ページ hash、類似画像推測、独自 reflow map は追加しない。

照合には worker が最終 items 順と一緒に作る identity index を拡張・再利用する。
`PreparedPageIdentityIndex::resolve` の raw fallback を通さず、`resolve_identity(identity)` の pure 入口を用意する。
Relative/Archive 照合を共通 adapter に寄せ、既存 resume の serde 形式・照合を無断で変更しない。
同名 directory / nested archive 衝突は既存非対応を拡げない。

## 4. 保存先・writer・確定境界

### 4.1 shift 専用 DB と単一 worker（R2 採択）

coordinator 指示により `<data_dir>/spread_shift.db` に次のテーブルを置く。

```text
spread_shifts(
    path TEXT PRIMARY KEY,       -- KeepDrive の logical BookScopeKey
    anchor_ref TEXT NOT NULL     -- 検証済み tagged relative PageRef、版1
)
```

一冊一行、upsert / delete を一 transaction。既存 `spreads` の mode/flow/direction や
末尾/単独の設定行は変更しない。変更履歴、old index、履歴 ON 状態、cache path、timestamp、
永続 revision / tombstone / pending 行は不要。サービス内の採用 sequence はプロセス内だけ。
新 DB の初回作成だけで、shift の旧形式 migration はない。ファイル未作成は正常な「保存なし」。
read-only probe が欠落 table を正常不在にできる descriptor と、稼働 service が自分の DB の
不正 schema / JSON を検出した場合とは分け、後者を「保存なし」で上書きしない（§4.4）。

`SpreadShiftService` は App 所有の単一 worker / writable connection owner。
既存 `SpreadDb` 接続・全 setter・commit後表示変更はそのまま。base mode/flow/direction の値を
新 worker へ捕捉して送らず、shift command は scope + ref の一行だけを扱う。
worker の基礎パターンは `src/book_resume_db.rs:327`、`src/book_resume_db.rs:350`。
UI / core ContainerEngine は immutable snapshot を使い、DB open / SELECT / write / join を
新たに入力・HUD 描画へ追加しない。shift の DB を開く読み取り準備も worker で行う。

spread.db 再利用は検討したが、UI同期保存追加または既存writer移管が必要になる。
後者は連続 mode→flow 入力が古い全状態を捕捉する問題や mode→nudge→reset の採用順を増やす。
専用 DB に限定すると既存 setter 非同期化自体がなくなり、R1 P2-2 の原因となる組合せを除ける。
DB lifecycle が一つ増える費用は §9 の登録で負担する。G の公開契約は DB 名に依存させない。

maintenance は既存操作の worker / modal lifecycle を再利用する。service の FIFO に
`BeginMaintenance` を送り、先行 command・成功結果の内容ID送信を回収して接続を閉じた ACK 後にのみ
既存 rename/purge/copy/import worker が DB を開く。writer lease はその一処理だけに渡す。
既存maintenanceの終了経路で接続を閉じてleaseを返し、serviceが再読込して新snapshotを公開してから通常受付へ戻る。
新 open の Restore は同じ maintenance 完了依存へ接続し、古い snapshot を完成済みと扱わない。
lease中は新しいDB commandをworkerへ流さず、既存保守operationの依存として保持する。
EndMaintenance/再openを待機Readの後ろへenqueueして相互待ちにしない。
本管理の成功済root改名/削除はlease中も同一ownerの待機FIFOへ受理し、その時点でadmittedを進める。
End/再読込の後、root操作とread/countを受付順で処理し、後続readは先行root操作の完了証明を待つ。
正常exitは保守中/起動待ちの位相を保持して受付を閉じ、leaseの終了送信経路を残して受理済依存をdrainする。
read-only export も開始時に先行保存を fence して一つの DB read transaction から採る。
第二 writer の常時並走・busy retry・独自の全 DB transaction 調停は作らない。
既存 modal 範囲外で新たな入力停止が必要になったら、通常利用への影響を利用者へ相談してから実装する。

### 4.3 新 shift command の一回の確定（番号は R1 参照互換）

共通受付は `SetShift { scope, anchor_ref }` / `ClearShift { scope }`。
入力元 context ID、items generation、操作 sequence、要求時の logical root、資格 snapshot を捕捉する。
受付で最新の共通資格を確認し、**送信に成功した時点で admitted sequence を公開**して worker へ送る。
commit sequence / 保存行 snapshot の公開とは別であり、前者は未完了の先行要求を open が見落とさないための受付順。
順序は完了時の mount / focus に依存しない。送信失敗は受理成功とせず §4.4 に従う。
新しい入力による保存を close 時まで遅らせない。close / fork / restore は新規 save の producer ではない。

1. Automatic の有効 nudge の後者 anchor を ref に変換する。端の no-op は write / 保存通知なし。
2. context の調整 state を一回で更新し、同じ owner の canonical unit を再解決して既存 page-nav へ target を渡す。
   記憶 ON なら SetShift、OFF なら session のみ。save のためにページ送りを block しない。
3. 成功 commit が保存行 snapshot / committed sequence を公開する唯一の時点。
   「保存しました」はこの完了後だけ。古い reply でも保存済み事実は service に反映し、
   古い seq を現 context の live state に戻さない。
4. Clear は context の shift を外して既存の位置正規化へ送り、保存行があれば削除する。
   **明示解除は記憶 OFF 中にもこの本の保存行を消す**（Q1決定）。
   Clear 成功後に「見開き調整を解除しました」。保存なしの session 解除は即時通知できる。
5. エラーは §4.4 の既定方針に従う。

長押し/連打は既存 navigation sequence の admission を維持し、受理済み command を FIFO で一回実行する。
後続 nudge は要求元 context の即時更新済み live state、後続 mode/flow/direction は現行同期 setter が
採用した状態を基準にする。Q3決定に従い、記憶ONではsetter成功または同mode再押下の現行resetに対応する
Clearをその受付順で送る（同modeでは既存DB write自体がない）。
例: nudge(Set s1)→同期 mode 成功(Clear s2)→nudge(Set s3) はこの順。setter 失敗で
現行 reset が起きなければ Clear も発行しない。既存全 spread 状態を保存 reply から採用し直すことはない。
save reply は target を動かさない。未受理 command と既に送信した command を同じ cancel にしない。
送信済みは元 scope に一度だけ確定し、元窓が消えていれば App の既存通知面へ結果を報告する。
正常再入場 / 正常終了の確定は §6.1 の fence、Remote 取得は §8.2 の drain を使う。
DBの完了・joinは worker / process終了ownerで扱い、UI スレッドで待たない。

### 4.4 まれな失敗の既定方針（Q4確認済み）

まれな失敗は既定で割り切る（ログ＋通知＋原本保持）。保存失敗ではsession調整、解除保存失敗では解除後の現表示を保ち、成功と通知しない。読めない原本は削除・再作成せず書込を止め、再生成可能な索引・cacheだけを必要なら作り直す。再試行・途中状態保持・失敗分類・多段回復・回復内の回復・失敗のための完了待ちは追加しない。相談は利用者作成データの損失・破損や通常利用で起こる問題だけとし、この既定方針は再相談しない。通常の保存中再入場・連続入力・正常終了は別の構造要件（§6.1）。

## 5. session state・設定変更・既存 reset

context は保存行の複製を正本にせず、**表示する一つの調整 state**を持つ。
案は `SpreadShiftState::None | Adjusted { anchor_ref, resolution, persistence }`。
resolution は accepted items 世代における `Resolved(idx) | NotInView`、persistence は
`SessionOnly | Saving(seq) | Saved(seq) | SaveFailed`。組合せを enum の有効 variant に制限し、
別々の saved/pending/dirty bool を App に足さない。saved row は service の immutable snapshot が正本。
描画に渡す `optional_shift` は、この state から派生した世代付きの resolved anchor。
現行 `spread_shift_anchor_idx` は互換投影として残す場合も独立に書き込めず、順次この owner へ置換する。

全体設定 `remember_spread_shift_per_book: bool = true` は Settings の通常保存経路を使い、
settings transfer では portable な利用設定として含める（接点: `src/settings_transfer.rs:332`）。
本別 anchor データは settings transfer には含めず、metadata transfer に含める。
履歴 ON/OFF、履歴クリア、resume クリアで shift を消さない。

Q1決定済みの ON/OFF 規則（2026-10-10）:

| 操作 | 現在表示 / 保存行 |
| --- | --- |
| ON→OFF | 現表示を保ち session 扱いへ。全保存行を保持。以後新規 open は保存 shift を使わず、nudge は書かない |
| OFF 中に本を再入場 | 従来どおり調整なし。旧 row は休眠。HUD に「保存済み調整あり（記憶 OFF）」を出せる |
| OFF→ON | 今の session 調整がある**操作元本だけ**を保存する。調整なしならその場で旧 row を復元せず、次の本再入場から使う |
| siblings / parked の設定切替 | 現表示を変えず、保存済みと偽らない。ON化だけで全窓の session を一斉保存しない。以後の明示 nudge は最新設定に従う |
| OFF 中の明示解除 | この本の session と保存行を消す。UI は保存済みも解除することを説明。OFF toggle 自体はデータを消さない |
| Composed / ExtractedResult の HUD | 本の調整操作 group は無効。構成ありの HUD から global 設定変更もこの group 経由では発火しない。global 設定は環境設定で変更できる |

ON→OFF 前に送信した write は取消/巻戻しせず完了させる。OFF は toggle 後の新規受付から適用。
ON化元が本でない時は設定変更だけで、row 作成なし。既存表示を旧 saved 値で上書きする専用 live-rebuild は不要。

None 代入の分類と置換方針:

| producer / lifecycle | 共通 owner に渡す意味 |
| --- | --- |
| 本初回/再入場、ZIP 階層切替、PDF direction adoption | `Restore(scope, accepted_rows)`。base 値だけを読む段では session を確定せず、最終列と saved ref を一緒に解く |
| 明示 mode 変更、同じ mode のキー再押下、flow / direction 変更 | 現行のsession resetを保つ。ONならsaved rowもClearし通知（Q3決定）。OFFなら従来session resetだけ。Composedのmode-onlyは休眠値を消さない |
| favorite / 全体既定 / 文書方向の hydration、import | ユーザーのずらし解除と推定しない。受入済み規則と保存 ref から再解決。import の明示空は §9 の転送契約で解除 |
| page-edit の一時 Single pivot | mode 永続変更ではない。調整を保存/消去しない（現行 `src/app.rs:11937`） |
| row 削除・追加・sort/filter/F5 | identity を新投影へ解き直す。shift だけのための DB 書込なし |
| fullscreen close、本切替、fork、park/mount/deposit/drop | 当該 context の session を移送または終了。close 時に古い anchor を保存しない |
| G composition 作成/編集/解除 | §10 の LayoutSource で切替。保存/消去を付随させない |

mode/flow/direction resetで保存済み調整も消す規則はQ3で承認済み。
reset後すぐhydrationで旧saved shiftを再適用して、既存の基本pairingへ戻す操作を無効にしない。

## 6. open・restore・一覧変更の一つの採用点

保存行 snapshot は専用 worker で初期準備し、先行要求が完了した warm open は memory lookup にする。
cold 未準備・保存中・maintenance 中は既存 typed open/hydration continuation が依存を所有する。
既存 `SidecarLoadContinuation` / `FirstDisplayAction`（`src/app/sidecar_restore.rs:91`、`:104`）の
同じ open recipe に依存を足し、ZIP/document/retained の対応する受入点へ渡す。既存 base spread 読取は移管しない。
D2の所有はfresh-openが`Common.ViewerBookRestore`、retainedがbundle内の
`ViewerSpreadShift.retained: RetainedReadoption::Idle | Reading`とする。既存のtarget-context
semantic gate/dialogで自分の依存だけを待ち、別contextのglobal Commonやretained読取と併存できる。
全窓modalへまとめる案は、global sidecar recovery中も無関係なsiblingsを操作できるreleased契約を
損なうため採らない。読取待ちはcontextごとの一つのtyped ownerに載せ、全interleavingを救済する機構を加えない。
新しい shift_pending bool や、表示後の別 save-restore callback を作らない。
**キャッシュ済み PDF の即時 placeholder / 最初の表示を全ページ列挙待ちへ変更しない**。
最初の unit に使う saved ref はその受入経路で準備し、後から基本 pair→復元 pair と画面を飛ばさない。
通常 warm fast path に新規 DB round-trip を強制しない。先行保存が未完了の時だけ、その本の
最初の復元 unit の確定を continuation で待つ。placeholder・一覧の準備は並行できる。
それ以外で従来 fast path を遅くする変更が必要なら、測定値と代案を示して利用者に相談する。性能は未測定。

採用順は `scope + rows/identity index + base rules → LayoutSource → shift resolution → resume target
→ canonical unit / 正規化 → first-display`。UI はこの完成結果を一回採用する。
明示ページ指定・しおり・同セッション位置は従来の優先順位を維持し、shift のために saved resume へ強制移動しない。
保存された resume は同じ実ページ identity を復元し、そのページを含む復元 unit の anchor へ正規化する。
「同じ HUD ordinal へ行く」より「同じ実ページを含む組」の再現を優先する。
resume と shift を一つの DB transaction にまとめる必要はない。
**通常の再入場は両 writer の先行要求完了を満たした結果だけを採用する**（§6.1）。
異常終了時の二DB同時確定までは保証せず、§4.4の既定方針を適用する。

### 6.1 先行保存・resume・open の整列（R1 P2-1）

現行 resume は `record_book_resume`→enqueue（`src/app.rs:56845`、
`src/app/book_resume_meter.rs:181`）と別接続 SELECT（`src/app.rs:57039`）に分かれる。
`BookResumeWriter` は FIFO の Record/Read/Clear を持つが、Read は meter 全行であり位置読取ではない
（`src/book_resume_db.rs:327`、`:389`、`:447`）。この非同期書込と別接続読取の競合を放置しない。

1. 共通 open resolver が対象 scope を確定して復元要求を受ける時点で、先行する通常 navigation の
   resume 記録を従来どおり enqueue 済みにする。まだ捕捉されていない最終ページを旧 context から
   捕捉する場合も、既存記録 owner の一回だけで、新 open の背景 adoption から書かない。
2. App の一つの受付で `RestoreReceipt { open_id, context_id, scope, items_generation,
   settings_generation, shift_cutoff, resume_cutoff }` を作る。cutoff は **admitted** sequence。
   shift scope と既存 resume key は別入力（rootのみの resume 制限を維持）。
   scope/ref が同じというだけで、commit前の snapshot をこの receipt の結果として使わない。
3. 完了証明付きsnapshot（次項）で充足できない依存だけ、shift worker に `ReadAfter(scope, cutoff)`、既存 resume writer に小さい
   `ReadPositionAfter(resume_key, cutoff)` を同じ受付で enqueue する。各々の FIFO 上、先行 Set/Clear /
   Record/Clear が terminal になってから、同じ writable connection で位置/保存行を読む。
   結果は `cutoff, completed_through, row_or_position, status` を返す。失敗を無行と偽らない。
   resume writer の変更はこの位置読取と受付/完了 watermark・読取 snapshot の公開のみで、
   既存 Record の意味や DB schema、既存 spread setter は変更しない。
4. 既に先行完了を満たした immutable snapshot があれば同じ receipt を即時充足できる。
   resumeは行準備workerの従来読取結果にも、**読取transaction開始前にcutoff完了済み**で、
   開始前/終了後の受付・完了watermarkが同じことを証明させる。未完了Record中のSELECTは
   admittedが変わらなくても使えない。証明できなければ同writerのReadPositionAfterへ。
   warm path 用に writer の位置 snapshot を公開する。UIで別接続SELECTして pendingを迂回しない。
5. continuation は rows/identity index、rules/layout、shift結果、必要なresume結果がそろうまで
   最初の復元 unit を確定しない。両結果が receipt の cutoff を満たすことと、open/context/scope/
   items世代/設定世代の一致を同じ adoption で検証する。取消/別本への supersession は既存
   continuation を破棄するだけ。送信済み保存は App service が最後まで所有する。

復元 snapshot の線形化点はこの受付から enqueue した FIFO 読取点。**その読取より後の兄弟窓操作は
既に受け付けた open の値を差し替えず、次の再入場に反映する**。初期準備だけの古い snapshot は使えない。
「常に最新commitと一致するまで再試行」や後着save replyを新contextへ再配送する方式にしない。
explicit target 等で resume を使わない時も shift の fence は必要。記憶 OFF / Composed / A2 なら
shift適用用読取は不要だが、送信済み保存の完了ownerは残る。構成解除は新しいreceiptで再評価する。

例: `Set s1→ページ移動 Record r1→close→同本open` は `ReadAfter(s1)` と
`ReadPositionAfter(r1)` の両完了後に s1 の組と r1 の実ページを採用。
`Clear s2→close→同本open` は s2 の解除後の base unit から開始する。
旧窓のreplyを適用できなくても、新窓の復元結果は自分のreceiptから得られる。
この保証は通常動作の構造要件であり、§4.4の異常終了扱いへ押し込まない。

**正常終了**では新 producer の受付を既存終了経路で閉じ、最後の通常resume記録と
metadata取消の適用済みbatchを確定した後、shift FIFOを閉じて末尾までdrainする。
正常 exit の基点は `src/app/runtime_ops.rs:370`（metadata worker回収は `:377`）。
既存 resume writer の送信済み Record は Drop の FIFO drain/join（`src/book_resume_db.rs:476`）を維持する。
shift は process owner に終了receipt/worker join権限を渡し、UI callbackに新joinを追加しない。
既存 outer process の終了owner（`src/lib.rs:1899`）と同型に、drainと成功結果の回収・内容ID送信が
終わってから processを終了する。窓closeだけではwriterを終了せず、timeoutで正常成功を偽らない。
通常の正常終了後は全受理済み成功保存が残る。まれな失敗は §4.4 に従い、kill/停電は正常終了と区別する。

### 6.2 入口・Missing・一覧変更

入口の inventory:

- folder scan / 本 folder / 製本、`start_loading_items`、直接 fullscreen open、最初の自動 fullscreen。
- ZIP/RAR/変換 cache の初回・F5・履歴・startup prefix、ZIP enter/back、Ctrl+上下 / 兄弟移動 / slideshow。
  各 `materialize_zip_grid` / `zip_nav_show_current_level` の**受入済み実効 prefix**で共通 Restore。
  接点は `src/app.rs:35583`、`src/app.rs:35986`、`src/app.rs:39196`。
- PDF/EPUB の cached adoption / enumerate completion / password / lease / cancel / error。
  document direction と shift の adoption を分離しない（`src/app.rs:37454`）。
- しおり・読書履歴から本を開く、Rating/Collection/Smart の本 child、required target / 遅延 open。
  抽出 leaf を読む場合は本別 shift を持ち込まない。G の recipe へ接続する。
- F12 fork / parked / retained Book 復帰、metadata import refresh、Remote ContainerEngine。

深い ZIP は scope ごとの shift を保存し、該当階層へ再入場した時に復元する。
**現在の book_resume は at_root 以外に保存位置を作らない**（`src/app.rs:56824`）。
そこでは履歴/しおり等が指定した階層・ページまたは既存の先頭を使い、shift だけを復元する。
存在する saved reading position と一緒に復元する要件は満たすが、outer ZIP reopen で最後の子章まで
自動再開する新 cross-book resume は本項へ便乗して導入しない。必要なら別 scope として相談する。

解決結果がない時は row を保持し、raw index の別ページへ置き換えない。
本内の filter / 種類除外なら `NotInView` とし、現在の view は shift=None、filter 解除後に再解決。
実削除/外部改名で Missing の場合も保存値を保持し、HUD に基点不在を示す。
移動ボタンは無効、解除は可能。同名 identity が復帰したら自動で再解決する。
一覧を開いただけで missing 行を purge しない。

削除の integer remap と identity index は既存 `remove_items_batch`（`src/app.rs:43053`）の同じ採用で更新する。
生き残る anchor の新 idx と世代を同時に採用し、anchor 削除なら未解決へ。
F5 待ち・近いページへの補正・全 cache reset で隠さない。prefix 外へのページ移動でも古い本に Missing を保持する。

## 7. 表示ユニット・キャッシュ・§1.342

Automatic の unit builder に渡す optional_shift は resolved anchor のみ。現行の横長境界、表紙、
forced page-alone、末尾補助、端配置、LTR/RTL の規則を再実装しない。
anchor が存在しても現在の規則で基本と同じ組になる場合は「基点指定あり」として扱い、
全 unit 比較で saved row を勝手に削除しない。

復元・nudge・解除・新 items 投影では、同じ owner の accepted state と
`SpreadDisplayUnitsToken`（items generation / mode / anchor / landscape epoch / page_alone / nav）を
一回で更新する。G 統合後は LayoutSource tag / composition token を追加し、
Composed token に休眠 shift を混ぜない。source/nav cache は同世代なら再利用できるが、
unit/geometry/seek unit 対応を旧 anchor のまま再利用しない。

unit 変更が表示に影響する場合は既存 `invalidate_fullscreen_display_unit_owners` と
`reconcile_fs_navigation_sequence_to_canonical_unit` の経路へ接続する
（同型の現行接点: `src/ui_fullscreen.rs:37439`）。
当該 context の display demand、seek、連結 layout、capture/holdover を更新する。
全部の fs texture / thumbnail / AI cache を捨てる根拠にはしない。
過去に実際に paint した holdover は過去の pair/role/geometry を保持する。
現行の shift 後 capture 回帰（`src/ui_fullscreen.rs:57428`）を維持する。

§1.342 の specified fit / PDF demand / captured A/O / continuous reference を落とさない。
shift で unit の membership が変わった場合だけ既存の unit 変更経路で指定サイズの基準を再解決し、
設定値・最大倍率・manual zoom を reset しない。resize / raster 後着だけでは anchor identity を変えない。
Single/Split では shift を pairing へ適用せず、単ページの移動をそのまま使う。
連結も paged と同じ自動 unit を消費し、別の「連結ずらし」保存値は作らない。

### 7.1 HUD の具体的な表示と操作

既存「…」の modeless overflow panel に「見開きの調整」ページを追加する。
`FsOverflowPanelState` の排他的なページ variant と `FsOverflowPanelView` の pure 投影を使い、
独立した popup bool を増やさない。接点は `src/ui_fullscreen.rs:5366`、`src/ui_fullscreen.rs:5385`、
`src/ui_fullscreen.rs:5393`、`src/ui_fullscreen.rs:31748`。row 数・panel 高さ・hit rect を同じ view から解き、
小画面では本文をスクロール可能にし、戻る/閉じる/操作行の到達性を保つ。
既存 front-popup wheel ownership と背面 key/tap の抑制を使う。

表示例（ordinal は表示時に解き、保存しない）:

```text
見開きの調整
  調整あり — 本ごとに記憶済み
  基点: 1 ページ目 / 001.jpg
  [基点のページへ移動]
  [調整を解除]
  [✓] 見開きの調整を本ごとに記憶する
```

状態行は `調整なし` / `調整あり（この閲覧中のみ）` / `調整あり（保存中）` /
`調整あり（記憶済み）` / `保存できませんでした` / `保存した基点が現在の一覧にありません` を
accepted state から解く。OFF の旧 row は `保存済み調整あり（記憶 OFF・現在は未適用）` と分ける。
row の有無を画面に適用中の証拠にしない。基点 Missing/除外時も保存した ref の名前を示し、
数字を同 idx から再構成しない。

「基点へ移動」は anchor ref を同じ accepted items/nav へ解き、既存 jump/navigation owner へ渡す。
shift を変更・再保存しない。resolved でも現在 view の nav にない場合は無効、filter を解除する案内。
Single では実基点ページへ、Spread ではそれを含む current shifted unit へ移動する。
内部 anchor と現在 unit anchor の区別を保ち、キーを押したページを「基点」と呼ばない。
解除後は選択中の実ページを含む base unit へ既存正規化し、anchor ページまで強制 jump しない。
adjustment なし・saved row なしの解除は無効で、記憶済み Missing row の解除は有効。
OFF の解除 tooltip は「この本の保存済み調整も解除します」。構成ありでは group の操作を無効表示し、
G の理由と有効な「本の構成を編集」導線を並べる。

見開き control の近くの印は小さい painter dot とし、追加文字 glyph や新しい押しボタンを必須にしない。
active resolved shift なら薄い塗り、NotInView の調整なら輪郭だけ、tooltip で状態と基点を示す。
Composed は G の「構成あり」indicator を使い、休眠 shift の印を同時に出さない。
常時説明文で HUD を広げず、設定は全冊に効くことを checkbox の tooltip へ書く。

ON の操作は保存中を状態行/印の tooltip に出し、成功後に「見開きの調整をこの本に記憶しました」。
OFF は既存 nudge toast を「見開きを1ページずらしました（この閲覧中のみ）」とする。
明示解除の成功と保存結果は §4.3/§4.4 に従って一度通知する。
seq の古い成功で最新失敗の表示を消したり、別本へ移った後に新本の成功として表示したりしない。
通知の表示制御だけを理由に commit 事実を捨てない。

## 8. F12 / 多窓 / Remote の所有

### 8.1 local contexts

service と global 記憶設定は App 所有。scope、調整 state、解決 index、nav/unit/cache、
取消・世代・表示 demand は ViewerContextBundle 所有。
registry の default/destructure/swap/fork/park/mount/drop すべてへ同じ state を載せる。
通常 open/close/mount/fork は read-only で、mutation 通知や兄弟窓 reload を発行しない。

同一本を A/B が読む場合、**表示中のずらしは窓ごとの session**、保存は最後に受理した明示 action が勝つ。
A の保存で B のページや組を live に変えない。B が後から nudge すれば B を保存。
B の close / passive capture / mount だけでは旧値を保存しない。
再入場時は §6.1 の receipt / 先行要求完了を満たした snapshot を使い、retained Book も既存復帰 adoption
で同じ causal fence を通す。commit済sequenceだけの照合では先行未完了Set/Clearを見落とすため禁止。
retainedのpending receiverはbundle所有で、`ContextAsyncOwner::RetainedSpreadShift`がpollと
cancel-on-parkを扱う。parkはreceiverをdropし受入済みsessionを保持する。将来の再採用は新しいcutoffを
捕捉し、forkは受入済みstateだけをcloneしてreceiverを共有しない。
退避中の別本には何もしない。memory OFF の live/retained session は従来の移送契約を維持する。
復帰は現実ページidentityを保ってreceiptの読取点の保存値でunitを解き、共有resume/historyへ背景writeしない。

reply は要求元 context/operation を registry へ配送する。要求元が retired でも保存事実は service に残し、
今 mount されている別 context へ state/target/cache を適用しない。
closed で後着した Set が Clear を追い越さない FIFO と、同じ scope の A/B 受理順を決定的テストで検証する。
adoption は自分のreceiptのcutoff/完了証明とcontext世代を照合する。
受付後の別窓saveを理由に復元を繰り返さず、§6.1 の固定した読取点を採用する。
修復 delay / try_lock+sleep / blanket sibling reset は不要。

detached 憲法への適合: 新しい detached bool、placement/viewport/geometry 捕捉経路、時間窓を足さない。
実装時は設計担当と独立 reviewer が「context 所有の調整投影を拡張する構造的変更であり、症状パッチではない」
と明示合意した後、触れる registry / predicate / viewport 範囲と理由を detached-rework-plan.md §11 へ記録する。
今回は同プランを書き換えず、将来の gate とする。

### 8.2 Remote

Remote は独立した表示 owner。PC の fullscreen_idx / session shift を使わない。
core ContainerEngine worker は同じ logical scope resolver、専用 DB の immutable read snapshot、PageRef resolver、
Automatic optional_shift / Composed builder を使い、Web は返された PageGroup を描くだけ。
現行 core の read-only spread 接続（`src/remote_ipc/container.rs:2599`）へ第二 writer は足さない。
shift 書込は IPC→App 所有 SpreadShiftService だけ。既存 spread 書込は現行 App setter のまま。
core の worker に shift snapshot の登録/取得を加え、DB準備は背景で、表示ownerはRemote自身。
`RemoteWriteRequest` の共通 owner は
`crates/remote-ipc/src/lib.rs:603`、現行 protocol は同ファイル `:31` の v69。
必要な payload 変更は統合時の最新版+1と Web asset token を一緒に更新する。

記憶 ON の Book open は PC 保存 shift を解き、OFF なら適用しない。
force_single_page / 端末 orientation は表示投影だけで、保存行を clear/save しない。
応答 budget 切詰めより前の full 本 scope で anchor を解く。途中応答に anchor がないことを
保存値 Missing や本の末尾と誤判定しない。group の相方・slot・role は同じ accepted layout から生成する。
resume は従来の RecordReadingProgress と再開規則、shift は別の保存正本を読み、履歴設定に依存しない。

現行 Remote に一ページ nudge の機能はない。**本項では新規 Remote nudge editor / memory toggle UI を追加しない**。
保存済み shift の復元・group 一致と、既存 SetSpread 等の Q3 reset / G の拒否が必要な接点。
将来 Remote shift 操作を追加する場合も同じ common action / IPC service を使う。
構成ありでは G の同じ無効理由を返し、休眠 row を削らない。

取得 barrier は既存の全 local viewer close（`src/remote_ipc/ui.rs:1081`）を使う。
close 自体は保存を生成しないが、取得前に受理した shift write の一回の完了と read snapshot 公開を
FinishAcquire 前に drain する。G editor の保存ticket、必要なresume先行要求の完了も同じ取得continuationの
前提に合流し、最新 snapshot をcoreへ渡してからFinishAcquire。queue mutexとSessionHandle.innerの
短い取得commit境界を混同しない。service は UI 待ちを作らず既存 session phase へ完了を返す。
Remote の返却では既存一覧 reload、次の PC open で同じ保存値を解く。
PC→Remote の設定変更検出は acquire refresh。Q3 により Remote の SetSpread が shift を変更した時は
shift の結果を同じrequestのcontinuationへ返してから、既存 write-success invalidation /
remote_state_generation と container group cache を更新する。base setterを非同期化する意味ではない。
新しいRemote openも必要な先行shift/resume fenceを通し、古いsnapshotのgroupを先に公開しない。
余分な毎ページ pre-flight、home の global 設定を generation へ足す経路は作らない。
Web の rendition key には既存の partner/slot と受入済み layout identity を使い、旧組の合成を再利用しない。

## 9. 専用 DB の全 lifecycle / rename / move / copy / purge / 転送

### 9.1 maintenance 登録

| 接点 | 必須契約 |
| --- | --- |
| data-dir / 起動 / 保存先移動 | `data_dir::get()`（`src/data_dir.rs:56`）の直下 `spread_shift.db`。通常/portable/`--data-dir`/test overrideを既存共通解決に従わせ、固定APPDATAや別worktreeを参照しない。起動workerがopen/schema確認/read snapshotを作る。data-dir一式の移送ではDBと存在するSQLite家族ファイルを含め、稼働writerを閉じてから扱う。既存settings単体backupとは別 |
| writer / 終了 / maintenance | App所有service一つ。通常は専用worker、共通保守期間だけ §4.1 のleaseを渡す。`rename_migration_writers_busy`（`src/app.rs:45417`）にも未完了command/未消費成功結果を登録し、旧key write後の移行を保証。通常close/forkでは閉じない。正常process終了は §6.1 のdrain→接続close→process側join。未消費成功reply/内容ID観測も保守開始前に完了 |
| `rename_key_migration::STORES`（`src/rename_key_migration.rs:939`） | `spread_shift.db / spread_shifts / path / unique=true / KeepDrive / OptionalInLegacySchema` を登録（constructorは `:914`）。未作成ファイル/保守probeの欠落tableは正常不在。serviceが検出した不正DBは §4.4。対象descriptorのlogical slash-family capabilityも登録 |
| root 改名・アプリ内 folder 移動 / copy | 永続storeのexact + `/` + `::` の既存 family 処理。相対 payload は root 改名では不変。destination があれば既存 conflict 規則を守る。rootの永続移行は開いているcontextの参照元・items・shift scopeを移さない（2026-10-11決定、§11.6） |
| 個別画像の改名 / 製本順替え | key だけでなく payload の RelativePath を同じ旧→最終 path mapping で変換。しおりの mapping（`src/book_bookmarks.rs:1102`）を共有。scope 内だけ更新、scope 外移動は旧本の Missing を保持し、新本へ shift を勝手にコピーしない |
| 外部 folder move / rename | Folder は内容 ID 非対応（`src/content_identity.rs:62`）。shift の新 sidecar を作らず、既存 per-book spread と同じく明示 metadata export/import を案内。旧 DB 行は保持。アプリ内移動との違いを明記 |
| 書庫/PDF/EPUB 外部移動・内容 ID restore | 論理元 root の observation と既存 copy/probe に参加。成功Setとimportのcommit済追加/置換が捕捉rootへEditを送る（下記）。destination既存値は上書きしない。shiftは元rootのexact/slash-familyだけをコピーし、cacheの物理base spreadや既存source/cache四面mappingは別descriptorのまま |
| 本/root の実削除・明示 purge | STORES の同じ hard purge に参加。ページ一枚の削除では親 shift を削らず Missing とする。cache の容量回収は元本の shift を purge しない |
| metadata cleanup / 件数 / 全行clear | STORES走査と `src/metadata_cleanup.rs:1027` のstoreラベルに「見開きの調整」を追加。KeepDriveは既存物理root判定（`:1051`）、切断driveはProtected。anchor不在は孤児本としない。専用serviceにcount/clearコマンド、専用storeの件数・削除結果を登録。既存SpreadDb::count/clear_allには混ぜない。全行削除は明示された当該メタ情報削除だけ。履歴/読書位置/thumbnail/cacheのclearは非対象 |
| 設定復元 / 完全reset | 現行 `settings_restore::full_reset_with_permit`（`src/settings_restore.rs:725`）はsettings家族だけを削除する（`:797`）。spread.db等と同じくshift DBは保持し、設定boolは復元/既定値へ。既存「設定を完全リセット」の削除範囲を拡大しない。全shiftを消す操作は上記の明示clear/cleanupで別に所有し、settings単体backup/restoreにDBを混ぜない |
| metadata export/import | §9.2 のlogical専用family、schema初期化/ATTACH/journal確認/preview/summary/changed flags/refreshまで登録。保守leaseの間だけimport workerがwrite。exportは先行保存fence後のread transaction |
| Remote | ContainerEngineに専用snapshot/lookup/identity resolutionを登録。coreはwriterを持たず、IPC→App serviceで変更。取得/再openの先行完了・公開は §8.2。休眠/Composedの変更でcomposition tokenを進めない |
| import / rename / purge 完了 | durable service snapshotを更新する。importのshift再採用は実際にcommitで変更したscopeのcontext / retained dependencyだけ。ページ改名は同じ本のPageRefへmappingを反映し、purgeは既存の対象限定削除経路に従う。root改名のdurable移行はopen contextやshift ownerのroot/scopeを移さず、pending openも旧scopeを保つ。単なるopen/closeは失効通知を発行しない（§11.6） |

**copy と restorable probe に共通の logical slash-family（R1 P2-3、G も共有）**:
`restore_copy_mappings`（`src/content_identity/restore.rs:414`）は root の Exact と `::` の VirtualPrefix を作る。
`copy_stores_at`（`src/rename_key_migration.rs:1254`）も VirtualPrefix を `old_key::` としてコピーする。
probeも exact と `key::` のみ（`src/rename_key_migration.rs:1371`）。新scopeは `root/level_prefix` なので、
**STORES登録だけではnested本行がcopyされず、nested行しかない復元元は候補から外され得る**。
`drop_sources_with_nothing_to_restore` は台帳restorable flagも下ろす（`src/content_identity.rs:1491`）。
copy/probeが共有するdescriptor capability `LogicalBookFamily` と同じkey-range生成helperを追加する。
shiftとG composition等の指定descriptorだけに root exact + root `/` descendant を有効にし、
他のpage storeの意味を広げない。slash rangeは境界付き半開区間（`root/`以上、`root0`未満）、
LIKEの`%`/`_`を使わず、`book.zip2`等の別本を拾わない。copy先prefixも同じhelperで変換。
root mappingからcopy範囲とprobe範囲を同じ正規化で生成し、slash行の存在をroot台帳の復元可能性として数える。
raw page の `::` copy、既存 source/cache の四面 mapping は維持する。root だけ成功する試験で完了しない。
構成側にも同じ scope copy が必要だが、G の PageRef や LayoutSource の再設計は不要。

**内容 ID の観測契約（R1 P2-4）**:
新規Setのcommit成功、およびmetadata importで実際にcommitしたshift追加/置換は、
操作準備時に捕捉した `BookScopeKey.logical_root` を既存
`record_content_identity_for_path(path, ContentIdentityTrigger::Edit)`（`src/app.rs:75947`）へ渡す。
current mountからrootを作る `record_current_container_content_identity`（`:75957`）をreply時に呼ばない。
nestedはouter logical root、変換書庫は元書庫、PDF/EPUBは元ファイル。Folderは既存非対応で送らない。
Clear・coverageによる削除・skip・同値で実変更なし・rollbackした項目/未commit batchには送らない。
importは項目SAVEPOINTの成功だけでは観測せず、batch commitでroot集合を成功結果として公開する。
取消時もcommitした現在batchを含む適用済み集合だけを引き継ぐ（`src/metadata_transfer.rs:955`、`:1010`）。
Appの保守完了ownerが集合をdedupしてrecorderへ送ってから完了を回収する。
終了でimportが回収された場合も同じ成功集合を送る。未閲覧本のimportにも観測を作り、
表示窓/refreshの有無・履歴設定に依存させない。既存recorderのworker/内容hashを再利用し、shiftをhashへ混ぜない。
観測/保存のまれな失敗は §4.4 の既定方針に従う。
受入は**nested shiftだけ（他の復元行なし）のrootが候補検出→flag保持→復元→nested再入場まで通ること**、
および**未閲覧本へimport→外部移動→内容ID候補→shift復元**の両方を必須とする。

### 9.2 metadata export/import

**含める**。既存本別 spread と同じ明示 metadata 操作、既存一括確認、バッチ / SAVEPOINT、
取消時の適用済み batch 保持に合わせる。resume / 閲覧履歴を shift と一緒に転送することはしない。
自動 mimageviewer.dat backup には含めない（既存 spread が自動 sidecar 対象ではないため）。
G4F の構成 sidecar へ dormant shift を便乗して書かない。

ただし物理 origin は同じ欄を使えない。現行 `PortableContainerState` は spread/trim 等を
一つの `container_key_base` へまとめる（`src/metadata_transfer.rs:379`、`:453`、`:2436`）。
新 shift をそこへ足すと、元 RAR shift と cache ZIP spread が正当なのに origin 競合になる。
G §7.3 と同じ分離理由で、**logical 本設定の shift family**を portable entry へ追加する。

```text
spread_shift_family: { coverage, scopes: [{ scope_prefix, anchor_ref }] }
```

root は bundle の logical 対象 entry に所有させ、cache origin を持たない。
G composition_family と同じ scope/coverage codec を共有できるが、値の所有・空の意味は独立する。
既存 container_state の source/cache origin 判定・挿入 key・既存不一致検出は変更しない。
画像本 folder の搬送 owner は folder entry で、子 Image の個別編集として送らない。
全保存行（memory OFF / composition 休眠 / anchor Missing を含む）を搬送する。

- field/section がない旧 bundle: shift は保持。
- 新 section の covered 空: その covered scope の保存 shift を解除。
- 非空: 記載されたcovered scopeの一冊一行をreplaceし、**coverage内の未記載scopeは解除、coverage外は保持**。
  例えばfamily全体を搬送しrootだけshiftあり・子章は未記載なら、搬入先の古い子章shiftは削除する。
- skipped/Missing/KindMismatch 等で本自体が受入対象でない: shift も変更しない。
- composition が存在しても dormant shift の import 自体は可能。明示 metadata 取込であり、
  禁止された HUD shift action ではない。Composed 表示には適用しない。G の解除後に同じ規則で解く。

import target は logical root + prefix の KeepDrive key。相対 identity と scope を検証してから書く。
coverageはrootのみ/明示prefix集合/family全体を明示し、coverage外記載・重複scopeは拒否する。
非空/空いずれもcovered集合を一つの論理familyとしてdelete→記載scope insertする同じ項目SAVEPOINTを使う。
旧字段不在はこの処理を一切発火せず、空や未記載scopeへ変換しない。
現在の v9 / v8 / v7 入力を保つ（`src/metadata_transfer.rs:27`）。出力版は G と統合した最新版+1を一度割り当て、
manifest / shard / section / is_empty / has_view_state / summary / changed flags を更新する。
新 DB はimport schema準備（`src/metadata_transfer.rs:4132`）とATTACH一覧（`:4298`）、
全participantのjournal確認（`:4320`）へ別名 `spread_shift` として追加する。
既存 `.cargo/config.toml:24` は `SQLITE_MAX_ATTACHED=20`。Gの同時追加後も上限内を確認し、
shiftだけ別commitにしてfamily内のdelete/insertが分離しないよう既存batch transactionへ参加する。
保存serviceの接続はlease前に閉じ、ATTACH書込との常時並走を作らない。
既存一括確認に調整の追加/置換/解除数と上書き説明を足し、本別 prompt や新カテゴリ選択は作らない。
refresh は既存 context 付き worker（`src/app/metadata_import_refresh.rs:54`、`:534`）へ logical shift を加え、
`src/app.rs:44563` の一律 None を Restore へ置換する。import は記憶 OFF の現 session を勝手に変えず、
ON の Book と再入場依存だけを更新する。Composed の cache には shift の変更を反映しない。

## 10. G が消費する固定インターフェース

次の意味契約を先に共有し、実装順によって同名型の duplicate を作らない。

**Rust配置決定（2026-10-10 coordinator）**: 永続keyの正本はcrate-level
`src/book_scope.rs` の `BookScopeKey { logical_root, level_prefix }`。
KeepDrive正規化・実効prefix正規化・scope/coverage codecをこのmoduleだけが所有し、
`archive(logical_root, &[String])` / `document` / `folder` constructorを供給する。
Gの`src/app/book_materialization.rs::BookScope`はProjectionRecipeの意味分類
（Archive/Document/ImageFolder/CompiledFolder）として維持する。
G3がそのenumから上記constructorへadaptする消費点であり、G側でkey正規化を再実装しない。
E固有のC1–C12 receipt/state/request/replyはcrate-level `src/spread_shift.rs`（`model`/`service`/`store`）に置く。
D1はpure契約とworker/lifecycleを供給し、現viewerのRestore/表示/inputへの接続はD2で行う。
`EffectiveLayoutSnapshot<T>`のtokenはaccepted layout ownerの既存tokenをそのままcaptureする。
shift側で代替hashや新しいlayout世代を生成せず、units/role/placementと同じcaptureに含める。
`ResolvedAutomaticRules.page_alone`は既存の実効`PageAloneSettings`を保持し、
`after_cover`と`last`を独立にAutomatic結果とPreparing captureへ渡す。

提案する公開型/入口（DBの接続や保存rowをGへ公開しない）:

```text
ResolveShiftRequest {
  receipt: RestoreReceipt,                  // §6.1 の受付・完了証明
  cause: FreshOpen | RetainedReadoption | CompositionRemoved,
  scope: BookScopeKey, manifest, nav_order, identity_index,
  rules: ResolvedAutomaticRules, memory_policy, layout_eligibility
}
ResolveShiftReply {
  receipt, manifest_generation, nav_generation, maintenance_epoch,
  state: SpreadShiftState,                  // None / Adjusted(ref, resolution, persistence)
  optional_shift: Option<ResolvedShift>     // 内部anchor ref + accepted idx + 世代
}
resolve_book_layout_source(prepared_recipe, request)
capture_effective_automatic_layout(context_token) -> EffectiveLayoutSnapshot
admit_pairing_action(context_token, action) -> Accepted | Rejected(reason)
```

Resolveは本復元のためのread-only API。FreshOpen/構成解除は現在の記憶policyとfenced保存行を使う。
Retainedは §8.1 の復帰契約、memory OFFの既存live移送は保存行で上書きしない。
optional_shiftは符号offsetや押したページでなく、**当該manifest/navで解決した内部re-pairing anchor**。
返答はreceipt/manifest/nav/設定世代が合うcontextの一回のadoptionだけで消費できる。
通常saveの後続cutoffは既受理openを差し替えない。maintenance epochが変わったrecipeは
既存reload/continuationで新snapshotを採り直し、shiftだけ古い列へ足さない。
Composed/Unreadable/A2はGの資格判定で先に排除し、このshift復元requestを作らない。
既存base spread規則の解決ownerは変えず、その結果だけをAutomaticへ渡す。

| 番号 | G の消費点 | 1.364 が供給する正確な契約 |
| --- | --- | --- |
| C1 | scope / PreparedBook（G3 adapter） | BookScopeKey の logical_root + raw 実効 level_prefix、KeepDrive key、階層 fallback なし。Gの意味分類BookScopeはconstructorへadaptする。physical base spread key は別入力 |
| C2 | `resolve_book_layout_source(scope)` | 構成 snapshot を先に判定。Composed なら shift の取得/解決/適用を行わず Composed を返す。構成なし Automatic だけ `resolved_rules + optional_shift`。構成読込不明をなし扱いしない |
| C3 | Automatic optional_shift | 上記request/replyと §6.1 の先行shift/resume完了を供給。ON新規入場はfenced saved ref、操作/Preparingはaccepted live ref。OFF新規入場はNone。Missing/filteredはNone + 保持HUD状態、旧形式行はない。DB未作成はNone、不正/未知schemaは §4.4 に従い、raw代用なし |
| C4 | 作成初期値 | Preparing受付の短い境界でscope/items/nav/rules/LayoutSource/live shift tokenと現在units/role/placementを一体capture。ON/OFF/Savingを問わず**現在の実効live組**。saved storeを再読込せず、save replyで初期値を差し替えない。saved rowを消去/更新/新規作成しない |
| C5 | Composed command gate | Single/Splitの通常nav fallbackを先に分離。その後PairingEditAvailabilityでComposed/Unreadable/A2を判定し、全producerと再送をshift変更前に拒否。record/revision/shift/global binding/resume/unit/cache のwriteゼロ。拒否理由とeditor案内を返す |
| C6 | 構成解除 | 新 Book adoption で現在の記憶設定・scope・identity 規則から一回再解決。ON + resolved のみ復活、OFF/Missing は復活しない。G 独自 fallback/offset は作らない |
| C7 | G Q1 休眠 | 構成作成/編集/保存/取消/保存失敗の全経路でspread_shifts rowを維持し、shift APIのwriteを付随させない。構成ありのOFF→ONもsaveなし。事前に受理済みE保存はEが完了ownerを保持。store二DB transaction/shift消費receiptは不要 |
| C8 | cache / multi-window | receiptのcutoff/completed_through、context/items/nav/設定世代とmaintenance epochの合う一回の投影。commit済seqだけで通常再入場を通さない。replyは元operationだけ、後着で新contextへ適用なし。Composed tokenに休眠shift seqを混ぜない。fork/read/closeはwriteなし |
| C9 | editor / HUD 説明 | 「本の構成で設定されています。組を変えるには本の構成を編集してください」。local editor 導線は G、shift group/marker は 1.364。Remote は PC 編集案内のみ |
| C10 | maintenance / metadata | Eが専用DBの全 §9 lifecycleを所有。descriptor限定slash-family helperをcopy/probe共有し、nestedのみでも候補/flag維持。Set/importのcommit済追加/置換は捕捉logical rootを既存recorderへ。coverage内未記載は解除/外は保持/旧不在は保持。実変更scopeのみE snapshot通知、G revision/transactionへ混ぜない。PageRefとscope/coverage codecだけ共有 |
| C11 | Single/Spread/Split mode-only | G の構成あり mode-only は dormant shift を Clear しない。既存結合 setter からの自動 reset を流用しない |
| C12 | 限定例外 / 共同fixture | document identityはG Q6既決定に従う。§12のQ1〜Q5決定をEの実効契約としてGへ供給。Gはfallback/記憶規則を再実装しない。§11.2のD1→D2→D3とG3接続で下記8項目の共同fixtureを採択する |

ExtractedResult は G A2 固定 Automatic(shift=None)、OrdinaryList は G の元 scope の操作資格を使用。
構成「存在」と表示 recipe を混同し、mixed/本扱い OFF で外部 shift を許可する裏口を作らない。
構成判定不明時は shift 編集を拒否して案内するが、単ページの従来移動を失わせない。

G0 handoff（`C:/home/mimageviewer-nextbook/target/G-1242-g0-msg.txt:22`–31）との対応:

| G0の8要求 | 回答する契約 / 共同確認 |
| --- | --- |
| 1 request/reply・世代・optional_shift | C2/C3/C8、上記型、§6.1。先行保存中再入場・世代違いreply |
| 2 保存資格/scope・元/物理cache | C1、§3。ZIP階層/変換元/PDF/EPUB/folder/OrdinaryListの別key |
| 3 identity→manifest/nav・Missing/旧/OFF | C3/C12、§3.2/§5/§6。ON/OFF/欠落/復帰、旧store未作成 |
| 4 Preparingの現在実効live組 | C4。保存中/ON/OFFで現unitsをcaptureし、DB再読取で変えない |
| 5 作成/取消/失敗で休眠維持・解除時再解決 | C6/C7。作成/保存/取消/失敗のrow不変、解除ON/OFF/Missing |
| 6 共通受付順とComposed/Unreadable/A2拒否 | C5/C9/C11。全producer拒否前writeゼロ、Single/Split移動維持 |
| 7 lifecycle担当・通知とG transaction分離 | C10、§9。nestedのみprobe/restore、import観測、coverage未記載解除、reset保持 |
| 8 joint fixtureと統合順 | C12、§11.2–11.3。E契約採択→D1/D2→G3/D3、同本兄弟session/再入場/A2/OrdinaryList |

## 11. 状態削減・実装 chunk・受入条件

### 11.1 明示的に検討した簡素化

| 検討 | 採否と理由 |
| --- | --- |
| 単一 owner | 採用。context 調整 state と service の durable snapshot を責務で分け、HUD/キー/gamepad/Remote を同じ資格へ。画面ごとの saved/pending bool を増やさない |
| 単一 commit 点 | 採用。Set/Clear の一行 transaction 成功だけで saved publication。読み取り・close・resize・初期化から write しない |
| 先行保存と再入場の全interleaving | 固定receipt + writer FIFO読取を既存open continuationへ合流。admitted/committedを区別し、後着reply救済/永続pending/二DB同時commitなしで通常再入場の組合せを削減 |
| 既存 open / reload / index / unit paths | 採用。準備 worker と既存 adoption に Restore を含める。設定 OFF で全窓を閉じる専用 rebuild は不要 |
| retained読取待ちをglobal Commonへ合流し全窓modalにする | 不採用。別窓のglobal sidecar復元中も無関係なsiblingsを操作できるreleased契約を失う。fresh-openはCommon、retainedはbundle内の一つのtyped `RetainedReadoption`が依存を所有し、既存target-context semantic gate/dialogで対象だけを止める。全interleavingの救済機構・全窓停止を追加しない |
| composition と shift の全組合せ | G 決定の二択 LayoutSource で排除。Composed は休眠保持だけ、解除時に一回再評価 |
| 保存を modal にして navigation/F12 を止める | 不採用。連続 nudge / 普通のページ送りが遅くなり、手間も増える。短い一行保存の scope/ref を捕捉すれば DB write で navigation を待つ必要がない。必要だと判明したら利用者へ相談 |
| 全窓に最後の saved shift を即時反映 | 不採用。読んでいる兄弟窓を移動/再pair させる。session は窓別、保存は last explicit action、再入場で最新値という単純規則 |
| OFF 時の全データ削除 | Q1決定により不採用。不可逆な利用者データ損失。保持+新規 read/write 停止で済む |
| spread.db再利用 / 既存writer非同期移管 | R2で不採用。連続setterが旧全状態を捕捉する問題と、commit後mode採用/nudge/resetの組合せが増える。同じDBの常設第二writerも作らない |
| shift専用DB / worker | R2採択。既存setterの同期契約を維持してP2-2を設計から除く。shiftだけ単一owner/commit点、増えるDB lifecycleは §9 に全登録。全状態の差分mergeや入力modal停止は不要 |
| maintenanceの別同時writer / 専用rebuild | 不採用。既存modal/workerへwriter leaseを渡し、同じ完了経路でsnapshot再読込。新openもその依存を待つ。新たな全窓live-rebuild網は作らない |
| 毎 open の anchor 内容 hash / 近いページ探索 | 不採用。既存 stable identity を越えるコストと誤適用。Missing を残して明示解除できる方が単純 |
| まれな失敗の追加機構 | §4.4の既定方針を採用。まれな失敗の組合せを仕組みで塞がない |

既存利用を削る候補は保存modal、warm PDF全列待ち、既存mode setterの非同期化、
兄弟窓強制更新、implicit resetの保存行削除。前四者は本案で採用しない。
先行保存中の同本再入場は誤復元防止に必要な完了依存のみ。通常nudgeやページ送りの入力は止めない。
OFF→ONで操作元sessionを保存するQ1とimplicit resetの保存行削除Q3は承認済み。
まれな失敗はQ4で確認済みの既定方針を適用する。これらに未決質問はない。
新規DB待ちやmaintenance入力停止が通常利用を遅くすると測定で判明したら、
実装担当は costs/options を返し、未承認の挙動変更を採用しない。
G の構成あり操作禁止と A2 は既に承認済みで、追加の承認待ちへ戻さない。

### 11.2 coherent chunks と責任

1. D0: R3の決定済み契約を前提に、独立 `gpt-6.1-sol / xhigh` が設計そのものを review。
   設計担当が指摘を裏取りして構造合意、detached §11 の予定範囲を確定。
2. D1: shift専用store/worker、受付watermark/FIFO読取、既存resume writerの位置読取/fence、
   open/正常exitの完了契約を先に実装。§9のlifecycle/転送/観測/copy/probe登録とpure tests。
   既存spread setterは変更しない。G0とC1–C12/型の共通配置を合意。
3. D2: common shift owner、Restore、items/remap/cache、F12、HUD と通知を同じ chunk で接続。
4. D3: Remote saved resolution / group parity / reset、G3とC1–C12の共同fixtureをintegration、関連docsと最終gate。

実装・狭い検証は `gpt-6.1-sol / high`、独立設計/完了レビューは別 context の
`gpt-6.1-sol / xhigh`。今回は利用者指定の designer として文書だけを作成し、モデル再設定や reviewer の
採択を行ったとは主張しない。実装開始時は G/E 統合後の symbol を再照合し、一つの shared file に一 writer。

### 11.3 テスト計画（今回は実行しない）

| 層 | 必須回帰 |
| --- | --- |
| pure nudge / resolution | LTR/RTL、cover ありなし、横長・非pairable・page-alone・末尾・連続 nudge。 `(new=2,anchor=0)` の後者を保存。端 no-op。Single/Split は従来移動 |
| identity / store | 途中追加/削除/順替え、Missing/filtered/復帰、違う同idxを代用しない。cache再生成、nested chain/collapse、prefix間非継承、KeepDrive非衝突。G Q6のPDF/EPUB再編/reflow制限 |
| reopening / P2-1 | workerを決定的にpauseし、Set/Clear受付→commit前close→同本openで旧snapshotを採用しない。resume Recordもpauseして二つのreply順を反転、両cutoff充足後だけfirst unit。explicit target/履歴ON/OFF/retainedも同じ。正常exit直前Set/Clear/Recordをdrain→次processで値一致。異常終了は §4.4 の扱いで正常保証と区別 |
| open lifecycle | 本切替、履歴/しおり/required target/startup、warm PDF/password/cancel/error、F5。通常warmの余分なDB往復/全列待ちなし。取消したopen replyは不採用、送信済writeは元scopeへ完了。後続兄弟saveは次の再入場だけに反映 |
| cache / UI | 旧 unit token を事前生成→saved restore で新組。paged/連結/seek/capture/holdover、指定サイズfit・PDF demand・zoom保持。印/状態/実基点 jump/解除/保存結果、dark/light・小画面・touch到達 snapshot |
| common handlers / P2-2除去 | 四KeyAction、カスタムキー、gamepad/ring、HUD、mode再押下/flow/direction/import reset。既存mode→flowが同期採用状態を保つ、Set→mode成功Clear→SetはFIFO、setter失敗でClearなし。既存setter非同期化なし。Composed/Unreadable/A2拒否は全状態/DB/cache無変更、Single/Split movementは通る |
| async / 多窓 | A/B別本、同本別session、fork/park/mount/drop、A save→B clear の順、旧 reply 後着、open 中 store変更、元窓 retired。close/fork/read が write/兄弟失効を作らない |
| maintenance / P2-3 | root/page改名、製本順替え、scope外移動、root+nested copy。**nested shiftのみ・他store復元行ゼロ**で候補検出/台帳flag保持/内容IDcopy/再入場まで通る。隣接root・%/_・別drive・descriptor限定を検証。cache削除と元本削除の差、purge/専用count/clear/Protected drive、settings-only resetは保持 |
| transfer観測 / P2-4 | 未閲覧ZIP/PDF/EPUB/元変換書庫へshiftのみimport→捕捉元rootのledger→外部移動→検出/復元。nestedの観測はouter。cancel現在batchcommitを含む適用済追加/置換のみ送信、rollback/skip/削除は観測なし。別窓mountや履歴OFFに依存なし |
| transfer coverage / P2-5 | v7/v8/v9字段不在は全既存値保持、新空はcovered解除。**非空familyでrootあり/子章未記載は子章の旧値を削除**。明示prefix集合の外の子章は保持。skipは全保持。summary解除数も一致、family項目rollbackでdelete/insert共に戻る |
| transfer一般 | folder/nested、元RAR shift + cache spread/trim + 元RAR構成の同居、休眠/Missing搬送、unsafe値拒否、一括確認一回、context限定refresh、専用DB lease/schema/ATTACH/journal登録 |
| Remote / G | memory ON/OFF、PC と group/anchor parity、forceSingle、budget途中、取得write drain、返却、既存 SetSpread reset。構成作成/cancel/編集/解除、OFF休眠、mode-only非消去、A2/OrdinaryList資格 |
| 通知 / 性能 | §4.4の既定方針に従う基本通知と原本保持を確認し、まれな失敗の組合せ専用の回復試験は追加しない。UI同期I/Oゼロ、warm fast path維持、連打保存でnavigationをblockしない。余分なdecode/hash/全列照合なし |

narrow unit/handler/integration/snapshot を先に実装者が所有。既存 §1.342 と detached tests を削除・弱体化しない。
shared behavior を触る最終実装では `scripts/test-full.ps1`、fmt、UI glyph check を実施し、
`scripts/build-dev.ps1` で利用者用検証 binary を用意する。live smoke は別の事前承認 suite。
今回は documentation-only かつ利用者が build/launch 禁止を指定したため、それらは実施しない。

実装時の文書更新対象: spec/manual の本・見開き・設定・保存先/転送、architecture-overview、
display-pipeline、async-architecture、virtual-folders、keymap、web-remote、docs index、detached §11。
新キーで解除/jump を追加する場合だけ KeyAction 一式を揃え、既定キーは衝突確認の上で別途決める。
本案は HUD の操作のみを追加し、Ctrl+左右の既定割当は変えない。

### 11.4 D2実装・自動検証記録（2026-10-10、独立完了レビュー待ち）

**履歴として保持。** 本節のroot/page live mappingのうちroot追従は最新の§11.6で撤回された。
現在のroot改名契約とD2cの範囲は同節を参照する。他のD2接続と下記検証結果は当時の記録。

基点はD1統合後の`33e862221`。この節は`next-viewer`作業差分の実装箇所を記録する。
自動検証の実施結果は本節末尾に記録する。独立レビューと実機受入は未実施。

| 所有境界 | 現在のD2接続 |
| --- | --- |
| live state | `src/app/spread_shift_owner.rs::ViewerSpreadShift`にscope、`ViewerShift::None / Ordinary / Book`、現在writeの`ShiftCommand`、`FencedResume`、typedな`RetainedReadoption`、identity/nav投影を集約。`App`の旧raw anchor fieldを独立ownerとして使わず、`spread_shift_anchor_idx()`で描画indexを投影する |
| 保存受付 | `set_live_spread_shift`は実re-pairing anchorからPageRefを捕捉し、記憶ONのBookだけSetを受理。ordinary listの従来ずらしはsession調整。Q3 resetは既存base setter成功後に`reset_live_spread_shift(false)`へ、HUD明示解除は`true`へ接続 |
| reply | sequence/context/items/operation/scopeが同じ現在操作だけが`accept_write_reply`を採用。Appの一つのpollがmounted ownerかregistryのat-rest bundleへ配送し、保存完了からページ移動・兄弟unit失効を行わない |
| Restore | fresh-openは`sidecar_restore::Common`に`ViewerBookRestore`を保持し、`begin_book_shift_restore`でreceipt/cutoffを捕捉、`poll_book_shift_restore`で先行shift/resume完了と世代を照合。最終identity/navへ調整を解決してから同continuationの位置復元・first-displayへ進む。resumeは既存root-only資格を保持し、`finish_fenced_book_resume`が同open continuationの初回採用完了で依存を終了する。後のsession内resume参照を初回値へ固定しない |
| items/cache | `PreparedPageIdentityIndex::resolve_shift_identity`とnav membershipでPageRefを解決。Missing/filteredは`NotInView`でref保持。削除とsnapshot remapはlive resolutionへ適用。snapshot化ではdurable scopeを切り離して、その時点のnumeric pairingをOrdinary sessionに保つ。共通display-unit/cacheにはownerから解決済みindexを渡す |
| retained | ONの`begin_retained_shift_readoption`は受入済みindexを再利用。待機はbundle内の`RetainedReadoption::Idle / Reading`が所有し、global Commonと別contextのreadを併存できる。既存target-context semantic gate/dialogで対象窓だけを停止する。`ContextAsyncOwner::RetainedSpreadShift`がpoll/cancel-on-parkを扱い、parkはreceiverをdropしてaccepted sessionを保つ。将来の再採用はfresh cutoffを捕捉し、forkはaccepted stateだけをclone。OFFはlive調整を変更しない |
| import採用 | 捕捉FIFO読取はContextRequest workerで待ち、context/items世代の採用後にcurrent scope + ON + Automaticを照合して採用。OFFでは結果を使わない。retained/import両採用とも`adopt_shift_layout_without_navigation`で現在の実ページを含むunitへ正規化し、共有resume/historyを書かない |
| live移行 | `spread_shift_live_mappings`は成功済みroot/page操作と同じmappingをmounted/at-rest ownerへpureに反映。追加save/mountを発生させず、durable移行は既存maintenance workerが所有する |
| F12 | `ViewerContextBundle`にlive ownerを保持して既存swapで移送。forkは`fork_accepted`でaccepted layoutだけを複製し、元contextのwrite受付・receiverを引き継がない。保存待ち/失敗の調整は新contextでsession扱い。park/mount/drop/closeは保存producerではない。OFF/ONでat-restもsession扱いにし、操作元BookだけがON復帰Setの対象 |
| 設定/HUD | Settingsの`remember_spread_shift_per_book`はdefault/旧設定ともtrue。preferences採用点で世代とQ1を更新。settings transferはboolのみ。HUD `… → 見開きの調整`に状態、実基点jump、Clear、全体checkboxを追加し、見開きcontrol近くの印も同ownerから作る。新KeyActionは追加しない |
| policy変更中の依存 | 明示ON/OFFで`ViewerSpreadShift::session_only`はretained pending receiverを取り消してaccepted sessionを保ち、表示済みsiblingsへ旧保存値を復元しない。fresh-openのCommon依存は元resume fenceを破棄せず、`refresh_pending_book_shift_memory_policy`が捕捉済みtarget/context/items/open/scopeと元resume cutoffを保って、新settings世代・現在のshift受付cutoffで読取依存を再受付する。明示設定変更の通常遷移だけで、retry/まれな失敗の復旧ではない |
| 文書 | spec/manual/製品ページ/privacyにQ1/Q3、物理ページidentityのPDF/EPUB制限、metadata v10の本別調整搬送・外部移動を反映。architecture/display/asyncとdetached §11に所有境界を記録 |

retained readoptionとmetadata import後のcontext限定refreshは同D2 chunkに接続した。
構造補正としてretained待機をglobal Commonからcontext ownerへ移した。global sidecar recovery中も
別の兄弟窓が操作できるreleased契約がコードで確認されたため、一括modal化は採らない。
既存target-context semantic gate/dialog、async inventory、park取消を使い、別の全interleaving machineryを作らない。
この実装接続説明だけをC1–C12共同fixtureの受入証拠として扱わない。
G3のLayoutSource/構成解除統合、Remote saved resolution/group parity/reset、共同fixtureはD3。
detached §11の記録は実変更範囲と構造的な判断理由であり、独立完了レビューによる確認待ち。
利用者指定のD2検証matrixを実施した。全libは指定どおり1回実行し、11,848成功・55 ignored・
3失敗だった。失敗はwriterを迂回したresume fixture、初回`FencedResume`の寿命、HUD追加行による
高さ前提であり、期待する終了・現在位置参照・48px touch/右上配置を保持して修正した。
修正後のfocused 84件、sidecar restore 51件、startup restore 73件、overflow 5件、context registry
74件は全成功。保存待ちF12 forkと初回採用後のresume更新も追加回帰で確認した。
修正の影響を受けない全libの成功結果は再利用し、全libの非ゼロ結果をclean passとは記録しない。
metadata refresh 9件、`cargo test --test ui_snapshot` 155件も成功。HUDの8基準画像
（light/dark・通常/狭幅・解決済み/一覧外）と更新したroot menu画像を目視確認した。
`cargo fmt --all --check`、UI glyph検査（危険glyph 0）、通常/portable core check、
`scripts/build-dev.ps1 -PreserveRuntime`は成功。normal featureのdev-runtime coreとcompanionsを用意し、
製品は起動していない。実装者handoffは`target/D-1364-d2-msg.txt`へ保存する。
workspace full gate・共同fixtureはD3の最終統合へ、独立構造確認・完了レビューと別承認の実機確認は
§11.3の検証ownerへ引き継ぐ。コミットは利用者の指示どおり行っていない。

### 11.5 D2bレビュー指摘補正

**履歴として保持。** P2-6に対して採用したopen-context/shift scopeのroot移行と
同本pending-open直列化は、2026-10-11の最新決定§11.6で撤回する。
下記の判断・検証結果は撤回前の差分の証拠であり、D2cの検証証拠には流用しない。

基点は利用者がcommitした`8bc7efc87`。D2の独立実装レビューはREVISE（P2 7件・P3 2件）。
本節は指摘の修正と自動検証を記録し、修正後の独立レビュー合格は主張しない。
P2-6は利用者が**選択肢1（同一本の未完了openと製本管理の改名を直列化）を採択**した。
同じ本を開いている途中だけ改名を待たせ、旧keyでの採用が終わってから既存の改名・移行境界へ進む。
2026-10-11に実装と下記自動gate・確認用buildを完了した。修正後の独立完了レビューは未実施・未主張。

| 指摘 | 所有境界での補正と回帰 |
| --- | --- |
| P2-1 保守中の通常操作 | Set/ClearをD1の既存保守後FIFOへ受理。実lease中の明示Clear・Q3 setter、混合root/read/write順序・正常exit drainを検証 |
| P2-2 元resume読取の破棄 | `RestoreReadDependencies::refresh_shift_dependency`でshiftだけを差し替え、resume receiver/取得済み結果を保持。同本後着RecordとOFF/ON遷移、取得済み値の回帰 |
| P2-3 resume資格/キーの分岐 | 記録producerとcaptureで`book_resume_recording_folder`を共用。`at_root()`と物理`current_folder`を使い、collapsed wrapper ZIP・変換cacheの先行/後着Recordを検証 |
| P2-4 一時snapshot欠落 | maintenance epochをservice admission ownerに保持。保守後、別本writeがsnapshotを所有する間にも固定readの完了証明を受入できる回帰 |
| P2-5 無変更importの再採用 | manifest flagではなく`ImportCommittedChanges.spread_shift_scopes`をdispatcherへ渡す。無変更・別scope・Set・covered Clear・cancel適用済commitと同本兄弟sessionを検証 |
| P2-6 root改名のsource不整合 | 製本管理の同一本の未完了openだけを待ち、旧keyでの採用後に既存root移行境界でmounted/at-restの参照元・items・prepared identityとshift scopeを一緒に移す。別本・通常改名には追加待機を設けない。別本不変・再解決・ON復帰と未完了openの直列化を検証 |
| P2-7 一覧から固定の旧slot | anchor remapをfullscreen成功分岐からitems交換へ移す。一覧から`[1,3,5]`固定で旧anchor 3がslot 1へ対応する回帰 |
| P3-1 casefold衝突 | D1と同じ受入列の先頭slotを明示選択。通常準備・追加mergeの`A.jpg/a.jpg`衝突を検証 |
| P3-2 retired Clear通知 | replyに`ShiftWriteKind::Set/Clear`を保持。元窓を閉じたClear完了も解除通知となり、後継ownerは変わらない回帰 |

状態削減の採択: open continuationの新しい所有・保守中入力停止・resume再読取・兄弟一括復元を作らず、
既存FIFO、依存owner、commit scope集合、root移行、items交換の一つの確定点へ揃える。
まれな失敗は既定のログ・通知で扱い、session shift/原本を保持し、retryを追加しない。
HUDの表示・既定キー・Single移動・設定/転送形式・D3範囲はD2の契約を維持する。
`BookOpPending`はdocument-open/modal受付を止めず、改名の完了pollもsidecar復元中に動くため、
改名対象のLive Commonや未採用の`FolderPaneOpenPending`候補と競合し得た。
単純な取消・reloadはCommonのmutable sidecar予約・初回display continuationや候補のexact target/
EPUB restoreを失う。App-globalな`pending_reload`をat-restから立てる案も、元のmounted siblingを
reloadしてしまうため撤去した。typed continuation全体のroot移行と比較し、利用者は同一本の
open採用を先に終える直列化を選択した。追加のUI操作は不要で、別本のopenや通常改名に追加待機はない。
製本管理の既存pending ownerを`BookManagerPending::{WaitingRename(captured_request), Running(BookOpPending)}`
へまとめ、待機と実行を排他的に所有する。Appのread-only queryが全contextとglobal Commonの
既存source targetを正確に照合し、同一本の未完了openがなくなれば捕捉済み改名を既存workerへ渡す。
openは旧keyで採用し、既存のsettled境界で参照元・index・scopeを一緒に移行する。
openのreceiver・continuationの所有は変えず、取消・reload・retry・新しいpending-open ownerを追加しない。
最終検証（2026-10-11、基点`8bc7efc87`の未commit差分、下記最終command exit 0）:

- `cargo test -p mimageviewer --lib`: 11,873成功、0失敗、55 ignored（1426.62秒）。最終source変更後の全lib。
- focused: `--lib spread_shift` 94成功、`--lib book_rename` 5成功、`--lib book_root_move` 1成功、
  `--lib pending_book_open_query` 7成功、`--lib saved_group_actions` 17成功、Collection password lease 1成功。
  先行checkpointのidentity 5件・snapshot remap 1件も最終全libへ含まれる。
- `cargo test --test ui_snapshot -- --test-threads=1`: 155成功、0失敗。HUD 8 baselineを含め画像は変更なし。
- 通常／`--features portable`の`cargo check -p mimageviewer --bin mimageviewer-core`、
  `cargo fmt --all --check`、UI glyph（危険glyph 0）、`git diff --check`が成功。
- `scripts/build-dev.ps1 -PreserveRuntime`: normal feature setのcore・remote・EPUB PDF workerをbuildし、
  runtime/PE検証成功。このworktreeのcoreが動いていないことを確認して実行し、製品は起動していない。

初回全libは11,872成功・1失敗・55 ignored。追加したroot移行回帰が、空suffixの`join`で
addressへ末尾separatorを追加する誤りを検出した。参照元を移す共通helperでroot自体はtargetを直接
使うよう修正し、期待値を変えずfocused成功後に全libを再実行した。
並列UI snapshotは154件成功後に既存Meiryo Boldの1件が停止したため、このworktreeの自分の
snapshot test processだけを終了し、全155件を直列で成功確認した。停止した実行は成功扱いにしない。
初回dispatcher fixtureは実際の別窓一覧登録へ修正し、新規query fixtureの借用・本資格も修正してから
最終全libを実行した。ログは`target/D-1364-d2b-*.log`、実装者handoffは`target/D-1364-d2b-msg.txt`。
commit・製品起動・実機受入・修正後の独立レビュー合格は行っていない／主張しない。

### 11.6 D2c: root追従と同本pending-open直列化の撤去（2026-10-11利用者決定）

利用者は、名前変更後に開いている窓を追従させる機能を§1.364へ含めないと決定した。
source inspectionの比較基点はmaster `02079f95d`。製本root改名後、main一覧は旧pathを再読込し、
F12窓は旧path/旧itemsを保持する。この既存挙動を維持し、新rootへのlive移行・自動reopenを追加しない。
実機で観測した結果ではない。D2/D2bのroot追従と、そのための同本pending-open待機は本決定で撤回する。
Q1〜Q5、同一本の通常shift保存→再入場のfence、retained/import採用、snapshot remap、HUD、
Single移動、ページ改名、D3の範囲は変更しない。

**所有境界と撤去範囲**:

- D1の永続storeのrootキー移行（相対payloadは不変）とページ改名時のRelativePath payload移行、
  root操作・Set/Clear/readの単一FIFOは維持する。
  exact/slash/virtual family、destination conflict、内容ID/copy/probe/転送/maintenance登録は変更しない。
- D2で追加した、mounted/at-restの参照元・items・prepared identity index・addressのroot移行を
  撤去する。pending loadや結果世代の移行・再要求は追加せず、既存の寿命を維持する。
  shift ownerのroot/scopeだけを新rootへ動かす処理も同時に撤去する。
  旧itemsを持つcontextの操作を新rowへ送ると、新rootの正しい保存値を古い操作で変更してしまうため。
  ページ改名のPageRef反映は同じ本のscope内に維持する。
- 受付済みopenのsource/scope、receiver、continuationを旧contextのまま保つ。これをlive移行する
  境界がなくなるため、同一本pending-open query、製本管理の`WaitingRename`直列化wrapperと
  待機分岐を撤去する。既存の改名worker/pending operationへ戻し、新しいpending-open完了契約を作らない。
  受理済みshift write/read/root操作の順序はD1の単一FIFOだけが所有する。
- 古い窓からの後続Set/Clearは捕捉済み旧scope/keyへ送る。root移行後に旧keyを再作成・解除し得る点は
  利用者了承済みの割り切りであり、新keyへの転送・retry・open待機・自動reopenを追加しない。
  DB等のまれな失敗は既定のログ・通知・原本保持に従う。

**簡素化の判断**: live追従を維持して未完了openを直列化する案と比較し、
利用者は機能の組合せ自体を§1.364から外す方針を選んだ。released masterのrename挙動を変えず、
contextの参照元・items・shift scopeは同じ旧rootに留める。待機対象の照合・live移行・reopenを
新たな責務として持たない。D2bのP2-6に対する方針変更であり、guardや部分resetによる症状パッチではない。
detached側の撤去範囲と判断理由は[detached-rework-plan.md §11](detached-rework-plan.md#11-リワーク外からの変更記録)へ記録する。

**回帰と検証の担当**: 永続root/相対payload移行のD1回帰を維持し、mounted/at-restの旧scope・参照元・
items・identity・pending load・addressがroot改名で書き換わらない回帰へ変更する。
旧ownerの後続操作が新rowを変えないことと、ページ改名等の維持対象を確認する。
同本pending-open直列化を約束するテストは撤去し、openの旧scopeが保たれる境界を確認する。
実装担当が関連自動検証と確認用buildを担当する。
本記録は独立構造合意・完了レビュー、製品起動、利用者実機受入を表さない。

自動検証（2026-10-11、基点`352a32881`の未commit差分）:

- focused: `--lib book_rename` 4成功、`--lib spread_shift` 92成功、
  `--lib book_root_rename` 1成功、`--lib saved_group_actions` 17成功。
- 最終Rust変更後の`cargo test -p mimageviewer --lib`を指定どおり1回実行した。
  **11,862成功・2失敗・55 ignored、exit 101（1456.05秒）。全lib成功とは扱わない。**
  失敗は未変更の`video::audio::tests::zero_start_pump_trims_latency_and_preserves_samples_through_eos_loop_and_seek`
  （3秒内のtail完了）と
  `video::decoder::audio_track_fixture_tests::exited_audio_worker_rejects_pending_switch_and_keeps_video_seek_alive`
  （10秒内のworker終了）。単独再実行は各1成功、関連群の直列再実行はaudio 62成功、
  decoder 33成功・1 ignoredだった。音声コードや期限・期待値を変更していない。
  並列全体実行での期限超過の原因は確定しておらず、成功した関連群で全libの非ゼロ結果を置き換えない。
- `cargo test --test ui_snapshot -- --test-threads=1`: 155成功・0失敗（178.62秒）。
  HUD baselineを含め画像は変更なし。
- 通常／`--features portable`のcore check、`cargo fmt --all --check`、UI glyph（危険glyph 0）、
  `git diff --check`が成功。
- `scripts/build-dev.ps1 -PreserveRuntime`が完了し、normal feature setのcore・remote・EPUB PDF workerと
  runtime/PE検証を成功確認した。このworktreeのcoreが動いていないことを事前確認し、
  起動中のinstalled coreを停止せず、製品も起動していない。全libの上記2失敗は未解決の検証結果として残す。

最初のfocusedコンパイルではwrapperの旧method呼出し1か所と`PathBuf` import不足を検出し、
修正してから上記検証を実行した。初回compileと全libの非ゼロログを保持する。
ログは`target/D-1364-d2c-*.log`、handoffは`target/D-1364-d2c-msg.txt`へ記録する。

## 12. 決定済み事項（番号固定）

2026-10-10の利用者回答で全推奨案を採択。Q2はG Q6の既決定、Q5はR2 coordinator採択を
利用者が再確認したもの。以下は利用者への質問ではなく、実装・レビューが守る決定記録。

| 番号 | 決定 | 決定・確認日 |
| --- | --- | --- |
| Q1 記憶OFF/ON・明示解除 | OFFでは保存shiftを休眠保持し現表示を変えない。ON復帰で旧値をその場に適用せず次回入場で使う。ただし適格な操作元Bookにsession調整があればその値を保存し、その本の旧休眠値を上書きする。siblings/parkedは一斉保存しない。OFF中も本別の明示解除はsessionと保存値を消す。Composedでは休眠値を維持 | 2026-10-10 |
| Q2 PDF/EPUB identity | G Q6に従い既存個別編集と同じ物理ページidentityを使う。内部再編・再変換/reflow後の内容ずれは受容しマニュアルに明記。items index/表示順番号は禁止 | 2026-10-10（G Q6に準拠・確認） |
| Q3 既存resetと保存値 | 記憶ONではmode再押下・mode/flow/direction変更のresetで保存shiftもClearし通知。OFFでは従来session resetだけ。Composedのmode-onlyでは休眠保存値を消さない | 2026-10-10 |
| Q4 まれな失敗 | §4.4の新しい既定方針を確認。通常再入場の先行保存/resume整列と正常終了drainは §6.1 の構造保証として独立に必須 | 2026-10-10 |
| Q5 保存先・writer | R2のshift専用 `spread_shift.db` と単一workerを採択。既存spread.db writerを移管せず、既存spread setterを非同期化しない | 2026-10-10（R2採択・再確認） |

**利用者への未決質問はない。** 独立設計レビューと構造合意は §11.2 の工程として別に行う。
