# v3.5.0 第4回 再レビュー指摘

対象: `b46bf8e1b6a15861b7ed8b7f594eb08434fc776a`。
前回N01/N05に残件2件 (P1:1 / P3:1)。N02/N03/N04は解消を確認。
延期承認済みのR07/R08/R09/R10/R11は再指摘していない。

## Q01 [P1 / N01残件] 終了batchの配送先が、リングを開いたcontextではなく現在の前面で決まる

- 主な場所: `src/app/gamepad_input.rs:2392–2393`。`GamepadFrameBatch` の追加情報は `session_ended: bool` (`46`) のみで、編集の所有contextを持たない。
- sampleで確定しなくなったことは改善。ただし `gamepad_batch_goes_to_active_context` (`631`付近) は現在の前面/last-inputから配り先を決める。`app.rs:68104–68117` のroot配送と `68137` 付近のactive-context配送は、リングを開いた時の所有者を参照していない。
- 条件: メインのフォルダAは★2、別窓Bは★1。Bのリングでコンテナ評価を★4へプレビューする。リングを保持したままメイン側が操作対象になった状態で切断/無効化すると、終了batchはAへ届く。例えばモーダル表示中は通常のパッド配送が抑止され、リングのstale検査も進まないが、今回の確定は `dispatch_allowed` **より前**なのでAに書き込む。フォーカス変更と切断が同じframeへ入る場合も同じ。
- `commit_ring_picker` → `finalize_live_picker_ratings` のコンテナ評価は、今も `preview_current_folder_rating` により**現在mount中のフォルダ**へ保存する。Aが★4へ変わり、UndoもBの変更前値1をAの変更前値として記録する。Bが前面のまま切断する前回の最小ケースは直るが、「通常配送先 = 編集の所有者」という前提はフォーカス/活性窓の変更で崩れる。
- 証拠: `flow_probe.py` / `flow_probe.log`。今回のproduction `dispatch_gamepad_batch` と確定/finalize/Undo構築を変更せず抽出し、DB/UI境界を代用した検証。通常配送を抑止した状態でも、Bに終了batchを配るcontrolは `A=2 B=4 undo=(B,1,4)`、Aへ配ると `A=4 B=4 undo=(A,1,4)`。実際の前面→配送先のresolver、stale検査より先に確定する順序もソースで確認。実HWND/パッドによる操作再現とは区別する。
- 今回の追加テストは「sampleで確定しない」「単一contextでUndoができる」を確認するが、所有者と配送先が異なる場合を検査しない。
- 修正境界: pickerを開いた時の所有contextを、そのpickerと終了要求が保持し、確定はその所有者へ配送する。現在の前面に関係する新規入力と、既存の編集を終わらせる要求を区別する。単にstaleとして捨てると、元のR12 (Undo欠落) に戻る。A/B別フォルダ、B→A/B→Cの活性変更、無効化/切断を含む所有者境界のテストが必要。

## Q02 [P3 / N05残件] 関連付けの後続batchは、アイドル中に開始されない

- 主な場所: `src/app.rs:53125–53130` (後続queue) と `53134–53142` (worker完了)、`53094` (完了を取り込むとreturn)。
- 8件で打ち切る問題は、queueを残すことで改善した。しかしworkerは依然として `tx.send(handlers)` のみで、UIへの起床通知がない。`association_prewarm` / `association_prewarm_queue` はApp末尾の再描画条件にも入っていない。さらに結果を取り込んだpollはreturnするため、次batchの開始にはもう一度pollが必要。
- 条件: サムネイル等の継続描画が終わった後に、Shellの先頭8拡張子の列挙が完了する。queueに残った9種類目以降は、新しい入力等でUIが動くまで始まらない。フォルダ再読込を更新契機とする既存cacheも、入力なしでは古い候補が残る。
- 証拠: `flow_probe.py` は現 `poll_association_handler_prewarm` をそのまま使用し、Shell列挙のみ決定的な代用品で実行。10種類で最初のworker完了後は `delivered=8 queued=2 UI_repaint_requested=false`。手動で1frame分pollしても `queued=2 next_worker_started=false UI_repaint_requested=false`。さらに手動pollを繰り返すcontrolでは9種類目も更新される。全呼出箇所/起床条件をソース照合し、実Shellの遅延実測とは区別した。
- 新テスト `every_extension_in_the_folder_eventually_gets_re_enumerated` はwhileループでpollし続けるため、実アプリのアイドル時に後続が進むことは証明しない。N04で修正した再走査と同じ通知境界が、こちらには残っている。
- 修正境界: worker完了で所有UIを起こし、その受信frameで次batchを開始するか、queueが残れば次frameを予約する。常時動き続けるpollに依存せず、開始→完了通知→受信→後続開始がつながるテストを加える。承認済みR10のcache miss同期列挙とは別の残件。
