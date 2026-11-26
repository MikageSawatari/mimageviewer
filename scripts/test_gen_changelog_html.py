"""Tests for gen-changelog-html.py: major-version tabs and the no-JavaScript fallback.

Run: python -m unittest scripts/test_gen_changelog_html.py
"""
import importlib.util
from pathlib import Path
import re
import unittest

SPEC = importlib.util.spec_from_file_location(
    "gen_changelog_html", Path(__file__).with_name("gen-changelog-html.py")
)
gen = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gen)

SAMPLE = """# title

## 更新履歴

### v5.1.0 (2026-11-10)

- **新しい機能**
  - 補足
- 修正

### v5.0.0 (2026-11-07)

- 最初の v5

### v4.11.0 (2026-11-02)

- v4 最後

### v0.3.0 (2026-04-11)

- 最初の公開

## 別の節

- 拾わない
"""


def sections(html):
    """{section id: [h2 ids inside it]} in document order."""
    out = {}
    for m in re.finditer(r'<section class="ver-group" id="([^"]+)">(.*?)</section>', html, re.S):
        out[m.group(1)] = re.findall(r'<h2 id="([^"]+)"', m.group(2))
    return out


class GroupingTest(unittest.TestCase):
    def test_groups_follow_changelog_order(self):
        versions = gen.parse_changelog(SAMPLE)
        groups = gen.group_by_major(versions)
        self.assertEqual([g["major"] for g in groups], ["v5", "v4", "v0"])
        self.assertEqual([v["ver"] for v in groups[0]["versions"]], ["v5.1.0", "v5.0.0"])

    def test_split_major_is_an_error(self):
        versions = [{"ver": "v5.1.0"}, {"ver": "v4.0.0"}, {"ver": "v5.0.0"}]
        with self.assertRaises(SystemExit):
            gen.group_by_major(versions)

    def test_heading_without_major_is_an_error(self):
        with self.assertRaises(SystemExit):
            gen.group_by_major([{"ver": "next"}])


class RenderTest(unittest.TestCase):
    def setUp(self):
        self.html = gen.render_versions(gen.parse_changelog(SAMPLE))

    def test_every_version_sits_in_its_major_section(self):
        self.assertEqual(
            sections(self.html),
            {"v5": ["v5.1.0", "v5.0.0"], "v4": ["v4.11.0"], "v0": ["v0.3.0"]},
        )

    def test_without_javascript_everything_is_shown(self):
        # The tab bar is hidden until the script reveals it; no section is hidden.
        self.assertRegex(self.html, r'<div class="ver-tabs"[^>]*\shidden>')
        self.assertNotRegex(self.html, r"<section[^>]*hidden")

    def test_one_tab_per_major_then_all(self):
        tabs = re.findall(r'data-major="([^"]+)"', self.html)
        self.assertEqual(tabs, ["v5", "v4", "v0", "all"])
        for major in ["v5", "v4", "v0"]:
            self.assertIn('aria-controls="{}"'.format(major), self.html)


class RealChangelogTest(unittest.TestCase):
    def test_real_changelog_groups_cleanly(self):
        text = Path(gen.CHANGELOG).read_text(encoding="utf-8")
        versions = gen.parse_changelog(text)
        html = gen.render_versions(versions)
        found = sections(html)
        self.assertEqual(sum(len(v) for v in found.values()), len(versions))
        newest_major = gen.major_of(versions[0]["ver"])
        self.assertEqual(next(iter(found)), newest_major)
        # Section ids must not collide with a version anchor.
        self.assertFalse(set(found) & {v["ver"] for v in versions})

    def test_page_keeps_query_string_and_script_placement(self):
        # Tabs are addressed by the hash only; replaceState with a bare "#..."
        # keeps ?version= that the app adds when it opens this page.
        self.assertIn('history.replaceState(null, "", "#" + major)', gen.TAB_SCRIPT)
        self.assertNotIn("location.search", gen.TAB_SCRIPT)
        page = gen.PAGE_TEMPLATE.format(sidebar="", content="", style=gen.TAB_STYLE, script=gen.TAB_SCRIPT)
        self.assertLess(page.index("</main>"), page.index("<script>"))


if __name__ == "__main__":
    unittest.main()
