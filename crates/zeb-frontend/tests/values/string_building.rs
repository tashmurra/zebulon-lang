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
const GEN: &str = "intrinsic 'tads-gen/030008' { toString(val, radix?, isSigned?); } ";

/// text built at run time belongs to the calling scope, so `+` may
/// join text when it initializes an owned local, including nested additions.
#[test]
fn runtime_text_building_lowers() {
    for text in [
        "main(){ local owned s = 'a' + 'b'; return s.length(); }",
        "main(){ local owned s = 'a' + toString(1) + 'c'; return s.length(); }",
        "main(){ local owned s = toString(42); local owned p = s.substr(1, 1); return p.length(); }",
        "main(){ local n = 2 + 3; return n; }",
    ] {
        let tree = parse(&format!("{GEN}{text}"));
        flow::check(&tree).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        llvm::emit_objects(&tree, llvm::Target::MacX86_64)
            .unwrap_or_else(|error| panic!("{text}: {error:?}"));
    }
}

#[test]
fn text_leaves_its_scope_only_through_an_owning_return() {
    let escaping = parse(&format!("{GEN}main(){{ return toString(42); }}"));
    assert_eq!(flow::check(&escaping).unwrap_err().code, "sem-owner");
    let owning = parse(&format!(
        "{GEN}owned make(){{ local owned s = toString(42); return move s; }} \
         main(){{ local owned t = make(); return t.length(); }}"
    ));
    flow::check(&owning).unwrap();
    // joining text still needs an owning initializer
    let borrowed = parse(&format!(
        "{GEN}main(){{ local s = 'a' + 'b'; return s.length(); }}"
    ));
    assert_eq!(flow::check(&borrowed).unwrap_err().code, "flow-type");
}

/// `propDefined(prop, mode)` answers directly, by inheritance, or
/// names the defining object; a bare call runs on `self`.
#[test]
fn property_definition_queries_lower() {
    let tree = parse(
        "class Base: object shared = 1; item: Base own = 2 \
         check() { return propDefined(&own, 2); }; \
         main(){ return item.propDefined(&shared, 4) == Base && item.check() ? 1 : 0; }",
    );
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
    let extra = parse("item: object p = 1; main(){ return item.propDefined(&p, 1, 2); }");
    assert_eq!(flow::check(&extra).unwrap_err().code, "sem-arity");
}

/// `+` joins two lists into a new list owned by the calling scope.
#[test]
fn list_joining_lowers() {
    let tree = parse("main(){ local a = [1]; local b = [2]; local owned j = a + b; return j[2]; }");
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
    let borrowed = parse("main(){ local a = [1]; local b = [2]; local j = a + b; return j[1]; }");
    assert_eq!(flow::check(&borrowed).unwrap_err().code, "flow-type");
}
