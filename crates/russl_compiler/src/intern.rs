//! Interned identifiers: every name in a module is stored once and compared as an integer.

use std::collections::HashMap;

/// An interned identifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(u32);

/// The strings behind [`Symbol`]s.
#[derive(Default, Debug)]
pub struct Interner {
    map: HashMap<Box<str>, Symbol>,
    strings: Vec<Box<str>>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = Symbol(self.strings.len() as u32);
        self.strings.push(s.into());
        self.map.insert(s.into(), sym);
        sym
    }

    pub fn get(&self, sym: Symbol) -> &str {
        &self.strings[sym.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_interns_to_one_symbol() {
        let mut i = Interner::default();
        let a = i.intern("uv");
        assert_eq!(i.intern("uv"), a);
        assert_ne!(i.intern("pos"), a);
        assert_eq!(i.get(a), "uv");
    }
}
