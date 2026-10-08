"""Reuse the production-library linker with round3-only output paths."""
from pathlib import Path
previous = Path(__file__).resolve().parent.parent / "round2/run_rust_probe.py"
exec(previous.read_text(encoding="utf-8").replace("target/review-v350-round2", "target/review-v350-round3"))
