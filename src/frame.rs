// Frame accumulator and thermal data extraction for the FLIR One (Lepton 3 sensor).
//
// The camera streams USB bulk chunks on EP 0x85.  Each chunk may be part of a
// larger frame.  A complete frame looks like:
//
//   Offset  Size  Content
//   0       4     Magic: EF BE 00 00
//   4       4     (unknown)
//   8       4     FrameSize   (u32 LE) – total payload after the 28-byte header
//   12      4     ThermalSize (u32 LE)
//   16      4     JpgSize     (u32 LE)
//   20      4     StatusSize  (u32 LE)
//   24      4     (unknown)
//   28      …     Thermal data  (ThermalSize bytes)
//   28+T    …     JPEG data     (JpgSize bytes)
//   28+T+J  …     Status JSON   (StatusSize bytes)
//
// Thermal data layout (160×120, 16-bit unsigned, row stride 164 pixels):
//   For pixel (x, y):
//     x < 80:  buf[32 + 2*(y*164 + x)]  (little-endian u16)
//     x ≥ 80:  buf[36 + 2*(y*164 + x)]
//   (Two 2-pixel gaps per row account for the 164-pixel stride vs 160-pixel image.)
//
// FFC (Flat-Field Correction) frames appear as "FFC" at offset
//   28 + ThermalSize + JpgSize + 17
// within the status block.  These frames and the one immediately after must
// be dropped to avoid a bright flash artefact.

pub const THERMAL_W: usize = 160;
pub const THERMAL_H: usize = 120;

const THERMAL_STRIDE: usize = 164;
const BUF_SIZE: usize = 1 << 20; // 1 MiB – same size used by original flirone-v4l2
const MAGIC: [u8; 4] = [0xEF, 0xBE, 0x00, 0x00];

pub struct ThermalFrame {
    /// Min-max normalised grayscale, indexed [y * THERMAL_W + x].
    pub gray: [u8; THERMAL_W * THERMAL_H],
}

pub struct FrameAccumulator {
    buf: Vec<u8>,
    len: usize,
    ffc: FfcState,
}

#[derive(Clone, Copy, PartialEq)]
enum FfcState {
    Normal,
    SkipOne, // skip the first frame after an FFC shutter event
}

impl FrameAccumulator {
    pub fn new() -> Self {
        FrameAccumulator {
            buf: vec![0u8; BUF_SIZE],
            len: 0,
            ffc: FfcState::Normal,
        }
    }

    /// Feed a raw USB chunk.  Returns a complete thermal frame when one is ready.
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Option<ThermalFrame> {
        if chunk.is_empty() || chunk.len() >= BUF_SIZE {
            return None;
        }

        // A magic header at the start of a chunk signals the beginning of a new
        // frame; also reset if the buffer would overflow.
        if chunk.starts_with(&MAGIC) || self.len + chunk.len() >= BUF_SIZE {
            self.len = 0;
        }

        self.buf[self.len..self.len + chunk.len()].copy_from_slice(chunk);
        self.len += chunk.len();

        // Sanity: the accumulated data must still start with the magic bytes.
        if self.len < 4 || self.buf[..4] != MAGIC {
            self.len = 0;
            return None;
        }

        if self.len < 28 {
            return None; // header not yet complete
        }

        let frame_size = u32_le(&self.buf, 8) as usize;
        let thermal_size = u32_le(&self.buf, 12) as usize;
        let jpg_size = u32_le(&self.buf, 16) as usize;

        let total = frame_size + 28;
        if self.len < total {
            return None; // still accumulating chunks
        }

        // Check for FFC shutter event in the status block.
        let ffc_offset = 28 + thermal_size + jpg_size + 17;
        let is_ffc = ffc_offset + 3 <= total
            && self.buf[ffc_offset..ffc_offset + 3] == *b"FFC";

        self.len = 0; // frame consumed regardless

        if is_ffc {
            self.ffc = FfcState::SkipOne;
            return None;
        }

        if self.ffc == FfcState::SkipOne {
            self.ffc = FfcState::Normal;
            return None;
        }

        Some(extract_thermal(&self.buf[..total]))
    }
}

fn extract_thermal(frame: &[u8]) -> ThermalFrame {
    let mut raw = [0u16; THERMAL_W * THERMAL_H];
    let mut min = u16::MAX;
    let mut max = u16::MIN;

    for y in 0..THERMAL_H {
        for x in 0..THERMAL_W {
            // The two-pixel gap in each row half is handled by the +4 offset for x ≥ 80.
            let base = if x < 80 {
                32 + 2 * (y * THERMAL_STRIDE + x)
            } else {
                36 + 2 * (y * THERMAL_STRIDE + x)
            };
            let v = u16::from_le_bytes([frame[base], frame[base + 1]]);
            raw[y * THERMAL_W + x] = v;
            if v < min {
                min = v;
            }
            if v > max {
                max = v;
            }
        }
    }

    // Min-max normalise to 8-bit.
    let delta = (max - min) as u32;
    let scale = if delta > 0 { 0x10000u32 / delta } else { 1 };

    let mut gray = [0u8; THERMAL_W * THERMAL_H];
    for i in 0..THERMAL_W * THERMAL_H {
        let v = ((raw[i] - min) as u32 * scale) >> 8;
        gray[i] = v.min(255) as u8;
    }

    ThermalFrame { gray }
}

fn u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}
