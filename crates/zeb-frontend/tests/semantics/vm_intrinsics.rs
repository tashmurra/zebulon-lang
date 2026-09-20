#![forbid(unsafe_code)]
use zeb_frontend::{
    flow, llvm, parser,
    source::{Encoding, Source},
};
fn parse(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
const T3VM: &str = "intrinsic 't3vm/010006' { t3RunGC(); t3GetVMPreinitMode(); \
                    t3DebugTrace(mode, ...); t3GetGlobalSymbols(which?); \
                    t3GetStackTrace(level?, flags?); } ";

/// an ahead-of-time build has no preinit mode, collector or symbol
/// table, so those t3vm functions are nil; a VM stack trace has no stand-in.
#[test]
fn stateless_t3vm_functions_compile_to_nil() {
    let tree = parse(&format!(
        "{T3VM} main(){{ t3RunGC(); t3DebugTrace(1); \
         return t3GetVMPreinitMode() == nil && t3GetGlobalSymbols() == nil; }}"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
}

#[test]
fn stack_traces_are_rejected() {
    let tree = parse(&format!("{T3VM} main(){{ return t3GetStackTrace(); }}"));
    assert_eq!(
        flow::check(&tree).unwrap_err().code,
        "sem-intrinsic-unavailable"
    );
}

/// `&name` is a function pointer when the name is a function, a property
/// address otherwise; the reference compiler forbids a symbol being both.
#[test]
fn address_of_a_function_is_a_function_value() {
    let tree = parse(
        "greet(n) { return n * 2; } holder: object tag = 3; \
         main(){ local f = &greet; local p = &tag; return f(21) + holder.(p); }",
    );
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
    let tree = parse("main(){ return &missingThing; }");
    assert_eq!(flow::check(&tree).unwrap_err().code, "sem-property");
}
