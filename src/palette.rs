/// FLIR Ironbow palette: black → violet → red → orange → yellow → white.
pub fn iron() -> [[u8; 3]; 256] {
    // Control points: (index, R, G, B)
    const CP: [(u8, u8, u8, u8); 7] = [
        (0,   0,   0,   0),
        (42,  30,  0,   200),
        (85,  120, 0,   160),
        (128, 220, 0,   50),
        (170, 255, 100, 0),
        (213, 255, 220, 0),
        (255, 255, 255, 255),
    ];

    let mut p = [[0u8; 3]; 256];
    let mut seg = 0usize;
    for i in 0u8..=255 {
        while seg + 2 < CP.len() && i >= CP[seg + 1].0 {
            seg += 1;
        }
        let (i0, r0, g0, b0) = CP[seg];
        let (i1, r1, g1, b1) = CP[seg + 1];
        let t = (i - i0) as f32 / (i1 - i0) as f32;
        p[i as usize] = [
            lerp(r0, r1, t),
            lerp(g0, g1, t),
            lerp(b0, b1, t),
        ];
    }
    p
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t + 0.5) as u8
}
