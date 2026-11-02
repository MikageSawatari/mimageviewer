#pragma once
#include <string>
inline constexpr char kPrompt[] = R"PROMPT(この画像に写っているものを日本語で説明してください。
caption: 1〜3 文、200 文字以内。写っていないことは書かない。
text: 画像の中で読み取れる文字 (題名・作者名・看板・ラベルなど) を、書かれているとおりに書き写す (200 文字以内)。無ければ空文字。推測で補わない。
keywords: 画像を検索するときに使える日本語の語を 5〜15 個。)PROMPT";
inline constexpr char kSchema[] = R"SCHEMA({"type": "object", "properties": {"caption": {"type": "string", "maxLength": 400}, "text": {"type": "string", "maxLength": 400}, "keywords": {"type": "array", "items": {"type": "string", "maxLength": 40}, "maxItems": 20}}, "required": ["caption", "text", "keywords"]})SCHEMA";
inline std::string fixed_prompt(const std::string & marker) {
    return "<|im_start|>user\n" + marker + kPrompt + "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";
}
