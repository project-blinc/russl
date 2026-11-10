// One file, included under two Rust modules: each imports the `lib` beside it.
russl::russl! {
    use super::lib::strength;

    #[fragment]
    #[location(0)]
    fn fs() -> Vec4 {
        Vec4::splat(strength())
    }
}
