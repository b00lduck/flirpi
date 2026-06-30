// Framebuffer display via Linux /dev/fbX.
//
// Queries FBIOGET_VSCREENINFO and FBIOGET_FSCREENINFO at open time to learn
// the display resolution, pixel format, and line stride.  Supports 16-bit and
// 32-bit pixel formats; the colour-channel layout (RGB/BGR offsets) is read
// from the hardware info so it works with both RGB565 and BGR565 SPI LCD
// drivers (fbtft, etc.).
//
// The 160×120 thermal image is scaled to fill the framebuffer using
// nearest-neighbour interpolation.

use libc::{c_int, c_ulong, c_void, MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE};
use std::ffi::CString;
use std::fmt;

use crate::frame::{ThermalFrame, THERMAL_H, THERMAL_W};

// ── Linux framebuffer ioctl numbers ──────────────────────────────────────────
const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: c_ulong = 0x4602;

// ── Kernel struct mirrors (must match linux/fb.h exactly with #[repr(C)]) ───

#[repr(C)]
#[derive(Default, Copy, Clone)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    fb_type: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    // 2 bytes of padding inserted by #[repr(C)] to align line_length to 4 bytes
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

// ── Public API ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct FbError(String);

impl fmt::Display for FbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FbError {}

pub struct Framebuffer {
    fd: c_int,
    ptr: *mut u8,
    map_size: usize,
    vinfo: FbVarScreeninfo,
    line_length: u32,
    shadow: Vec<u8>,
}

impl Framebuffer {
    pub fn open(path: &str) -> Result<Self, FbError> {
        let cpath = CString::new(path).map_err(|_| FbError("invalid path".into()))?;

        let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDWR) };
        if fd < 0 {
            return Err(FbError(format!(
                "open {}: {}",
                path,
                std::io::Error::last_os_error()
            )));
        }

        let mut vinfo = FbVarScreeninfo::default();
        let mut finfo = FbFixScreeninfo::default();

        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut vinfo as *mut _) < 0 {
                libc::close(fd);
                return Err(FbError(format!(
                    "FBIOGET_VSCREENINFO: {}",
                    std::io::Error::last_os_error()
                )));
            }
            if libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut finfo as *mut _) < 0 {
                libc::close(fd);
                return Err(FbError(format!(
                    "FBIOGET_FSCREENINFO: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }

        let map_size = (vinfo.yres * finfo.line_length) as usize;
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut::<c_void>(),
                map_size,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };

        if ptr == MAP_FAILED {
            unsafe { libc::close(fd) };
            return Err(FbError(format!(
                "mmap: {}",
                std::io::Error::last_os_error()
            )));
        }

        Ok(Framebuffer {
            fd,
            ptr: ptr as *mut u8,
            map_size,
            vinfo,
            line_length: finfo.line_length,
            shadow: vec![0u8; map_size],
        })
    }

    pub fn width(&self) -> u32 {
        self.vinfo.xres
    }
    pub fn height(&self) -> u32 {
        self.vinfo.yres
    }
    pub fn bpp(&self) -> u32 {
        self.vinfo.bits_per_pixel
    }

    pub fn pixel_format_str(&self) -> String {
        let vi = &self.vinfo;
        format!(
            "R[off={} len={}] G[off={} len={}] B[off={} len={}]",
            vi.red.offset,
            vi.red.length,
            vi.green.offset,
            vi.green.length,
            vi.blue.offset,
            vi.blue.length,
        )
    }

    /// Write a thermal frame to the framebuffer, scaling to fill the screen.
    pub fn draw_thermal(&mut self, frame: &ThermalFrame, palette: &[[u8; 3]; 256]) {
        let fb_w = self.vinfo.xres as usize;
        let fb_h = self.vinfo.yres as usize;
        let bpp = self.vinfo.bits_per_pixel as usize;
        let bytes_pp = bpp / 8;
        let stride = self.line_length as usize;

        let fb = &mut self.shadow;

        let tw = THERMAL_W as f32;
        let th = THERMAL_H as f32;

        for dy in 0..fb_h {
            // Map output pixel centre into source space.
            let sy_f = (dy as f32 + 0.5) * th / fb_h as f32 - 0.5;
            let sy0 = (sy_f as isize).clamp(0, THERMAL_H as isize - 1) as usize;
            let sy1 = (sy0 + 1).min(THERMAL_H - 1);
            let ty = sy_f - sy0 as f32;
            let row_off = dy * stride;

            for dx in 0..fb_w {
                let sx_f = (dx as f32 + 0.5) * tw / fb_w as f32 - 0.5;
                let sx0 = (sx_f as isize).clamp(0, THERMAL_W as isize - 1) as usize;
                let sx1 = (sx0 + 1).min(THERMAL_W - 1);
                let tx = sx_f - sx0 as f32;

                // Bilinear interpolation in grayscale space, then palette lookup.
                let g00 = frame.gray[sy0 * THERMAL_W + sx0] as f32;
                let g10 = frame.gray[sy0 * THERMAL_W + sx1] as f32;
                let g01 = frame.gray[sy1 * THERMAL_W + sx0] as f32;
                let g11 = frame.gray[sy1 * THERMAL_W + sx1] as f32;
                let gray = (g00 * (1.0 - tx) * (1.0 - ty)
                    + g10 * tx * (1.0 - ty)
                    + g01 * (1.0 - tx) * ty
                    + g11 * tx * ty
                    + 0.5) as u8;

                let [r, g, b] = palette[gray as usize];

                let off = row_off + dx * bytes_pp;

                match bpp {
                    16 => {
                        // Encode using the hardware-reported channel layout (handles both
                        // RGB565 and BGR565 drivers).
                        let pixel = encode565(r, g, b);
                        // Use native byte order; fbtft drivers expect host-endian values.
                        let bytes = pixel.to_le_bytes();
                        fb[off] = bytes[0];
                        fb[off + 1] = bytes[1];
                    }
                    _ => {}
                }
            }
        }

        // Center crosshair
        draw_crosshair(fb, stride, bytes_pp, fb_w, fb_h, fb_w / 2, fb_h / 2, 5, 14);

        // Min/max crosshairs (smaller)
        let scale_x = fb_w as f32 / THERMAL_W as f32;
        let scale_y = fb_h as f32 / THERMAL_H as f32;
        let (mnx, mny) = frame.min_pos;
        let (mxx, mxy) = frame.max_pos;
        let mn_fx = (mnx as f32 * scale_x + 0.5) as usize;
        let mn_fy = (mny as f32 * scale_y + 0.5) as usize;
        let mx_fx = (mxx as f32 * scale_x + 0.5) as usize;
        let mx_fy = (mxy as f32 * scale_y + 0.5) as usize;
        draw_crosshair(fb, stride, bytes_pp, fb_w, fb_h, mn_fx, mn_fy, 3, 7);
        draw_crosshair(fb, stride, bytes_pp, fb_w, fb_h, mx_fx, mx_fy, 3, 7);

        // Palette legend — physical top edge (screen left), centred left-right physically.
        // On CCW display: physical left-right → screen y, physical top-inward → screen x.
        let scale = (fb_h / 120).max(1);
        let leg_len = 200usize; // along screen y = physical left-right
        let leg_w   = 10usize;  // along screen x = physical top-inward
        let leg_x0  = 2usize;                    // near screen left = physical top edge
        let leg_y0  = fb_h / 2 - leg_len / 2;   // centred in screen y
        // White border
        for x in leg_x0.saturating_sub(1)..=(leg_x0 + leg_w).min(fb_w - 1) {
            for y in leg_y0.saturating_sub(1)..=(leg_y0 + leg_len).min(fb_h - 1) {
                if x < leg_x0 || x >= leg_x0 + leg_w || y < leg_y0 || y >= leg_y0 + leg_len {
                    put16(fb, y * stride + x * bytes_pp, 0xFF, 0xFF, 0xFF);
                }
            }
        }
        // Gradient along screen y: small y = physical right (hot), large y = physical left (cold)
        for by in 0..leg_len {
            let idx = 255 - by * 255 / (leg_len - 1);
            let [r, g, b] = palette[idx];
            for bx in 0..leg_w {
                put16(fb, (leg_y0 + by) * stride + (leg_x0 + bx) * bytes_pp, r, g, b);
            }
        }
        // Labels below the bar (+3px down = +3 screen x), max right-aligned, min left-aligned.
        let char_step = 5 * scale + 1;
        let label_ox  = leg_x0 + leg_w + 2 + 3;
        let max_label = format!("{:.1}C", lepton2_celsius(frame.max_raw));
        let min_label = format!("{:.1}C", lepton2_celsius(frame.min_raw));
        // max right-aligned: last char's right edge at leg_y0 (hot/right end of bar)
        let hot_oy  = leg_y0 + (max_label.chars().count() - 1) * char_step;
        // min left-aligned: first char's left edge at leg_y0 + leg_len (cold/left end of bar)
        let cold_oy = leg_y0 + leg_len - 1 - 4 * scale;
        draw_text_ccw(fb, stride, bytes_pp, fb_w, fb_h, label_ox, hot_oy,  &max_label, scale);
        draw_text_ccw(fb, stride, bytes_pp, fb_w, fb_h, label_ox, cold_oy, &min_label, scale);

        // Center temperature just to the right of the center crosshair (screen x).
        let ctr_label = format!("{:.1}C", lepton2_celsius(frame.center_raw));
        let n = ctr_label.chars().count();
        let ctr_ox = fb_w / 2 + 20; // past crosshair arm (14) + gap (5) + small margin
        let ctr_oy = fb_h / 2 + n * (5 * scale + 1) / 2;
        draw_text_ccw(fb, stride, bytes_pp, fb_w, fb_h, ctr_ox, ctr_oy, &ctr_label, scale);

        // FLIRPI logo — physical bottom-right corner (screen top-right), 50% transparent.
        let logo_scale = (scale + 1).max(2);
        let logo_text = "FLIRPI";
        let n_logo = logo_text.chars().count();
        let logo_ox = fb_w.saturating_sub(7 * logo_scale + 7);
        let logo_oy = (4 + n_logo * (5 * logo_scale + 1)).saturating_sub(3);
        draw_logo_ccw(fb, stride, bytes_pp, fb_w, fb_h, logo_ox, logo_oy,
                      logo_text, logo_scale, 0xFF, 0xFF, 0xFF, 128);

        // Flush shadow buffer to framebuffer in one shot to avoid tearing.
        let hw = unsafe { std::slice::from_raw_parts_mut(self.ptr, self.map_size) };
        hw.copy_from_slice(&self.shadow);
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut c_void, self.map_size);
            libc::close(self.fd);
        }
    }
}

// Safety: Framebuffer owns its mmap'd memory exclusively.
unsafe impl Send for Framebuffer {}

// ── Temperature conversion ───────────────────────────────────────────────────

// Inverse Planck law for this Lepton 2 unit.
// Scale and offset fitted via 2-point calibration (3010→28.4°C, 3500→36.5°C).
// ponytail: B and R1/R2 are Lepton 2 Planck constants; retune scale/offset if swapping units.
fn draw_crosshair(fb: &mut [u8], stride: usize, bytes_pp: usize, fb_w: usize, fb_h: usize,
                  cx: usize, cy: usize, gap: usize, arm: usize) {
    let pixels: Vec<(usize, usize)> = {
        let mut v = Vec::new();
        for dx in cx.saturating_sub(arm + gap)..=((cx + arm + gap).min(fb_w - 1)) {
            if dx < cx.saturating_sub(gap) || dx > cx + gap {
                v.push((dx, cy));
            }
        }
        for dy in cy.saturating_sub(arm + gap)..=((cy + arm + gap).min(fb_h - 1)) {
            if dy < cy.saturating_sub(gap) || dy > cy + gap {
                v.push((cx, dy));
            }
        }
        v
    };
    for &(px, py) in &pixels {
        for (dx, dy) in [(-1i32,0),(1,0),(0,-1i32),(0,1)] {
            let qx = px as i32 + dx;
            let qy = py as i32 + dy;
            if qx >= 0 && (qx as usize) < fb_w && qy >= 0 && (qy as usize) < fb_h {
                put16(fb, qy as usize * stride + qx as usize * bytes_pp, 0, 0, 0);
            }
        }
    }
    for &(px, py) in &pixels {
        put16(fb, py * stride + px * bytes_pp, 0xFF, 0xFF, 0xFF);
    }
}

fn lepton2_celsius(raw: u16) -> f32 {
    const B: f32 = 1435.0;
    const R1_OVER_R2: f32 = 18417.0 / 0.0125; // = 1_473_360
    let signal = raw as f32 * 3.608 + 1851.0;
    B / (R1_OVER_R2 / signal + 1.0).ln() - 273.15
}

// ── Pixel encoding helpers ───────────────────────────────────────────────────

fn encode565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

#[inline]
fn put16(fb: &mut [u8], off: usize, r: u8, g: u8, b: u8) {
    if off + 1 < fb.len() {
        let p = encode565(r, g, b).to_le_bytes();
        fb[off] = p[0];
        fb[off + 1] = p[1];
    }
}

// 5×7 bitmap font — each entry is 7 rows; bit 4 = leftmost column.
fn char_bitmap(c: char) -> Option<[u8; 7]> {
    Some(match c {
        '0' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b01110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00001, 0b00010, 0b00100, 0b00100, 0b00100],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'M' => [0b10001, 0b11011, 0b10101, 0b10001, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'X' => [0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b01010, 0b10001],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        ' ' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        _ => return None,
    })
}

// Draw a string with glyphs rotated 90° CCW, for a display mounted 90° CCW.
// Characters stack so the string reads left-to-right on the physical display.
// ox: screen x (= physical y, near physical top when small)
// oy: screen y of the physical-left edge of the first character (near screen bottom)
fn draw_text_ccw(
    fb: &mut [u8],
    stride: usize,
    bytes_pp: usize,
    fb_w: usize,
    fb_h: usize,
    ox: usize,
    oy: usize,
    text: &str,
    scale: usize,
) {
    // Rotated glyph: screen_x = ox + row*scale, screen_y = oy + (4-col)*scale
    // Each glyph is 7*scale wide × 5*scale tall in screen coords.
    // Going right physically = decreasing screen_y → subtract char_step per char.
    let char_step = 5 * scale + 1;
    for (i, c) in text.chars().enumerate() {
        let Some(bm) = char_bitmap(c) else { continue };
        let char_oy = oy as isize - i as isize * char_step as isize;
        if char_oy + 5 * scale as isize <= 0 { break; }
        let char_oy = char_oy.max(0) as usize;
        for pass in 0..2u8 {
            for row in 0..7usize {
                for col in 0..5usize {
                    if bm[row] & (0x10 >> col) == 0 { continue; }
                    for sy in 0..scale {
                        let py = char_oy + (4 - col) * scale + sy;
                        for sx in 0..scale {
                            let px = ox + row * scale + sx;
                            if pass == 0 {
                                for dx in -2i32..=2 {
                                    for dy in -2i32..=2 {
                                        let qx = px as i32 + dx;
                                        let qy = py as i32 + dy;
                                        if qx >= 0 && (qx as usize) < fb_w && qy >= 0 && (qy as usize) < fb_h {
                                            put16(fb, qy as usize * stride + qx as usize * bytes_pp, 0, 0, 0);
                                        }
                                    }
                                }
                            } else if px < fb_w && py < fb_h {
                                put16(fb, py * stride + px * bytes_pp, 0xFF, 0xFF, 0xFF);
                            }
                        }
                    }
                }
            }
        }
    }
}

// Like draw_text_ccw but no outline; blends fill color with the existing pixel
// at `alpha` (0 = fully transparent, 255 = opaque).
fn draw_logo_ccw(
    fb: &mut [u8],
    stride: usize,
    bytes_pp: usize,
    fb_w: usize,
    fb_h: usize,
    ox: usize,
    oy: usize,
    text: &str,
    scale: usize,
    r: u8, g: u8, b: u8,
    alpha: u8,
) {
    let char_step = 5 * scale + 1;
    for (i, c) in text.chars().enumerate() {
        let Some(bm) = char_bitmap(c) else { continue };
        let char_oy = oy as isize - i as isize * char_step as isize;
        if char_oy + 5 * scale as isize <= 0 { break; }
        let char_oy = char_oy.max(0) as usize;
        for row in 0..7usize {
            for col in 0..5usize {
                if bm[row] & (0x10 >> col) == 0 { continue; }
                for sy in 0..scale {
                    let py = char_oy + (4 - col) * scale + sy;
                    for sx in 0..scale {
                        let px = ox + row * scale + sx;
                        if px >= fb_w || py >= fb_h { continue; }
                        let off = py * stride + px * bytes_pp;
                        // Read back current RGB565 pixel and blend
                        let cur = u16::from_le_bytes([fb[off], fb[off + 1]]);
                        let br = (((cur >> 11) & 0x1F) as u8) << 3;
                        let bg = (((cur >> 5)  & 0x3F) as u8) << 2;
                        let bb = ((cur         & 0x1F) as u8) << 3;
                        let a = alpha as u16;
                        let nr = ((r as u16 * a + br as u16 * (255 - a)) / 255) as u8;
                        let ng = ((g as u16 * a + bg as u16 * (255 - a)) / 255) as u8;
                        let nb = ((b as u16 * a + bb as u16 * (255 - a)) / 255) as u8;
                        put16(fb, off, nr, ng, nb);
                    }
                }
            }
        }
    }
}

