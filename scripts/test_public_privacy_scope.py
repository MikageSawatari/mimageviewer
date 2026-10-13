"""Guard final privacy wording and reject obsolete blanket claims."""
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parent.parent
PUBLIC = ROOT / 'htdocs/mimageviewer'
UPDATE_JP = ('音響調整を開いている間、画面表示に使う Windows の機能 (WebView2) が、'
             'Microsoft に自身の更新を確認することがあります。曲名や画像は送信しません。')
UPDATE_EN = ('while audio adjustment is open, the Windows feature used to display it (WebView2) '
             'may contact Microsoft to check for its own updates. It does not send song titles or artwork.')


def normalize(text):
    return re.sub(r'\s+', ' ', text).strip()


class Blocks(HTMLParser):
    def __init__(self):
        super().__init__()
        self.active = []
        self.blocks = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == 'meta' and 'content' in attrs:
            self.blocks.append(normalize(attrs['content']))
        if tag in ('p', 'span', 'h2', 'h3', 'li', 'div'):
            self.active.append((tag, []))

    def handle_data(self, data):
        for _, parts in self.active:
            parts.append(data)

    def handle_endtag(self, tag):
        if tag == 'br':
            self.handle_data(' ')
        for index in range(len(self.active) - 1, -1, -1):
            if self.active[index][0] == tag:
                _, parts = self.active.pop(index)
                self.blocks.append(normalize(''.join(parts)))
                break


def blocks(source):
    parser = Blocks()
    parser.feed(source)
    return parser.blocks


def json_strings(value):
    """Inspect every JSON-LD value, including descriptions and nested graphs."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for child in value.values():
            yield from json_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from json_strings(child)


def text_assurances(text):
    result = []
    text = normalize(text)
    # The approved final wording allows simple advertising/analytics statements
    # and the safety heading. Preserve rejection of the actual legacy guarantees
    # and exclusive network claims, not the superseded blanket scope requirement.
    for match in re.finditer(r'(?:広告|アクセス解析|利用状況)[^。]*?(?:一切含みません|含まず)|no analytics tools and no advertising|この\s*4\s*つ以外に通信しません|通信は増えません|本アプリが外部と通信するのは、次の場合に限られます|connects to the internet only|EPUB conversion is not another network case', text, re.I):
        result.append(match.group())
    for match in re.finditer(r'確認済み|今回の実機確認|実機確認が残|公開前|確認では|この確認では|verified features|has been verified|observed session|hardware session|still pending|finalized before release|were not used in that session', text, re.I):
        result.append(match.group())
    # A scope elsewhere in a paragraph/description cannot qualify a blanket
    # AI sentence: the claim itself must identify the main app's image AI.
    for sentence in re.split(r'[。!?！？]', text):
        ai_claim = re.search(r'AI\s*処理[^。]*?(?:すべて|全て|完結|PC\s*(?:内|の中))', sentence)
        if ai_claim and not ('本体' in sentence and '画像' in sentence):
            result.append(sentence.strip())
    return result


def unscoped_assurances(source):
    result = []
    for text in blocks(source):
        result.extend(text_assurances(text))
    # JSON-LD is not rendered text and must be checked independently.
    for script in re.findall(r'<script type="application/ld\+json">(.*?)</script>', source, re.S):
        for text in json_strings(json.loads(script)):
            result.extend(text_assurances(text))
    return result


def faq_answer(source):
    body = source.split('<h3 id="before-free">', 1)[1].split('</h3>', 1)[1].split('<h3', 1)[0]
    visible = normalize(' '.join(blocks(body)))
    script = re.search(r'<script type="application/ld\+json">(.*?)</script>', source, re.S)
    document = json.loads(script[1])
    answer = next(item['acceptedAnswer']['text'] for item in document['mainEntity'] if item['name'].startswith('無料ですか？'))
    return visible, normalize(answer)


class PublicPrivacyScopeTests(unittest.TestCase):
    def test_all_pages_scope_assurances_including_metadata(self):
        for name in ('index.html', 'privacy.html', 'manual/faq.html'):
            with self.subTest(page=name):
                self.assertEqual(unscoped_assurances((PUBLIC / name).read_text(encoding='utf-8')), [])

    def test_reviewed_unqualified_claims_are_detected(self):
        examples = [
            '<p>広告、アクセス解析ツール、利用状況の送信を<strong>一切含みません</strong>。</p>',
            '<p>本アプリは、アクセス解析ツール・広告を<strong>一切含みません</strong>。</p>',
            '<p>The App contains <strong>no analytics tools and no advertising</strong>.</p>',
            '<p>EPUB の変換も通信は増えません。</p>',
            '<p>この4つ以外に通信しません。</p>',
            '<p>EPUB conversion is not another network case.</p>',
            '<p>本アプリが外部と通信するのは、次の場合に限られます。</p>',
            '<p>AI 処理はすべて PC 内で完結します。</p>',
            '<script type="application/ld+json">' + json.dumps({'mainEntity': [{'acceptedAnswer': {'text': '広告・アクセス解析・利用状況の送信は含まず、アカウント登録も不要です。'}}]}, ensure_ascii=False) + '</script>',
        ]
        for source in examples:
            with self.subTest(source=source):
                self.assertTrue(unscoped_assurances(source))
        self.assertEqual(unscoped_assurances('<h3>広告はありますか？画像が外部に送られることはありませんか？</h3>'), [])
        self.assertEqual(unscoped_assurances(
            '<h2>安心して使えます</h2><p>広告・アクセス解析ツール・利用状況の送信はありません。</p>'
            '<meta name="description" content="無料・広告なしの画像ビューア">'), [])

    def test_real_previous_network_and_ai_wordings_fail_in_pages(self):
        cases = [
            ('privacy.html',
             '本アプリでは、次のような通信が発生します。',
             '本アプリが外部と通信するのは、次の場合に限られます。'),
            ('index.html',
             'mImageViewer 本体の画像 AI 処理は PC 内で完結します。',
             'AI 処理はすべて PC 内で完結します。'),
            ('index.html',
             'mImageViewer 本体の画像 AI 処理は利用者の PC 内で完結します。',
             'AI 処理はすべて利用者の PC 内で完結します。'),
            ('manual/faq.html',
             'mImageViewer 本体の画像 AI 処理は PC の中で行い、画像を外部のサーバーへ送りません。',
             'AI アップスケールなどの AI 処理はすべて PC の中で行い、画像を外部のサーバーへ送りません。'),
        ]
        for name, current, previous in cases:
            source = (PUBLIC / name).read_text(encoding='utf-8')
            # Check each occurrence independently (FAQ body and JSON-LD).
            occurrences = list(re.finditer(re.escape(current), source))
            self.assertTrue(occurrences, (name, current))
            for occurrence in occurrences:
                with self.subTest(page=name, offset=occurrence.start(), previous=previous):
                    restored = source[:occurrence.start()] + previous + source[occurrence.end():]
                    self.assertTrue(unscoped_assurances(restored))

    def test_json_ld_values_outside_faq_are_checked(self):
        previous_ai = 'AI 処理はすべて利用者の PC 内で完結します。'
        documents = [
            {'@type': 'SoftwareApplication', 'description': previous_ai},
            {'@graph': [{'@type': 'SoftwareApplication', 'description': previous_ai}]},
            [{'description': previous_ai}],
            {'nested': {'claims': ['本アプリが外部と通信するのは、次の場合に限られます。']}},
        ]
        for document in documents:
            with self.subTest(document=document):
                # JSON escaping must not hide the claim from the scanner.
                source = '<script type="application/ld+json">' + json.dumps(document) + '</script>'
                self.assertTrue(unscoped_assurances(source))
        self.assertTrue(unscoped_assurances(
            '<p>mImageViewer 本体の画像 AI にも対応します。' + previous_ai + '</p>'))

    def test_faq_search_answer_matches_visible_answer(self):
        source = (PUBLIC / 'manual/faq.html').read_text(encoding='utf-8')
        visible, search = faq_answer(source)
        self.assertEqual(visible, search)
        self.assertIn('広告・アクセス解析ツール・利用状況の送信はありません。', search)
        self.assertIn(UPDATE_JP, search)

    def test_japanese_update_notice_is_consistent(self):
        for name in ('index.html', 'privacy.html', 'manual/faq.html'):
            with self.subTest(page=name):
                paragraphs = blocks((PUBLIC / name).read_text(encoding='utf-8'))
                self.assertTrue(any(UPDATE_JP in text for text in paragraphs))
        privacy = (PUBLIC / 'privacy.html').read_text(encoding='utf-8')
        network = privacy.split('<h2>ネットワーク通信</h2>', 1)[1].split('</section>', 1)[0]
        self.assertEqual(sum(UPDATE_JP in normalize(text) for text in re.findall(r'<li>(.*?)</li>', network, re.S)), 1)

    def test_english_final_wording_and_epub(self):
        text = (PUBLIC / 'privacy.html').read_text(encoding='utf-8')
        self.assertIn(UPDATE_EN, text)
        self.assertIn('Network activity includes the following:', text)
        self.assertIn('blocks requests to external sites included in the book', text)
        self.assertIn('本に含まれる外部サイトへの要求を遮断します', text)
        network = text.split('<h2>Network communication</h2>', 1)[1].split('</section>', 1)[0]
        self.assertEqual(sum(UPDATE_EN in normalize(item) for item in re.findall(r'<li>(.*?)</li>', network, re.S)), 1)

    def test_sfz_storage_and_learning_match_hardware_facts(self):
        text = normalize((PUBLIC / 'privacy.html').read_text(encoding='utf-8'))
        self.assertEqual(text.count('%APPDATA%\\Frieve\\EffeTunePlugin\\sfz-references.json'), 2)
        for claim in ('名前と保存場所を記録します', 'コピーしません', '元のファイルは削除しません',
                      '学習内容はメモリ内で使い、ファイルには保存しません',
                      'instrument names and locations', 'are not copied', 'leaving the source files untouched',
                      'keeps learned data in memory and does not save it to files'):
            with self.subTest(claim=claim):
                self.assertIn(claim, text)

    def test_old_pending_process_wording_is_rejected_in_pages(self):
        replacements = [
            ('privacy.html', '本アプリでは、次のような通信が発生します。',
             'mImageViewer 本体で確認済みの外部通信は、次の場面で発生します。'),
            ('privacy.html', 'Network activity includes the following:',
             'The following network activity has been verified for mImageViewer itself:'),
            ('index.html', UPDATE_JP,
             '同梱 EffeTune v0.13.0 の新機能については、実機確認が残っています。公開前に説明を確定します。'),
            ('manual/faq.html', UPDATE_JP,
             'SFZ instruments and adaptive audio prediction were not used in that session.'),
        ]
        for name, final, pending in replacements:
            source = (PUBLIC / name).read_text(encoding='utf-8')
            self.assertIn(final, source)
            for occurrence in re.finditer(re.escape(final), source):
                with self.subTest(page=name, offset=occurrence.start()):
                    restored = source[:occurrence.start()] + pending + source[occurrence.end():]
                    self.assertTrue(unscoped_assurances(restored))


if __name__ == '__main__':
    unittest.main()
