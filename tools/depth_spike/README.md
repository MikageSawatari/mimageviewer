# Depth-parallax spike

Research scripts for `docs/depth-parallax-research.md` (not product code).

```powershell
python -m venv target\depth-spike\venv
target\depth-spike\venv\Scripts\python -m pip install onnxruntime-directml numpy pillow pillow-heif opencv-python-headless imageio imageio-ffmpeg
# Put models under target\depth-spike\models\{dav2s,da3s,moge2s,da3monol}\ and images under target\depth-spike\samples\
target\depth-spike\venv\Scripts\python tools\depth_spike\run_compare.py target\depth-spike
target\depth-spike\venv\Scripts\python tools\depth_spike\make_anim.py target\depth-spike grid <stem>...
target\depth-spike\venv\Scripts\python tools\depth_spike\make_anim.py target\depth-spike export DA3Mono-L <stem>...
```

Model sources: onnx-community/depth-anything-v2-small, onnx-community/depth-anything-v3-small,
Ruicheng/moge-2-vits-normal-onnx, SpatialHub/depth-anything-3-onnx (da3mono_large only).
