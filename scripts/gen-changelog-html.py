#!/usr/bin/env python3
"""Generate htdocs/mimageviewer/manual/changelog.html from CHANGELOG.md.

The "## 更新履歴" section of CHANGELOG.md is the single source of truth for the
changelog (it also feeds the GitHub Release body). It lived in README.md until
2026-09; it moved so the README can open with an overview instead of 800 lines
of history. This script transcribes that
section into the manual's HTML design so mikage.to can show the same history in
the manual look-and-feel instead of only the programmer-oriented GitHub Releases
page.

changelog.html is a GENERATED ARTIFACT. Do not edit it by hand — edit CHANGELOG.md
and re-run this script (see the release checklist in CLAUDE.md, Phase 1).

The page chrome (header / breadcrumb / footer) and the SEO meta block are templated
here. The meta block must stay in the template: every other manual page carries a
hand-written one, and a generated page that only copied the chrome would silently
drop its description / canonical / OpenGraph tags on the next release. The sidebar
nav is copied verbatim from a sibling manual page (getting-started.html) so that
adding a new manual page and re-running this script keeps changelog.html's
sidebar in sync automatically. That sibling page must already contain the
"更新履歴" link (added once to every manual page's sidebar).

The versions are grouped into one tab per major version (v5 / v4 / …, plus
"すべて"), newest major open first. A version anchor (#v4.2.0) opens the tab that
holds it. Without JavaScript the tab bar stays hidden and every version is shown.
Tests: python -m unittest scripts/test_gen_changelog_html.py

Usage:
    python scripts/gen-changelog-html.py
"""

import os
import re
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CHANGELOG = os.path.join(REPO, "CHANGELOG.md")
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
    Only absolute http(s) targets are recognised: CHANGELOG paths are relative to
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


# ── Parse the CHANGELOG changelog section ──────────────────────────────────────
def parse_changelog(changelog_text):
    lines = changelog_text.split("\n")
    start = None
    for i, line in enumerate(lines):
        if line.strip() == "## 更新履歴":
            start = i + 1
            break
    if start is None:
        sys.exit("ERROR: '## 更新履歴' section not found in CHANGELOG.md")
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


def render_version(v):
    if v.get("date"):
        head = '<h2 id="{ver}">{ver} <span class="ver-date">{date}</span></h2>'.format(
            ver=v["ver"], date=v["date"]
        )
    else:
        head = '<h2 id="{ver}">{ver}</h2>'.format(ver=v["ver"])
    return head + "\n\n" + render_items(v["items"])


def major_of(ver):
    """'v5.8.0' -> 'v5'. Every heading must start with v<major>; anything else
    would land in no tab, so stop instead of guessing."""
    m = re.match(r"^v(\d+)\.", ver)
    if not m:
        sys.exit("ERROR: version heading '{}' does not start with v<major>.".format(ver))
    return "v" + m.group(1)


def group_by_major(versions):
    """Keep CHANGELOG order (newest first) and split it into runs of one major
    version. A major that reappears after another one means the CHANGELOG is out
    of order, which would split a tab in two, so it is an error."""
    groups = []
    for v in versions:
        major = major_of(v["ver"])
        if groups and groups[-1]["major"] == major:
            groups[-1]["versions"].append(v)
            continue
        if any(g["major"] == major for g in groups):
            sys.exit("ERROR: {} versions are not contiguous in CHANGELOG.md.".format(major))
        groups.append({"major": major, "versions": [v]})
    return groups


# The tab bar starts hidden and every group starts shown, so a browser without
# JavaScript (or with the script failing) still shows the whole history. The
# script below reveals the bar and hides the groups that are not selected.
def render_tabs(groups):
    out = ['<div class="ver-tabs" role="tablist" aria-label="メジャー版" hidden>']
    for g in groups:
        out.append(
            '  <button type="button" role="tab" class="ver-tab" id="tab-{m}" '
            'aria-controls="{m}" data-major="{m}">{m}</button>'.format(m=g["major"])
        )
    out.append(
        '  <button type="button" role="tab" class="ver-tab" id="tab-all" '
        'data-major="all">すべて</button>'
    )
    out.append("</div>")
    return "\n".join(out)


def render_versions(versions):
    groups = group_by_major(versions)
    blocks = [render_tabs(groups)]
    for g in groups:
        body = "\n\n".join(render_version(v) for v in g["versions"])
        # role="tabpanel" is added by the script, together with the tabs, so a
        # page without JavaScript does not announce panels with no tabs.
        blocks.append(
            '<section class="ver-group" id="{m}">\n{body}\n</section>'.format(
                m=g["major"], body=indent(body, 2)
            )
        )
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


# Tab switching for the major-version groups. It runs inline right after the
# content, before the page is first shown, so the other groups never flash.
# Links that carry a version anchor (#v4.2.0) open the tab holding that version;
# the tab itself is addressable as #v4, and #all shows every version. The query
# string (the app opens this page with ?version=<app version>) is left alone.
TAB_SCRIPT = """<script>
(function () {
  var tabs = document.querySelector(".ver-tabs");
  if (!tabs || !window.history || !Element.prototype.closest) return;
  var buttons = Array.prototype.slice.call(tabs.querySelectorAll(".ver-tab"));
  var groups = Array.prototype.slice.call(document.querySelectorAll(".ver-group"));
  var content = tabs.parentNode;
  if (!buttons.length || !groups.length) return;

  function hashId() {
    try { return decodeURIComponent(location.hash.slice(1)); } catch (e) { return ""; }
  }
  // The tab that holds the element with this id, "all", or null.
  function majorFor(id) {
    if (id === "all") return "all";
    var el = id ? document.getElementById(id) : null;
    var group = el ? el.closest(".ver-group") : null;
    return group ? group.id : null;
  }
  function select(major, focus) {
    buttons.forEach(function (b) {
      var on = b.getAttribute("data-major") === major;
      b.setAttribute("aria-selected", on ? "true" : "false");
      b.tabIndex = on ? 0 : -1;
      if (on && focus) b.focus();
    });
    groups.forEach(function (g) { g.hidden = major !== "all" && g.id !== major; });
    content.classList.toggle("ver-show-all", major === "all");
  }
  // Jump without the page's smooth scrolling: the content has just changed, so
  // gliding across thousands of pixels of it only blurs where we landed.
  // behavior: "instant" is not honoured everywhere, so switch the stylesheet's
  // scroll-behavior off around the call instead.
  function scrollToTop(el) {
    var root = document.documentElement;
    var prev = root.style.scrollBehavior;
    root.style.scrollBehavior = "auto";
    el.scrollIntoView(true);
    root.style.scrollBehavior = prev;
  }
  function jumpTo(id) {
    var el = id && id !== "all" ? document.getElementById(id) : null;
    if (el) scrollToTop(el);
  }
  // A tab was chosen: show it from its top when the bar has stuck to the header.
  function choose(button, focus) {
    var major = button.getAttribute("data-major");
    var stuck = tabs.getBoundingClientRect().top <= parseFloat(getComputedStyle(tabs).top) + 1;
    select(major, focus);
    history.replaceState(null, "", "#" + major);
    if (stuck) scrollToTop(major === "all" ? groups[0] : document.getElementById(major));
  }

  groups.forEach(function (g) {
    g.setAttribute("role", "tabpanel");
    g.setAttribute("aria-labelledby", "tab-" + g.id);
  });
  buttons.forEach(function (b, i) {
    b.addEventListener("click", function () { choose(b, false); });
    b.addEventListener("keydown", function (e) {
      var next = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: buttons.length - 1 }[e.key];
      if (next === undefined) return;
      e.preventDefault();
      choose(buttons[(next + buttons.length) % buttons.length], true);
    });
  });
  window.addEventListener("hashchange", function () {
    var id = hashId();
    var major = majorFor(id);
    if (!major) return;
    select(major);
    jumpTo(id);
  });

  var id = hashId();
  select(majorFor(id) || groups[0].id);
  tabs.hidden = false;
  content.classList.add("ver-tabs-on");
  if (majorFor(id)) jumpTo(id);
})();
</script>"""


PAGE_TEMPLATE = """<!DOCTYPE html>
<!-- AUTO-GENERATED by scripts/gen-changelog-html.py from CHANGELOG.md. Do not edit by hand. -->
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

{script}

<footer>
  <p>mImageViewer &copy; 2025-2026 Mikage Sawatari</p>
</footer>

</body>
</html>
"""


def main():
    with open(CHANGELOG, "r", encoding="utf-8", newline="") as f:
        changelog_text = f.read()
    versions = parse_changelog(changelog_text)
    content = indent(render_versions(versions), 4)
    sidebar = build_sidebar()
    page = PAGE_TEMPLATE.format(sidebar=sidebar, content=content, script=TAB_SCRIPT)
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(page)
    print("Wrote {} ({} versions)".format(OUT, len(versions)))


if __name__ == "__main__":
    main()
