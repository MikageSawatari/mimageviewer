#[derive(Clone, Debug)]
pub struct ScreenColorSample {
    pub cursor: (i32, i32),
    pub radius: i32,
    pub pixels: Vec<[u8; 3]>,
    pub center_rgb: [u8; 3],
}

impl ScreenColorSample {
    pub fn side(&self) -> usize {
        (self.radius.saturating_mul(2).saturating_add(1)) as usize
    }

    pub fn pixel(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        let side = self.side();
        self.pixels
            .get(y.checked_mul(side)?.checked_add(x)?)
            .copied()
    }
}

#[cfg(windows)]
pub fn sample_cursor(radius: i32) -> Option<ScreenColorSample> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let radius = radius.clamp(0, 24);
    let side = radius.saturating_mul(2).saturating_add(1) as usize;
    let mut pt = POINT::default();
    unsafe {
        GetCursorPos(&mut pt).ok()?;
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let mut pixels = Vec::with_capacity(side.saturating_mul(side));
        for y in -radius..=radius {
            for x in -radius..=radius {
                let color = GetPixel(hdc, pt.x.saturating_add(x), pt.y.saturating_add(y));
                let raw = color.0;
                if raw == 0xFFFF_FFFF {
                    pixels.push([0, 0, 0]);
                } else {
                    pixels.push([
                        (raw & 0xff) as u8,
                        ((raw >> 8) & 0xff) as u8,
                        ((raw >> 16) & 0xff) as u8,
                    ]);
                }
            }
        }
        let _ = ReleaseDC(None, hdc);
        let center_idx = (radius as usize)
            .saturating_mul(side)
            .saturating_add(radius as usize);
        let center_rgb = pixels.get(center_idx).copied()?;
        Some(ScreenColorSample {
            cursor: (pt.x, pt.y),
            radius,
            pixels,
            center_rgb,
        })
    }
}

#[cfg(not(windows))]
pub fn sample_cursor(_radius: i32) -> Option<ScreenColorSample> {
    None
}

#[cfg(windows)]
pub fn primary_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 }
}

#[cfg(not(windows))]
pub fn primary_button_down() -> bool {
    false
}
