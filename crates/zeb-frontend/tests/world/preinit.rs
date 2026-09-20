#![forbid(unsafe_code)]
//! preinit is ordered at build time, not at startup.
//!
//! The spelling is adv3Lite's — `PreinitObject`, `execute`, `execBeforeMe` —
//! so that source written against its tutorials compiles unchanged. The
//! mechanism is not: the order is a property of the declarations, so it is
//! settled while compiling and the emitted program runs a flat sequence.
//!
//! Expectations are self-authored. There is no reference to compare an ordering
//! against, because the reference derives its order at run time.
use zeb_frontend::{
    parser::{self, Model},
    preinit,
    source::{Encoding, Source},
};

const CLASS: &str = "class PreinitObject: object execBeforeMe = [];\n";

fn tree(text: &str) -> Result<parser::Ast, zeb_frontend::Diagnostic> {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        Model::Lifetimes,
    )
}

fn order(text: &str) -> Result<Vec<String>, zeb_frontend::Diagnostic> {
    let ast = tree(text)?;
    preinit::order(&ast.nodes, &ast.objects)
}

/// Declared third, first, second; run first, second, third. The sort has work to
/// do, so passing cannot be an accident of declaration order.
#[test]
fn preinits_run_in_dependency_order_not_declaration_order() {
    let ordered = order(&format!(
        "{CLASS}\
         third: PreinitObject execute() {{ return nil; }} execBeforeMe = [second];\n\
         first: PreinitObject execute() {{ return nil; }}\n;\n\
         second: PreinitObject execute() {{ return nil; }} execBeforeMe = [first];\n"
    ))
    .unwrap();
    assert_eq!(ordered, vec!["first", "second", "third"]);
}

/// adv3Lite subclasses `PreinitObject` freely — `Relation`, `Scenery`,
/// `RuleBook` and 18 others — so an instance of a subclass is still a preinit.
#[test]
fn a_subclass_of_preinit_object_is_still_a_preinit() {
    let ordered = order(&format!(
        "{CLASS}\
         class LibraryPreinit: PreinitObject;\n\
         class DeepPreinit: LibraryPreinit;\n\
         deep: DeepPreinit execute() {{ return nil; }} execBeforeMe = [base];\n\
         base: PreinitObject execute() {{ return nil; }}\n;\n"
    ))
    .unwrap();
    assert_eq!(ordered, vec!["base", "deep"]);
}

/// A class is a declaration, not something to run.
#[test]
fn a_preinit_class_is_not_itself_run() {
    let ordered = order(&format!(
        "{CLASS}class LibraryPreinit: PreinitObject;\none: LibraryPreinit execute() {{ return nil; }}\n;\n"
    ))
    .unwrap();
    assert_eq!(ordered, vec!["one"]);
}

/// The improvement on the thing being copied: a cycle cannot be ordered, and
/// saying so while compiling beats discovering it at every startup.
#[test]
fn a_dependency_cycle_is_refused() {
    let error = order(&format!(
        "{CLASS}\
         a: PreinitObject execute() {{ return nil; }} execBeforeMe = [b];\n\
         b: PreinitObject execute() {{ return nil; }} execBeforeMe = [a];\n"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-preinit");
    assert!(error.message.contains("cycle"), "{error:?}");
}

#[test]
fn a_preinit_that_must_run_before_itself_is_refused() {
    let error = order(&format!(
        "{CLASS}a: PreinitObject execute() {{ return nil; }} execBeforeMe = [a];\n"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-preinit");
}

/// adv3Lite ignores a name no preinit answers to, which turns a typo in a
/// dependency list into an order that is wrong and silent.
#[test]
fn a_name_that_is_not_a_preinit_is_refused() {
    let error = order(&format!(
        "{CLASS}a: PreinitObject execute() {{ return nil; }} execBeforeMe = [nosuch];\n"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-preinit");
    assert!(error.message.contains("not a preinit"), "{error:?}");
}

#[test]
fn exec_before_me_must_be_a_list_of_names() {
    let error = order(&format!(
        "{CLASS}a: PreinitObject execute() {{ return nil; }} execBeforeMe = [1];\n"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-preinit");
}

/// A program that never mentions `PreinitObject` gets no phase at all, so
/// nothing about it changes. Every program predating this decision is one.
#[test]
fn a_program_with_no_preinit_object_has_no_preinit_phase() {
    let ast = tree("thing: object n = 1;\nturn(toks) { return nil; }").unwrap();
    assert!(preinit::order(&ast.nodes, &ast.objects).unwrap().is_empty());
    assert!(
        !ast.functions.iter().any(|id| matches!(
            &ast.nodes[id.0].syntax,
            parser::Syntax::Function { name, .. } if name == "$preinit"
        )),
        "a program with no preinits synthesized a preinit function"
    );
}

/// Declaring the class is not using it: the phase exists only once something
/// derives from it.
#[test]
fn declaring_the_class_alone_is_not_a_preinit() {
    let ast = tree(&format!("{CLASS}turn(toks) {{ return nil; }}")).unwrap();
    assert!(preinit::order(&ast.nodes, &ast.objects).unwrap().is_empty());
}

/// The synthesized body is what the entry runs, so its length is the number of
/// preinits and its order is the sorted one.
#[test]
fn the_synthesized_phase_is_a_flat_sequence() {
    let ast = tree(&format!(
        "{CLASS}\
         b: PreinitObject execute() {{ return nil; }} execBeforeMe = [a];\n\
         a: PreinitObject execute() {{ return nil; }}\n;\n\
         turn(toks) {{ return nil; }}"
    ))
    .unwrap();
    let body = ast
        .functions
        .iter()
        .find_map(|id| match &ast.nodes[id.0].syntax {
            parser::Syntax::Function { name, body, .. } if name == "$preinit" => Some(*body),
            _ => None,
        })
        .expect("no preinit phase was synthesized");
    match &ast.nodes[body.0].syntax {
        // Two calls and the return.
        parser::Syntax::Block(statements) => assert_eq!(statements.len(), 3),
        other => panic!("preinit body is not a block: {other:?}"),
    }
}
