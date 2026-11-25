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

// 本を開いたとき、パスに \manga\ を含むなら読む向きを右→左にします。
fn on_event(s) {
    if s.context != "fs_image" { return []; }
    if "open" in s.events && s.book.path.contains("\\manga\\") {
        if s.view.direction != "rtl" { return ["FsReadingDirectionToggle"]; }
    }
    []
}
