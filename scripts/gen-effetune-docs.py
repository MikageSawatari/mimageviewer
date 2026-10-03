#!/usr/bin/env python3
"""Offline, deterministic Japanese EffeTune snapshot -> mIV manual.

Usage: python scripts/gen-effetune-docs.py [snapshot] [--version v0.11.1] [--check]
VERSION in vendor/effetune-mixwright takes precedence over --version.
Selection and post-conversion corrections live in docs/effetune-docs-overrides.
Unpublished Markdown links become plain text; root-relative links go upstream.
Plugin lists define category/plugin splits; corrections precede link remapping.
The version-independent effetune/index.html entry redirects to the bundle.
Only the Markdown/HTML subset used by the published snapshot is accepted.
"""
from __future__ import annotations

import argparse
import html
from html.parser import HTMLParser
import json
import posixpath
from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
MANUAL = ROOT / "htdocs/mimageviewer/manual"
RULES = ROOT / "docs/effetune-docs-overrides"
DEFAULT_SOURCE = ROOT / "third_party/effetune-docs/f3189f3d"
UPSTREAM = "https://effetune.frieve.com"


class ConversionError(ValueError):
    pass


def front_matter(source):
    lines = source.splitlines()
    metadata = {}
    if lines and lines[0] == "---":
        try:
            end = lines.index("---", 1)
        except ValueError as exc:
            raise ConversionError("Unclosed front matter") from exc
        for line in lines[1:end]:
            match = re.fullmatch(r'(title|description|lang):\s*(.*)', line)
            if not match or match[1] in metadata:
                raise ConversionError(f"Unknown/duplicate front matter: {line}")
            value = match[2]
            if value.startswith('"'):
                value = json.loads(value)
            metadata[match[1]] = value
        if metadata.get("lang", "ja") != "ja":
            raise ConversionError("Only Japanese documentation may be published")
        lines = lines[end + 1:]
    return metadata, "\n".join(lines)


def expand_liquid(source, directory, source_root, stack=()):
    def include(match):
        tag = match[1].strip()
        parsed = re.fullmatch(r'include_relative ([\w./-]+\.md)', tag)
        if not parsed:
            # No Jekyll _includes files are supplied in this snapshot.
            raise ConversionError(f"Unknown/unavailable Liquid tag: {tag}")
        path = (directory / parsed[1]).resolve()
        if not path.is_relative_to(source_root.resolve()) or path in stack:
            raise ConversionError(f"Unsafe/recursive include: {tag}")
        if not path.is_file():
            raise ConversionError(f"Missing include: {path}")
        _, body = front_matter(path.read_text(encoding="utf-8"))
        return expand_liquid(body, path.parent, source_root, stack + (path,))
    source = re.sub(r'\{%\s*(.*?)\s*%\}', include, source, flags=re.S)
    if "{%" in source or "{{" in source or "%}" in source or "}}" in source:
        raise ConversionError("Unknown Liquid expression")
    return source


def heading_id(text):
    text = re.sub(r'<[^>]*>', '', text)
    text = re.sub(r'[`*_]', '', text).lower()
    return re.sub(r'\s', '-', re.sub(r'[^\w\s-]', '', text))


def select_sections(source, rule):
    """Select exact Markdown headings before parsing platform-only README HTML.

    Each selected heading includes its descendants, ending at the next heading
    of the same or higher level. Missing, duplicate and overlapping selections
    fail rather than silently losing instructions when the snapshot changes.
    """
    if 'sections' not in rule:
        return source
    headings = list(re.finditer(r'^(#{1,6}) (.+)$', source, re.M))
    selected, spans = [], []
    for name in rule['sections']:
        matches = [h for h in headings if h[2] == name]
        if len(matches) != 1:
            raise ConversionError(f'Selected heading must match once: {name}')
        match = matches[0]
        end = next((h.start() for h in headings if h.start() > match.start()
                    and len(h[1]) <= len(match[1])), len(source))
        if any(match.start() < b and a < end for a, b in spans):
            raise ConversionError(f'Overlapping selected heading: {name}')
        spans.append((match.start(), end))
        # Selected subsections now belong directly beneath the guide's h1.
        section = source[match.start():end].strip()
        selected.append(re.sub(r'^(#{1,6}) ',
                               lambda h: '#' * (len(h[1]) - len(match[1]) + 2) + ' ',
                               section, flags=re.M))
    if not selected or not rule.get('title'):
        raise ConversionError('Section selection requires sections and a title')
    return '# ' + rule['title'] + '\n\n' + '\n\n'.join(selected)


class ImageTag(HTMLParser):
    def __init__(self, rewrite):
        super().__init__(convert_charrefs=True)
        self.rewrite = rewrite
        self.output = None

    def handle_starttag(self, tag, attrs):
        allowed = {"src", "alt", "width", "height", "align", "title"}
        if tag != "img" or self.output is not None or any(k not in allowed or v is None for k, v in attrs):
            raise ConversionError("Unsupported inline HTML")
        if len(dict(attrs)) != len(attrs) or "src" not in dict(attrs):
            raise ConversionError("Invalid img attributes")
        rendered = []
        for key, value in attrs:
            if key == "src":
                value = self.rewrite(value, True)
            rendered.append(f'{key}="{html.escape(value, quote=True)}"')
        self.output = '<img ' + ' '.join(rendered) + '>'

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)

    def handle_data(self, data):
        if data.strip():
            raise ConversionError("Unexpected text in img tag")

    def handle_endtag(self, tag):
        raise ConversionError(f"Unknown HTML end tag: {tag}")


class Converter:
    def __init__(self, page, published, known, images=()):
        self.page = page
        self.published = set(published)
        self.known = set(known)
        self.images = set(images)
        self.used_ids = {}

    def rewrite(self, url, image=False):
        parts = urlsplit(url)
        if parts.scheme:
            if parts.scheme not in {"https", "http", "mailto"} or image and parts.scheme == "mailto":
                raise ConversionError(f"Unsupported URL: {url}")
            return url
        if url.startswith("//"):
            raise ConversionError(f"Protocol-relative URL: {url}")
        if url.startswith("/"):
            return UPSTREAM + url
        if image:
            match = re.fullmatch(r'(?:\.\./)*images/([\w.-]+)', parts.path)
            if not match or match[1] not in self.images or parts.query or parts.fragment:
                raise ConversionError(f"Unknown snapshot image: {url}")
            return "images/" + match[1]
        if not parts.path:
            return url
        resolved = posixpath.normpath(posixpath.join(posixpath.dirname(self.page), unquote(parts.path)))
        if resolved not in self.known:
            raise ConversionError(f"Unknown local link in {self.page}: {url}")
        if resolved not in self.published:
            return None
        return Path(resolved).stem + ".html" + ("?" + parts.query if parts.query else "") + ("#" + parts.fragment if parts.fragment else "")

    def inline(self, text):
        output = []
        i = 0
        while i < len(text):
            tail = text[i:]
            if tail.startswith("<!--"):
                end = text.find("-->", i + 4)
                if end < 0:
                    raise ConversionError("Unclosed HTML comment")
                output.append(text[i:end + 3])
                i = end + 3
                continue
            if tail.startswith("\\"):
                if i + 1 == len(text):
                    output.append("<br>")
                    i += 1
                elif text[i + 1] in r'\`*_{}[]()#+-.!<>|':
                    output.append(html.escape(text[i + 1]))
                    i += 2
                else:
                    output.append("\\")
                    i += 1
                continue
            if tail.startswith("`"):
                match = re.match(r'(`+)(.*?)\1', tail, re.S)
                if not match:
                    raise ConversionError("Unclosed code span")
                output.append("<code>" + html.escape(match[2]) + "</code>")
                i += match.end()
                continue
            is_image = tail.startswith("![")
            if is_image or tail.startswith("["):
                start = i + (2 if is_image else 1)
                depth, end = 1, start
                while end < len(text) and depth:
                    if text[end] == "[":
                        depth += 1
                    elif text[end] == "]":
                        depth -= 1
                    end += 1
                if depth or text[end:end + 1] != "(":
                    raise ConversionError(f"Unsupported link syntax: {tail[:80]}")
                finish = end + 1
                depth = 1
                while finish < len(text) and depth:
                    if text[finish] == "(":
                        depth += 1
                    elif text[finish] == ")":
                        depth -= 1
                    finish += 1
                if depth:
                    raise ConversionError("Unclosed link target")
                target = text[end + 1:finish - 1]
                match = re.fullmatch(r'(\S+?)(?:\s+"([^"]*)")?', target)
                if not match:
                    raise ConversionError(f"Unsupported link target: {target}")
                url = self.rewrite(match[1], is_image)
                title = f' title="{html.escape(match[2], quote=True)}"' if match[2] else ''
                label = text[start:end - 1]
                if is_image:
                    output.append(f'<img src="{html.escape(url, quote=True)}" alt="{html.escape(label, quote=True)}"{title}>')
                else:
                    label = self.inline(label)
                    output.append(label if url is None else f'<a href="{html.escape(url, quote=True)}"{title}>{label}</a>')
                i = finish
                continue
            match = re.match(r'<(https?://[^<>\s]+)>', tail)
            if match:
                url = html.escape(match[1], quote=True)
                output.append(f'<a href="{url}">{url}</a>')
                i += match.end()
                continue
            match = re.match(r'<img\b[^>]*>', tail)
            if match:
                parser = ImageTag(self.rewrite)
                parser.feed(match[0])
                parser.close()
                if parser.output is None:
                    raise ConversionError("Invalid img tag")
                output.append(parser.output)
                i += match.end()
                continue
            match = re.match(r'<br\s*/?>', tail)
            if match:
                output.append("<br>")
                i += match.end()
                continue
            if re.match(r'</?[A-Za-z][^>]*>', tail) or tail.startswith("{%") or tail.startswith("{{"):
                raise ConversionError(f"Unsupported HTML/Liquid: {tail[:80]}")
            marker = next((m for m in ("**", "__", "*", "_") if tail.startswith(m)), None)
            if marker:
                # Underscores inside words/file names are ordinary text.
                if marker == "_" and i and text[i - 1].isalnum():
                    marker = None
                if marker:
                    end = text.find(marker, i + len(marker))
                    if end < 0:
                        raise ConversionError(f"Unclosed emphasis: {tail[:80]}")
                    tag = "strong" if len(marker) == 2 else "em"
                    output.append(f'<{tag}>' + self.inline(text[i + len(marker):end]) + f'</{tag}>')
                    i = end + len(marker)
                    continue
            if tail.startswith("~~") or re.match(r'!\[|\[\^', tail):
                raise ConversionError(f"Unsupported inline syntax: {tail[:80]}")
            output.append(html.escape(text[i]))
            i += 1
        return ''.join(output)

    @staticmethod
    def list_match(line):
        return re.match(r'^( *)([-+*]|\d+\.) +(.*)$', line)

    def render(self, source):
        lines = source.splitlines()
        output = []
        i = 0
        while i < len(lines):
            line = lines[i]
            if not line.strip():
                i += 1
                continue
            if re.match(r'^\s*\d+\)\s+', line):
                raise ConversionError(f"Unsupported ordered-list delimiter: {line[:80]}")
            if i + 1 < len(lines) and re.fullmatch(r'(?:=+|-+)\s*', lines[i + 1]) and line.strip() and not line.startswith(('#', '```')):
                raise ConversionError(f"Unsupported Setext heading: {line[:80]}")
            if line.startswith("<!--"):
                comment = [line]
                while "-->" not in comment[-1]:
                    i += 1
                    if i == len(lines):
                        raise ConversionError("Unclosed block comment")
                    comment.append(lines[i])
                output.append(self.inline('\n'.join(comment)))
                i += 1
                continue
            fence = re.fullmatch(r'```([\w-]*)\s*', line)
            if fence:
                code = []
                i += 1
                while i < len(lines) and lines[i] != "```":
                    code.append(lines[i])
                    i += 1
                if i == len(lines):
                    raise ConversionError("Unclosed code fence")
                attr = f' class="language-{fence[1]}"' if fence[1] else ''
                output.append(f'<pre><code{attr}>' + html.escape('\n'.join(code)) + '</code></pre>')
                i += 1
                continue
            heading = re.fullmatch(r'(#{1,6}) +(.*)', line)
            if heading:
                ident = heading_id(heading[2])
                count = self.used_ids.get(ident, 0)
                self.used_ids[ident] = count + 1
                actual = ident + (f'-{count}' if count else '')
                level = len(heading[1])
                cls = ' class="page-title"' if level == 1 else ''
                output.append(f'<h{level} id="{actual}"{cls}>' + self.inline(heading[2]) + f'</h{level}>')
                i += 1
                continue
            if re.fullmatch(r'(?:---+|\*\*\*+|___+)\s*', line):
                output.append('<hr>')
                i += 1
                continue
            if line.startswith('>'):
                quoted = []
                while i < len(lines) and lines[i].startswith('>'):
                    quoted.append(re.sub(r'^> ?', '', lines[i]))
                    i += 1
                output.append('<blockquote>' + self.render('\n'.join(quoted)) + '</blockquote>')
                continue
            if line.startswith('|'):
                def cells(row):
                    return [c.strip() for c in row.strip().strip('|').split('|')]
                heads = cells(line)
                if i + 1 == len(lines) or not all(re.fullmatch(r':?-{3,}:?', c) for c in cells(lines[i + 1])):
                    raise ConversionError("Unsupported table header")
                separators = cells(lines[i + 1])
                if len(heads) != len(separators):
                    raise ConversionError("Table column mismatch")
                aligns = ['center' if c.startswith(':') and c.endswith(':') else 'right' if c.endswith(':') else 'left' for c in separators]
                def row(values, tag):
                    if len(values) != len(heads):
                        raise ConversionError("Table column mismatch")
                    return '<tr>' + ''.join(f'<{tag} style="text-align:{align}">{self.inline(c)}</{tag}>' for c, align in zip(values, aligns)) + '</tr>'
                table = ['<div class="table-wrap"><table><thead>', row(heads, 'th'), '</thead><tbody>']
                i += 2
                while i < len(lines) and lines[i].startswith('|'):
                    table.append(row(cells(lines[i]), 'td'))
                    i += 1
                output.append('\n'.join(table) + '</tbody></table></div>')
                continue
            item = self.list_match(line)
            if item:
                indent = len(item[1])
                ordered = item[2][0].isdigit()
                tag = 'ol' if ordered else 'ul'
                start = f' start="{int(item[2][:-1])}"' if ordered and item[2] != '1.' else ''
                items = [f'<{tag}{start}>']
                while i < len(lines):
                    item = self.list_match(lines[i])
                    if not item or len(item[1]) != indent or item[2][0].isdigit() != ordered:
                        break
                    content_indent = item.start(3)
                    body = [item[3]]
                    i += 1
                    while i < len(lines):
                        nxt = self.list_match(lines[i])
                        if nxt and len(nxt[1]) <= indent:
                            break
                        if lines[i].strip() and len(lines[i]) - len(lines[i].lstrip(' ')) <= indent:
                            break
                        body.append(lines[i][min(content_indent, len(lines[i]) - len(lines[i].lstrip(' '))):])
                        i += 1
                    rendered = self.render('\n'.join(body))
                    items.append('<li>' + rendered + '</li>')
                output.append('\n'.join(items) + f'</{tag}>')
                continue
            if line.startswith(('    ', '\t', '~~~', '```', '#', '[^')) or re.match(r'^\[[^]]+\]:', line):
                raise ConversionError(f"Unsupported block: {line[:80]}")
            paragraph = []
            while i < len(lines) and lines[i].strip():
                if paragraph and (self.list_match(lines[i]) or lines[i].startswith(('#', '```', '>', '|', '<!--')) or re.fullmatch(r'---+', lines[i])):
                    break
                raw = lines[i]
                if re.match(r'^\s*\d+\)\s+', raw) or i + 1 < len(lines) and re.fullmatch(r'(?:=+|-+)\s*', lines[i + 1]):
                    raise ConversionError(f"Unsupported Markdown block in paragraph: {raw[:80]}")
                paragraph.append(self.inline(raw.rstrip()) + ('<br>' if raw.endswith('  ') else ''))
                i += 1
            output.append('<p>' + '\n'.join(paragraph) + '</p>')
        return '\n'.join(output)


def note_box(content):
    return '<div class="box info"><div class="box-body">' + content + '</div></div>'


def section_span(body, heading):
    matches = list(re.finditer(rf'<h([1-6]) id="{re.escape(heading)}"[^>]*>.*?</h\1>', body))
    if len(matches) != 1:
        raise ConversionError(f"Override heading must match once: {heading}")
    match = matches[0]
    following = re.search(rf'<h[1-{int(match[1])}]\b', body[match.end():])
    end = match.end() + following.start() if following else len(body)
    return match, end


def apply_overrides(body, rules, converter):
    """Apply exact, unique HTML anchors. Never silently skip a stale rule."""
    for rule in rules:
        prefix = suffix = ''
        if rule.get('scope_heading_id'):
            scope, end = section_span(body, rule['scope_heading_id'])
            prefix, body, suffix = body[:scope.start()], body[scope.start():end], body[end:]
        op = rule['operation']
        if op == 'replace_paragraph':
            anchor = '<p>' + converter.inline(rule['anchor']) + '</p>'
            replacement = '<p>' + converter.inline(rule['replacement']) + '</p>'
        elif op in {'remove_section', 'insert_note'}:
            match, end = section_span(body, rule['heading_id'])
            if op == 'insert_note':
                anchor = match[0]
                replacement = anchor + '\n' + note_box(converter.inline(rule['note']))
            else:
                anchor = body[match.start():end]
                replacement = match[0] + '\n' if rule.get('keep_heading') else ''
                replacement += note_box(converter.inline(rule['note'])) + '\n' if rule.get('note') else ''
        elif op == 'insert_before_heading':
            match, _ = section_span(body, rule['heading_id'])
            anchor = match[0]
            replacement = converter.render(rule['content']) + '\n' + anchor
        elif op == 'replace_html':
            anchor, replacement = rule['anchor'], rule['replacement']
        elif op == 'replace_inline':
            anchor = converter.inline(rule['anchor'])
            replacement = converter.inline(rule['replacement'])
        else:
            raise ConversionError(f"Unknown override operation: {op}")
        if body.count(anchor) != 1:
            raise ConversionError(f"Override anchor must match once ({body.count(anchor)}): {rule}")
        body = prefix + body.replace(anchor, replacement, 1) + suffix
    return body


def split_category(body, filename):
    """Use the category's explicit list, retaining globally assigned heading ids.

    Only sections before/including the list are introductory. Every subsequent
    h2 must be listed, even if the source order differs from the list order.
    Corrections run first, so removed subsections cannot acquire dangling links.
    """
    listing, end = section_span(body, 'プラグイン一覧')
    if listing[1] != '2':
        raise ConversionError(f'Plugin list must be h2: {filename}')
    content = body[listing.end():end].strip()
    if not content.startswith('<ul>') or not content.endswith('</ul>'):
        raise ConversionError(f'Unsupported plugin list: {filename}')
    entries = []
    items = re.findall(r'<li>(.*?)</li>', content, re.S)
    for item in items:
        match = re.fullmatch(r'<p><a href="#([^"<>]+)">([^<>]+)</a>(?:[^<>]*)</p>', item, re.S)
        if not match or not re.fullmatch(r'[\w-]+', match[1]):
            raise ConversionError(f'Unsupported plugin list entry: {filename}: {item}')
        entries.append((match[1], html.unescape(match[2])))
    expected = '<ul>\n' + '\n'.join('<li>' + item + '</li>' for item in items) + '</ul>'
    if not entries or re.sub(r'\s+', '', content) != re.sub(r'\s+', '', expected):
        raise ConversionError(f'Unknown content in plugin list: {filename}')
    anchors = [anchor for anchor, _ in entries]
    if len(anchors) != len(set(anchors)):
        raise ConversionError(f'Duplicate plugin list anchor: {filename}')
    sections = list(re.finditer(r'<h([12]) id="([^"]+)"[^>]*>(.*?)</h\1>', body[end:], re.S))
    if any(section[1] != '2' for section in sections) or {s[2] for s in sections} != set(anchors) or len(sections) != len(entries):
        raise ConversionError(f'Plugin list/sections mismatch: {filename}: listed={anchors}, sections={[s[2] for s in sections]}')
    by_anchor = {}
    for i, section in enumerate(sections):
        start = end + section.start()
        finish = end + sections[i + 1].start() if i + 1 < len(sections) else len(body)
        label = html.unescape(section[3])
        by_anchor[section[2]] = (label, body[start:finish].strip())
    plugins = []
    for anchor, label in entries:
        section_label, section_body = by_anchor[anchor]
        if label != section_label:
            raise ConversionError(f'Plugin list/heading label mismatch: {filename}#{anchor}')
        plugins.append({'anchor': anchor, 'label': label, 'filename': filename[:-5] + '-' + anchor + '.html', 'body': section_body})
    return body[:end].strip(), plugins


def split_link_map(bodies, categories):
    """Map original category fragments to their post-correction owner page."""
    destinations = {}
    for filename, (_, _, body) in bodies.items():
        for ident in re.findall(r'\bid="([^"]+)"', body):
            key = (filename, ident)
            if key in destinations:
                raise ConversionError(f'Duplicate generated id: {filename}#{ident}')
            destinations[key] = (filename, ident)
    for category, plugins in categories.items():
        for plugin in plugins:
            for ident in re.findall(r'\bid="([^"]+)"', plugin['body']):
                destinations[(category, ident)] = (plugin['filename'], '' if ident == plugin['anchor'] else ident)
    return destinations


def rewrite_split_links(body, origin, destinations):
    # Overrides see the original converted links. Remap only after validating
    # their anchors, and resolve in-page links against the source category.
    def replace(match):
        parts = urlsplit(html.unescape(match[1]))
        if parts.scheme or parts.path.startswith('../') or not parts.fragment:
            return match[0]
        key = (parts.path or origin, unquote(parts.fragment))
        if key not in destinations:
            raise ConversionError(f'Unknown split fragment: {origin} -> {match[1]}')
        filename, fragment = destinations[key]
        target = filename + ('?' + parts.query if parts.query else '') + ('#' + fragment if fragment else '')
        return 'href="' + html.escape(target, quote=True) + '"'
    return re.sub(r'href="([^"]+)"', replace, body)


def plugin_body(body):
    """Promote plugin h2 and its subsections while preserving fragment ids."""
    def promote(match):
        slash, level, attrs = match.groups()
        level = int(level) - 1
        cls = ' class="page-title"' if level == 1 and not slash else ''
        return f'<{slash}h{level}{attrs}{cls}>'
    return re.sub(r'<(/?)h([2-6])([^>]*)>', promote, body)


def chrome(title, description, body, filename, pages, version, commit, categories, category=None):
    esc = html.escape
    nav = ['<a href="../../effetune.html">音響調整 (EffeTune)</a>', '<a href="../../index.html">マニュアル目次</a>']
    for name, label in [('index.html', 'EffeTune 説明書の目次'), *pages, ('license.html', 'MIT License')]:
        active = ' class="active"' if name == filename else ''
        nav.append(f'<a href="{name}"{active}>{esc(label)}</a>')
        if name == category:
            for plugin in categories[name]:
                active = 'sub active' if plugin['filename'] == filename else 'sub'
                current = ' aria-current="page"' if plugin['filename'] == filename else ''
                nav.append(f'<a href="{plugin["filename"]}" class="{active}"{current}>{esc(plugin["label"])}</a>')
    breadcrumb = '<a href="index.html">EffeTune 説明書</a>'
    if category:
        breadcrumb += f'<span class="sep">›</span><a href="{category}">{esc(dict(pages)[category])}</a>'
    if filename not in {'index.html', category}:
        breadcrumb += f'<span class="sep">›</span><span aria-current="page">{esc(re.sub(r" - EffeTune$", "", title))}</span>'
    url = f'https://mikage.to/mimageviewer/manual/effetune/{version}/{filename}'
    attribution = f'この説明は EffeTune (作者: Frieve-A) の<a href="https://github.com/Frieve-A/effetune/tree/{commit}/docs/i18n/ja">公式ドキュメント</a> (<a href="license.html">MIT License</a>) を mImageViewer 同梱版 ({esc(version)}) に合わせて転載・一部修正したものです'
    return f'''<!DOCTYPE html>
<!-- Generated by scripts/gen-effetune-docs.py; edit the script/overrides, not this file. -->
<html lang="ja">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{esc(title)}｜mImageViewer</title>
  <meta name="description" content="{esc(description, quote=True)}">
  <link rel="canonical" href="{url}">
  <link rel="stylesheet" href="../../style.css">
  <style>.content img {{ max-width:100%; height:auto; }} .content pre {{ overflow-x:auto; }} .content pre.license {{ white-space:pre-wrap; overflow-wrap:anywhere; }} .content li p {{ margin-bottom:0; }} .content ul ul, .content ol ul {{ margin:0; }}</style>
</head>
<body>
<header class="site-header"><span class="logo">mImageViewer</span><nav class="breadcrumb"><a href="../../../index.html">ホーム</a><span class="sep">›</span><a href="../../index.html">マニュアル</a><span class="sep">›</span>{breadcrumb}</nav></header>
<div class="layout">
  <aside class="sidebar"><div class="sidebar-label">EffeTune</div><nav>
    {chr(10).join(nav)}
  </nav></aside>
  <main class="content">
    {note_box(attribution + '。')}
{body}
  </main>
</div>
<footer><p>原文: Frieve-A (MIT License) / mImageViewer 向け編集: Mikage Sawatari</p></footer>
</body>
</html>
'''


def generate(snapshot, version):
    selection = json.loads((RULES / 'selection.json').read_text(encoding='utf-8'))
    source_dir = snapshot / 'ja'
    known = {p.relative_to(source_dir).as_posix() for p in source_dir.rglob('*.md')}
    if known != set(selection):
        raise ConversionError(f"Selection inventory changed: {sorted(known ^ set(selection))}")
    published = {p for p, rule in selection.items() if rule['include']}
    stems = [Path(p).stem for p in published]
    if len(stems) != len(set(stems)) or {'index', 'license'} & set(stems):
        raise ConversionError("Output filename collision")
    provenance = (snapshot / 'SOURCE.md').read_text(encoding='utf-8')
    association = re.search(r'submodule of EffeTune Mixwright (v\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?)', provenance)
    if not association or association[1] != version:
        documented = association[1] if association else 'unspecified'
        raise ConversionError(f"Snapshot is associated with Mixwright {documented}, not {version}; supply/review matching SOURCE.md and snapshot")
    match = re.search(r'^- Commit: ([0-9a-f]{40})\b', provenance, re.M)
    if not match:
        raise ConversionError("SOURCE.md must identify the 40-character upstream commit")
    commit = match[1]
    images = {p.name for p in (snapshot / 'images').iterdir() if p.is_file()}
    pages, bodies, categories, contexts = [], {}, {}, {}
    overrides = {p.stem: json.loads(p.read_text(encoding='utf-8')) for p in sorted(RULES.glob('*.json')) if p.name != 'selection.json'}
    if set(overrides) - set(stems):
        raise ConversionError("Override for an unpublished page")
    for page in sorted(published):
        path = source_dir / page
        meta, source = front_matter(path.read_text(encoding='utf-8'))
        source = expand_liquid(source, path.parent, source_dir, (path.resolve(),))
        source = select_sections(source, selection[page])
        converter = Converter(page, published, known, images)
        body = converter.render(source)
        body = apply_overrides(body, overrides.get(path.stem, []), converter)
        title = meta.get('title') or re.search(r'^# (.+)$', source, re.M)[1]
        label = re.sub(r' - EffeTune$', '', title)
        filename = path.stem + '.html'
        pages.append((filename, label))
        bodies[filename] = (title, meta.get('description', label), body)
        if page.startswith('plugins/'):
            _, categories[filename] = split_category(body, filename)
            contexts[filename] = []
            for rule in overrides.get(path.stem, []):
                if rule.get('repeat_on_plugins'):
                    heading, _ = section_span(body, rule['heading_id'])
                    if rule['operation'] != 'insert_note' or heading[1] != '1' or rule.get('scope_heading_id'):
                        raise ConversionError('Only category h1 notes may repeat on plugin pages')
                    contexts[filename].append(note_box(converter.inline(rule['note'])))
    destinations = split_link_map(bodies, categories)
    owners = {}
    for category, plugins in categories.items():
        title, description, full_body = bodies[category]
        intro, _ = split_category(full_body, category)
        bodies[category] = (title, description, rewrite_split_links(intro, category, destinations))
        owners[category] = category
        for i, plugin in enumerate(plugins):
            name = plugin['filename']
            if name in bodies or name in {'index.html', 'license.html'}:
                raise ConversionError(f'Split filename collision: {name}')
            links = []
            if i:
                previous = plugins[i - 1]
                links.append(f'<a rel="prev" href="{previous["filename"]}">前へ: {html.escape(previous["label"])}</a>')
            if i + 1 < len(plugins):
                following = plugins[i + 1]
                links.append(f'<a rel="next" href="{following["filename"]}">次へ: {html.escape(following["label"])}</a>')
            navigation = '<nav aria-label="カテゴリ内の前後のエフェクト">' + ' / '.join(links) + '</nav>' if links else ''
            # Include a category introduction link so shared instructions (e.g.
            # spectrum overlay) remain discoverable from direct plugin visits.
            context = '\n'.join(contexts[category]) + f'\n<p><a href="{category}">カテゴリの説明とプラグイン一覧</a></p>'
            body = plugin_body(plugin['body'])
            heading_end = body.index('</h1>') + len('</h1>')
            body = rewrite_split_links(body[:heading_end] + '\n' + context + body[heading_end:], category, destinations)
            bodies[name] = (plugin['label'] + ' - EffeTune', plugin['label'] + ' の設定と使い方。', body + '\n' + navigation)
            owners[name] = category
    for name in set(bodies) - set(owners):
        title, description, body = bodies[name]
        bodies[name] = (title, description, rewrite_split_links(body, name, destinations))
    listing = []
    for name, label in pages:
        plugins = categories.get(name, [])
        sublist = '<ul>' + ''.join(f'<li><a href="{p["filename"]}">{html.escape(p["label"])}</a></li>' for p in plugins) + '</ul>' if plugins else ''
        listing.append(f'<li><a href="{name}">{html.escape(label)}</a>{sublist}</li>')
    listing = '\n'.join(listing)
    bodies['index.html'] = ('EffeTune 説明書', 'mImageViewer 同梱 EffeTune のエフェクトと操作の説明。', '<h1 class="page-title">EffeTune 説明書</h1>\n<p>mImageViewer での開き方、設定の保存先、リモート配信の注意点は<a href="../../effetune.html">音響調整 (EffeTune)</a>をご覧ください。エフェクトごとの設定と画面内の操作を以下にまとめています。</p>\n<ul>' + listing + '</ul>')
    license_text = (snapshot / 'LICENSE').read_text(encoding='utf-8')
    bodies['license.html'] = ('EffeTune — MIT License', '転載元 EffeTune の MIT License 全文。', '<h1 class="page-title">MIT License</h1>\n<pre class="license">' + html.escape(license_text) + '</pre>')
    result = {name: chrome(title, description, body, name, pages, version, commit, categories, owners.get(name)).encode('utf-8') for name, (title, description, body) in bodies.items()}
    # Check all generated Markdown links after corrections (including fragments).
    ids = {name: set(re.findall(r'\bid="([^"]+)"', content.decode())) for name, content in result.items()}
    for name, content in result.items():
        for target in re.findall(r'href="([^"]+)"', content.decode()):
            parts = urlsplit(html.unescape(target))
            if parts.scheme or parts.path.startswith('../'):
                continue
            destination = parts.path or name
            if destination not in result or parts.fragment and unquote(parts.fragment) not in ids[destination]:
                raise ConversionError(f"Broken generated link: {name} -> {target}")
    for name in sorted(images):
        result['images/' + name] = (snapshot / 'images' / name).read_bytes()
    return result


def entry_page(version):
    target = version + '/index.html'
    return f'''<!DOCTYPE html>
<!-- Generated by scripts/gen-effetune-docs.py; edit the script, not this file. -->
<html lang="ja">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="robots" content="noindex">
  <meta http-equiv="refresh" content="0; url={target}">
  <link rel="canonical" href="https://mikage.to/mimageviewer/manual/effetune/{target}">
  <title>EffeTune 説明書｜mImageViewer</title>
</head>
<body><p><a href="{target}">EffeTune 説明書を開く</a></p></body>
</html>
'''.encode('utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('snapshot', nargs='?', type=Path, default=DEFAULT_SOURCE)
    parser.add_argument('--version', help='Bundled version, only if vendor VERSION is absent')
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    version_file = ROOT / 'vendor/effetune-mixwright/VERSION'
    version = version_file.read_text(encoding='utf-8').strip() if version_file.is_file() else args.version
    if not version or not re.fullmatch(r'v\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?', version):
        parser.error('A valid bundled version is required (vendor VERSION or --version)')
    try:
        output = generate(args.snapshot.resolve(), version)
    except (ConversionError, OSError, KeyError, json.JSONDecodeError) as exc:
        print(f'ERROR: {exc}', file=sys.stderr)
        return 1
    destination = MANUAL / 'effetune' / version
    existing = {p.relative_to(destination).as_posix() for p in destination.rglob('*') if p.is_file()}
    stale = sorted(name for name, data in output.items() if not (destination / name).is_file() or (destination / name).read_bytes() != data)
    extra = sorted(existing - set(output))
    entry = MANUAL / 'effetune/index.html'
    entry_data = entry_page(version)
    entry_stale = not entry.is_file() or entry.read_bytes() != entry_data
    if args.check:
        if stale or extra or entry_stale:
            print(f'Stale EffeTune output: changed/missing={stale}, unexpected={extra}, entry_stale={entry_stale}', file=sys.stderr)
            return 1
        print(f'EffeTune output is current: {len(output)} version files + entry ({version})')
        return 0
    # Remove only known generated page/image files; refuse arbitrary extra files.
    if extra:
        print(f'ERROR: unexpected output files; review/remove explicitly: {extra}', file=sys.stderr)
        return 1
    for name, data in sorted(output.items()):
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if name in stale:
            path.write_bytes(data)
    if entry_stale:
        entry.write_bytes(entry_data)
    print(f'Generated {len(output)} version files + entry ({version}); updated {len(stale) + int(entry_stale)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
