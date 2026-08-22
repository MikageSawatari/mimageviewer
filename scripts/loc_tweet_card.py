#!/usr/bin/env python3
"""Render a 1200x675 milestone card (X/Twitter aspect) from loc_history.json.

    python scripts/loc_history.py                                   # refresh the data
    python scripts/loc_tweet_card.py --png --meta "v3.2.0 ・ 2026-XX-XX"

Stacked weekly columns with Rust at the base, so its share reads at a glance,
and a dashed line at the milestone.  The card is static HTML+SVG -- no scripts
to wait on -- so the headless screenshot is deterministic.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import shutil
import subprocess
import sys
from pathlib import Path

W, H = 1200, 675
PAD = 44
SURFACE, INK, INK2, MUTED, GRID = "#1a1a19", "#ffffff", "#c3c2b7", "#898781", "#2c2c2a"
# Dark-mode categorical slots 1-4, in the validated order (adjacent CVD dE >= 8).
COLORS = ["#3987e5", "#d95926", "#199e70", "#c98500"]
KEEP = ["Rust", "Markdown", "JavaScript"]
OTHER = "その他"

CHART_W, CHART_H = 830, 372
LEGEND_W = 252
AXIS_W = 62  # room for the y labels, inside CHART_W


def fmt(n: int) -> str:
    return f"{n:,}"


def build_svg(points: list[dict], series: list[tuple[str, list[int]]], milestone: int) -> str:
    totals = [sum(s[1][i] for s in series) for i in range(len(points))]
    max_y = max(max(totals), milestone) * 1.10
    x0, x1 = AXIS_W, CHART_W
    n = len(points)
    slot = (x1 - x0) / n
    bw = min(slot * 0.68, 34)

    def y(v: float) -> float:
        return CHART_H - 22 - (v / max_y) * (CHART_H - 34)

    out = [f'<svg width="{CHART_W}" height="{CHART_H}" viewBox="0 0 {CHART_W} {CHART_H}">']

    for v in (0, milestone // 2):  # recessive gridlines at zero and the halfway mark
        label = "0" if v == 0 else f"{v // 1000}k"
        out.append(
            f'<line x1="{x0}" x2="{x1}" y1="{y(v):.1f}" y2="{y(v):.1f}" '
            f'stroke="{GRID}" stroke-width="1"/>'
        )
        out.append(
            f'<text x="{x0 - 10}" y="{y(v) + 4:.1f}" text-anchor="end" fill="{MUTED}" '
            f'font-size="13">{label}</text>'
        )

    for i in range(n):
        cx = x0 + slot * (i + 0.5)
        lower = 0
        for k, (_, values) in enumerate(series):
            upper = lower + values[i]
            top, bot = y(upper), y(lower)
            topmost = k == len(series) - 1
            gap = 0.0 if topmost else 1.5  # surface gap between stacked segments
            height = bot - top - gap
            if height > 0.4:
                out.append(
                    f'<rect x="{cx - bw / 2:.1f}" y="{top + gap:.1f}" width="{bw:.1f}" '
                    f'height="{height:.1f}" rx="{3 if topmost else 0}" fill="{COLORS[k]}"/>'
                )
            lower = upper

    my = y(milestone)  # drawn over the bars it explains
    out.append(
        f'<line x1="{x0}" x2="{x1}" y1="{my:.1f}" y2="{my:.1f}" stroke="{INK}" '
        f'stroke-width="1.5" stroke-dasharray="6 5" opacity="0.85"/>'
    )
    out.append(
        f'<text x="{x0 - 10}" y="{my + 4:.1f}" text-anchor="end" fill="{INK}" '
        f'font-size="13" font-weight="600">{milestone // 1000}k</text>'
    )

    prev_month = None  # one x label per month
    for i, p in enumerate(points):
        month = int(p["date"].split("-")[1])
        if month != prev_month:
            prev_month = month
            out.append(
                f'<text x="{x0 + slot * (i + 0.5):.1f}" y="{CHART_H - 2}" text-anchor="middle" '
                f'fill="{MUTED}" font-size="13">{month}月</text>'
            )

    out.append("</svg>")
    return "\n".join(out)


def shoot_png(html: Path) -> int:
    browser = next(
        (
            p
            for p in (
                r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                shutil.which("chrome"),
                shutil.which("msedge"),
            )
            if p and Path(p).exists()
        ),
        None,
    )
    if not browser:
        print("no Chrome or Edge found -- open the HTML and screenshot it", file=sys.stderr)
        return 1
    png = html.with_suffix(".png")
    subprocess.run(
        [
            browser,
            "--headless=new",
            "--disable-gpu",
            "--hide-scrollbars",
            "--force-device-scale-factor=2",  # 2400x1350, so X's own resize stays crisp
            f"--window-size={W},{H}",
            f"--screenshot={png.resolve()}",
            html.resolve().as_uri(),
        ],
        check=True,
        capture_output=True,
    )
    print(f"wrote {png}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--json", default="target/loc-history/loc_history.json")
    ap.add_argument("--out", default="target/loc-history/loc_milestone_card.html")
    ap.add_argument("--milestone", type=int, default=1_000_000)
    ap.add_argument("--headline", default="ソースコードが 100 万行を突破しました")
    ap.add_argument("--meta", default=None, help="the small line under the headline")
    ap.add_argument("--png", action="store_true", help="also shoot a PNG with headless Chrome/Edge")
    args = ap.parse_args()

    points = json.loads(Path(args.json).read_text(encoding="utf-8"))["points"]
    last = points[-1]
    total = last["total"]

    series: list[tuple[str, list[int]]] = [
        (k, [p["by_language"].get(k, 0) for p in points]) for k in KEEP
    ]
    series.append(
        (OTHER, [sum(v for k, v in p["by_language"].items() if k not in KEEP) for p in points])
    )

    days = (dt.date.fromisoformat(last["date"]) - dt.date.fromisoformat(points[0]["date"])).days
    commits = subprocess.run(
        ["git", "rev-list", "--count", last["commit"]], capture_output=True, text=True, check=True
    ).stdout.strip()
    meta = args.meta or f"{last['date']} 時点 ・ 最初のコミットから {days} 日"

    legend = "\n".join(
        f'<div class="row"><i style="background:{COLORS[k]}"></i>'
        f'<span class="name">{name}</span>'
        f'<span class="num">{fmt(values[-1])}</span>'
        f'<span class="pct">{values[-1] / total * 100:.1f}%</span></div>'
        for k, (name, values) in enumerate(series)
    )

    html = f"""<!doctype html>
<html lang="ja"><head><meta charset="utf-8"><title>milestone</title><style>
  html, body {{ margin:0; padding:0; background:{SURFACE}; }}
  .card {{
    width:{W}px; height:{H}px; box-sizing:border-box; padding:{PAD}px {PAD}px 32px;
    background:{SURFACE}; color:{INK}; overflow:hidden;
    font-family:"Yu Gothic UI","Yu Gothic",Meiryo,"Segoe UI",system-ui,sans-serif;
    display:flex; flex-direction:column;
  }}
  .brand {{ font-size:21px; color:{INK2}; letter-spacing:.04em; }}
  .hero {{ font-size:92px; font-weight:700; line-height:1.05; margin:6px 0 0; letter-spacing:-.02em; }}
  .hero span {{ font-size:34px; font-weight:400; color:{INK2}; margin-left:10px; }}
  .headline {{ font-size:27px; margin-top:10px; }}
  .meta {{ font-size:17px; color:{MUTED}; margin-top:7px; }}
  .body {{ display:flex; gap:20px; align-items:flex-end; margin-top:auto; }}
  .legend {{ width:{LEGEND_W}px; padding-bottom:26px; }}
  .row {{ display:flex; align-items:center; gap:9px; font-size:16px; padding:5px 0; }}
  .row i {{ width:11px; height:11px; border-radius:3px; flex:none; }}
  .row .name {{ flex:1; color:{INK2}; }}
  .row .num {{ font-variant-numeric:tabular-nums; }}
  .row .pct {{ width:50px; text-align:right; color:{MUTED}; font-variant-numeric:tabular-nums; }}
  .foot {{ font-size:14px; color:{MUTED}; margin-top:14px; }}
</style></head><body><div class="card">
  <div class="brand">mImageViewer</div>
  <div class="hero">{fmt(total)}<span>行</span></div>
  <div class="headline">{args.headline}</div>
  <div class="meta">{meta}</div>
  <div class="body">
    {build_svg(points, series, args.milestone)}
    <div class="legend">{legend}</div>
  </div>
  <div class="foot">コメント・空行を含む物理行数。テストとドキュメントを含み、第三者コード (vendor/) とバイナリは除く ・ {int(commits):,} コミット</div>
</div></body></html>
"""

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(html, encoding="utf-8")
    print(f"wrote {out}")
    return shoot_png(out) if args.png else 0


if __name__ == "__main__":
    raise SystemExit(main())
