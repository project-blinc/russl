mod fade {
    russl::russl! {
        #[pipeline] const FEAT_FADE: bool = true;

        #[fragment]
        #[location(0)]
        fn fs() -> Vec4 {
            Vec4::splat(if FEAT_FADE { 0.5 } else { 1.0 })
        }
    }
}

mod variant {
    russl::russl! {
        #![specialize(super::fade)]
        const FEAT_FAED: bool = false;
    }
}

fn main() {}
