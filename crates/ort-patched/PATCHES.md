# Local patches to ort 2.0.0-rc.12

This directory contains the crates.io source for
[`ort` 2.0.0-rc.12](https://crates.io/crates/ort/2.0.0-rc.12), licensed under
MIT or Apache-2.0. The original license texts are kept alongside the source.

## Dynamic-load failure re-entry

The local source contains the exact three-file change from upstream commit
[`17ed727`](https://github.com/pykeio/ort/commit/17ed727). Dynamic library open,
missing `OrtGetApiBase`, and incompatible-version failures now use
`LoadDynamicError`, whose construction does not call the ONNX Runtime API. This
prevents `G_ORT_LIB` initialization from re-entering its own `OnceLock` after a
load failure.

Remove this fork when mImageViewer can upgrade to an upstream `ort` release that
contains the fix and the bundled DirectML and TensorRT ONNX Runtime binaries have
been migrated and verified with that release.
