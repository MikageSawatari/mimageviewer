`fixtures.json` contains synthetic rule-boundary cases. `evaluation_fixtures.json`
contains all saved segment rows from three public evaluation materials: m1 (22
rows), m3 (31 rows), and m5 (10 rows), before cleanup. Expectations come from the
unmodified evaluation branch's `postprocess.py::clean(segments, None)`, rather than
from the Rust port. Start/end values are converted losslessly from seconds to
integer milliseconds; text is unchanged. The fixture records source paths and
SHA-256 digests. No private user-video transcript is copied.

The saved ASR run is `turbo-q5_0-vulkan-mc0` (large-v3-turbo Q5_0, Vulkan,
Japanese, previous-text context disabled). Source/provenance is the evaluation
repository `exp-asr-eval`, documented in `scripts/asr-eval/README.md` (materials)
and `build_materials.py` (`TTS_SOURCE` / `FLEURS_SOURCE`).

- m1_explainer and m5_video: scripts written for that evaluation, synthesized
  with Windows OneCore Japanese voices. The material metadata states that the
  text has no third-party rights. No audio is included here.
- m3_fleurs_clean: [Google FLEURS](https://huggingface.co/datasets/google/fleurs),
  ja_jp test split, [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/),
  human-read Wikipedia-derived sentences. The included text is ASR output of
  the concatenated evaluation recording and may contain transcription errors;
  the original dataset has been transformed by transcription and segmentation.

Regenerate both JSON files from the repository root:

```powershell
python src/ai/transcript_postprocess/generate_fixtures.py
```
