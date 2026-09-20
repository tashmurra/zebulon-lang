#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn ast(source: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn string_methods_compile_with_their_reference_arities() {
    let tree = ast("main(){local s='ab';\
         local t = s.startsWith('a') && s.endsWith('b');\
         local i = s.find('b'); local j = s.find('b', 2);\
         local owned l = s.toLower(); local owned u = s.toUpper();\
         local owned h = s.htmlify(); local owned f = s.htmlify(1);\
         local owned r = s.findReplace('a', 'c');\
         local owned m = s.findReplace(['a'], ['c'], 1);\
         return t && i == j && l != nil && u != nil && h != nil && f != nil\
             && r != nil && m != nil;}");
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn text_results_still_need_an_owned_local_and_correct_arity() {
    for source in [
        // A new string cannot be left unowned.
        "main(){local s='ab'; local t = s.toLower(); return t;}",
        "main(){local s='ab'; return s.startsWith();}",
        "main(){local s='ab'; return s.toLower('x');}",
        "main(){local s='ab'; return s.find();}",
    ] {
        assert!(flow::check(&ast(source)).is_err(), "{source}");
    }
}
#[test]
fn input_key_reads_one_key_as_owned_text() {
    let tree = ast("main(){local owned k = inputKey(); return k.length();}");
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    // It takes no arguments, and its result is text like any other.
    assert!(flow::check(&ast("main(){local owned k = inputKey(1); return k;}")).is_err());
    assert!(flow::check(&ast("main(){local k = inputKey(); return k;}")).is_err());
}
#[test]
fn a_console_session_reads_lines_and_flushes_what_it_has_written() {
    let tree = ast("main(){local turns = 0;\
         for (;;) { \"> \"; flushOutput(); local owned line = inputLine();\
         if (line == nil) break; ++turns; }\
         return turns;}");
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    // Both take no arguments; a line is text like any other result.
    assert!(flow::check(&ast("main(){local owned l = inputLine(1); return l;}")).is_err());
    assert!(flow::check(&ast("main(){local l = inputLine(); return l;}")).is_err());
    assert!(flow::check(&ast("main(){flushOutput(1); return nil;}")).is_err());
}
