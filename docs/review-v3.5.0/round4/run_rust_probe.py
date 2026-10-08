"""Production-library probe linker, with round4-only outputs."""
from pathlib import Path
previous=Path(__file__).resolve().parent.parent/'round2/run_rust_probe.py'
exec(previous.read_text(encoding='utf-8').replace('target/review-v350-round2','target/review-v350-round4'))
