# Upstream parser test fixtures (sd-parsers)

These AI-metadata sample images are vendored verbatim from the **sd-parsers**
project's test corpus, used here as real-world fixtures for mIV's
`png_metadata` parser.

- Source: https://github.com/d3x-at/sd-parsers (path: `tests/resources/`)
- License: MIT (see `LICENSE.sd-parsers`)
- Copyright (c) 2023 d3x-at

Content is benign (duck / cat / landscape / abstract test images). Layout
mirrors upstream: one folder per generator plus `bad_images/` for robustness
(empty / truncated / stealth / text-after-IDAT). Do not edit; refresh by
re-downloading from upstream if the corpus changes.

See `../ai_metadata/_fixtures.md` for the synthetic, mIV-specific fixtures and
the captured parser behaviour (including the known bugs these also exercise).
