mod card {
    russl::russl! {
        #[fragment]
        #[location(0)]
        fn fs(#[location(0)] local: Vec2) -> Vec4 {
            let d = local.length() - 0.5;
            Vec4::new(local, d)
        }
    }
}

fn main() {}
