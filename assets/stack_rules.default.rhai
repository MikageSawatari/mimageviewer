// mImageViewer ファイル名スタック 分割ルール (既定)
// =====================================================================
// この Rhai スクリプトは、フォルダ内の **画像** を「スタック」へ畳むためのグループキーを
// 決めます。詳しくはオンラインマニュアルの「スタック表示」ページ(カスタマイズ節)を参照。
//
// 渡されるのは画像だけです。フォルダ / ZIP / PDF はグリッド先頭にそのまま表示され、動画は
// 常に単独で並びます (= いずれもスクリプトには渡らず、ここで束ねる対象外)。
//
// 入口は fn group(files)。files と同じ長さのグループキー配列を返します。
//   files[i] = #{ name, stem, ext, mtime, size }   (画像のみ)
//     name:String  拡張子つきファイル名   stem:String 拡張子なし
//     ext:String   小文字の拡張子(ドット無し)
//     mtime:INT    更新時刻(秒)           size:INT    バイト数
//   戻り値: 各ファイルのキー文字列。同じキー = 同じスタック(2 枚以上で畳む)。
//           キーを () にしたファイルは「このルールでは束ねない」を意味します。
//   戻り値を #{ rule: "表示名", keys: [...] } にすると採用ルール名を表示できます。
//
// 使えるヘルパー関数 (いずれも本体側の native 実装 = 高速。全ファイルに対する判定・集計は
// 必ずこれらを使い、Rhai のループ/配列/マップを自前で全件回さないこと。特に Rhai のマップに
// 1 件ずつ追加する処理は要素数の二乗で激遅になる → 10 万件で十数秒):
//   regex_is_match(text, pattern) -> bool             正規表現に一致するか
//   regex_capture(text, pattern, group) -> String     一致した丸かっこ番号 group の中身(不一致 "")
//   regex_replace(text, pattern, replacement) -> String  一致部分を置換した文字列
//   argsort_int(整数の配列) -> 添字配列               値を昇順に並べたときの添字(連番順/時刻順に)
//   stack_all_matched(keys) -> bool                   keys に () が 1 つも無い(= 全ファイル該当)か
//   stack_distinct(keys) -> INT                       keys 中の異なるキーの数(() は無視)
//   正規表現は Rust の regex 構文(後方参照・先読みは非対応、線形時間で安全)。
//
// 動作: 下記ルールを上から順に試し、「全ファイルが該当」(汎用ルールはさらに
//   「2 つ以上のスタックができる」)最初のルールを採用します。どれも該当しなければ
//   全て単独表示(畳まない)。意図しない巨大スタックを避けるための安全側の作りです。
//
// 連写とみなす更新時刻の最大間隔は rule_time_burst() 内の gap_sec (秒) で調整できます。

fn group(files) {
    // 1) 命名パターン: 末尾に固定接尾辞が付き途中の番号だけ変わる形 (mXD 等)
    let k = rule_suffix_fixed(files);
    if stack_all_matched(k) {
        return #{ rule: "命名パターン", keys: k };
    }
    // 2) 末尾が連番 / ページ番号 ("name_001" "id_p0" など)
    k = rule_trailing_number(files);
    if stack_all_matched(k) && stack_distinct(k) >= 2 {
        return #{ rule: "末尾連番", keys: k };
    }
    // 3) 先頭が連番 ("0001_xxxx") を連続したかたまりごとに (切れ目で分割)
    k = rule_leading_serial(files);
    if stack_all_matched(k) && stack_distinct(k) >= 2 {
        return #{ rule: "先頭連番", keys: k };
    }
    // 4) 更新時刻が近いもの (連写): クラスタが 2 つ以上できたら採用
    k = rule_time_burst(files);
    if stack_distinct(k) >= 2 {
        return #{ rule: "更新時刻", keys: k };
    }
    // どれも該当せず -> 全て単独
    files.map(|f| f.name)
}

// ---- ルール1: 末尾に固定接尾辞 + 途中連番 (配布しているダウンローダ mXD の命名例) ----
// 例: 20260429_1100_0003_1234567890_p01_m01_@user -> "_m<番号>_@..." の手前をキーに。
fn rule_suffix_fixed(files) {
    files.map(|f|
        if regex_is_match(f.stem, "^\\d{8}_\\d{4}_\\d+_\\d+_p\\d+_m\\d+_@") {
            regex_capture(f.stem, "^(.+)_m\\d+_@", 1)
        } else {
            () // 非該当
        }
    )
}

// ---- ルール2: 末尾が「(文字)+数字」 ----
// "scan_001"->"scan" / "id_p0"->"id" / "12345678_p12"->"12345678"
// 末尾が数字で終わらない (cover 等) は非該当。
fn rule_trailing_number(files) {
    files.map(|f| {
        let key = regex_capture(f.stem, "^(.+)_([A-Za-z]*\\d+)$", 1);
        if key == "" { () } else { key }
    })
}

// ---- ルール3: 先頭が連番 (連続したかたまり、切れ目で分割) ----
// "0001_xxxx" の先頭数値が 1,2,3,... と連続する区間を 1 スタックに。
fn rule_leading_serial(files) {
    let nums = files.map(|f| regex_capture(f.name, "^(\\d+)", 1));
    // 1 つでも先頭数値が無ければこのルールは非該当 (全て () のまま)。
    for n in nums {
        if n == "" {
            return files.map(|f| ());
        }
    }
    let order = argsort_int(nums.map(|n| parse_int(n)));
    let keys = files.map(|f| ());
    let run = 0;
    let prev = 0;
    let has_prev = false;
    for i in order {
        let v = parse_int(nums[i]);
        if has_prev && v - prev > 1 {
            run += 1;
        }
        keys[i] = "serial_" + run.to_string();
        prev = v;
        has_prev = true;
    }
    keys
}

// ---- ルール4: 更新時刻が近いもの (連写) ----
fn rule_time_burst(files) {
    let gap_sec = 5; // 連写とみなす更新時刻の最大間隔 (秒)。ここを変えると調整できます。
    let order = argsort_int(files.map(|f| f.mtime));
    let keys = files.map(|f| ());
    let burst = 0;
    let prev = 0;
    let has_prev = false;
    for i in order {
        let t = files[i].mtime;
        if has_prev && t - prev > gap_sec {
            burst += 1;
        }
        keys[i] = "burst_" + burst.to_string();
        prev = t;
        has_prev = true;
    }
    keys
}

// 補助関数 stack_all_matched(keys) / stack_distinct(keys) は本体側が提供する native 関数
// (Rust で O(n))。Rhai で書くと要素ごとに interpreter を回り、特に distinct はマップ挿入の
// たびに Rhai がデータサイズを再計算して O(n^2) になるため (10 万件で 16 秒)、native 化している。
