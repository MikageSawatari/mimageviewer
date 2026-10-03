"""Offline subset, correction anchors, links and reproducible-output tests."""
import contextlib
import importlib.util
import io
from pathlib import Path
import shutil
import unittest
import uuid
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('effetune_docs', Path(__file__).with_name('gen-effetune-docs.py'))
docs = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(docs)


@contextlib.contextmanager
def workspace_temporary_directory():
    # Keep fixtures inside the checkout in restricted Windows agent sessions.
    parent = docs.ROOT / 'target/effetune-docs-tests'
    parent.mkdir(parents=True, exist_ok=True)
    # tempfile's 0o700 mkdir creates an inaccessible Windows sandbox ACL.
    temporary = parent / str(uuid.uuid4())
    temporary.mkdir()
    try:
        yield str(temporary)
    finally:
        if not temporary.resolve().is_relative_to(parent.resolve()) or temporary.is_symlink() or temporary.is_junction():
            raise RuntimeError('Test directory escaped the workspace')
        shutil.rmtree(temporary)


class ConverterTests(unittest.TestCase):
    def converter(self, page='plugins/eq.md'):
        return docs.Converter(page, {'plugins/eq.md', 'plugins/delay.md'}, {'plugins/eq.md', 'plugins/delay.md', 'README.md'}, {'bus_function.png'})

    def test_front_matter_and_japanese_only(self):
        metadata, body = docs.front_matter('---\ntitle: "音響"\ndescription: "説明"\nlang: ja\n---\n# 本文')
        self.assertEqual(metadata, {'title': '音響', 'description': '説明', 'lang': 'ja'})
        self.assertEqual(body, '# 本文')
        for text in ['---\nlayout: default\n---', '---\nlang: en\n---', '---\nlang: ja\nlang: ja\n---', '---\ntitle: "未終了"']:
            with self.subTest(text=text), self.assertRaises(docs.ConversionError):
                docs.front_matter(text)
        self.assertEqual(docs.front_matter('---\n---\n本文'), ({}, '本文'))

    def test_section_selection_preserves_children_and_rejects_stale_rules(self):
        source = '# Upstream\n\n<div>Web only</div>\n\n## Setup\n\nInstall\n\n## Use\n\n### Chain\n\nDrag\n\n#### Detail\n\nFine control\n\n### Presets\n\nSave\n\n## Browser\n\nWeb only'
        rule = {'title': 'Basic operations', 'sections': ['Chain', 'Presets']}
        selected = docs.select_sections(source, rule)
        self.assertEqual(selected, '# Basic operations\n\n## Chain\n\nDrag\n\n### Detail\n\nFine control\n\n## Presets\n\nSave')
        self.assertEqual(docs.select_sections(source, {}), source)
        for sections in [['Missing'], ['Chain', 'Chain'], ['Use', 'Chain'], []]:
            with self.subTest(sections=sections), self.assertRaises(docs.ConversionError):
                docs.select_sections(source, {'title': 'Basic operations', 'sections': sections})
        with self.assertRaises(docs.ConversionError):
            docs.select_sections(source + '\n\n### Chain\nDuplicate', rule)
        with self.assertRaises(docs.ConversionError):
            docs.select_sections(source, {'sections': ['Chain']})

    def test_inline_escaping_formatting_and_nested_image_link(self):
        value = self.converter().inline('**強調** *斜体* `x < y & z` [![図](../../../../images/bus_function.png)](https://example.com/?a=1&b=2)')
        self.assertIn('<strong>強調</strong> <em>斜体</em>', value)
        self.assertIn('<code>x &lt; y &amp; z</code>', value)
        self.assertIn('<a href="https://example.com/?a=1&amp;b=2"><img src="images/bus_function.png" alt="図"></a>', value)

    def test_link_rewriting_and_unpublished_plain_text(self):
        converter = self.converter('README.md')
        self.assertEqual(converter.inline('[EQ](plugins/eq.md#5band-peq)'), '<a href="eq.html#5band-peq">EQ</a>')
        self.assertEqual(converter.inline('[**概要**](README.md#setup)'), '<strong>概要</strong>')
        self.assertEqual(converter.rewrite('/dsp/?x=1#api'), 'https://effetune.frieve.com/dsp/?x=1#api')
        self.assertEqual(self.converter().rewrite('delay.md#time-alignment'), 'delay.html#time-alignment')
        self.assertEqual(self.converter().rewrite('../plugins/eq.md'), 'eq.html')
        self.assertEqual(self.converter().rewrite('#日本語'), '#日本語')
        for url in ['missing.md', '../../README.md', 'javascript:alert(1)', '//example.com/x']:
            with self.subTest(url=url), self.assertRaises(docs.ConversionError):
                converter.rewrite(url)

    def test_inline_html_comments_image_and_break(self):
        text = '<img src="../../../images/bus_function.png" alt="音 &amp; 図" width="30" height="30" align="bottom"> <!-- retained --> <br />'
        actual = self.converter().inline(text)
        self.assertIn('src="images/bus_function.png"', actual)
        self.assertIn('alt="音 &amp; 図"', actual)
        self.assertIn('align="bottom"', actual)
        self.assertIn('<!-- retained --> <br>', actual)
        for value in ['<iframe src="https://example.com"></iframe>', '<img src="../../../images/bus_function.png" onerror="x">', '<img src="../../../images/missing.png">', '{% include missing.html %}', '~~removed~~', '`unclosed', '**unclosed', '[x](missing.md)']:
            with self.subTest(value=value), self.assertRaises(docs.ConversionError):
                self.converter().inline(value)

    def test_nested_lists_continuation_fence_and_numbering(self):
        source = '# 音 & 響\n\n3. 手順\n   - 子 **項目**\n     - 孫\n   - コード\n     ```text\n     Filter 1: Gain < 3\n     ```\n\n4. 続き\n\n段落  \n改行'
        body = self.converter().render(source)
        self.assertIn('<h1 id="音--響" class="page-title">音 &amp; 響</h1>', body)
        self.assertIn('<ol start="3">', body)
        self.assertEqual(body.count('<ul>'), 2)
        self.assertIn('<pre><code class="language-text">Filter 1: Gain &lt; 3</code></pre>', body)
        self.assertIn('<p>段落<br>\n改行</p>', body)
        self.assertEqual(body.count('<li>'), body.count('</li>'))

    def test_tables_quotes_headings_and_duplicate_ids(self):
        body = self.converter().render('## A\n\n> **注記**\n\n| 項目 | 値 |\n|---|---:|\n| & | `x` |\n\n## A\n\n---')
        self.assertIn('<blockquote><p><strong>注記</strong></p></blockquote>', body)
        self.assertIn('<td style="text-align:right"><code>x</code></td>', body)
        self.assertIn('id="a-1"', body)
        self.assertTrue(body.endswith('<hr>'))
        for source in ['| x |\n| -- |', '| x | y |\n| --- | --- |\n| one |', '```\nunclosed', '~~~\nunknown', '[label]: https://example.com', 'Heading\n===', 'Heading\n---', '1) Step', 'Paragraph\n1) Step']:
            with self.subTest(source=source), self.assertRaises(docs.ConversionError):
                self.converter().render(source)

    def test_liquid_relative_include_cycle_and_unknown(self):
        with workspace_temporary_directory() as temporary:
            folder = Path(temporary)
            (folder / 'README.md').write_text('---\ntitle: "概要"\nlang: ja\n---\n# 概要', encoding='utf-8')
            expanded = docs.expand_liquid('{% include_relative README.md %}', folder, folder)
            self.assertEqual(expanded, '# 概要')
            for value in ['{% include unavailable.html %}', '{% unknown %}', '{{ site.url }}', '{% include_relative ../README.md %}']:
                with self.subTest(value=value), self.assertRaises(docs.ConversionError):
                    docs.expand_liquid(value, folder, folder)
            (folder / 'README.md').write_text('{% include_relative README.md %}', encoding='utf-8')
            with self.assertRaises(docs.ConversionError):
                docs.expand_liquid('{% include_relative README.md %}', folder, folder)

    def test_overrides_exact_anchor_missing_or_duplicate_failure(self):
        converter = self.converter()
        original = converter.render('# 題\n\n## A\n\n旧 **説明**\n\n### 子\n\n子の本文\n\n## B\n\n次')
        rules = [
            {'operation': 'replace_paragraph', 'anchor': '旧 **説明**', 'replacement': '新 `説明`'},
            {'operation': 'remove_section', 'heading_id': 'a', 'keep_heading': True, 'note': '対象外'},
            {'operation': 'insert_note', 'heading_id': 'b', 'note': '追加'},
        ]
        fixed = docs.apply_overrides(original, rules, converter)
        self.assertIn('<h2 id="a">A</h2>\n' + docs.note_box('対象外'), fixed)
        self.assertNotIn('子の本文', fixed)
        self.assertIn('<h2 id="b">B</h2>\n' + docs.note_box('追加'), fixed)
        for body, rule in [(original, {'operation': 'replace_paragraph', 'anchor': '不明', 'replacement': 'x'}), (original + original, rules[0]), (original, {'operation': 'remove_section', 'heading_id': 'missing'}), (original + original, rules[1])]:
            with self.subTest(rule=rule), self.assertRaises(docs.ConversionError):
                docs.apply_overrides(body, [rule], converter)

    def test_override_scope_disambiguates_identical_clauses(self):
        converter = self.converter()
        body = converter.render('## A\n\n古い説明\n\n## B\n\n古い説明')
        rule = {'operation': 'replace_inline', 'scope_heading_id': 'b', 'anchor': '古い説明', 'replacement': '新しい説明'}
        fixed = docs.apply_overrides(body, [rule], converter)
        self.assertIn('<h2 id="a">A</h2>\n<p>古い説明</p>', fixed)
        self.assertIn('<h2 id="b">B</h2>\n<p>新しい説明</p>', fixed)
        with self.assertRaises(docs.ConversionError):
            docs.apply_overrides(fixed, [rule], converter)
        rule['scope_heading_id'] = 'missing'
        with self.assertRaises(docs.ConversionError):
            docs.apply_overrides(body, [rule], converter)


class GenerationTests(unittest.TestCase):
    def test_snapshot_cannot_be_silently_relabelled_for_new_bundle(self):
        with self.assertRaisesRegex(docs.ConversionError, 'associated with Mixwright v0.11.1, not v0.12.0'):
            docs.generate(docs.DEFAULT_SOURCE, 'v0.12.0')

    def test_snapshot_matches_output_and_preserves_license_images(self):
        output = docs.generate(docs.DEFAULT_SOURCE, 'v0.11.1')
        self.assertEqual(len(output), 131)  # 19 category/guide/index/license + 108 plugin pages + 4 images
        for name, data in output.items():
            with self.subTest(name=name):
                self.assertEqual((docs.MANUAL / 'effetune/v0.11.1' / name).read_bytes(), data)
                if name.endswith('.html'):
                    text = data.decode()
                    self.assertNotIn('sidebar-section', text)
                    self.assertIn('href="../../style.css"', text)
                    self.assertIn('href="license.html"', text)
                    self.assertIn('f3189f3d9c6a4d692c5709107131f3e1c50a710c', text)
                    self.assertNotIn('{%', text)
        license_text = (docs.DEFAULT_SOURCE / 'LICENSE').read_text(encoding='utf-8')
        self.assertIn(docs.html.escape(license_text), output['license.html'].decode())
        for path in (docs.DEFAULT_SOURCE / 'images').iterdir():
            self.assertEqual(output['images/' + path.name], path.read_bytes())

    def test_snapshot_split_navigation_index_and_override_survival(self):
        output = {name: data.decode() for name, data in docs.generate(docs.DEFAULT_SOURCE, 'v0.11.1').items() if name.endswith('.html')}
        eq = output['eq.html'].split('<main class="content">')[1]
        self.assertIn('id="スペクトラムオーバーレイ"', eq)
        self.assertIn('href="eq-15band-geq.html"', eq)
        self.assertNotIn('id="15band-geq"', eq)
        self.assertNotIn('id="room-eq"', eq)
        geq = output['eq-15band-geq.html']
        self.assertIn('<h1 id="15band-geq" class="page-title">15Band GEQ</h1>', geq)
        self.assertIn('href="eq-15band-geq.html" class="sub active" aria-current="page"', geq)
        self.assertIn('href="eq-15band-peq.html" class="sub"', geq)
        self.assertNotIn('href="lofi-bit-crusher.html" class="sub"', geq)
        self.assertIn('rel="next" href="eq-15band-peq.html"', geq)
        self.assertNotIn('rel="prev"', geq)
        self.assertIn('rel="prev" href="eq-tilt-eq.html"', output['eq-tone-control.html'])
        self.assertNotIn('rel="next"', output['eq-tone-control.html'])
        self.assertIn('<a href="index.html">EffeTune 説明書</a><span class="sep">›</span><a href="eq.html">イコライザープラグイン</a><span class="sep">›</span><span aria-current="page">15Band GEQ</span>', geq)
        self.assertIn('href="eq-15band-geq.html"', output['index.html'])
        self.assertIn('href="control-section.html"', output['control.html'])
        self.assertNotIn('rel="next"', output['control-section.html'])
        self.assertNotIn('rel="prev"', output['control-section.html'])
        self.assertIn('mImageViewer の EffeTune 画面には、マイクを使う周波数応答の測定機能はありません', output['eq-room-eq.html'])
        self.assertIn('4チャンネル以上の出力が必要', output['basics-channel-divider.html'])
        self.assertNotIn('id="使用方法', output['basics-channel-divider.html'])
        self.assertIn('mImageViewer の音響調整はステレオ', output['basics-volume.html'])
        self.assertIn('mImageViewer の音響調整は左右2チャンネル', output['spatial-ms-matrix.html'])
        self.assertIn('mImageViewer のデータフォルダ', output['reverb-ir-reverb.html'])
        self.assertIn('bus-function.html', output)
        self.assertIn('visualizer.html', output)

    def test_readme_basic_operations_keep_vst_features_and_exclude_other_platforms(self):
        output = {name: data.decode() for name, data in docs.generate(docs.DEFAULT_SOURCE, 'v0.11.1').items() if name.endswith('.html')}
        body = output['README.html'].split('<main class="content">')[1].split('</main>')[0]
        for heading in ['Visualizerで音を表示する', 'エフェクトチェーンの作成', 'プリセットの使用', '保存データのバックアップと復元', 'セクション機能の使用方法', 'ABパイプライン機能の使用', 'エフェクト選択とキーボードショートカット', 'よく使われるエフェクトの組み合わせ']:
            self.assertIn(heading, body)
        for retained in ['上から下へ順番に処理', 'ON/OFF状態やルーティングは変わりません', 'A → B', 'B → A', 'Ctrl + Z', 'Shift+', '256 MB', '既定のブラウザ', 'EffeTune 固有のキー操作', 'href="bus-function.html"', 'href="control.html"', 'href="visualizer.html"']:
            self.assertIn(retained, body)
        for excluded in ['Webアプリを開く', 'PWA', 'セットアップガイド', 'Music Library', 'モバイル', 'macOS', 'ブラインドテスト', 'プレイヤー使用時', 'MIDI、ゲームパッド', 'オーディオファイルの処理', '周波数特性測定と補正', '推奨サンプルレート']:
            self.assertNotIn(excluded, body)
        for tag in ['ul', 'ol', 'li']:
            self.assertEqual(body.count('<' + tag + '>') + body.count('<' + tag + ' '), body.count('</' + tag + '>'))
        self.assertNotIn('<ul>\n</ul>', body)
        self.assertNotIn('???', body)
        self.assertIn('ファイルの上限は256 MBです', body)
        self.assertIn('共有前に選択内容を確認してください', body)
        self.assertNotIn('デバイス設定', body)
        self.assertNotIn('URLルール', body)
        self.assertIn('href="README.html">基本操作', output['index.html'])
        self.assertIn('href="README.html"', output['eq-15band-geq.html'])

    def test_entry_redirect(self):
        text = docs.entry_page('v0.11.1').decode()
        self.assertIn('<meta name="robots" content="noindex">', text)
        self.assertIn('<meta http-equiv="refresh" content="0; url=v0.11.1/index.html">', text)
        self.assertIn('<link rel="canonical" href="https://mikage.to/mimageviewer/manual/effetune/v0.11.1/index.html">', text)
        self.assertIn('<a href="v0.11.1/index.html">', text)
        self.assertNotEqual(docs.entry_page('v0.11.1'), docs.entry_page('v0.12.0'))
        self.assertEqual((docs.MANUAL / 'effetune/index.html').read_bytes(), docs.entry_page('v0.11.1'))

    def test_check_exit_status_and_version_fallback_without_writes(self):
        with workspace_temporary_directory() as temporary:
            root = Path(temporary)
            with patch.object(docs, 'ROOT', root), patch.object(docs, 'MANUAL', root / 'manual'), patch.object(docs, 'generate', return_value={'index.html': b'current'}), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                def run(*args):
                    with patch('sys.argv', ['gen-effetune-docs.py', *args]):
                        return docs.main()
                self.assertEqual(run('--version', 'v0.11.1', '--check'), 1)
                self.assertFalse((root / 'manual').exists())
                self.assertEqual(run('--version', 'v0.11.1'), 0)
                output = root / 'manual/effetune/v0.11.1/index.html'
                entry = root / 'manual/effetune/index.html'
                self.assertEqual(run('--version', 'v0.11.1', '--check'), 0)
                self.assertEqual(entry.read_bytes(), docs.entry_page('v0.11.1'))
                entry.unlink()
                self.assertEqual(run('--version', 'v0.11.1', '--check'), 1)
                self.assertFalse(entry.exists())
                self.assertEqual(run('--version', 'v0.11.1'), 0)
                entry.write_bytes(b'stale entry')
                self.assertEqual(run('--version', 'v0.11.1', '--check'), 1)
                self.assertEqual(entry.read_bytes(), b'stale entry')
                self.assertEqual(run('--version', 'v0.11.1'), 0)
                output.write_bytes(b'stale')
                self.assertEqual(run('--version', 'v0.11.1', '--check'), 1)
                self.assertEqual(output.read_bytes(), b'stale')
                output.write_bytes(b'current')
                output.with_name('obsolete.html').write_bytes(b'extra')
                self.assertEqual(run('--version', 'v0.11.1', '--check'), 1)
                self.assertEqual(run('--version', 'v0.11.1'), 1)
                vendor = root / 'vendor/effetune-mixwright'
                vendor.mkdir(parents=True)
                (vendor / 'VERSION').write_text('v0.12.0\n', encoding='utf-8')
                self.assertEqual(run('--version', 'v0.11.1'), 0)
                self.assertTrue((root / 'manual/effetune/v0.12.0/index.html').exists())
                self.assertEqual(entry.read_bytes(), docs.entry_page('v0.12.0'))
                self.assertEqual(output.read_bytes(), b'current')  # Older version remains intact.


class SplitTests(unittest.TestCase):
    SOURCE = '''# EQ

## 共通操作

intro

## プラグイン一覧

- [A](#a) - first
- [B](#b) - second

## B

### パラメータ

[A](#a) [Own](#パラメータ) [Intro](#共通操作)

## A

### パラメータ

[B](eq.md#b) [Subsection](eq.md#パラメータ) [Own](#パラメータ-1)
'''

    def fixture(self):
        converter = docs.Converter('plugins/eq.md', {'plugins/eq.md'}, {'plugins/eq.md'})
        return converter, converter.render(self.SOURCE)

    def test_explicit_list_intro_order_and_globally_stable_ids(self):
        _, body = self.fixture()
        intro, plugins = docs.split_category(body, 'eq.html')
        self.assertIn('id="共通操作"', intro)
        self.assertNotIn('id="a"', intro)
        self.assertEqual([p['filename'] for p in plugins], ['eq-a.html', 'eq-b.html'])
        self.assertIn('id="パラメータ-1"', plugins[0]['body'])
        self.assertIn('id="パラメータ"', plugins[1]['body'])
        self.assertIn('<h2 id="パラメータ-1">', docs.plugin_body(plugins[0]['body']))

    def test_missing_unlisted_duplicate_malformed_sections_fail(self):
        _, body = self.fixture()
        variants = [
            body.replace('id="a"', 'id="missing"'),
            body + '\n<h2 id="unlisted">Unlisted</h2>',
            body.replace('<h2 id="b">B</h2>', '<h2 id="unlisted">Unknown</h2><h2 id="b">B</h2>'),
            body.replace('href="#b">B', 'href="#a">B'),
            body.replace('href="#b">B', 'href="eq.md#b">B'),
            body.replace('id="プラグイン一覧"', 'id="not-a-list"'),
            body.replace('<h2 id="a">A</h2>', '<h1 id="a">A</h1>'),
            body.replace('<h2 id="a">A</h2>', '<h2 id="a">Wrong</h2>'),
            body.replace('<li><p><a href="#a">', '<li><p>Not a link</p></li><li><p><a href="#a">'),
        ]
        for body in variants:
            with self.subTest(body=body), self.assertRaises(docs.ConversionError):
                docs.split_category(body, 'eq.html')

    def test_cross_page_in_page_subsection_and_intro_mapping(self):
        converter, body = self.fixture()
        intro, plugins = docs.split_category(body, 'eq.html')
        mapping = docs.split_link_map({'eq.html': ('EQ', '', body)}, {'eq.html': plugins})
        self.assertEqual(docs.rewrite_split_links(converter.inline('[A](../plugins/eq.md#a)'), 'eq.html', mapping), '<a href="eq-a.html">A</a>')
        self.assertEqual(docs.rewrite_split_links('<a href="eq.html#a">A</a>', 'bus-function.html', mapping), '<a href="eq-a.html">A</a>')
        self.assertEqual(docs.rewrite_split_links(converter.inline('[Sub](eq.md?x=1#パラメータ-1)'), 'eq.html', mapping), '<a href="eq-a.html?x=1#パラメータ-1">Sub</a>')
        rewritten = docs.rewrite_split_links(plugins[1]['body'], 'eq.html', mapping)
        self.assertIn('href="eq-a.html"', rewritten)
        self.assertIn('href="eq-b.html#パラメータ"', rewritten)
        self.assertIn('href="eq.html#共通操作"', rewritten)
        for origin, target in [('eq.html', '#missing'), ('eq.html', 'unknown.html#a')]:
            with self.subTest(target=target), self.assertRaises(docs.ConversionError):
                docs.rewrite_split_links(f'<a href="{target}">x</a>', origin, mapping)
        self.assertEqual(docs.rewrite_split_links('<a href="https://example.com/#a">x</a>', 'eq.html', mapping), '<a href="https://example.com/#a">x</a>')
        with self.assertRaises(docs.ConversionError):
            docs.split_link_map({'eq.html': ('EQ', '', body + body)}, {'eq.html': plugins})

    def test_override_survives_split_and_stale_anchor_fails(self):
        converter, body = self.fixture()
        rules = [{'operation': 'replace_inline', 'scope_heading_id': 'a', 'anchor': '[Own](#パラメータ-1)', 'replacement': 'Corrected'}]
        fixed = docs.apply_overrides(body, rules, converter)
        _, plugins = docs.split_category(fixed, 'eq.html')
        self.assertIn('Corrected', plugins[0]['body'])
        self.assertNotIn('Corrected', plugins[1]['body'])
        with self.assertRaises(docs.ConversionError):
            docs.apply_overrides(body.replace('id="a"', 'id="renamed"'), rules, converter)
        with self.assertRaises(docs.ConversionError):
            docs.apply_overrides(fixed, rules, converter)

    def test_control_list_correction_is_explicit_and_anchored(self):
        converter = docs.Converter('plugins/control.md', {'plugins/control.md'}, {'plugins/control.md'})
        rule = {'operation': 'insert_before_heading', 'heading_id': 'section', 'content': '## プラグイン一覧\n\n- [Section](#section)'}
        body = converter.render('# 制御\n\n## Section\n\n内容')
        corrected = docs.apply_overrides(body, [rule], converter)
        _, plugins = docs.split_category(corrected, 'control.html')
        self.assertEqual([p['filename'] for p in plugins], ['control-section.html'])
        with self.assertRaises(docs.ConversionError):
            docs.apply_overrides(body.replace('id="section"', 'id="renamed"'), [rule], converter)
        with self.assertRaises(docs.ConversionError):
            docs.split_category(corrected + '<h2 id="unknown">Unknown</h2>', 'control.html')


if __name__ == '__main__':
    unittest.main()
