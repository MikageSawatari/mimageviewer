"""Run production geometry on the original cases plus mixed-trim spreads.

Shares round2's extraction/compiler harness; it copies function bodies without
rewriting their arithmetic. No application or user profile is started.
"""
from pathlib import Path

here = Path(__file__).resolve().parent
old = (here.parent / "round2/geometry_probe.py").read_text(encoding="utf-8")
prefix, remaining = old.split("parts.append(r'''", 1)
_, suffix = remaining.split("''')", 1)
prefix = prefix.replace("target/review-v350-round2", "target/review-v350-round3")
exec(prefix)
tie = re.search(r"^const PHYSICAL_PIXEL_TIE_EPSILON:.*;$", transform, re.M)
if tie:
    parts.insert(parts.index("mod displayed_image_transform { use crate::rotation_db::Rotation;") + 1, tie[0])
parts.append((here / "geometry_cases.rs").read_text(encoding="utf-8"))
exec(suffix)
