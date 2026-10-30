"""Touch down on a cell, switch list with Alt+Left (only if mIV is foreground), then lift.
usage: hold_switch.py PID x y   (1456x819 frame)"""
import ctypes, ctypes.wintypes as W, sys, time, importlib.util
spec = importlib.util.spec_from_file_location("t", __file__.replace("hold_switch.py", "touch.py"))
pid = int(sys.argv[1]); x, y = float(sys.argv[2]), float(sys.argv[3]); vk = 0x27 if sys.argv[4] == "right" else 0x25; mode = sys.argv[5] if len(sys.argv) > 5 else "lift"
sys.argv = [sys.argv[0], str(pid), "noop"]
t = importlib.util.module_from_spec(spec)
try:
    spec.loader.exec_module(t)
except SystemExit:
    pass
u = t.u
fg = u.GetForegroundWindow(); o = W.DWORD(); u.GetWindowThreadProcessId(fg, ctypes.byref(o))
if o.value != pid:
    print("ABORT: foreground pid", o.value); sys.exit(3)
px, py = t.phys(x, y); t.check(px, py)
def hold(sec):
    end = time.time() + sec
    while time.time() < end:
        t.send(px, py, t.UPDATE); time.sleep(0.016)
if mode == "noupdate":
    hold = lambda sec: time.sleep(sec)
t.send(px, py, t.DOWN); hold(0.15)
VK_MENU, UPF = 0x12, 0x2
for k, f in [(VK_MENU, 0), (vk, 0), (vk, UPF), (VK_MENU, UPF)]:
    u.keybd_event(k, 0, f, 0); hold(0.04)
hold(0.15)
if mode == "timeout":
    time.sleep(1.5); print("left contact to time out at", px, py)
else:
    try:
        t.send(px, py, t.UP); print("held, switched, lifted at", px, py)
    except SystemExit:
        print("lift failed (contact already timed out)")
