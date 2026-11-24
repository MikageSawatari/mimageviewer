// 番号・表示名・状態を受け取る関数を指定します。
fn slots() {
    [
        #{ slot: 1, label: "最後のページなら次の本へ", run: "next_or_next_book" },
        #{ slot: 2, label: "★3 を付けて次へ", run: "rate3_next" },
    ]
}

fn next_or_next_book(s) {
    if s.context != "fs_image" { return []; }
    if s.page.is_last { ["FsSiblingNext"] } else { ["FsPageNext"] }
}

fn rate3_next(s) {
    if s.context != "fs_image" { return []; }
    ["RatingItem3", "FsPageNext"]
}

// イベントの接続と「イベント可」の操作の確定は後続の段で行います。
fn on_event(s) {
    if s.context != "fs_image" { return []; }
    if ("view" in s.events || "page" in s.events) && s.book.path.contains("\\manga\\") {
        if s.view.direction != "rtl" { return ["FsReadingDirectionToggle"]; }
    }
    []
}
