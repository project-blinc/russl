//! A pass over naga's SPIR-V. naga 26 decorates every array and struct type with its buffer
//! layout (`ArrayStride`, `Offset`, `MatrixStride`), and Vulkan allows explicit layout only in
//! the storage classes buffers live in: workgroup, function and private memory may not have it.
//! The decorations come off every type no buffer reaches.

use std::collections::{HashMap, HashSet};

const OP_DECORATE: u32 = 71;
const OP_MEMBER_DECORATE: u32 = 72;
const OP_TYPE_POINTER: u32 = 32;
const OP_TYPE_ARRAY: u32 = 28;
const OP_TYPE_RUNTIME_ARRAY: u32 = 29;
const OP_TYPE_STRUCT: u32 = 30;

/// Storage classes whose types are laid out explicitly.
const EXPLICIT: [u32; 4] = [2, 9, 12, 5349];

const ARRAY_STRIDE: u32 = 6;
const MATRIX_STRIDE: u32 = 7;
const OFFSET: u32 = 35;
const ROW_MAJOR: u32 = 4;
const COL_MAJOR: u32 = 5;

/// The instructions after the five-word header: (start, word count, opcode).
fn instructions(words: &[u32]) -> Vec<(usize, usize, u32)> {
    let mut out = Vec::new();
    let mut i = 5;
    while i < words.len() {
        let count = (words[i] >> 16) as usize;
        if count == 0 || i + count > words.len() {
            break;
        }
        out.push((i, count, words[i] & 0xffff));
        i += count;
    }
    out
}

pub(crate) fn strip_unneeded_layout(words: &[u32]) -> Vec<u32> {
    let ins = instructions(words);
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut types = Vec::new();
    let mut roots_explicit = Vec::new();
    for &(at, count, op) in &ins {
        let w = &words[at..at + count];
        match op {
            OP_TYPE_ARRAY | OP_TYPE_RUNTIME_ARRAY => {
                children.entry(w[1]).or_default().push(w[2]);
                types.push(w[1]);
            }
            OP_TYPE_STRUCT => {
                children.entry(w[1]).or_default().extend(&w[2..]);
                types.push(w[1]);
            }
            OP_TYPE_POINTER if EXPLICIT.contains(&w[2]) => roots_explicit.push(w[3]),
            _ => {}
        }
    }
    let reach = |roots: Vec<u32>| {
        let mut seen = HashSet::new();
        let mut stack = roots;
        while let Some(t) = stack.pop() {
            if seen.insert(t) {
                stack.extend(children.get(&t).into_iter().flatten());
            }
        }
        seen
    };
    let explicit = reach(roots_explicit);
    let strip: HashSet<u32> = types
        .into_iter()
        .filter(|t| !explicit.contains(t))
        .collect();
    if strip.is_empty() {
        return words.to_vec();
    }
    let mut out = words[..5].to_vec();
    for &(at, count, op) in &ins {
        let w = &words[at..at + count];
        let drop = match op {
            OP_DECORATE => strip.contains(&w[1]) && w[2] == ARRAY_STRIDE,
            OP_MEMBER_DECORATE => {
                strip.contains(&w[1])
                    && [OFFSET, MATRIX_STRIDE, ROW_MAJOR, COL_MAJOR].contains(&w[3])
            }
            _ => false,
        };
        if !drop {
            out.extend_from_slice(w);
        }
    }
    out
}
