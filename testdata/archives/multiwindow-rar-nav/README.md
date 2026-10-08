# multiwindow-rar-nav fixtures

Real archives for backlog 1.270 (Ctrl+Up/Down tree-order navigation from a RAR in a
multi-window separate window). RAR cannot be written from Rust, so the files are
checked in. Copied from the backlog 1.99 sample set generated on 2026-08-21; each page image
draws the archive name and page number (no third-party content).

| File | Kind | Open path |
| --- | --- | --- |
| `01-direct.rar` | non-solid RAR, images only | direct read |
| `02-solid.rar` | solid RAR | conversion to ZIP |
| `06-control.zip` | ZIP control | native |
| `08-direct.cbr` | non-solid RAR with the `.cbr` extension | direct read |

Used by `folder_tree::tests::rar_nav_*`, `app::multiwindow_scenario_tests::multiwindow_rar_nav_*`
and `scripts/ui-smoke.ps1 -Scenario MultiWindowRarNav`. Tests copy them into a temp directory.
