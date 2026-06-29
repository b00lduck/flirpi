// Classic "iron" thermal colour palette.
// Control points: (index, R, G, B).  Values between points are linearly
// interpolated so the result looks like the palette shipped with FLIR tools.
const CTRL: &[(u8, u8, u8, u8)] = &[
    (0,   0,   0,   0),   // black  (cold)
    (32,  30,   0,  60),  // deep purple
    (64,  90,   0, 200),  // blue-purple
    (96,  180,  0, 240),  // lavender
    (128, 255,  0,  80),  // red
    (160, 255,  80,  0),  // orange-red
    (192, 255, 190,  0),  // amber
    (224, 255, 255,  80), // bright yellow
    (255, 255, 255, 255), // white  (hot)
];

pub fn iron() -> [[u8; 3]; 256] {
    let mut p = [[0u8; 3]; 256];

    for w in CTRL.windows(2) {
        let (i0, r0, g0, b0) = (w[0].0 as usize, w[0].1 as i32, w[0].2 as i32, w[0].3 as i32);
        let (i1, r1, g1, b1) = (w[1].0 as usize, w[1].1 as i32, w[1].2 as i32, w[1].3 as i32);
        let steps = (i1 - i0) as i32;

        for i in i0..i1 {
            let t = (i - i0) as i32;
            p[i][0] = (r0 + (r1 - r0) * t / steps) as u8;
            p[i][1] = (g0 + (g1 - g0) * t / steps) as u8;
            p[i][2] = (b0 + (b1 - b0) * t / steps) as u8;
        }
    }
    // Last entry (index 255) is set by the final control point directly.
    let last = CTRL.last().unwrap();
    p[255] = [last.1, last.2, last.3];

    p
}
