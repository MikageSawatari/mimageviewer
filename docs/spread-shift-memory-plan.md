# §1.364 見開きの一ページずらしの本別記憶 — 設計 R1

更新: 2026-10-10。対象: `mimageviewer-nextview` / `next-viewer`、照合基点
`fa280d0d07509cf0242662c805559efaf9ada0b6`（§1.342 指定サイズ表示の統合後）。
状態: **設計案。利用者質問と独立設計レビューは未完了。実装の許可・検収ではない。**
今回の変更は本書と `target/D-1364-design-r1-msg.txt` だけ。コード変更、テスト実行、ビルド、
製品起動、コミットは行わない。以下の型・メソッド・テーブル名は、現行と明記したもの以外は提案。

## 1. 正本・確定要件

正本は [next-release-backlog.md §1.364](next-release-backlog.md)。全節と指定された
CLAUDE.md のバグ修正、状態削減、まれな失敗、通常利用の退行相談、永続データ、detached の規則、
AGENTS.md の役割を読んだ。関連資料は docs/README.md、architecture-overview.md、
display-pipeline.md、async-architecture.md、virtual-folders.md、keymap-spec.md、
key-customization-impl-plan.md、ui-responsiveness.md §4、detached-rework-plan.md §2、
web-remote-plan.md（特に §2 / §12.7–12.8 / §12.16 / §14）。

G の参照正本は **`C:/home/mimageviewer-nextbook/docs/book-composition-plan.md` を読み取り専用で参照**。
読んだ版は 2026-10-10 R6（§3–5 / §7 / §9–12 / §14）。その冒頭には再レビュー未実施の記載が残るが、
今回の依頼にある「独立 ACCEPT 済み」を引き継ぎ事実として扱う。G の文書・worktreeは変更しない。
特に Q1、Q2、Q14、Q17 の利用者決定を再質問しない。本書の判断待ちは末尾 §12 に集約する。

確定している仕様:

- 一ページずらしを**本ごとに記憶する全体設定**を追加し、既定 ON。
  閲覧履歴の記録設定と独立する。「本ごと」は保存単位であり、本別の ON/OFF 行を増やす意味ではない。
- 再入場で読書位置とずらしを解決し、復元した組で表示する。解除はその本の基本の自動見開きへ戻す。
- 上 HUD「…」から調整の有無、実際の内部アンカーへの移動、解除、記憶設定を操作できる。
  調整中は見開き操作の近くに控えめな印。保存・解除の結果を通知する。
- OFF は従来のセッション内ずらし。通常ページ送り、シーク、単ページ時の Ctrl+左右による一ページ移動を維持。
- **Composed では一時ずらし・保存・解除を共通 action owner で拒否**し、構成 editor を案内する。
  G の作成時には現在の組を取り込み、保存済みずらしは削除せず休眠保持（Q1）。
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
| App の spread 接続が同期 writer | `src/app.rs:18962`、`src/ui_fullscreen.rs:37392`、`src/remote_ipc/ui.rs:2552` | 新しい同期 DB write を Ctrl+左右に追加しない。§4 の writer 所有移管が再利用の前提 |
| Remote の共通 builder は shift=None | `src/ui_fullscreen.rs:15237`、`src/ui_fullscreen.rs:15270`、`src/remote_ipc/container.rs:6100` | 保存を PC のみに足すと Remote の組が変わる。optional_shift を共通 builder へ渡す |
| metadata は本別表示設定を搬送 | `src/metadata_transfer.rs:453`、`src/metadata_transfer.rs:2436`、`src/metadata_transfer.rs:4645` | shift も利用者の本別表示設定として明示転送に含める。origin は別解決が必要 |

守る不変条件は「**受入済みの同一本 scope / 同じページ identity の調整だけが、
その context の Automatic unit を作る**」。保存位置の数字が同じだから別ページへ適用する、
mount 中の App.current_folder を保存 reply の宛先にする、兄弟窓の close で最新保存値を上書きする、
という経路をなくす。今回の未記憶という症状は新機能の対象だが、既存 reset / 削除 / 非同期 adoption を
横断してこの不変条件を設計する。

## 3. 本とページのキー契約

### 3.1 本の scope と論理・物理キー

G と共通の `BookScope { logical_root, level_prefix }` を使う。prefix は raw tree で同定した、
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
| PDF | 元 PDF / 既存 document page identity（§3.2、Q2 待ち） | 元 PDF の既存設定 / PDF read target |
| EPUB | 元 EPUB / 既存 document page identity（§3.2、Q2 待ち） | 元 EPUB の既存設定 / 世代 PDF read target |

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
  `PdfPage(u32)`（`src/book_bookmarks.rs:102`）。推奨はこの**0-origin 物理ページ identity に限る例外**（Q2）。
  表示順番号とは異なるが、整数を文字列や hash へ包んで「番号を保存していない」とは主張しない。
  Q2 承認前には PDF/EPUB の永続形式は確定しない。厳密な番号禁止ならページ内容同定の別設計が必要で、
  PDF 対応を黙って外すことも、番号方式を黙って実装することもしない。
- 同じ ZIP member / folder file の追加・削除・並べ替えでは参照を再解決する。
  基点以降は**現在の自動規則と現在順**で組む。全ペアを固定する機能ではない。
  PDF の内部挿入・再編、EPUB の reflow 再変換で同番号が別内容になることは既存方式でも追跡できず、
  Q2 の許容範囲とマニュアルへ明記する。全ページ hash、類似画像推測、独自 reflow map は追加しない。

照合には worker が最終 items 順と一緒に作る identity index を拡張・再利用する。
`PreparedPageIdentityIndex::resolve` の raw fallback を通さず、`resolve_identity(identity)` の pure 入口を用意する。
Relative/Archive 照合を共通 adapter に寄せ、既存 resume の serde 形式・照合を無断で変更しない。
同名 directory / nested archive 衝突は既存非対応を拡げない。

## 4. 保存先・writer・確定境界

### 4.1 spread.db を再利用する案

推奨は `<data_dir>/spread.db` に次の**独立した新テーブル**を追加する。

```text
spread_shifts(
    path TEXT PRIMARY KEY,       -- KeepDrive の logical BookScope key
    anchor_ref TEXT NOT NULL     -- 検証済み tagged relative PageRef、版1
)
```

一冊一行、upsert / delete を一 transaction。既存 `spreads` の mode/flow/direction や
末尾/単独の設定行は変更しない。変更履歴、old index、履歴 ON 状態、cache path、timestamp、
永続 revision / tombstone / pending 行は不要。サービス内の採用 sequence はプロセス内だけ。
`CREATE TABLE IF NOT EXISTS` による追加で既存行を保護する。新 shift の旧形式 migration はない。
古い spread.db / read-only 接続にテーブルがないことは正常な「保存なし」であり、接続や JSON の
異常を「保存なし」とすることとは分ける。

同じ per-book 表示設定の DB を使うため、data-dir、reset、件数、purge、transfer に新 DB ファイルを
増やさず済む。一方、**DB ファイルを同じにするだけでは所有境界は解決しない**。
現在は App が writable SpreadDb を保持し、Remote 書込もそこを通る。
新たな shift worker が第2 writable connection を常時持つ案は採用しない。
web-remote-plan.md §2.1 の境界と UI 応答性に同時に合うことが再利用の条件である。

### 4.2 必須の所有移管（Q5 の採否後に、独立した最初の実装 chunk）

`SpreadStoreService` を App 所有の**単一 worker / writable connection owner**とし、
既存 spread の全 setter / clear と新 shift を同じ FIFO へ移す。App の現在の接続は writer として
残さない。UI / core ContainerEngine は immutable read snapshot を使い、DB open / SELECT / write / join を
新たに UI 入力・HUD 描画へ追加しない。移管は既存読み書きを保った一つの切替として review する。
worker の基礎パターンは `src/book_resume_db.rs:327`、`src/book_resume_db.rs:350`。

移管対象は `persist_spread_presentation`、末尾/端/単独 setter、HUD 結合設定、キー/gamepad、
preferences の表示設定 clear、Remote SetSpread 等（`src/remote_ipc/ui.rs:2320`）。
G の mode-only setter も同 service を使い、休眠 shift を触らない。
既存 setter が「DB 成功後に表示を変更」する契約を、移管だけを理由に楽観更新へ変えない。
新 shift は元々 session 即時操作なので、表示は即時更新し、DB 保存成功を別に報告する。
既存入力と保存成否のタイミングが不便になる懸念は Q5 で相談する。今の同期経路へ新 write を足す近道は使わない。

rename / purge / import / 内容 ID copy / reset は、同じ service の maintenance command により
writer 所有を直列化した期間に既存 worker 処理を実行し、完了して snapshot を公開してから通常 command へ戻る。
別 writer を同時に動かして busy retry で調停しない。read-only export は既存 read transaction を使える。
既存共通 maintenance が複数 DB を更新する仕組みを二重実装せず、spread の書込期間をこの owner と接続する。
この境界を保てないと判明したら実装を止め、Q5 の専用 DB 案と範囲を再相談する。

### 4.3 新 shift command の一回の確定

共通受付は `SetShift { scope, anchor_ref }` / `ClearShift { scope }`。
入力元 context ID、items generation、操作 sequence、要求時の logical root、資格 snapshot を捕捉する。
受付で最新の共通資格を確認し、受理した順に worker へ送る。順序は完了時の mount / focus に依存しない。
新しい入力による保存を close 時まで遅らせない。close / fork / restore は新規 save の producer ではない。

1. Automatic の有効 nudge の後者 anchor を ref に変換する。端の no-op は write / 保存通知なし。
2. context の調整 state を一回で更新し、同じ owner の canonical unit を再解決して既存 page-nav へ target を渡す。
   記憶 ON なら SetShift、OFF なら session のみ。save のためにページ送りを block しない。
3. 成功 commit が保存行 snapshot / scope sequence を公開する唯一の時点。
   「保存しました」はこの完了後だけ。古い reply でも保存済み事実は service に反映し、
   古い seq を現 context の live state に戻さない。
4. Clear は context の shift を外して既存の位置正規化へ送り、保存行があれば削除する。
   **明示解除は記憶 OFF 中にもこの本の保存行を消す**という推奨（Q1）。
   Clear 成功後に「見開き調整を解除しました」。保存なしの session 解除は即時通知できる。
5. エラーなら成功と表示せず、session と durable の差を通知する（Q4）。自動 retry / rollback 網は作らない。

長押し/連打は既存 navigation sequence の admission を維持し、受理済み command を FIFO で一回実行する。
save reply は target を動かさない。未受理 command と既に送信した command を同じ cancel にしない。
送信済みは元 scope に一度だけ確定し、元窓が消えていれば App の既存通知面へ結果を報告する。
終了 / Remote 取得の drain は worker で行い、UI スレッドで待たない。

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

Q1 推奨の ON/OFF 規則:

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
| 明示 mode 変更、同じ mode のキー再押下、flow / direction 変更 | 現行の session reset を保つ。ON なら saved row も Clear にする案（Q3）。OFF なら従来 session reset だけ |
| favorite / 全体既定 / 文書方向の hydration、import | ユーザーのずらし解除と推定しない。受入済み規則と保存 ref から再解決。import の明示空は §9 の転送契約で解除 |
| page-edit の一時 Single pivot | mode 永続変更ではない。調整を保存/消去しない（現行 `src/app.rs:11937`） |
| row 削除・追加・sort/filter/F5 | identity を新投影へ解き直す。shift だけのための DB 書込なし |
| fullscreen close、本切替、fork、park/mount/deposit/drop | 当該 context の session を移送または終了。close 時に古い anchor を保存しない |
| G composition 作成/編集/解除 | §10 の LayoutSource で切替。保存/消去を付随させない |

mode/flow reset が保存済み調整を消す Q3 は利用者データに関わるため未承認で実装しない。
逆に reset 後すぐ hydration で saved shift を再適用して、既存の基本 pairing へ戻す操作を無効にもしない。

## 6. open・restore・一覧変更の一つの採用点

保存行 snapshot は worker で準備する。初期化で既存 base spread snapshot と合わせて用意し、
warm open は memory lookup にする。cold 未準備は既存 typed open/hydration continuation が依存を所有する。
新しい shift_pending bool や、表示後の別 save-restore callback を作らない。
**キャッシュ済み PDF の即時 placeholder / 最初の表示を全ページ列挙待ちへ変更しない**。
最初の unit に使う saved ref はその受入経路で準備し、後から基本 pair→復元 pair と画面を飛ばさない。
snapshot 準備待ちで従来 fast path が遅れるなら測定値を持って Q5 へ戻す。性能成立は現段階では未測定。

採用順は `scope + rows/identity index + base rules → LayoutSource → shift resolution → resume target
→ canonical unit / 正規化 → first-display`。UI はこの完成結果を一回採用する。
明示ページ指定・しおり・同セッション位置は従来の優先順位を維持し、shift のために saved resume へ強制移動しない。
保存された resume は同じ実ページ identity を復元し、そのページを含む復元 unit の anchor へ正規化する。
「同じ HUD ordinal へ行く」より「同じ実ページを含む組」の再現を優先する。
resume と shift を一つの DB transaction にまとめる必要はない。通常の reopening は両保存を解く。
異常終了時の二 DB 同時確定までは保証せず Q4 の割り切り候補とする。

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
明示解除の成功、保存失敗、解除保存失敗は §4.3/Q4 の結果を一度通知する。
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
再入場時は service の最新 snapshot を使い、retained Book も既存復帰 adoption で scope の保存 sequence を照合する。
退避中の別本には何もしない。memory OFF の live/retained session は従来の移送契約を維持する。
復帰は現実ページ identity を保って最新保存値で unit を解き、共有 resume/history へ背景 write しない。

reply は要求元 context/operation を registry へ配送する。要求元が retired でも保存事実は service に残し、
今 mount されている別 context へ state/target/cache を適用しない。
closed で後着した Set が Clear を追い越さない FIFO と、同じ scope の A/B 受理順を決定的テストで検証する。
read した snapshot がその後の write で古くなった場合は adoption で最新 scope sequence と照合する。
修復 delay / try_lock+sleep / blanket sibling reset は不要。

detached 憲法への適合: 新しい detached bool、placement/viewport/geometry 捕捉経路、時間窓を足さない。
実装時は設計担当と独立 reviewer が「context 所有の調整投影を拡張する構造的変更であり、症状パッチではない」
と明示合意した後、触れる registry / predicate / viewport 範囲と理由を detached-rework-plan.md §11 へ記録する。
今回は同プランを書き換えず、将来の gate とする。

### 8.2 Remote

Remote は独立した表示 owner。PC の fullscreen_idx / session shift を使わない。
core ContainerEngine worker は同じ logical scope resolver、read snapshot、PageRef resolver、
Automatic optional_shift / Composed builder を使い、Web は返された PageGroup を描くだけ。
現行 core の read-only spread 接続（`src/remote_ipc/container.rs:2599`）へ第二 writer は足さない。
書込は IPC→App 所有 service だけ。`RemoteWriteRequest` の共通 owner は
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
FinishAcquire 前に drain する。service は UI 待ちを作らず既存 session phase へ完了を返す。
Remote の返却では既存一覧 reload、次の PC open で同じ保存値を解く。
PC→Remote の設定変更検出は acquire refresh。Q3 により Remote の SetSpread が shift を変更した時は
既存 write-success invalidation / remote_state_generation と container group cache を更新する。
余分な毎ページ pre-flight、home の global 設定を generation へ足す経路は作らない。
Web の rendition key には既存の partner/slot と受入済み layout identity を使い、旧組の合成を再利用しない。

## 9. rename / move / copy / purge / 転送

### 9.1 maintenance 登録

| 接点 | 必須契約 |
| --- | --- |
| `rename_key_migration::STORES`（`src/rename_key_migration.rs:939`） | `spread.db / spread_shifts / path / unique=true / KeepDrive / OptionalInLegacySchema` を登録。既存 DB に新 table がない時は正常不在 |
| root 改名・アプリ内 folder 移動 / copy | exact + `/` + `::` の既存 family 処理。相対 payload は root 改名では不変。destination があれば既存 conflict 規則を守る |
| 個別画像の改名 / 製本順替え | key だけでなく payload の RelativePath を同じ旧→最終 path mapping で変換。しおりの mapping（`src/book_bookmarks.rs:1102`）を共有。scope 内だけ更新、scope 外移動は旧本の Missing を保持し、新本へ shift を勝手にコピーしない |
| 外部 folder move / rename | Folder は内容 ID 非対応（`src/content_identity.rs:62`）。shift の新 sidecar を作らず、既存 per-book spread と同じく明示 metadata export/import を案内。旧 DB 行は保持。アプリ内移動との違いを明記 |
| 書庫/PDF/EPUB 外部移動・内容 ID restore | 論理元 root の observation と既存 copy/probe に参加。成功 Set 時に捕捉した logical root へ ContentIdentityTrigger::Edit を送る。reply 時の current_folder を使わない（現行接点 `src/app.rs:75957`）。destination 既存値は上書きしない |
| 本/root の実削除・明示 purge | STORES の同じ hard purge に参加。ページ一枚の削除では親 shift を削らず Missing とする。cache の容量回収は元本の shift を purge しない |
| metadata cleanup / 件数 / reset | KeepDrive 行は既存物理 root 判定へ。切断 drive は Protected、anchor 不在は孤児本としない。`SpreadDb::count/clear_all`（`src/spread_db.rs:507`、`:520`）に新 table を含め、文言に調整を含むことを説明。既存明示 clear/reset だけが全行消去をする |
| import / rename / purge 完了 | service snapshot を更新し、実際に変えた Book scope の context / retained dependency だけを更新。単なる open/close は失効通知を発行しない |

**現行 copy の注意（G も共有する修正点）**:
`restore_copy_mappings`（`src/content_identity/restore.rs:414`）は root の Exact と `::` の VirtualPrefix を作る。
`copy_stores_at`（`src/rename_key_migration.rs:1254`）も VirtualPrefix を `old_key::` としてコピーする。
新 scope は既存 spread と同型の `root/level_prefix` なので、**STORES 登録だけでは nested 本行はコピーされない**。
logical 本設定用の slash-prefix copy mapping を追加し、対象 descriptor を明示限定する共通契約を G と共有する。
raw page の `::` copy、既存 source/cache の四面 mapping は維持する。root だけ成功する試験で完了しない。
構成側にも同じ scope copy が必要だが、G の PageRef や LayoutSource の再設計は不要。

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
- 非空: covered scope の一冊一行を replace、coverage 外は保持。
- skipped/Missing/KindMismatch 等で本自体が受入対象でない: shift も変更しない。
- composition が存在しても dormant shift の import 自体は可能。明示 metadata 取込であり、
  禁止された HUD shift action ではない。Composed 表示には適用しない。G の解除後に同じ規則で解く。

import target は logical root + prefix の KeepDrive key。相対 identity と scope を検証してから書く。
明示空による saved shift 解除と legacy 不在を混同しない。
現在の v9 / v8 / v7 入力を保つ（`src/metadata_transfer.rs:27`）。出力版は G と統合した最新版+1を一度割り当て、
manifest / shard / section / is_empty / has_view_state / summary / changed flags を更新する。
既存一括確認に調整の追加/置換/解除数と上書き説明を足し、本別 prompt や新カテゴリ選択は作らない。
refresh は既存 context 付き worker（`src/app/metadata_import_refresh.rs:54`、`:534`）へ logical shift を加え、
`src/app.rs:44563` の一律 None を Restore へ置換する。import は記憶 OFF の現 session を勝手に変えず、
ON の Book と再入場依存だけを更新する。Composed の cache には shift の変更を反映しない。

## 10. G が消費する固定インターフェース

次の意味契約を先に共有し、実装順によって同名型の duplicate を作らない。Rust の配置は実装レビューで定める。

| 番号 | G の消費点 | 1.364 が供給する正確な契約 |
| --- | --- | --- |
| C1 | scope / PreparedBook | BookScope の logical_root + raw 実効 level_prefix、KeepDrive key、階層 fallback なし。physical base spread key は別入力 |
| C2 | `resolve_book_layout_source(scope)` | 構成 snapshot を先に判定。Composed なら shift の取得/解決/適用を行わず Composed を返す。構成なし Automatic だけ `resolved_rules + optional_shift`。構成読込不明をなし扱いしない |
| C3 | Automatic optional_shift | accepted items 世代・PageRef に相関した resolved anchor。保存 ON + resolved saved ref、または有効な live session ref。OFF 新規入場は None。Missing/filtered は None + HUD 状態、数字代用なし |
| C4 | 作成初期値 | accepted Automatic の現在の自動 units / role / placement を取得。shift ON/OFF を問わず**現在の live 組**を取り込む。saved row を消去/更新/新規作成しない。cancel も無変更 |
| C5 | Composed command gate | 共通 PairingEditAvailability が CompositionOwned を返す。shift action / HUD save / clear は消費して拒否。record/revision/shift/global binding/resume/unit/cache の write ゼロ。Single/Split の従来移動は通す |
| C6 | 構成解除 | 新 Book adoption で現在の記憶設定・scope・identity 規則から一回再解決。ON + resolved のみ復活、OFF/Missing は復活しない。G 独自 fallback/offset は作らない |
| C7 | Q1 休眠 | 構成作成/保存で spread_shifts row を維持。構成ありで OFF→ON でも session shift を save しない。store の二 DB transaction / shift 消費 receipt は不要 |
| C8 | cache / multi-window | LayoutSource tag と accepted token、context ID/items 世代が一致する投影だけを消費。Composed の token に dormant shift sequence を混ぜない。fork/read/close は write なし |
| C9 | editor / HUD 説明 | 「本の構成で設定されています。組を変えるには本の構成を編集してください」。local editor 導線は G、shift group/marker は 1.364。Remote は PC 編集案内のみ |
| C10 | maintenance / metadata | 同じ relative PageRef adapter、scope/coverage codec、nested slash-prefix copy 契約を共有。composition family と shift family、cache 側の既存 container family は独立 |
| C11 | Single/Spread/Split mode-only | G の構成あり mode-only は dormant shift を Clear しない。既存結合 setter からの自動 reset を流用しない |
| C12 | 限定例外 | Q2 の document identity 採否、Q1 の記憶切替、Q3 の自動 reset はこの resolver に閉じる。G は番号/記憶ルールを再実装せず、解除確認は resolver の実効結果を説明する |

ExtractedResult は G A2 固定 Automatic(shift=None)、OrdinaryList は G の元 scope の操作資格を使用。
構成「存在」と表示 recipe を混同し、mixed/本扱い OFF で外部 shift を許可する裏口を作らない。
構成判定不明時は shift 編集を拒否して案内するが、単ページの従来移動を失わせない。

## 11. 状態削減・実装 chunk・受入条件

### 11.1 明示的に検討した簡素化

| 検討 | 採否と理由 |
| --- | --- |
| 単一 owner | 採用。context 調整 state と service の durable snapshot を責務で分け、HUD/キー/gamepad/Remote を同じ資格へ。画面ごとの saved/pending bool を増やさない |
| 単一 commit 点 | 採用。Set/Clear の一行 transaction 成功だけで saved publication。読み取り・close・resize・初期化から write しない |
| 既存 open / reload / index / unit paths | 採用。準備 worker と既存 adoption に Restore を含める。設定 OFF で全窓を閉じる専用 rebuild は不要 |
| composition と shift の全組合せ | G 決定の二択 LayoutSource で排除。Composed は休眠保持だけ、解除時に一回再評価 |
| 保存を modal にして navigation/F12 を止める | 不採用。連続 nudge / 普通のページ送りが遅くなり、手間も増える。短い一行保存の scope/ref を捕捉すれば DB write で navigation を待つ必要がない。必要だと判明したら利用者へ相談 |
| 全窓に最後の saved shift を即時反映 | 不採用。読んでいる兄弟窓を移動/再pair させる。session は窓別、保存は last explicit action、再入場で最新値という単純規則 |
| OFF 時の全データ削除 | 不採用推奨。不可逆な利用者データ損失。保持+新規 read/write 停止で済む（Q1） |
| shift 専用 DB | 代案。単一 spread writer の移管を避けられるが DB ライフサイクルが一つ増える。Q5 で比較し、無断で第2 spread writer を作らない |
| 毎 open の anchor 内容 hash / 近いページ探索 | 不採用。既存 stable identity を越えるコストと誤適用。Missing を残して明示解除できる方が単純 |
| disk/DB failure の自動 retry / durable pending / 二 DB rollback | 不採用推奨。ログ/通知/利用者の再操作（Q4）。失敗回復の中の失敗を設計しない |

既存利用を削る候補は保存 modal、warm PDF 待ち、既存 mode setter の待ち時間/成功規則変更、
兄弟窓強制更新、implicit reset の保存行削除。前二者と強制更新は本案で避ける。
writer 移管と implicit reset は Q3/Q5 待ち。測定で避け切れない遅延が判明したら、
実装担当は costs/options を返し、未承認の挙動変更を採用しない。
G の構成あり操作禁止と A2 は既に承認済みで、追加の承認待ちへ戻さない。

### 11.2 coherent chunks と責任

1. D0: Q1–Q5 の回答を本書へ反映し、独立 `gpt-6.1-sol / xhigh` が設計そのものを review。
   設計担当が指摘を裏取りして構造合意、detached §11 の予定範囲を確定。
2. D1: Q5 採択済みの store 所有 / snapshot / maintenance 直列化を先に実装し、
   既存 spread の全 producer/consumer で機能同等性を検証。新保存 row / key / PageRef / 転送 / migration 登録。
3. D2: common shift owner、Restore、items/remap/cache、F12、HUD と通知を同じ chunk で接続。
4. D3: Remote saved resolution / group parity / reset、G C1–C12 の integration、関連 docs と最終 gate。

実装・狭い検証は `gpt-6.1-sol / high`、独立設計/完了レビューは別 context の
`gpt-6.1-sol / xhigh`。今回は利用者指定の designer として文書だけを作成し、モデル再設定や reviewer の
採択を行ったとは主張しない。実装開始時は G/E 統合後の symbol を再照合し、一つの shared file に一 writer。

### 11.3 テスト計画（今回は実行しない）

| 層 | 必須回帰 |
| --- | --- |
| pure nudge / resolution | LTR/RTL、cover ありなし、横長・非pairable・page-alone・末尾・連続 nudge。 `(new=2,anchor=0)` の後者を保存。端 no-op。Single/Split は従来移動 |
| identity / store | 途中追加/削除/順替え、Missing/filtered/復帰、違う同 idx を代用しない。cache 再生成、nested chain/collapse、prefix 間非継承、KeepDrive 非衝突。Q2 の PDF/EPUB 制限 |
| reopening | 履歴 ON/OFF 両方、正常 close/restart、本切替、履歴/しおり/required target/startup、warm PDF/password/cancel/error、F5。resume と復元 pair を初回 unit から一致 |
| cache / UI | 旧 unit token を事前生成→saved restore で新組。paged/連結/seek/capture/holdover、指定サイズfit・PDF demand・zoom保持。印/状態/実基点 jump/解除/保存結果、dark/light・小画面・touch到達 snapshot |
| common handlers | 四 KeyAction、カスタムキー、gamepad/ring、HUD、mode再押下/flow/direction/import reset。Composed 拒否は全状態/DB/cache 無変更、Single movement は通る |
| async / 多窓 | A/B別本、同本別session、fork/park/mount/drop、A save→B clear の順、旧 reply 後着、open 中 store変更、元窓 retired。close/fork/read が write/兄弟失効を作らない |
| maintenance | root/page 改名、製本順替え、scope 外移動、root+nested copy、内容 ID restore/probe、cache削除と元本削除の差、purge/reset/count/Protected drive |
| transfer | v7/v8/v9 保持、新空/旧不在/coverage/skip、folderとnested、元RAR shift + cache spread/trim + 元RAR構成の同居、休眠/Missing 搬送、unsafe値拒否、一括確認一回、context限定 refresh |
| Remote / G | memory ON/OFF、PC と group/anchor parity、forceSingle、budget途中、取得write drain、返却、既存 SetSpread reset。構成作成/cancel/編集/解除、OFF休眠、mode-only非消去、A2/OrdinaryList資格 |
| 失敗 / 性能 | Q4 採択済みの write/clear/open failure と abrupt終了。成功通知を偽らない。UI 同期 I/O ゼロ、warm fast path維持、連打保存で navigation を blockしない。余分な decode/hash/全列照合なし |

narrow unit/handler/integration/snapshot を先に実装者が所有。既存 §1.342 と detached tests を削除・弱体化しない。
shared behavior を触る最終実装では `scripts/test-full.ps1`、fmt、UI glyph check を実施し、
`scripts/build-dev.ps1` で利用者用検証 binary を用意する。live smoke は別の事前承認 suite。
今回は documentation-only かつ利用者が build/launch 禁止を指定したため、それらは実施しない。

実装時の文書更新対象: spec/manual の本・見開き・設定・保存先/転送、architecture-overview、
display-pipeline、async-architecture、virtual-folders、keymap、web-remote、docs index、detached §11。
新キーで解除/jump を追加する場合だけ KeyAction 一式を揃え、既定キーは衝突確認の上で別途決める。
本案は HUD の操作のみを追加し、Ctrl+左右の既定割当は変えない。

## 12. 利用者への未決質問（一項目一問）

以下は今回の設計を review 可能にするための選択肢であり、回答がないまま実装方針として確定しない。

1. **記憶を OFF にした時、保存済み調整は保持する規則でよいですか。**
   推奨: 全保存行を休眠保持し、現在表示はそのまま、次回入場と新規 nudge の read/write だけ停止。
   ON に戻した時は操作元の session 調整があれば保存し、なければ次回入場で旧値を使う。
   明示「解除」は OFF 中でもこの本の保存値を消す、と UI に説明する。
   利点: データ損失なし、ON/OFFで全窓 reload 不要。代案の「OFFで全削除」は単純だが**全冊の利用者調整を失う**。

2. **PDF/EPUB に限り、既存の物理ページ identity（0-origin page_num）の保存を許容しますか。**
   推奨: 表示順ページ番号と items index は禁止したまま、この既存 identity だけ例外を承認。
   利点: G / resume / しおりと同じ方式で、追加 decode/全ページ hash なし。
   費用: PDF 内部再編や EPUB reflow で同番号が別内容になる制限を受容する必要がある。
   代案: 厳密番号禁止を維持し document 内容 identity を別設計する。時間・I/O・ページ同定の不確実性が増え、Gにも追加adapterが要る。

3. **既存の mode 再押下・mode/flow/方向変更でずらしを reset する時、記憶 ON なら保存値も解除してよいですか。**
   推奨: 既存の「基本の組へ戻る」表示を維持し、ON時だけ保存行も Clear、解除を通知。
   利点: 開き直して旧ずらしが復活しない。費用: これらの既存操作でも保存した利用者調整が消える。
   代案: 保存値は休眠保持して現 session だけ reset。データは残るが、次回入場で再適用される。
   G の構成あり mode-only はどちらの回答でも休眠値を消さない。

4. **まれな DB 失敗・異常終了は、ログと通知、利用者の再操作で扱ってよいですか。**
   推奨: 保存失敗時は現在の session 調整を維持し「記憶できませんでした」。解除保存失敗時は現在を基本に戻したまま
   「保存済み調整は解除できませんでした。開き直すと戻る場合があります」。DB の旧値を保持し自動再試行なし。
   読込不明は通知/原本保持/その保存機能への write 停止、旧 DB を作り直さない。
   急な終了では最後の未 commit 調整や resume との同時確定が失われ得るが、再開時に残る二つの正本を解く。
   利点: 多段回復/再試行/journal を増やさない。代案: modal確定待ちや二 DB 回復を設計する。
   費用: 通常の nudge / 終了が遅くなり、検証すべき状態が増える。既存保存データの破棄はどちらでも許可しない。

5. **spread.db 再利用のため、既存 spread の単一 writer を worker へ移す先行 chunk を含めてよいですか。**
   推奨: §4.2 の所有移管を独立 review し、既存機能・fast pathを保ってから同 DB に shift table を追加。
   利点: DB ファイルと maintenance を増やさず、UI 同期保存も第2 writerも避けられる。
   費用: 既存 HUD/キー/Remote の保存完了受付まで変更範囲が広がる。commit後適用の操作は worker配送分の待ちが増え得るため、
   warm PDF の初回表示と入力応答を測定し、不便になる場合は実装を止めて再相談する。
   代案: shift専用 `spread_shift.db` と独立 worker に限定。既存 spread 操作の完了時点を保ちやすいが、
   data-dir/reset/purge/transfer/Remote の DB ライフサイクルが一つ増える。G の C1–C12 はどちらでも同じ。
