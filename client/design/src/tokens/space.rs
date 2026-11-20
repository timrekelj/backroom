//! Spacing: a 4pt scale. Layout uses these and nothing else (2 exists for hairline gaps only).

pub const S0_5: f32 = 2.0;
pub const S1: f32 = 4.0;
pub const S2: f32 = 8.0;
pub const S3: f32 = 12.0;
pub const S4: f32 = 16.0;
pub const S5: f32 = 20.0;
pub const S6: f32 = 24.0;
pub const S8: f32 = 32.0;
pub const S10: f32 = 40.0;
pub const S12: f32 = 48.0;
pub const S16: f32 = 64.0;

/// Every step, named, for docs and the gallery.
pub const SCALE: [(&str, f32); 11] = [
    ("space-0.5", S0_5),
    ("space-1", S1),
    ("space-2", S2),
    ("space-3", S3),
    ("space-4", S4),
    ("space-5", S5),
    ("space-6", S6),
    ("space-8", S8),
    ("space-10", S10),
    ("space-12", S12),
    ("space-16", S16),
];
