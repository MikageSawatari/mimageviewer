"""Shared logical-window geometry for release-check fixtures and evidence."""

from __future__ import annotations

import json
from pathlib import Path


_TABLE = json.loads(Path(__file__).with_suffix(".json").read_text(encoding="utf-8"))
if _TABLE.get("schema_version") != 1:
    raise ValueError("unsupported release-check geometry schema")

# Main CentralPanel has 8-point margins; thumbnail available_width is measured
# before ScrollArea. At 96 DPI: floor((1420 - 16) / 5) = 280, portrait height
# round(280 * 4/3) = 373px. At 144 DPI: floor((1328 - 16) / 7) = 187,
# round(187 * 4/3) = 249 logical px, or ~374 physical px. Actual request evidence
# still has to satisfy the existing 374 +/- 8 regression gate.


def static_geometry(dpi: int) -> dict:
    """Return an independent geometry row, refusing uncalibrated DPI values."""
    if type(dpi) is int:
        for row in _TABLE["static_rows"]:
            if row["dpi"] == dpi:
                return {"dpi": dpi, "window_size": list(row["window_size"]),
                        "grid_cols": row["grid_cols"]}
    raise ValueError(f"unsupported release-check DPI: {dpi!r}; expected 96 or 144")


def case_geometry(case_id: str, dpi: int) -> dict:
    """Keep nonstatic layouts unchanged; only static checks need calibration."""
    static = static_geometry(dpi)
    if case_id in ("static-foreground", "static-background", "seed-profile"):
        return static
    default = _TABLE["default"]
    return {"dpi": dpi, "window_size": list(default["window_size"]),
            "grid_cols": 10 if case_id.startswith("tray-") else default["grid_cols"]}
