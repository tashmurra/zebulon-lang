#![forbid(unsafe_code)]
//! under lifetimes a growable collection needs no owning local, and a
//! list derived from a query is written as a comprehension. Expectations here
//! are self-authored: this diverges from TADS deliberately, so there is no
//! external oracle for the forms themselves.
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser::{self, Ast, Model},
    source::{Encoding, Source},
};

fn tree(text: &str, model: Model) -> Result<Ast, zeb_frontend::Diagnostic> {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        model,
    )
}

fn compiles(text: &str) {
    let ast = tree(text, Model::Lifetimes).unwrap_or_else(|e| panic!("{}: {text}", e.message));
    flow::check(&ast).unwrap_or_else(|e| panic!("{}: {text}", e.message));
    llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
}

/// The compiler's answer for `text`, whether it comes from parsing or flow.
fn refusal(text: &str, model: Model) -> String {
    match tree(text, model) {
        Err(diagnostic) => diagnostic.code.to_owned(),
        Ok(ast) => flow::check(&ast)
            .err()
            .unwrap_or_else(|| panic!("expected a refusal: {text}"))
            .code
            .to_owned(),
    }
}

#[test]
fn a_plain_local_reaches_the_intrinsic_collections() {
    compiles("main(){local v = new Vector(4); v.append(1); return v.length();}");
    compiles("main(){local t = new LookupTable(8, 8); t['k'] = 1; return t['k'];}");
    compiles("main(){local t = new LookupTable(); return t;}");
    compiles("main(){local b = new StringBuffer(); return b;}");
    compiles("main(){local c = new OwnedCollection(); return c;}");
}

#[test]
fn the_ownership_model_cannot_reach_them_from_a_plain_local() {
    // Without lifetimes a bare `new` has no ownership form at all, so every one
    // of these forms is refused where it would be accepted above.
    for text in [
        "main(){local v = new Vector(4); return v;}",
        "main(){local t = new LookupTable(8, 8); return t;}",
        "main(){local b = new StringBuffer(); return b;}",
    ] {
        assert_eq!(refusal(text, Model::Ownership), "parse-expected", "{text}");
    }
    // The lookup literal keeps its owning-local rule under ownership.
    assert_eq!(
        refusal("main(){local t = [1 -> 2]; return t;}", Model::Ownership),
        "sem-owner"
    );
}

#[test]
fn a_comprehension_builds_a_list_from_a_query() {
    compiles("main(){local xs = [1, 2, 3]; local ys = [for n in xs : n * 2]; return ys[1];}");
    compiles(
        "c: object name = 'c'; main(){local xs = [c]; local ns = [for i in xs : i.name]; return ns[1];}",
    );
}

#[test]
fn a_comprehension_is_a_lifetimes_form_and_needs_its_three_parts() {
    // Under ownership `[for...]` is not a form at all; the list literal parser
    // meets a keyword where it wants an element.
    assert_eq!(
        refusal(
            "main(){local xs = [1]; return [for n in xs : n];}",
            Model::Ownership
        ),
        "parse-expected"
    );
    for text in [
        "main(){local xs = [1]; return [for n xs : n];}",
        "main(){local xs = [1]; return [for n in xs n];}",
    ] {
        assert_eq!(refusal(text, Model::Lifetimes), "parse-expected", "{text}");
    }
}
