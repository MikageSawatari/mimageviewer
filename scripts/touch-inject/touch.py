"""Synthetic touch for the isolated mIV test window only.
usage: touch.py PID drag x1 y1 x2 y2 steps ms_per_step   (coords in 1456x819 frame)
       touch.py PID tap x y
       touch.py PID info
Aborts if the window under any point is not owned by PID."""
import ctypes, ctypes.wintypes as W, sys, time
u = ctypes.WinDLL("user32", use_last_error=True)
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
SW, SH = u.GetSystemMetrics(0), u.GetSystemMetrics(1)
FW, FH = 1456, 819
class POINTER_INFO(ctypes.Structure):
    _fields_ = [("pointerType", W.DWORD), ("pointerId", W.UINT), ("frameId", W.UINT), ("pointerFlags", W.UINT),
                ("sourceDevice", W.HANDLE), ("hwndTarget", W.HWND), ("ptPixelLocation", W.POINT),
                ("ptHimetricLocation", W.POINT), ("ptPixelLocationRaw", W.POINT), ("ptHimetricLocationRaw", W.POINT),
                ("dwTime", W.DWORD), ("historyCount", W.UINT), ("InputData", ctypes.c_int32),
                ("dwKeyStates", W.DWORD), ("PerformanceCount", ctypes.c_uint64), ("ButtonChangeType", ctypes.c_int)]
class POINTER_TOUCH_INFO(ctypes.Structure):
    _fields_ = [("pointerInfo", POINTER_INFO), ("touchFlags", W.UINT), ("touchMask", W.UINT),
                ("rcContact", W.RECT), ("rcContactRaw", W.RECT), ("orientation", W.UINT), ("pressure", W.UINT)]
PT_TOUCH = 2
DOWN = 0x2 | 0x4 | 0x10000  # INRANGE|INCONTACT|DOWN
UPDATE = 0x2 | 0x4 | 0x20000
UP = 0x40000
MASK = 0x1 | 0x2 | 0x4  # contactarea|orientation|pressure
pid = int(sys.argv[1]); cmd = sys.argv[2]
def phys(x, y): return int(x * SW / FW), int(y * SH / FH)
def check(px, py):
    h = u.WindowFromPoint(W.POINT(px, py)); o = W.DWORD(); u.GetWindowThreadProcessId(h, ctypes.byref(o))
    if o.value != pid:
        print(f"ABORT: window at {px},{py} belongs to pid {o.value}"); sys.exit(3)
if cmd == "info":
    print(SW, SH); fg = u.GetForegroundWindow(); o = W.DWORD(); u.GetWindowThreadProcessId(fg, ctypes.byref(o)); print("fg pid", o.value); sys.exit(0)
if not u.InitializeTouchInjection(1, 1):
    print("InitializeTouchInjection failed", ctypes.get_last_error()); sys.exit(2)
def send(px, py, flags):
    c = POINTER_TOUCH_INFO(); c.pointerInfo.pointerType = PT_TOUCH; c.pointerInfo.pointerId = 0
    c.pointerInfo.ptPixelLocation = W.POINT(px, py); c.pointerInfo.pointerFlags = flags
    c.touchMask = MASK; c.orientation = 90; c.pressure = 32000
    c.rcContact = W.RECT(px - 2, py - 2, px + 2, py + 2)
    if not u.InjectTouchInput(1, ctypes.byref(c)):
        print("InjectTouchInput failed", ctypes.get_last_error()); sys.exit(2)
if cmd == "tap":
    px, py = phys(float(sys.argv[3]), float(sys.argv[4])); check(px, py)
    send(px, py, DOWN); time.sleep(0.05); send(px, py, UP); print("tapped", px, py)
elif cmd == "drag":
    x1, y1, x2, y2 = map(float, sys.argv[3:7]); steps = int(sys.argv[7]); ms = float(sys.argv[8])
    a = phys(x1, y1); b = phys(x2, y2); check(*a); check(*b)
    send(*a, DOWN); time.sleep(ms / 1000)
    for i in range(1, steps + 1):
        px = int(a[0] + (b[0] - a[0]) * i / steps); py = int(a[1] + (b[1] - a[1]) * i / steps)
        send(px, py, UPDATE); time.sleep(ms / 1000)
    send(*b, UP); print("dragged", a, b)
