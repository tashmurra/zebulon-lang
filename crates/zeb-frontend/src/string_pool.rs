//! Immutable literal metadata shared by native emission and runtime data generation.
use crate::{
    Diagnostic,
    parser::{Ast, Syntax},
};
use std::collections::HashMap;

pub struct Pool<'a> {
    pub text: Vec<&'a str>,
    pub ids: Vec<Option<u32>>,
}

pub fn collect(ast: &Ast) -> Result<Pool<'_>, Diagnostic> {
    let mut pool = Pool {
        text: Vec::new(),
        ids: Vec::new(),
    };
    pool.ids
        .try_reserve(ast.nodes.len())
        .map_err(|_| Diagnostic::resource(0))?;
    pool.ids.resize(ast.nodes.len(), None);
    let mut existing = HashMap::new();
    for (index, node) in ast.nodes.iter().enumerate() {
        let Syntax::String(literal) = &node.syntax else {
            continue;
        };
        let id = if let Some(id) = existing.get(literal.text.as_str()) {
            *id
        } else {
            let id =
                u32::try_from(pool.text.len()).map_err(|_| Diagnostic::resource(node.start))?;
            pool.text
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(node.start))?;
            existing
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(node.start))?;
            pool.text.push(literal.text.as_str());
            existing.insert(literal.text.as_str(), id);
            id
        };
        pool.ids[index] = Some(id);
    }
    Ok(pool)
}

/// Generate data only, never author executable code. Rust escaping prevents text
/// from becoming a token, attribute, interpolation or item in generated glue.
pub fn rust_data(ast: &Ast) -> Result<String, Diagnostic> {
    let pool = collect(ast)?;
    let mut out = String::from("pub static LITERALS: &[&str] = &[\n");
    for text in pool.text {
        out.try_reserve(4).map_err(|_| Diagnostic::resource(0))?;
        out.push('"');
        for ch in text.chars().flat_map(char::escape_default) {
            out.try_reserve(ch.len_utf8())
                .map_err(|_| Diagnostic::resource(0))?;
            out.push(ch);
        }
        out.try_reserve(4).map_err(|_| Diagnostic::resource(0))?;
        out.push_str("\",\n");
    }
    out.try_reserve(3).map_err(|_| Diagnostic::resource(0))?;
    out.push_str("];\n");
    Ok(out)
}
