# §1.364 D3: G3 へ渡す C1–C12 fixture

基点: `next-viewer` `2419058c0`。契約の正本は
[spread-shift-memory-plan.md §10–11.3](spread-shift-memory-plan.md#10-g-が消費する固定インターフェース)。
G の照合資料は `C:/home/mimageviewer-nextbook/docs/book-composition-plan.md`
§9 / §13 G3（読み取り専用）。この worktree へ G の製品実装は統合していない。
以下は **E が供給する境界の実行可能な fixture** であり、G editor / mode-only / composition
store / revision / transaction / cache の統合受入完了を表さない。

`src/spread_shift/contract_fixtures.rs` は `cfg(test)` の crate-visible module。
G3 のテストは `crate::spread_shift::contract_fixtures::{four_page_book, JointBookFixture,
completed_read, resolve_automatic, preparing_snapshot}` を使用できる。
`JointBookFixture` の相対参照と現在順を G manifest adapter へ渡し、同じ scope / receipt / rules
で E が解決した内部 anchor と比較する。`completed_read` は pure test 用の証明値であり、
製品の読取証明は service の `read_after` から受け取る。
`preparing_snapshot` へ渡す nav generation / layout token は呼出し側が所有する既存の値。
E は token を生成・hash 化しない。units は実 App の canonical builder から取得する。
この helper は **spread mode 専用**で、入口で `is_spread()` を assert する。
G 側で初期spread規則を解決して App に設定してから呼ぶ。Single / Split の Preparing capture
には使えない（pair anchor の列挙を singleton に変換するとページが欠落するため）。

## G0 の八項目とテスト

| 項目 | `spread_shift::contract_fixtures` のテスト | E の検証範囲 |
| --- | --- | --- |
| 1 request/reply・世代・optional_shift | `joint_01_fenced_reentry_consumes_both_reads_once_and_rejects_other_generations` | 実 shift/resume worker を pause。両完了順、先行受付 cutoff、固定読取点、後着 sibling save、receipt/items/nav/settings/scope/epoch の不一致と一回消費 |
| 2 保存資格/scope・元/物理cache | `joint_02_scope_identity_separates_logical_source_cache_and_nested_books` | logical RAR と物理 cache の別 key、outer entry chain、実効 nested prefix、KeepDrive、親・cache へ非継承 |
| 3 identity・Missing・OFF | `joint_03_identity_reorders_filters_returns_and_document_exception_is_physical` | 実 identity adapter、並べ替え・除外・欠落・復帰、同 slot の別ページを代用しない、OFF fresh の不適用、PDF/EPUB 物理 page 42 と表示 slot 1 の区別 |
| 4 Preparing の現在実効組 | `joint_04_preparing_captures_nonempty_live_units_roles_placement_and_owner_token` | 非空の実 canonical compositions を capture。ON/OFF、SessionOnly/Saving/Saved/SaveFailed、rules/role/placement/各世代/opaque owner token を保持し、後続 live 変更で差替えない |
| 5 休眠維持・解除時再解決 | `joint_05_composed_is_opaque_dormant_row_survives_and_removal_uses_current_policy` | opaque Composed を Automatic の不正入力より先に返す。保存 row / watermarks 不変、構成解除 ON/OFF/Missing の E resolver 出力 |
| 6 共通受付と拒否 | `joint_06_pairing_gate_rejects_before_shift_writes_and_preserves_single_split_navigation` | Composed/Unreadable/A2 × Nudge/Clear/Memory の pure admission、共通説明、Single/Split の NavigationFallback、E service の追加 write ゼロ |
| 7 lifecycle・通知分離 | `joint_07_nested_only_copy_probe_and_import_coverage_publish_logical_scope_deltas` | nested shift しかない DB の実 probe/copy/再入場。`%/_` root と隣接 root 境界。公開 export/import API で nonempty family の未記載 child 解除、範囲外保持、確定 delta と捕捉 logical root 集合 |
| 8 joint fixture・兄弟・一覧 | `joint_08_sibling_sessions_fork_and_ordinary_list_do_not_publish_writes` | 同本二 owner は別 session、後の明示 save が再入場で採用、fork は accepted state のみ、Ordinary session への切離し、A2 の E recipe、read/fork/drop で受付数不変 |

## C1–C12 の対応と残る G3 検証

| 契約 | 上記 joint 項目 | G が統合時に検証する部分 |
| --- | --- | --- |
| C1 | 2 / 7 | G の semantic BookScope → shared constructor adapter、accepted source/cache 対応 |
| C2 | 5 / 8 | G snapshot の Composed/Unreadable/A2 分類と E API への実 routing |
| C3 | 1 / 3 | G Automatic builder への解決済み optional_shift、初回 adoption |
| C4 | 4 | G Preparing の一つの受付境界、実 accepted token と初期 record、DB 再読取なし |
| C5 | 6 | 全 KeyAction / カスタムキー / gamepad / HUD / IPC / 再送の実 handler。fixture は pure gate だけで producer の統合を代替しない |
| C6 | 5 | 実構成解除後の新 adoption から E を一回呼ぶ |
| C7 | 5 | 実作成/編集/保存/取消/保存失敗で E の row/write 不変。fixture は G phase を模倣していない |
| C8 | 1 / 8 | G revision/token に dormant shift seq を混ぜない、実 registry の配送/cache 境界 |
| C9 | 6 | local editor 導線と Remote PC 編集案内。fixture は共通拒否理由を検証 |
| C10 | 7 | G composition descriptor と shared slash-family/coverage codec の接続、G と E の transaction/通知の分離 |
| C11 | 6 / 5（前提） | G の実 mode-only setter から dormant shift Clear を発行しない。G 未統合なので、この fixture で mode-only 成功を主張しない |
| C12 | 3 / 全項目 | Q1–Q5 は E の既存実装・回帰を正本とし、G 側で fallback を複製しない。文書 page identity の内容追跡/reflow 対応は追加していない |

この fixture は既存 D1/D2 回帰を置換しない。たとえば全 legacy v7/v8/v9 字段不在、明示 prefix
coverage、台帳 flag/内容 ID 候補選別、正常終了、settings reset、retained epoch/import取消は
既存の model/service/store/metadata-transfer/content-identity/owner テストを併用する。
Remote group parity / reset / acquire は別 D3 テストの担当である。保存基点はfull本scopeで解き、
group parityは同じrules・列の完全応答を対象とする。bounded応答のcap・ページアクセスと
変換書庫のlogical元keyによるbase設定は維持する。切詰め境界のgroup差はshiftなしのmasterにも
存在する既存制限で、E→G fixtureの受入範囲を広げて変更しない。
master比較の根拠とD3検証記録は[実装計画 §11.10](spread-shift-memory-plan.md)
および`target/D-1364-d3-msg.txt`を参照する。

焦点実行: `cargo test -p mimageviewer --lib spread_shift::contract_fixtures`。
全体 gate / 利用者確認用 build / 独立完了レビュー / 実アプリ検証は root の別工程。
この文書の作成だけで検証成功を表さない。

## next-book G3 consumer（2026-10-11）

上記はE供給時点の履歴。next-bookではG3のsemantic scope adapter、Composed/Unreadable/A2の実admission、
独立resume fence、休眠shiftを除くbuilder/cache、解除後の既存read owner、mode-only setterを接続した。
`spread_shift_composition_tests`と`ui_fullscreen/book_composition_tests`がC1–C6/C8/C11/C12の実consumerを検証し、
初期化oracleにはEの実canonical compositionと`preparing_snapshot`を使う。
G2のstore/metadata familyがC10を所有する。G4 editorの作成/取消/保存phase（C7/C9）は次段であり、
Eのpure fixture成功で代用しない。検証実数/exit/logは`target/G-1242-g3-msg.txt`を参照する。
