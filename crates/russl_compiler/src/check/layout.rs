//! WGSL's layout rules for the uniform address space, which are stricter than storage: array
//! elements sit 16 bytes apart, a struct member starts on a 16-byte boundary, and the member
//! after a struct leaves that struct's size rounded up to 16. A breach is reported at the field.

use super::Checker;
use crate::diag::SpanId;
use crate::types::{Type, TypeId, round_up};

impl Checker<'_> {
    /// Check a uniform binding's type; `span` is the binding's, for a type that is not a struct.
    pub(super) fn uniform_layout(&mut self, ty: TypeId, span: SpanId) {
        match self.types.get(ty) {
            Type::Array { elem, .. } => {
                self.array_stride(ty, elem, span);
                self.uniform_layout(elem, span);
            }
            Type::Struct(s) => {
                let fields = self.types.structs[s.0 as usize].fields.clone();
                let spans = self.field_spans(ty);
                let mut end = 0;
                let mut prev_struct: Option<(String, u32, u32)> = None;
                for (i, &(name, t)) in fields.iter().enumerate() {
                    let at = spans.get(i).copied().unwrap_or(span);
                    let name = self.names.get(name).to_string();
                    let (size, align) = self.types.size_align(t);
                    let offset = round_up(align, end);
                    if let Some((before, start, need)) = prev_struct.take()
                        && offset - start < need
                    {
                        self.sink.error(
                            self.spans.get(at),
                            format!("in a uniform buffer the member after the struct `{before}` starts at least {need} bytes after it (its size rounded up to 16), and `{name}` starts {} bytes after: add padding before it", offset - start),
                        );
                    }
                    match self.types.get(t) {
                        Type::Struct(_) => {
                            if !offset.is_multiple_of(16) {
                                self.sink.error(
                                    self.spans.get(at),
                                    format!("in a uniform buffer the struct member `{name}` starts on a 16-byte boundary, and here it starts at byte {offset}: move it, or add padding before it"),
                                );
                            }
                            prev_struct = Some((name, offset, round_up(16, size)));
                            self.uniform_layout(t, at);
                        }
                        Type::Array { elem, .. } => {
                            self.array_stride(t, elem, at);
                            self.uniform_layout(elem, at);
                        }
                        _ => {}
                    }
                    end = offset + size;
                }
            }
            _ => {}
        }
    }

    fn array_stride(&mut self, array: TypeId, elem: TypeId, at: SpanId) {
        let (size, align) = self.types.size_align(elem);
        let stride = round_up(align, size);
        if !stride.is_multiple_of(16) {
            let (a, e) = (self.ty_name(array), self.ty_name(elem));
            self.sink.error(
                self.spans.get(at),
                format!("a uniform buffer places array elements 16 bytes apart, and the `{e}` elements of `{a}` are {stride}: pack them into `Vec4`s, or use a storage buffer"),
            );
        }
    }

    /// The span of each field of the struct `ty`, in order.
    fn field_spans(&self, ty: TypeId) -> Vec<SpanId> {
        self.module
            .structs
            .iter()
            .find(|s| s.ty == ty)
            .map(|s| s.fields.iter().map(|f| f.span).collect())
            .unwrap_or_default()
    }
}
