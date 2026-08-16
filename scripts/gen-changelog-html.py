#!/usr/bin/env python3
"""Generate htdocs/mimageviewer/manual/changelog.html from README.md.

The "## 更新履歴" section of README.md is the single source of truth for the
changelog (it also feeds the GitHub Release body). This script transcribes that
section into the manual's HTML design so mikage.to can show the same history in
the manual look-and-feel instead of only the programmer-oriented GitHub Releases
page.

changelog.html is a GENERATED ARTIFACT. Do not edit it by hand — edit README.md
and re-run this script (see the release checklist in CLAUDE.md, Phase 1).

The page chrome (header / breadcrumb / footer) and the SEO meta block are templated
here. The meta block must stay in the template: every other manual page carries a
hand-written one, and a generated page that only copied the chrome would silently
drop its description / canonical / OpenGraph tags on the next release. The sidebar
nav is copied verbatim from a sibling manual page (getting-started.html) so that
adding a new manual page and re-running this script keeps changelog.html's
sidebar in sync automatically. That sibling page must already contain the
"更新履歴" link (added once to every manual page's sidebar).

Usage:
    python scripts/gen-changelog-html.py
"""

import os
import re
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
README = os.path.join(REPO, "README.md")
MANUAL = os.path.join(REPO, "htdocs", "mimageviewer", "manual")
SIDEBAR_SOURCE = os.path.join(MANUAL, "getting-started.html")
OUT = os.path.join(MANUAL, "changelog.html")


# ── Inline markdown → HTML ──────────────────────────────────────────────────
def conv_inline(text):
    """Convert one line of inline markdown to HTML.

    Handles **bold**, `code`, [text](https://…) links, and passes <kbd>…</kbd>
    through untouched while HTML-escaping the surrounding plain text. <kbd>
    spans (which may appear inside **bold**) are protected first so escaping
    does not mangle them.

    Links are converted last so their text can carry the earlier inline markup.
    Only absolute http(s) targets are recognised: README paths are relative to
    the repository root, but this page is generated into manual/, so a relative
    target would silently point somewhere else. Anything else stays literal,
    which is visible on the page rather than quietly wrong.
    """
    kbds = []

    def protect(m):
        kbds.append(m.group(0))
        return "\x00K{}\x00".format(len(kbds) - 1)

    text = re.sub(r"<kbd>.*?</kbd>", protect, text)
    text = text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    text = re.sub(r"`([^`]+)`", lambda m: "<code>" + m.group(1) + "</code>", text)
    text = re.sub(r"\*\*(.+?)\*\*", lambda m: "<strong>" + m.group(1) + "</strong>", text)
    text = re.sub(
        r"\[([^\]]+)\]\((https?://[^)\s]+)\)",
        lambda m: '<a href="{}">{}</a>'.format(m.group(2), m.group(1)),
        text,
    )
    for i, k in enumerate(kbds):
        text = text.replace("\x00K{}\x00".format(i), k)
    return text


# ── Parse the README changelog section ──────────────────────────────────────
def parse_changelog(readme_text):
    lines = readme_text.split("\n")
    start = None
    for i, line in enumerate(lines):
        if line.strip() == "## 更新履歴":
            start = i + 1
            break
    if start is None:
        sys.exit("ERROR: '## 更新履歴' section not found in README.md")
    end = len(lines)
    for i in range(start, len(lines)):
        if lines[i].startswith("## "):
            end = i
            break

    versions = []
    cur = None
    for raw in lines[start:end]:
        line = raw.rstrip("\r")
        if line.startswith("### "):
            head = line[4:].strip()
            m = re.match(r"^(\S+?)(?:\s+\((\d{4}-\d{2}-\d{2})\))?\s*$", head)
            ver, date = (m.group(1), m.group(2)) if m else (head, None)
            cur = {"ver": ver, "date": date, "items": []}
            versions.append(cur)
        elif line.startswith("- "):
            cur["items"].append({"text": line[2:], "subs": [], "tails": []})
        elif re.match(r"^\s+- ", line):
            if cur and cur["items"]:
                cur["items"][-1]["subs"].append(re.sub(r"^\s+- ", "", line))
        elif line.strip() == "":
            continue
        elif re.match(r"^\s+\S", line):
            # indented continuation paragraph belonging to the current bullet
            if cur and cur["items"]:
                cur["items"][-1]["tails"].append(line.strip())
    return versions


# ── Render versions → HTML ──────────────────────────────────────────────────
def render_items(items):
    out = ["<ul>"]
    for it in items:
        text = conv_inline(it["text"])
        if it["subs"] or it["tails"]:
            out.append("  <li>{}".format(text))
            if it["subs"]:
                out.append("    <ul>")
                for s in it["subs"]:
                    out.append("      <li>{}</li>".format(conv_inline(s)))
                out.append("    </ul>")
            for t in it["tails"]:
                out.append("    <p>{}</p>".format(conv_inline(t)))
            out.append("  </li>")
        else:
            out.append("  <li>{}</li>".format(text))
    out.append("</ul>")
    return "\n".join(out)


def render_versions(versions):
    blocks = []
    for v in versions:
        if v.get("date"):
            blocks.append(
                '<h2 id="{ver}">{ver} <span class="ver-date">{date}</span></h2>'.format(
                    ver=v["ver"], date=v["date"]
                )
            )
        else:
            blocks.append('<h2 id="{ver}">{ver}</h2>'.format(ver=v["ver"]))
        blocks.append(render_items(v["items"]))
    return "\n\n".join(blocks)


def indent(text, n):
    pad = " " * n
    return "\n".join((pad + line if line else line) for line in text.split("\n"))


# ── Sidebar (copied from a sibling page, active link re-marked) ──────────────
def build_sidebar():
    with open(SIDEBAR_SOURCE, "r", encoding="utf-8", newline="") as f:
        base = f.read()
    m = re.search(r'<aside class="sidebar">.*?</aside>', base, re.S)
    if not m:
        sys.exit("ERROR: sidebar block not found in {}".format(SIDEBAR_SOURCE))
    sidebar = m.group(0)
    if 'href="changelog.html"' not in sidebar:
        sys.exit(
            "ERROR: sidebar in {} has no changelog.html link. Add the "
            "'更新履歴' link to every manual page's sidebar first.".format(SIDEBAR_SOURCE)
        )
    sidebar = re.sub(r'\s+class="active"', "", sidebar)
    sidebar = sidebar.replace(
        '<a href="changelog.html">更新履歴</a>',
        '<a href="changelog.html" class="active">更新履歴</a>',
    )
    # normalise line endings inherited from the sibling page to LF
    return sidebar.replace("\r\n", "\n")


PAGE_TEMPLATE = """<!DOCTYPE html>
<!-- AUTO-GENERATED by scripts/gen-changelog-html.py from README.md. Do not edit by hand. -->
<html lang="ja">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>更新履歴 — mImageViewer マニュアル</title>
  <!-- meta:begin (generated) -->
  <meta name="description" content="mImageViewer のバージョンごとの変更点の一覧。新機能・改善・修正をリリース順にまとめています。">
  <link rel="canonical" href="https://mikage.to/mimageviewer/manual/changelog.html">
  <meta property="og:type" content="article">
  <meta property="og:site_name" content="mImageViewer">
  <meta property="og:locale" content="ja_JP">
  <meta property="og:url" content="https://mikage.to/mimageviewer/manual/changelog.html">
  <meta property="og:title" content="更新履歴｜mImageViewer">
  <meta property="og:description" content="mImageViewer のバージョンごとの変更点の一覧。新機能・改善・修正をリリース順にまとめています。">
  <meta property="og:image" content="https://mikage.to/mimageviewer/ogp.jpg">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="更新履歴｜mImageViewer">
  <meta name="twitter:description" content="mImageViewer のバージョンごとの変更点の一覧。新機能・改善・修正をリリース順にまとめています。">
  <meta name="twitter:image" content="https://mikage.to/mimageviewer/ogp.jpg">
  <!-- meta:end -->
  <link rel="stylesheet" href="style.css">
</head>
<body>

<header class="site-header">
  <span class="logo">mImageViewer</span>
  <nav class="breadcrumb">
    <a href="../index.html">ホーム</a>
    <span class="sep">›</span>
    <a href="index.html">マニュアル</a>
    <span class="sep">›</span>
    <span>更新履歴</span>
  </nav>
</header>

<div class="layout">
  {sidebar}

  <main class="content">
    <h1 class="page-title">更新履歴</h1>
    <p class="page-desc">mImageViewer のバージョンごとの変更点をまとめています。最新版は<a href="../index.html#download">ダウンロードページ</a>から入手できます。</p>

{content}
  </main>
</div>

<footer>
  <p>mImageViewer &copy; 2025-2026 Mikage Sawatari</p>
</footer>

</body>
</html>
"""


def main():
    with open(README, "r", encoding="utf-8", newline="") as f:
        readme_text = f.read()
    versions = parse_changelog(readme_text)
    content = indent(render_versions(versions), 4)
    sidebar = build_sidebar()
    page = PAGE_TEMPLATE.format(sidebar=sidebar, content=content)
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(page)
    print("Wrote {} ({} versions)".format(OUT, len(versions)))


if __name__ == "__main__":
    main()
