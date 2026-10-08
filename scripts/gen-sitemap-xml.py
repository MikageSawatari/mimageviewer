#!/usr/bin/env python3
"""Generate htdocs/sitemap.xml from the HTML files under htdocs/.

sitemap.xml is a GENERATED ARTIFACT. Do not edit it by hand — add or remove
pages under htdocs/ and re-run this script (see the release checklist in
CLAUDE.md, Phase 1).

Why this exists: the sitemap was hand-written when the site's technical SEO was
set up (2026-07). By 2026-09 it had silently drifted — 37 pages had changed
without a single <lastmod> being touched, and manual/external-tools.html had
been added without ever entering the sitemap. Nothing in the release process
pointed at the file, so it could only rot. Generating it from the files on disk
removes the chance to forget.

<lastmod> comes from git, not from the filesystem: a fresh clone or a checkout
rewrites every mtime to "now", which would tell Google that all 68 pages changed
today. The last commit that touched a file is the closest reproducible stand-in
for "when this page's content last changed". A file with uncommitted edits is
dated today, since that is the version about to be uploaded.

Usage:
    python scripts/gen-sitemap-xml.py            # rewrite htdocs/sitemap.xml
    python scripts/gen-sitemap-xml.py --check    # exit 1 if it is out of date
"""

import datetime
import os
import subprocess
import sys
from xml.sax.saxutils import escape

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HTDOCS = os.path.join(REPO, "htdocs")
OUT = os.path.join(HTDOCS, "sitemap.xml")
BASE = "https://mikage.to/"

# Paths served as a directory URL rather than by filename. Only these two: every
# other index.html (manual/index.html) has accumulated search history under its
# explicit filename, and rewriting it to the directory form would be a URL change
# that throws that away.
DIRECTORY_FORM = {
    "index.html": "",
    "mimageviewer/index.html": "mimageviewer/",
}


def priority(rel):
    """Crawl priority for one site-relative URL path."""
    if rel == "":
        return "0.8"  # mikage.to top page: a link list, not the product
    if rel == "mimageviewer/":
        return "1.0"  # the product page
    if rel.startswith("mimageviewer/migrate-"):
        return "0.7"  # migration guides: the main non-brand entry points
    return "0.5"


def git_lines(args):
    out = subprocess.run(
        ["git"] + args, cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    )
    if out.returncode != 0:
        return None
    return out.stdout.decode("utf-8", "replace").splitlines()


def dirty_paths():
    """Repo-relative paths under htdocs/ with uncommitted or untracked changes."""
    lines = git_lines(["status", "--porcelain", "--", "htdocs"])
    if lines is None:
        return set()
    paths = set()
    for line in lines:
        # "XY path" or "XY old -> new" for renames; the new name is what ships.
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        paths.add(path.strip('"'))
    return paths


def last_commit_dates():
    """Map repo-relative htdocs path -> YYYY-MM-DD its content last changed.

    Merge commits list no files under --name-only, so a file is dated by the
    commit that actually edited it rather than by the merge that later carried it
    to master. That is what <lastmod> means. (The exception is a merge that
    resolves a conflict inside the file: that edit is invisible here, so the date
    comes out a few days early. Erring old is the safe direction -- it never tells
    Google a page is fresher than it is.)

    Dates are maxed rather than first-seen: git log walks the commit graph, not a
    date-sorted list, so with branches an older commit can be emitted first.
    """
    lines = git_lines(["log", "--format=%x00%cs", "--name-only", "--", "htdocs"])
    if lines is None:
        return {}
    dates = {}
    current = None
    for line in lines:
        if line.startswith("\x00"):
            current = line[1:]
        elif line and current:
            if current > dates.get(line, ""):  # ISO dates compare as strings
                dates[line] = current
    return dates


def build():
    today = datetime.date.today().isoformat()
    dirty = dirty_paths()
    committed = last_commit_dates()

    entries = []
    for root, _dirs, files in os.walk(HTDOCS):
        for name in sorted(files):
            if not name.endswith(".html"):
                continue
            abs_path = os.path.join(root, name)
            rel_file = os.path.relpath(abs_path, HTDOCS).replace(os.sep, "/")
            repo_rel = "htdocs/" + rel_file

            with open(abs_path, "r", encoding="utf-8", errors="replace") as f:
                head = f.read(4096)
            if "noindex" in head.lower():
                continue

            rel_url = DIRECTORY_FORM.get(rel_file, rel_file)
            lastmod = today if repo_rel in dirty else committed.get(repo_rel, today)
            entries.append((rel_url, lastmod, priority(rel_url)))

    entries.sort(key=lambda e: e[0])

    out = ['<?xml version="1.0" encoding="UTF-8"?>']
    out.append('<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">')
    for rel_url, lastmod, prio in entries:
        out.append("  <url>")
        out.append("    <loc>%s</loc>" % escape(BASE + rel_url))
        out.append("    <lastmod>%s</lastmod>" % lastmod)
        out.append("    <priority>%s</priority>" % prio)
        out.append("  </url>")
    out.append("</urlset>")
    return "\n".join(out) + "\n", entries


def main():
    check_only = "--check" in sys.argv[1:]
    new, entries = build()

    old = ""
    if os.path.exists(OUT):
        with open(OUT, "r", encoding="utf-8", newline="") as f:
            old = f.read()

    if old == new:
        print("sitemap.xml is up to date (%d URLs)" % len(entries))
        return 0

    old_locs = {
        line.strip()[5:-6] for line in old.splitlines() if line.strip().startswith("<loc>")
    }
    new_locs = {BASE + rel for rel, _, _ in entries}
    for loc in sorted(new_locs - old_locs):
        print("  added   %s" % loc)
    for loc in sorted(old_locs - new_locs):
        print("  removed %s" % loc)

    if check_only:
        print("sitemap.xml is out of date. Run: python scripts/gen-sitemap-xml.py")
        return 1

    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(new)
    print("wrote %s (%d URLs)" % (os.path.relpath(OUT, REPO), len(entries)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
