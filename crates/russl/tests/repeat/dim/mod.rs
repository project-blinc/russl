pub mod lib {
    russl::russl! {
        pub fn strength() -> f32 { 0.25 }
    }
}

#[path = "../shader.rs"]
pub mod shader;
