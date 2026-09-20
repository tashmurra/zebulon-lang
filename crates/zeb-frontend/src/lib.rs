#![forbid(unsafe_code)]

pub mod effects;
pub mod flow;
pub mod grammar;
pub mod ir;
pub mod ir_verify;
pub mod lexer;
pub mod llvm;
pub mod object_init;
pub mod parser;
pub mod preinit;
pub mod preprocess;
mod ranges;
pub mod roots;
pub mod sema;
pub mod source;
pub mod specialize;
pub mod string_pool;
pub mod strings;
pub mod tables;
pub mod templates;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: std::borrow::Cow<'static, str>,
    pub byte: usize,
}

impl Diagnostic {
    pub const fn new(code: &'static str, message: &'static str, byte: usize) -> Self {
        Self {
            code,
            message: std::borrow::Cow::Borrowed(message),
            byte,
        }
    }

    pub const fn resource(byte: usize) -> Self {
        Self::new(
            "resource-exhausted",
            "source buffer reservation failed",
            byte,
        )
    }
}
