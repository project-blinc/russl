mod plain {
    pub fn not_a_shader() {}
}

mod user {
    russl::russl! {
        use super::plain::sd_circle;

        #[fragment]
        #[location(0)]
        fn fs() -> Vec4 {
            Vec4::splat(sd_circle(Vec2::ZERO, 1.0))
        }
    }
}

fn main() {}
