"""Check the length of card text on the product page (htdocs/mimageviewer/index.html).

Cards sit side by side in a grid, so one long card stretches the whole row and
breaks the layout (2026-10-11: the "safety" cards had grown to several
paragraphs each). Policy: CLAUDE.md "製品ページのカードは短く保つ".

Each card may have one heading and at most MAX_PARAGRAPHS paragraphs whose
visible text (tags removed, whitespace collapsed) totals at most MAX_CHARS
characters. Details belong in privacy.html or the manual, linked from the page.

Usage: python scripts/check_site_cards.py [path]   (exit 1 on violations)
"""

import html
import re
import sys
from html.parser import HTMLParser

CARD_CLASSES = {"trust-item", "feature-card", "usecase-card", "migration-card"}
MAX_CHARS = 150
MAX_PARAGRAPHS = 1


class CardParser(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.depth = 0
        self.card = None  # dict while inside a card
        self.cards = []
        self.in_p = False

    def handle_starttag(self, tag, attrs):
        if tag in ("br", "img", "input", "meta", "link", "hr", "source"):
            return
        classes = set((dict(attrs).get("class") or "").split())
        self.depth += 1
        if self.card is None and classes & CARD_CLASSES:
            self.card = {"cls": sorted(classes & CARD_CLASSES)[0], "depth": self.depth,
                         "title": "", "in_title": False, "paras": [], "line": self.getpos()[0]}
            return
        if self.card is not None:
            if tag in ("h3", "h4"):
                self.card["in_title"] = True
            elif tag == "p":
                self.in_p = True
                self.card["paras"].append("")

    def handle_endtag(self, tag):
        if tag in ("br", "img", "input", "meta", "link", "hr", "source"):
            return
        if self.card is not None:
            if tag in ("h3", "h4"):
                self.card["in_title"] = False
            elif tag == "p":
                self.in_p = False
            if self.depth == self.card["depth"]:
                self.cards.append(self.card)
                self.card = None
        self.depth -= 1

    def handle_data(self, data):
        if self.card is None:
            return
        if self.card["in_title"]:
            self.card["title"] += data
        elif self.in_p and self.card["paras"]:
            self.card["paras"][-1] += data


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "htdocs/mimageviewer/index.html"
    parser = CardParser()
    parser.feed(open(path, encoding="utf-8").read())
    bad = 0
    for c in parser.cards:
        text = re.sub(r"\s+", "", "".join(c["paras"]))
        title = re.sub(r"\s+", " ", c["title"]).strip()
        problems = []
        if len(c["paras"]) > MAX_PARAGRAPHS:
            problems.append(f"{len(c['paras'])} paragraphs (max {MAX_PARAGRAPHS})")
        if len(text) > MAX_CHARS:
            problems.append(f"{len(text)} chars (max {MAX_CHARS})")
        if problems:
            bad += 1
            print(f"{path}:{c['line']}: {c['cls']} '{title}': " + ", ".join(problems))
    print(f"{len(parser.cards)} cards checked, {bad} over the limit")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
