//! Icon sizes. Phosphor's regular stroke is 16/256 of the glyph, so 16pt draws a 1pt line.

/// Inside 24pt controls (0.875pt stroke); the smallest size we draw.
pub const XS: f32 = 14.0;
/// The default: inline with 13-14pt text, in 32pt controls, nav rows. 1pt stroke.
pub const SM: f32 = 16.0;
/// Standalone and toolbar icons, empty states. 1.25pt stroke.
pub const MD: f32 = 20.0;
