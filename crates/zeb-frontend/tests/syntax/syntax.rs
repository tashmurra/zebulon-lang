#![forbid(unsafe_code)]

use zeb_frontend::{
    parser::{Ast, Id, Syntax, parse},
    source::{Encoding, Source},
};

fn ast(text: &str) -> Ast {
    parse(&Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap()).unwrap()
}

fn returned(tree: &Ast) -> Id {
    let Syntax::Function { body, .. } = tree.nodes[tree.functions[0].0].syntax else {
        panic!("function");
    };
    let Syntax::Block(ref children) = tree.nodes[body.0].syntax else {
        panic!("block");
    };
    let Syntax::Return(Some(value)) = tree.nodes[children[0].0].syntax else {
        panic!("return");
    };
    value
}

#[test]
fn precedence_and_right_associative_assignment() {
    let tree = ast("main() { return a = b = 1 + 2 * 3; }");
    let Syntax::Binary("=", _, rhs) = tree.nodes[returned(&tree).0].syntax else {
        panic!("outer assignment");
    };
    let Syntax::Binary("=", _, sum) = tree.nodes[rhs.0].syntax else {
        panic!("right assignment");
    };
    let Syntax::Binary("+", _, product) = tree.nodes[sum.0].syntax else {
        panic!("sum");
    };
    assert!(matches!(
        tree.nodes[product.0].syntax,
        Syntax::Binary("*", _, _)
    ));
    let tree = ast("main() { return 8 - 3 - 2; }");
    let Syntax::Binary("-", left, _) = tree.nodes[returned(&tree).0].syntax else {
        panic!("subtraction");
    };
    assert!(matches!(
        tree.nodes[left.0].syntax,
        Syntax::Binary("-", _, _)
    ));
}

#[test]
fn calls_delimit_commas_and_preserve_grouping() {
    let tree = ast("f(a,b) { return g(a, (b = 1, b + 2)); }");
    let Syntax::Call(_, ref args) = tree.nodes[returned(&tree).0].syntax else {
        panic!("call");
    };
    assert_eq!(args.len(), 2);
    let Syntax::Group(inner) = tree.nodes[args[1].0].syntax else {
        panic!("group");
    };
    assert!(matches!(
        tree.nodes[inner.0].syntax,
        Syntax::Binary(",", _, _)
    ));
    let tree = ast("f() { return -((2147483648)); }");
    let Syntax::Unary("-", group, false) = tree.nodes[returned(&tree).0].syntax else {
        panic!("negation");
    };
    assert!(matches!(tree.nodes[group.0].syntax, Syntax::Group(_)));
}

#[test]
fn dangling_else_and_loop_body_structure() {
    let tree =
        ast("f(a,b) { if (a) if (b) return 1; else return 2; while (true) { break; } return; }");
    let Syntax::Function { body, .. } = tree.nodes[tree.functions[0].0].syntax else {
        panic!();
    };
    let Syntax::Block(ref statements) = tree.nodes[body.0].syntax else {
        panic!();
    };
    let Syntax::If(_, inner, None) = tree.nodes[statements[0].0].syntax else {
        panic!("else attached to outer if");
    };
    assert!(matches!(
        tree.nodes[inner.0].syntax,
        Syntax::If(_, _, Some(_))
    ));
    assert!(matches!(
        tree.nodes[statements[1].0].syntax,
        Syntax::While(_, _)
    ));
}

#[test]
fn conditional_and_local_delimiters() {
    let tree = ast("f(a,b,c) { local x = a ? b : c, y = 2; return a ? b : c ? x : y; }");
    let Syntax::Function { body, .. } = tree.nodes[tree.functions[0].0].syntax else {
        panic!();
    };
    let Syntax::Block(ref statements) = tree.nodes[body.0].syntax else {
        panic!();
    };
    let Syntax::Local(ref declarations) = tree.nodes[statements[0].0].syntax else {
        panic!();
    };
    assert_eq!(declarations.len(), 2);
    let Syntax::Return(Some(result)) = tree.nodes[statements[1].0].syntax else {
        panic!();
    };
    let Syntax::Conditional(_, _, no) = tree.nodes[result.0].syntax else {
        panic!();
    };
    assert!(matches!(
        tree.nodes[no.0].syntax,
        Syntax::Conditional(_, _, _)
    ));
}

#[test]
fn malformed_and_unavailable_input_is_rejected() {
    for text in [
        "f(a,) {}",
        "f(){ return g(1,); }",
        "f(){ return (1; }",
        "f(){ if (true) }",
        "f(){ return 1 +; }",
        "f(){ local x = 1,; }",
        "f(){ return a?b; }",
        "f(){",
    ] {
        let source = Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap();
        assert!(parse(&source).is_err(), "accepted {text}");
    }
    let source = Source::decode(b"replace X;".to_vec(), Encoding::Utf8).unwrap();
    assert_eq!(parse(&source).unwrap_err().code, "parse-unavailable");
    // modify layers over a defined object or class; an unknown target is an error.
    let source = Source::decode(b"modify X;".to_vec(), Encoding::Utf8).unwrap();
    assert_eq!(parse(&source).unwrap_err().code, "parse-expected");
    let source = Source::decode(
        b"base: object p = 1; modify base p = 2; main(){ return base.p; }".to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    parse(&source).unwrap();
    // foreach introduces a scoped iteration binding.
    let source = Source::decode(b"f(){ foreach(x in y) {} }".to_vec(), Encoding::Utf8).unwrap();
    parse(&source).unwrap();
}

#[test]
fn deep_expression_and_block_trees_parse_and_drop_iteratively() {
    let text = "f(){ return ".to_owned() + &"(".repeat(20_000) + "1" + &")".repeat(20_000) + "; }";
    assert!(ast(&text).nodes.len() > 20_000);
    let text = "f()".to_owned() + &"{".repeat(20_000) + "return;" + &"}".repeat(20_000);
    assert!(ast(&text).nodes.len() > 20_000);
    assert!(ast("").functions.is_empty());
}
