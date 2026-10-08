"""Run round3's exact case matrix against the new production layout owner."""
from pathlib import Path
here=Path(__file__).resolve().parent
old=(here.parent/'round2/geometry_probe.py').read_text(encoding='utf-8')
prefix,remaining=old.split("parts.append(r'''",1)
_,suffix=remaining.split("''')",1)
prefix=prefix.replace('target/review-v350-round2','target/review-v350-round4')
prefix=prefix.replace('"struct ContinuousUnitDrawnSpan"','"struct ContinuousUnitLayout"')
prefix=prefix.replace('"continuous_unit_drawn_span"','"continuous_unit_layout"')
exec(prefix)
tie=re.search(r'^const PHYSICAL_PIXEL_TIE_EPSILON:.*;$',transform,re.M)
if tie:
    parts.insert(parts.index('mod displayed_image_transform { use crate::rotation_db::Rotation;')+1,tie[0])
cases=(here.parent/'round3/geometry_cases.rs').read_text(encoding='utf-8')
cases=cases.replace('let span = continuous_unit_drawn_span(s,ppp);\n                        span.y_max-span.y_min',
                    'continuous_unit_layout(s,ppp).drawn_size.y.max(1.0)')
assert 'continuous_unit_drawn_span' not in cases
parts.append(cases)
exec(suffix)
