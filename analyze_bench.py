"""bench_thumbs.tsv の解析スクリプト"""
import csv
import sys
from collections import Counter, defaultdict


def pct(sorted_vals, p):
    if not sorted_vals:
        return 0.0
    k = (len(sorted_vals) - 1) * p / 100.0
    f, c = int(k), min(int(k) + 1, len(sorted_vals) - 1)
    if f == c:
        return sorted_vals[f]
    return sorted_vals[f] * (c - k) + sorted_vals[c] * (k - f)


def hist(sorted_vals, bins, label):
    """ヒストグラム風のテキスト出力"""
    print(f"\n--- {label} 分布 ---")
    total = len(sorted_vals)
    if total == 0:
        print("  (データなし)")
        return
    counts = [0] * (len(bins) + 1)
    idx = 0
    for v in sorted_vals:
        while idx < len(bins) and v > bins[idx]:
            idx += 1
        counts[idx] += 1
    # 再度カウント (sorted_vals をもう一度見る方が確実)
    counts = [0] * (len(bins) + 1)
    for v in sorted_vals:
        placed = False
        for i, th in enumerate(bins):
            if v <= th:
                counts[i] += 1
                placed = True
                break
        if not placed:
            counts[-1] += 1

    cumulative = 0
    for i, c in enumerate(counts):
        if i < len(bins):
            lo = bins[i - 1] if i > 0 else 0
            hi = bins[i]
            label_s = f"{lo:>6.1f}~{hi:<6.1f}"
        else:
            label_s = f"{bins[-1]:>6.1f}~    +  "
        cumulative += c
        bar_w = int(c / total * 40)
        bar = "#" * bar_w
        print(f"  {label_s}  {c:>6} ({c/total*100:>5.1f}%)  cum {cumulative/total*100:>5.1f}%  {bar}")


def size_label(b):
    if b >= 1024 * 1024:
        return f"{b/1024/1024:.2f} MB"
    if b >= 1024:
        return f"{b/1024:.1f} KB"
    return f"{b} B"


def main():
    path = "bench_thumbs.tsv"
    rows = []
    with open(path, "r", encoding="utf-8") as f:
        reader = csv.DictReader(f, delimiter="\t")
        for row in reader:
            rows.append(row)

    print(f"総行数: {len(rows)}")
    print()

    # --- ステータス別集計 ---
    status_counts = Counter()
    for r in rows:
        s = r["status"]
        if s.startswith("FAIL"):
            status_counts["FAIL"] += 1
        else:
            status_counts[s] += 1
    print("=== ステータス ===")
    for s, c in status_counts.most_common():
        print(f"  {s:>8}: {c} ({c/len(rows)*100:.1f}%)")

    # --- kind 別・拡張子別集計 ---
    print()
    print("=== kind 別 ===")
    kind_counts = Counter(r["kind"] for r in rows)
    for k, c in kind_counts.most_common():
        print(f"  {k:>6}: {c} ({c/len(rows)*100:.1f}%)")

    print()
    print("=== 拡張子別 ===")
    ext_counts = Counter(r["ext"] for r in rows)
    for e, c in ext_counts.most_common():
        print(f"  {e:>6}: {c} ({c/len(rows)*100:.1f}%)")

    # --- FAIL の内訳 ---
    fails = [r for r in rows if r["status"].startswith("FAIL")]
    if fails:
        print()
        print(f"=== FAIL ({len(fails)}) ===")
        fail_ext = Counter(r["ext"] for r in fails)
        for e, c in fail_ext.most_common(10):
            print(f"  ext={e:<6}: {c}")
        print("  先頭 5 件:")
        for f in fails[:5]:
            print(f"    {f['file']} ({f['ext']}, {f['size_bytes']}B)")
            print(f"    → {f['status'][:80]}")

    # --- 画像のみで各指標の分布 ---
    imgs = [r for r in rows if r["kind"] == "image" and not r["status"].startswith("FAIL")]
    vids = [r for r in rows if r["kind"] == "video" and not r["status"].startswith("FAIL")]
    print()
    print(f"=== 成功した画像: {len(imgs)} / 動画: {len(vids)} ===")

    def num(r, k):
        v = r.get(k, "")
        return float(v) if v else 0.0

    open_times = sorted(num(r, "open_ms") for r in imgs)
    resize_times = sorted(num(r, "resize_ms") for r in imgs)
    encode_times = sorted(num(r, "encode_ms") for r in imgs)
    dims_times = sorted(num(r, "dims_ms") for r in imgs)
    sizes = sorted(int(r["size_bytes"]) for r in imgs)

    # 合計 = キャッシュ無しパス (open + resize)
    nocache_total = sorted(num(r, "open_ms") + num(r, "resize_ms") for r in imgs)
    # 合計 = キャッシュ有りパス (open + resize + encode)
    withcache_total = sorted(
        num(r, "open_ms") + num(r, "resize_ms") + num(r, "encode_ms") for r in imgs
    )

    print()
    print("=== 画像の時間分布 (ms) ===")
    print(f"{'':<22} {'min':>7} {'p25':>7} {'p50':>7} {'p75':>7} {'p90':>7} {'p95':>7} {'p99':>7} {'max':>7}")
    for name, vals in [
        ("dims_ms (header)", dims_times),
        ("open_ms (decode)", open_times),
        ("resize_ms (Lanczos3)", resize_times),
        ("encode_ms (WebP)", encode_times),
        ("no-cache total", nocache_total),
        ("with-cache total", withcache_total),
    ]:
        print(
            f"{name:<22} "
            f"{min(vals):>7.2f} {pct(vals,25):>7.2f} {pct(vals,50):>7.2f} "
            f"{pct(vals,75):>7.2f} {pct(vals,90):>7.2f} {pct(vals,95):>7.2f} "
            f"{pct(vals,99):>7.2f} {max(vals):>7.2f}"
        )

    print()
    print("=== ファイルサイズ分布 ===")
    for p in [25, 50, 75, 90, 95, 99]:
        print(f"  p{p:<3}: {size_label(int(pct(sizes, p)))}")
    print(f"  max:  {size_label(sizes[-1])}")
    print(f"  合計: {size_label(sum(sizes))}")

    # ヒストグラム
    hist(open_times, [1, 2, 5, 10, 20, 30, 50, 100, 200, 500], "open_ms")
    hist(nocache_total, [5, 10, 15, 20, 30, 50, 100, 200, 500], "no-cache total (open+resize)")

    # --- Auto モードしきい値ごとの cache 率 ---
    print()
    print("=== Auto モードしきい値スタディ ===")
    print("  (open_ms >= しきい値 のファイルを cache する場合)")
    print(f"  {'threshold':<12} {'cache率':>8} {'cache枚数':>10}")
    for th in [5, 10, 15, 20, 25, 30, 40, 50, 75, 100]:
        n_cache = sum(1 for v in open_times if v >= th)
        pct_cache = n_cache / len(open_times) * 100
        print(f"  >= {th:<5}ms  {pct_cache:>6.1f}%  {n_cache:>10}")

    # しきい値を open + resize で判定するパターンも
    print()
    print("  (open+resize >= しきい値 のファイルを cache する場合)")
    print(f"  {'threshold':<12} {'cache率':>8} {'cache枚数':>10}")
    for th in [10, 15, 20, 25, 30, 40, 50, 75, 100]:
        n_cache = sum(1 for v in nocache_total if v >= th)
        pct_cache = n_cache / len(nocache_total) * 100
        print(f"  >= {th:<5}ms  {pct_cache:>6.1f}%  {n_cache:>10}")

    # --- ファイルサイズと decode 時間の相関 ---
    print()
    print("=== ファイルサイズ別の平均 decode 時間 ===")
    size_buckets = [
        (0, 100_000, "< 100 KB"),
        (100_000, 500_000, "100-500 KB"),
        (500_000, 1_000_000, "500KB-1MB"),
        (1_000_000, 2_000_000, "1-2 MB"),
        (2_000_000, 5_000_000, "2-5 MB"),
        (5_000_000, 10_000_000, "5-10 MB"),
        (10_000_000, 50_000_000, "10-50 MB"),
        (50_000_000, float("inf"), "50 MB+"),
    ]
    for lo, hi, label in size_buckets:
        bucket = [num(r, "open_ms") for r in imgs if lo <= int(r["size_bytes"]) < hi]
        if not bucket:
            print(f"  {label:<15}: (0 件)")
            continue
        bs = sorted(bucket)
        print(
            f"  {label:<15}: {len(bucket):>6} 件  "
            f"p50={pct(bs,50):>6.1f}ms  "
            f"p95={pct(bs,95):>6.1f}ms  "
            f"max={max(bs):>7.1f}ms"
        )

    # --- 拡張子別 ---
    print()
    print("=== 拡張子別の decode 時間 ===")
    ext_imgs = defaultdict(list)
    for r in imgs:
        ext_imgs[r["ext"]].append(num(r, "open_ms"))
    for e, vals in sorted(ext_imgs.items(), key=lambda x: -len(x[1])):
        bs = sorted(vals)
        if len(bs) < 5:
            print(f"  {e:<6}: {len(bs)} 件 (サンプル少)")
            continue
        print(
            f"  {e:<6}: {len(bs):>6} 件  "
            f"p50={pct(bs,50):>6.1f}ms  "
            f"p95={pct(bs,95):>6.1f}ms  "
            f"max={max(bs):>7.1f}ms"
        )

    # --- 動画 ---
    if vids:
        print()
        print("=== 動画 (Shell API) ===")
        shell_times = sorted(num(r, "shell_ms") for r in vids)
        print(
            f"  n={len(shell_times)}  "
            f"min={min(shell_times):.1f}  p50={pct(shell_times,50):.1f}  "
            f"p90={pct(shell_times,90):.1f}  p95={pct(shell_times,95):.1f}  "
            f"p99={pct(shell_times,99):.1f}  max={max(shell_times):.1f}"
        )
        hist(shell_times, [10, 20, 50, 100, 200, 500, 1000, 2000], "shell_ms (動画)")

    # --- WebP エンコード後のサイズ削減率 ---
    print()
    print("=== WebP 変換後サイズ / 元ファイルサイズ ===")
    ratios = []
    webp_bytes_total = 0
    src_bytes_total = 0
    for r in imgs:
        wb = int(r.get("webp_bytes", "") or 0)
        sb = int(r["size_bytes"])
        if wb > 0 and sb > 0:
            ratios.append(wb / sb)
            webp_bytes_total += wb
            src_bytes_total += sb
    if ratios:
        sr = sorted(ratios)
        print(f"  p25={pct(sr,25)*100:.1f}%  p50={pct(sr,50)*100:.1f}%  p75={pct(sr,75)*100:.1f}%  p95={pct(sr,95)*100:.1f}%")
        print(f"  元合計: {size_label(src_bytes_total)}")
        print(f"  WebP合計: {size_label(webp_bytes_total)}")
        print(f"  総圧縮率: {webp_bytes_total/src_bytes_total*100:.1f}%")


if __name__ == "__main__":
    main()
