//! The resolved IR. Names are resolved to ids, every expression has a type once the analyzer
//! has run, and each node keeps the span of the source it came from.

use crate::diag::SpanId;
use crate::intern::Symbol;
use crate::types::TypeId;

macro_rules! id {
    ($name:ident) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id!(ExprId);
id!(LocalId);
id!(FnId);
id!(ConstId);
id!(GlobalId);

#[derive(Default)]
pub struct Module {
    pub structs: Vec<Struct>,
    pub consts: Vec<Const>,
    pub globals: Vec<Global>,
    pub functions: Vec<Function>,
}

pub struct Struct {
    /// Index of the source module: 0 is the module being compiled.
    pub module: u32,
    pub name: Symbol,
    pub ty: TypeId,
    pub fields: Vec<Field>,
    pub span: SpanId,
}

pub struct Field {
    pub name: Symbol,
    pub ty: TypeId,
    pub io: Option<Io>,
    pub span: SpanId,
}

/// How an entry point's input or output is bound.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Io {
    Location { location: u32, flat: bool },
    Builtin(Builtin),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Builtin {
    Position { invariant: bool },
    VertexIndex,
    InstanceIndex,
    FrontFacing,
    FragDepth,
    SampleIndex,
    SampleMask,
    LocalInvocationId,
    LocalInvocationIndex,
    GlobalInvocationId,
    WorkgroupId,
    NumWorkgroups,
}

pub struct Const {
    /// Index of the source module: 0 is the module being compiled.
    pub module: u32,
    pub name: Symbol,
    pub ty: TypeId,
    /// `None` for a pipeline constant with no default.
    pub value: Option<Body>,
    /// `Some` for a pipeline constant, with its `#[pipeline(id)]` if given.
    pub pipeline: Option<Option<u16>>,
    pub span: SpanId,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Space {
    Uniform,
    Storage {
        writable: bool,
    },
    /// Textures and samplers.
    Handle,
    Private,
    Workgroup,
}

pub struct Global {
    /// Index of the source module: 0 is the module being compiled.
    pub module: u32,
    pub name: Symbol,
    pub space: Space,
    pub ty: TypeId,
    pub binding: Option<(u32, u32)>,
    pub init: Option<Body>,
    pub span: SpanId,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute([u32; 3]),
}

pub struct Function {
    /// Index of the source module: 0 is the module being compiled.
    pub module: u32,
    pub name: Symbol,
    /// A generic function: the index of its template, which the front end instantiates. The
    /// function itself has no body and is never checked or lowered.
    pub template: Option<u32>,
    /// An instance of a generic function, as it reads in messages: `lerp::<Vec3>`.
    pub instance: Option<String>,
    pub stage: Option<Stage>,
    pub params: Vec<Param>,
    pub ret: Option<(TypeId, Option<Io>)>,
    pub ret_span: SpanId,
    pub body: Body,
    pub span: SpanId,
}

pub struct Param {
    pub local: LocalId,
    pub io: Option<Io>,
}

/// A function body, or a constant's initializer.
#[derive(Default)]
pub struct Body {
    pub exprs: Vec<Expr>,
    pub locals: Vec<Local>,
    pub block: Block,
    /// The expression a constant's initializer evaluates to.
    pub value: Option<ExprId>,
}

impl Body {
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.index()]
    }
}

pub struct Local {
    pub name: Symbol,
    pub ty: TypeId,
    pub mutable: bool,
    pub is_param: bool,
    pub used: bool,
    pub span: SpanId,
}

pub type Block = Vec<Stmt>;

pub struct Stmt {
    pub kind: StmtKind,
    pub span: SpanId,
}

pub enum StmtKind {
    Let {
        local: LocalId,
        init: Option<ExprId>,
    },
    Assign {
        target: ExprId,
        op: Option<BinOp>,
        value: ExprId,
    },
    Expr(ExprId),
    If {
        cond: ExprId,
        then: Block,
        otherwise: Option<Block>,
    },
    While {
        cond: ExprId,
        body: Block,
    },
    Loop {
        body: Block,
    },
    For {
        var: LocalId,
        start: ExprId,
        end: ExprId,
        inclusive: bool,
        body: Block,
    },
    Switch {
        selector: ExprId,
        cases: Vec<(Option<Vec<ExprId>>, Block)>,
    },
    Break,
    Continue,
    Return(Option<ExprId>),
    /// `discard!()`: the fragment writes nothing; fragment shaders only.
    Discard,
    Block(Block),
}

pub struct Expr {
    pub kind: ExprKind,
    pub ty: TypeId,
    pub span: SpanId,
}

/// A generic function's argument: a type, or a const parameter's value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GenericArg {
    Type(TypeId),
    Const(i128),
}

#[derive(Clone, Debug)]
pub enum Lit {
    Int(i128),
    Float(f64),
    Bool(bool),
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Lit(Lit),
    Local(LocalId),
    Const(ConstId),
    Global(GlobalId),
    Unary(UnOp, ExprId),
    Binary(BinOp, ExprId, ExprId),
    Call(FnId, Vec<ExprId>),
    /// A call naming a generic function's arguments: `blur::<4>(x)`. The analyzer turns it,
    /// like a `Call` of a generic function, into a call of the instance.
    GenericCall(FnId, Vec<GenericArg>, Vec<ExprId>),
    Builtin(crate::builtins::Builtin, Vec<ExprId>),
    /// A vector, matrix, array or struct built from parts; the type says which.
    Construct(Vec<ExprId>),
    /// A vector with one value in every component.
    Splat(ExprId),
    Zero,
    /// `base.name` before the analyzer knows whether it is a struct field or a swizzle.
    Member(ExprId, Symbol),
    Field(ExprId, u32),
    Swizzle(ExprId, Vec<u8>),
    Index(ExprId, ExprId),
    Cast(ExprId),
    Bitcast(ExprId),
    Select {
        cond: ExprId,
        then: ExprId,
        otherwise: ExprId,
    },
    /// Already reported; lowering never sees it.
    Error,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}
