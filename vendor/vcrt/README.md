# App-local Microsoft Visual C++ runtime

These four unmodified x64 DLLs come from the licensed Visual Studio Build Tools
`VC/Redist` directory recorded in `provenance.json`. They are redistributed with
mImageViewer under the Visual Studio licensing terms. Do not replace them with
copies from `System32` or re-sign them.

The app-local runtime is not serviced by Windows Update. Any toolchain or native
dependency update must rerun the repository's VC runtime provenance and PE
dependency gate, then update the DLLs and manifest together when required.
