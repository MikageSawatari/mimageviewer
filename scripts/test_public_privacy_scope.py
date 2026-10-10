"""Guard the reviewed scope of public privacy claims, including search metadata."""
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parent.parent
PUBLIC = ROOT / 'htdocs/mimageviewer'


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
        if tag in ('p', 'span', 'h2', 'h3'):
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
    topic = re.search(r'広告|アクセス解析|利用状況|advertis|analytics|usage reporting', text, re.I)
    assurance = re.search(r'一切含みません|含まず|含みません|ありません(?!か)|広告なし|no advertising|no analytics', text, re.I)
    scope = re.search(r'本体|mImageViewer itself', text, re.I)
    if topic and assurance and not scope:
        result.append(text)
    for match in re.finditer(r'安心して使えます|この\s*4\s*つ以外に通信しません|通信は増えません|本アプリが外部と通信するのは、次の場合に限られます|connects to the internet only|EPUB conversion is not another network case', text, re.I):
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
            '<meta name="description" content="無料・広告なしの画像ビューア">',
            '<h2>安心して使えます</h2>',
            '<p>EPUB の変換も通信は増えません。</p>',
            '<script type="application/ld+json">' + json.dumps({'mainEntity': [{'acceptedAnswer': {'text': '広告・アクセス解析・利用状況の送信は含まず、アカウント登録も不要です。'}}]}, ensure_ascii=False) + '</script>',
        ]
        for source in examples:
            with self.subTest(source=source):
                self.assertTrue(unscoped_assurances(source))
        self.assertEqual(unscoped_assurances('<h3>広告はありますか？画像が外部に送られることはありませんか？</h3>'), [])

    def test_real_previous_network_and_ai_wordings_fail_in_pages(self):
        cases = [
            ('privacy.html',
             'mImageViewer 本体で確認済みの外部通信は、次の場面で発生します。',
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
        self.assertIn('確認済みの mImageViewer 本体の機能', search)

    def test_japanese_hardware_draft_is_consistent_and_bounded(self):
        descriptions = []
        for name in ('index.html', 'privacy.html', 'manual/faq.html'):
            paragraphs = blocks((PUBLIC / name).read_text(encoding='utf-8'))
            description = next(text for text in paragraphs if text.startswith('音響調整の画面表示に使う Windows の表示機能'))
            descriptions.append(description)
            self.assertIn('Microsoft に自身の更新を確認することがあります', description)
            self.assertIn('音楽の再生と曲送りを行った確認では', description)
            self.assertIn('SFZ 音源と音声予測は、この確認では使っていません', description)
            self.assertNotIn('component updater', description)
        self.assertEqual(len(set(descriptions)), 1)

    def test_english_draft_matches_observation_scope(self):
        text = (PUBLIC / 'privacy.html').read_text(encoding='utf-8')
        self.assertIn('The verified features of mImageViewer itself contain', text)
        self.assertIn('may contact Microsoft to check for its own updates', text)
        self.assertIn('In the observed session of opening audio adjustment, playing music and changing tracks', text)
        self.assertIn('SFZ instruments and adaptive audio prediction were not used in that session', text)
        self.assertIn('during EPUB conversion was not examined in this hardware session', text)


if __name__ == '__main__':
    unittest.main()
