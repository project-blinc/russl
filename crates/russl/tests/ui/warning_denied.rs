#![deny(deprecated)]

mod shader {
    russl::russl! {
        #[fragment]
        #[location(0)]
        fn fs() -> Vec4 {
            let unused = 1.0;
            Vec4::ZERO
        }
    }
}

fn main() {}
