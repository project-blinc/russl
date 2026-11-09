//! Shapes for the cross-crate import test. `sdf` re-exports from a private module, so the
//! consumer reaches `basics` only through `sdf`'s re-export.

mod basics {
    russl::russl! {
        pub fn sd_circle(p: Vec2, r: f32) -> f32 { p.length() - r }
    }
}

pub mod sdf {
    russl::russl! {
        pub use super::basics::sd_circle;
        pub fn sd_ring(p: Vec2, r: f32, w: f32) -> f32 { sd_circle(p, r).abs() - w }
    }
}
