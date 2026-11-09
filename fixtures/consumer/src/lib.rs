//! A shader importing russl modules from another crate.

pub mod badge {
    russl::russl! {
        use russl_fixture_shapes::sdf::{sd_circle, sd_ring};

        #[fragment]
        #[location(0)]
        fn fs(#[builtin(position)] p: Vec4) -> Vec4 {
            let q = p.xy * 0.01 - 0.5;
            Vec4::splat(sd_ring(q, 0.3, 0.02).min(sd_circle(q, 0.05)))
        }
    }
}
