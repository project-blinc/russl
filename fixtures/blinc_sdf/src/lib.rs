//! Blinc's SDF core shader written in russl. `sdf_core` keeps every feature; `simple` is the same
//! shader with the blocks a plain solid shape never uses switched off at build time.

pub mod clip;
pub mod common;
pub mod sdf_core;

pub mod simple {
    russl::russl! {
        #![specialize(super::sdf_core)]
        const FEAT_GRADIENT: bool = false;
        const FEAT_MASK: bool = false;
        const FEAT_FILTER: bool = false;
        const FEAT_SHADOW: bool = false;
        const FEAT_3D: bool = false;
        const FEAT_CLIP_SHAPE: bool = false;
    }
}
