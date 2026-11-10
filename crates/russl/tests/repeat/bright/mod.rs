pub mod lib {
    russl::russl! {
        pub fn strength() -> f32 { 0.75 }
    }
}

#[path = "../shader.rs"]
pub mod shader;
