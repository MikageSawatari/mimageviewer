"""Send each V0 evaluation image to a local llama-server and record the results.

Used by the local VLM V0 evaluation (docs/local-vlm-v0-procedure.md).
Standard library only. Run with ``python -I``.

    python -I run_eval.py --server http://127.0.0.1:PORT --inputs DIR --out results.jsonl --label C1
"""

import argparse
import base64
import json
import pathlib
import statistics
import time
import urllib.request

PROMPTS = {
    "v0-1": (
        "この画像に写っているものを日本語で説明してください。\n"
        "caption: 1〜3 文、200 文字以内。写っていないことは書かない。\n"
        "keywords: 画像を検索するときに使える日本語の語を 5〜15 個。"
    ),
    # v0-2: also copy visible text, title and authors (book covers etc.).
    "v0-2": (
        "この画像に写っているものを日本語で説明してください。\n"
        "caption: 1〜3 文、200 文字以内。写っていないことは書かない。\n"
        "title: 本・雑誌・作品の表紙などで題名が読み取れれば、書かれているとおりに書き写す。無ければ空文字。\n"
        "authors: 作者名・著者名・サークル名が読み取れれば、書かれているとおりに書き写す。無ければ空の配列。\n"
        "text: 画像の中で読み取れるその他の文字を、書かれているとおりに書き写す (200 文字以内)。無ければ空文字。推測で補わない。\n"
        "keywords: 画像を検索するときに使える日本語の語を 5〜15 個。"
    ),
    # v0-3: one plain "text" field (no title/author split); text is for search only.
    "v0-3": (
        "この画像に写っているものを日本語で説明してください。\n"
        "caption: 1〜3 文、200 文字以内。写っていないことは書かない。\n"
        "text: 画像の中で読み取れる文字 (題名・作者名・看板・ラベルなど) を、書かれているとおりに書き写す (200 文字以内)。"
        "無ければ空文字。推測で補わない。\n"
        "keywords: 画像を検索するときに使える日本語の語を 5〜15 個。"
    ),
}
SCHEMAS = {
    "v0-1": {
        "type": "object",
        "properties": {
            "caption": {"type": "string"},
            "keywords": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["caption", "keywords"],
    },
    "v0-2": {
        "type": "object",
        "properties": {
            "caption": {"type": "string"},
            "title": {"type": "string"},
            "authors": {"type": "array", "items": {"type": "string"}},
            "text": {"type": "string"},
            "keywords": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["caption", "title", "authors", "text", "keywords"],
    },
    "v0-3": {
        "type": "object",
        "properties": {
            "caption": {"type": "string"},
            "text": {"type": "string"},
            "keywords": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["caption", "text", "keywords"],
    },
    # v0-3 prompt with the plan's limits (§6.4) written into the schema, so the
    # grammar stops a runaway keyword list (seen once with v0-3).
    "v0-3l": {
        "type": "object",
        "properties": {
            "caption": {"type": "string", "maxLength": 400},
            "text": {"type": "string", "maxLength": 400},
            "keywords": {"type": "array", "items": {"type": "string", "maxLength": 40}, "maxItems": 20},
        },
        "required": ["caption", "text", "keywords"],
    },
}
PROMPTS["v0-3l"] = PROMPTS["v0-3"]


def post(url, body, timeout):
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", required=True)
    ap.add_argument("--inputs", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--max-tokens", type=int, default=None, help="default 400 (v0-1) / 800 (others)")
    ap.add_argument("--no-schema", action="store_true")
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--prompt-version", default="v0-1", choices=sorted(PROMPTS))
    a = ap.parse_args()
    if a.max_tokens is None:
        a.max_tokens = 400 if a.prompt_version == "v0-1" else 800

    images = sorted(p for p in pathlib.Path(a.inputs).iterdir() if p.suffix.lower() == ".png")
    out = open(a.out, "w", encoding="utf-8")
    walls = []
    for i, p in enumerate(images):
        b64 = base64.b64encode(p.read_bytes()).decode("ascii")
        body = {
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "image_url", "image_url": {"url": "data:image/png;base64," + b64}},
                        {"type": "text", "text": PROMPTS[a.prompt_version]},
                    ],
                }
            ],
            "temperature": 0,
            "max_tokens": a.max_tokens,
            "chat_template_kwargs": {"enable_thinking": False},
        }
        if not a.no_schema:
            body["response_format"] = {
                "type": "json_schema",
                "json_schema": {"name": "caption", "schema": SCHEMAS[a.prompt_version]},
            }
        t0 = time.perf_counter()
        rec = {"label": a.label, "prompt_version": a.prompt_version, "image": p.name, "index": i}
        try:
            r = post(a.server.rstrip("/") + "/v1/chat/completions", body, a.timeout)
            rec["wall_s"] = round(time.perf_counter() - t0, 3)
            msg = r["choices"][0]["message"]
            content = msg.get("content") or ""
            rec["content"] = content
            if msg.get("reasoning_content"):
                rec["reasoning_chars"] = len(msg["reasoning_content"])
            rec["finish_reason"] = r["choices"][0].get("finish_reason")
            rec["usage"] = r.get("usage")
            rec["timings"] = r.get("timings")
            try:
                obj = json.loads(content)
                rec["json_ok"] = isinstance(obj.get("caption"), str) and isinstance(obj.get("keywords"), list)
                rec["caption"] = obj.get("caption")
                rec["keywords"] = obj.get("keywords")
                rec["caption_chars"] = len(obj.get("caption") or "")
                for f in ("title", "authors", "text"):
                    if f in obj:
                        rec[f] = obj[f]
            except Exception:
                rec["json_ok"] = False
        except Exception as e:  # recorded, the run continues
            rec["wall_s"] = round(time.perf_counter() - t0, 3)
            rec["error"] = repr(e)
        out.write(json.dumps(rec, ensure_ascii=False) + "\n")
        out.flush()
        if i > 0 and "error" not in rec:
            walls.append(rec["wall_s"])
        print(i, p.name[:40], rec.get("wall_s"), "json_ok=" + str(rec.get("json_ok")), rec.get("error", ""))
    out.close()
    if walls:
        print(
            "summary (excluding the first image): n=%d median=%.2fs mean=%.2fs max=%.2fs"
            % (len(walls), statistics.median(walls), statistics.mean(walls), max(walls))
        )


if __name__ == "__main__":
    main()
