//! Builtin functions: their Rust spellings, typing rules and the naga function each lowers to.

use naga::MathFunction as M;

/// A builtin the analyzer resolved a call or method to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Builtin {
    Math(M),
    Any,
    All,
}

/// How a builtin's argument types determine its result.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// Float scalars or vectors, all one type; the result is that type.
    Float,
    /// Numeric scalars or vectors, all one type; the result is that type.
    Numeric,
    /// Signed numbers or floats, one type.
    Signed,
    /// Float vectors, one type; the result is that type.
    FloatVector,
    /// Two vectors of one type; the result is their scalar.
    Dot,
    /// Two three-component float vectors.
    Cross,
    /// A float scalar or vector; the result is its scalar.
    Length,
    /// Two floats of one type; the result is their scalar.
    Distance,
    /// Two floats of one type and a weight of that type or its scalar.
    Mix,
    /// Two float vectors and a ratio of their scalar.
    Refract,
    /// A matrix; the result has rows and columns swapped.
    Transpose,
    /// A square matrix; the result is its scalar.
    Determinant,
    /// A bool or bool vector; the result is bool.
    Reduce,
}

/// One builtin: its method name (receiver first) and free-function name.
pub struct Def {
    pub method: &'static str,
    pub function: &'static str,
    pub builtin: Builtin,
    pub rule: Rule,
    /// Argument count, receiver included.
    pub arity: usize,
    /// For methods whose receiver is the last argument of the builtin, as in `x.step(edge)`.
    pub receiver_last: bool,
}

const fn def(
    method: &'static str,
    function: &'static str,
    fun: M,
    rule: Rule,
    arity: usize,
) -> Def {
    Def {
        method,
        function,
        builtin: Builtin::Math(fun),
        rule,
        arity,
        receiver_last: false,
    }
}

/// Every builtin. Method names follow Rust's `f32` and glam where they exist; free functions
/// follow WGSL.
pub const DEFS: &[Def] = &[
    def("abs", "abs", M::Abs, Rule::Numeric, 1),
    def("min", "min", M::Min, Rule::Numeric, 2),
    def("max", "max", M::Max, Rule::Numeric, 2),
    def("clamp", "clamp", M::Clamp, Rule::Numeric, 3),
    def("signum", "sign", M::Sign, Rule::Signed, 1),
    def("saturate", "saturate", M::Saturate, Rule::Float, 1),
    def("sqrt", "sqrt", M::Sqrt, Rule::Float, 1),
    def(
        "inverse_sqrt",
        "inverseSqrt",
        M::InverseSqrt,
        Rule::Float,
        1,
    ),
    def("sin", "sin", M::Sin, Rule::Float, 1),
    def("cos", "cos", M::Cos, Rule::Float, 1),
    def("tan", "tan", M::Tan, Rule::Float, 1),
    def("asin", "asin", M::Asin, Rule::Float, 1),
    def("acos", "acos", M::Acos, Rule::Float, 1),
    def("atan", "atan", M::Atan, Rule::Float, 1),
    def("atan2", "atan2", M::Atan2, Rule::Float, 2),
    def("sinh", "sinh", M::Sinh, Rule::Float, 1),
    def("cosh", "cosh", M::Cosh, Rule::Float, 1),
    def("tanh", "tanh", M::Tanh, Rule::Float, 1),
    def("exp", "exp", M::Exp, Rule::Float, 1),
    def("exp2", "exp2", M::Exp2, Rule::Float, 1),
    def("ln", "log", M::Log, Rule::Float, 1),
    def("log2", "log2", M::Log2, Rule::Float, 1),
    def("powf", "pow", M::Pow, Rule::Float, 2),
    def("floor", "floor", M::Floor, Rule::Float, 1),
    def("ceil", "ceil", M::Ceil, Rule::Float, 1),
    def("round", "round", M::Round, Rule::Float, 1),
    def("trunc", "trunc", M::Trunc, Rule::Float, 1),
    def("fract", "fract", M::Fract, Rule::Float, 1),
    def("mul_add", "fma", M::Fma, Rule::Float, 3),
    def("to_radians", "radians", M::Radians, Rule::Float, 1),
    def("to_degrees", "degrees", M::Degrees, Rule::Float, 1),
    def("dot", "dot", M::Dot, Rule::Dot, 2),
    def("cross", "cross", M::Cross, Rule::Cross, 2),
    def("length", "length", M::Length, Rule::Length, 1),
    def("distance", "distance", M::Distance, Rule::Distance, 2),
    def("normalize", "normalize", M::Normalize, Rule::FloatVector, 1),
    def("lerp", "mix", M::Mix, Rule::Mix, 3),
    def("reflect", "reflect", M::Reflect, Rule::FloatVector, 2),
    def("refract", "refract", M::Refract, Rule::Refract, 3),
    def("transpose", "transpose", M::Transpose, Rule::Transpose, 1),
    def(
        "determinant",
        "determinant",
        M::Determinant,
        Rule::Determinant,
        1,
    ),
    Def {
        method: "step",
        function: "step",
        builtin: Builtin::Math(M::Step),
        rule: Rule::Float,
        arity: 2,
        receiver_last: true,
    },
    Def {
        method: "smoothstep",
        function: "smoothstep",
        builtin: Builtin::Math(M::SmoothStep),
        rule: Rule::Float,
        arity: 3,
        receiver_last: true,
    },
    Def {
        method: "any",
        function: "any",
        builtin: Builtin::Any,
        rule: Rule::Reduce,
        arity: 1,
        receiver_last: false,
    },
    Def {
        method: "all",
        function: "all",
        builtin: Builtin::All,
        rule: Rule::Reduce,
        arity: 1,
        receiver_last: false,
    },
];

pub fn method(name: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.method == name)
}

pub fn function(name: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.function == name)
}
