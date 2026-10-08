# サイドカー確認の再訪時再利用計画（2026-09-21、実装・検証中）

## 1. 現象と測定上の限界

SmartFolder／Collection の `Ctrl+↑/↓` から実フォルダ・PDF を開くたびに、通常の物理 load は `begin_sidecar_restore` を開始する（`src/app.rs:27744-27796`, `src/app/sidecar_restore.rs:670-762`）。同じ親 `D:\home\scan\comic` 内の PDF への 3 回の移動でも、保存確認から入力解放まで約 160／170／162 ms、`sli_sidecar_import` は約 149／157／149 ms だった（`target/ctrl-nav-sidecar-20260921/perf_events.jsonl`）。同親の `mimageviewer.dat` は約 666 KB。SmartFolder／Collection 一覧の再走査とは別の、共通の物理 load の待機である。

`sli_sidecar_import` は `restore_started_at` から復帰までの経過で、quiescence、worker の strict flush、probe、スケジューリング／poll を含む。現在の計測だけでは JSON parse／`prepare` が約 150 ms の主因とは言えない。root のダイアログは 100 ms 猶予後に描く一方、detached fullscreen の表示経路はその猶予を使わない（`src/ui_dialogs/sidecar_restore.rs:20-35`, `src/ui_fullscreen.rs:18589-18596,18733-18738,18964-18968`）。この表示差は別の不整合として記録し、本計画の速度改善と混同しない。

## 2. 守る契約

`docs/sidecar-import-async-plan.md` §5.1–5.6、`docs/ui-responsiveness.md`、`docs/async-architecture.md`、`docs/preset-and-adjustment.md` §9、`docs/virtual-folders.md` §6 に従う。中央 DB が正本で、サイドカーは編集／タグの復旧用バックアップである。既存の App-global `SidecarRestoreState` の `Quiescing → Checking → (必要時 Running／CacheRefreshing) → Resuming`、continuation の context／items generation と cancel、modal／入力遮断、厳密な writer fence と in-place cache reservation、成功後の exact clean-owner eviction、失敗時の dirty／write-disabled owner 保護を維持する。UI thread に filesystem、SQLite、hash、JSON、展開を移さない。

同期済みの判定を path、一覧 snapshot、mtime、サイズだけで省略しない。外部ツールが同じ長さ・mtime のまま bytes を書き換え得るため、現行 `SidecarDiskToken` の metadata 前後＋全 bytes SHA-256、marker 再読、最後の source revalidation を worker で毎回行う（`src/sidecar.rs:187-234,309-395,475-605`）。pending／failed writer は disk より優先する。Missing の作成／削除と旧 marker は現行 probe／marker-clear に任せ、推測で Current にしない。復旧 commit に probe の prepared value／token を流用しない。

## 3. 提案する限定 fast path

### 3.1 証明と所有

通常の `Checking` が成功した際、要求 family の outcome が**すべて `AlreadySynchronized`**、`source_validation_error=None`、flush／strict writer fence 成功、source が `Disk`、かつ sidecar が **clean (`!is_dirty()`)** である場合だけ、前回の「検証済み読取」証明候補を worker の typed result に載せる。旧 mask の decode で `load_for_import` が dirty にした sidecar は marker が一致しても対象外であり、必要な flush を飛ばさない。`SidecarImportProbe::Current` 自体は family `Failed` を含み得るため、Current だけを証明にしない（`src/sidecar_import.rs:425-584`）。UI は同一 restore request の採用と matching cache owner の install 成功後にだけ候補を有効にする。既存の write-disabled dirty owner との衝突 warning、target 消失、cancel では候補を捨てる。証明は正規化 folder key、正確な disk token、要求 family、前回検証した immutable parsed `SidecarFile` の items `Arc` と semantic-validity を含む。表示／編集に使う `App.sidecars` owner とは分離し、これを writable owner にしてはならない。A→B→A の実フォルダ再訪にも対応できるよう小数の folder-keyed LRU とし、**最大 4 件／推定展開後 heap 合計 256 MiB／単体 192 MiB** の固定上限を設け、超過時は従来 probe に戻す。実fixtureの JSON は 666 KB でも local-adjust の 9 raster alpha だけで約 148 MB あるため、64 MiB では対象そのものを除外する。上限計算は worker で `SidecarEntry` の mask data・vector、local-adjust alpha/labels・layers、comic、tag 等の可変長 payload を保守的に加算する。初回証明作成時と再訪 hit 時には、Checking worker 内で可視 owner 用の外側 `BTreeMap` を一度 clone し、owner の items `Arc` を単独所有にする。通常編集時の UI thread 上の全 entry copy-on-write を避けつつ、内側の大きな raster `Arc` は共有する。可視 owner の追加 outer map は proof cache の定常予算とは別の表示所有量として扱う。証明の更新で同じ folder の旧 entry を置換し、参照中の Arc は worker／owner の寿命まで保持する。候補と LRU が同時に保持する一時的な数も bounded とし、大きい snapshot の最後の Arc を UI で drop しないよう、既存の worker drop/ACK pattern を使う。

再訪時も先に既存 `Quiescing` と `Checking` worker の strict flush／global writer idle を完了する。その worker 内で pending／failed writer を先に調べ、disk の metadata 前後＋全 bytes SHA-256 から新 token を得る。token、raw folder path、data directory、family が証明と完全一致した場合にのみ、要求された edit／tag marker を**毎回**読み直す。すべて現 token の sync marker と一致し、cancel と再度の source revalidation も通れば、保持した immutable parsed items から `Current` を返す。ここで省けるのは JSON decode と全 entry の `prepare`／key・mask・tag 検証だけであり、barrier、内容確認、marker 読み、source revalidation は省かない。`SidecarImportProbe` の既存 outcome／elapsed を維持し、worker 判定は `nav/sli_sidecar_reuse_check` に hit と typed miss 理由（prior 不在、key、writer、token、marker、cancel、予算、admission）および probe/load/prepare 時間を記録する。App の exact request 採用と proof publish は `nav/sli_sidecar_reuse_apply` に分け、遅着 worker hit を採用済みとして数えない。

token／family／marker の不一致、missing、pending／failed writer、読取・検証 error、cancel、semantic validity の欠如では証明を使わず、従来の strict `probe` とその typed 結果へ戻す。とくに marker mismatch の `ImportRequired` は既存の再 load→`prepare`→commit 前 revalidation を必ず通す。`SourceChanged`、corrupt／unsupported／semantic-invalid、marker 読取 `Failed`、write-disabled dirty owner に対して成功証明を発行・採用しない。fallback で二重読取になり得るのは稀な変更時だけであり、変更を「同期済み」と誤認するより優先する。

証明は App 側の bounded immutable reuse owner とし、`SidecarRestoreState` の mutually-exclusive phase を新しい bool／pending sentinel で分割しない。worker へ渡すのは immutable な値だけで、結果は既存 restore request ID／context／generation を通じて採用する。別 context へ completion、cache invalidation、cancel を転送しない。正規化 folder key は marker と cache eviction の grouping にだけ使い、`App.sidecars` の writable owner identity は raw `PathBuf` 完全一致とする。raw path の表記が異なる alias は strict probe へ戻し、要求 path を持つ owner を作る。証明の作成は同一 request の terminal Current と flush 成功後だけで、途中 cancel／shutdown、target 消失、worker disconnect、commit／cache refresh、warning を伴う Current では作らない。旧証明が残っても token／marker の毎回検証が採用を制限するが、書込・失敗・invalid source の terminal では積極的に破棄する。

この 256 MiB は**定常 proof cache の予算**であり、可視 owner、単一 Checking worker の候補、退役 worker がまだ持つ Arc を含む process 全体の絶対上限ではない。実 fixture の約 146.8 MiB は従来なら folder owner eviction 後に解放された parsed raster を navigation 跨ぎで保持する定常量となり、同種の大きな proof は合計予算上、実質 1 件だけ保持される。既存の退役 worker 起動／送信失敗で deferred payload backlog が残る間は新 proof admission を停止し、失敗の繰り返しで候補を際限なく積み上げない。

### 3.2 性能上の停止条件

まず worker 内を quiescence、strict flush／idle、disk read＋hash、JSON decode、`prepare`、marker read、source revalidation、UI terminal poll に分けて測る。実 profile には書かず、現存 sidecar は必要なら read-only で `target/` の隔離 fixture にコピーする。DB は通常 profile を read-only で観察するか隔離コピーだけを使い、書込測定は隔離 fixture の DB に限定する。666 KB の現物相当＋多数 entry／mask＋Missing の fixture で cold／warm を比較し、timer は worker 内の区間と総 modal 所有時間の両方を取る。Missing や SQLite marker open／writer fence が支配的なら parsed reuse による短縮は乏しい。その場合はこの fast path を「改善済み」として出荷せず、証拠と次の所有境界を親・独立 reviewer に報告する。ファイル identity を metadata のみに落とす、入力遮断を外す、モーダル猶予だけ伸ばす、旧 snapshot を無検証で採用する、といった代替は採らない。

## 4. 回帰と検証

- 同一 folder の変更なし再訪は fast hit し、従来の Current と同じ sidecar 表示・hydration・marker を得る。別 folder／family 切替は証明を共有しない。
- 内容変更、削除、新規作成、同サイズ・同 mtime の bytes 変更は必ず miss／`SourceChanged`／現行 import／marker clear の適切な経路を通る。編集／タグそれぞれの marker 変化を個別に照合し、同じ disk token でも未同期なら復旧を省かない。
- pending writer と failed writer、flush 失敗、dirty write-disabled cache owner、semantic-invalid／unsupported sidecar、marker read failure は fast hit しない。未保存変更を捨てず、error を Current と誤認しない。
- cancel、rapid switch、main／detached sibling、target 消失、worker disconnect、shutdown の stale completion は別 context／generation に適用しない。既存 fullscreen continuation と表示保持を維持する。
- まず `sidecar`／`sidecar_import`／restore handler の焦点テストと隔離 fixture benchmark、次に共有変更の `scripts/test-full.ps1`、`cargo fmt --check`、UI文字を変更する場合 glyph check、最後に `scripts/build-dev.ps1 -PreserveRuntime`。通常 profile のアプリを agent は起動／停止しない。build 前に resident process が無いことを確認する。

## 5. 合意が必要な境界

親と独立 reviewer が §3 の限定 fast path に構造合意した。実 sidecar の read-only コピーを使った隔離 opt2 worker ベンチでは、従来 probe の中央値 124.897 ms に対し、worker 内の可視 owner 外側 map 分離まで含めた proof hit は 1.893 ms（16 warm iterations）だった。初回証明作成は 134.930 ms、証明の推定保持量 146.8 MiB は単体 192 MiB／総 256 MiB の枠内だった。これは UI modal の壁時間ではなく、quiescence／flush／UI poll を含まない worker probe の比較である。詳細は `target/sidecar-confirmation-reuse-20260921/measurement.md`。実装は既存 barrier と typed restore owner の中で行い、§4 の回帰と共有 gate を完了してから user handoff する。

## 6. 実装後の検証記録（2026-09-21）

`sidecar_import::tests` は 25 pass／計測用 1 ignored、`app::sidecar_restore::tests` は 37 pass、`sidecar::tests` は 50 pass。前二者には unchanged 再訪、同サイズ・同 mtime の内容変更、削除、marker/family 変更、cancel、exact owner install、4 folder の LRU／予算会計を含む。`cargo fmt --check`、`python scripts/check_ui_glyphs.py`、`git diff --check` は通過した。独立 reviewer は clean 証明、worker 内の外側 map 分離、内側 raster Arc 共有、全 candidate terminal の worker 退役を source で受理した。

`$env:RUST_TEST_THREADS='1'; powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-full.ps1 -SuppressCrashDialogs` は PASS（本体 lib 8812 pass／46 ignored、補助 bin・snapshot・vendor egui/egui-wgpu/eframe を含む）。ログは `target/sidecar-confirmation-reuse-20260921/test-full.log`。mImageViewer の稼働 process が無いことを確認してから `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-dev.ps1 -PreserveRuntime` は PASS。vcrt-pe は runtime=4／pe=2。ログは同ディレクトリの `build-dev.log`。生成物 SHA-256 は `target/dev-runtime/mimageviewer-core.exe` が `D584F270DAA79D82AA33AC9F6BF42786A395F4B5088B73E196F128289A58275F`、`target/dev-runtime/mimageviewer-remote.exe` が `9FBF80F94403CDF78E8698A0BD31271B1B874DAEB8D5C7063EA6F780BEEC45E5`。agent はアプリを起動・停止せず、実 profile／通常 DB を書き換えていない。実 UI modal の wall time と体感はユーザーの検証で確認する。

## 7. 再レビュー S2 修正（2026-09-21）

raw folder path 完全一致を proof hit 条件へ追加し、alias は strict probe へ戻すことで
`App.sidecars` の owner key を要求 path に統一した。hit/miss の worker 判定と App の exact request
採用・proof publish は別 perf event とし、遅着 hit を採用として記録しない。cache は正規化 folder slot の
候補だけを worker へ渡し、raw path / data directory / family の exact 判定は worker に一元化する。
mismatch は LRU hit として昇格させない。pending/failed writer、
dirty write-disabled owner、semantic-invalid/unsupported/dirty source、Missing、folder/data-dir/family
mismatch、marker read failure、予算境界を直接回帰へ追加した。単体 192 MiB が合計 256 MiB を超えない
ことと folder 件数が 0 でないことは compile-time assertion で固定し、`SidecarDiskToken` と
`SidecarImportProbe` は crate 内 visibility に戻した。

S2-5 は pending／failed writer の優先を維持したうえで、長さまたは mtime の不一致を full hash 前の
typed miss とした。metadata が一致した場合は全 bytes SHA-256 と metadata 前後の再検証を必ず行い、
strict fallback を省略しない。S2-9 は warm hit が App の exact request 採用前に取消・退役された場合、
既存 proof を LRU 昇格なしで維持し、worker が返した候補 Arc だけを退役 worker へ渡す。cold 未採用候補は
publish せず、dirty／warning／failed source は既存 proof も失効する。S2-8 の publish 不採用時 outer-map
cloneは候補の worker 退役を維持し、parse を保持しない軽量 proof との比較（S2-12）は別設計・別計測とする。
