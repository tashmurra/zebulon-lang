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
#[test]
fn static_lists_lower_nested_values_and_borrowed_reads() {
    let ast = parse(
        "a: object values=static [1,[2,3],true,nil]; main(){local v=a.values; return v[2][1] + v.length();}",
    );
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
}
#[test]
fn unsupported_ownership_and_invalid_index_forms_fail() {
    // A list built at run time belongs to its scope and cannot escape it.
    for text in ["main(){local x=1;return [x];}", "main(){return 3[1];}"] {
        assert!(flow::check(&parse(text)).is_err(), "{text}");
    }
    // Bound to an owned local, the same construction is accepted.
    let ast = parse("main(){local x=1;local owned v=[x, x + 1];return v.length();}");
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
    // All-constant literals borrow module-owned storage.
    let ast = parse("a: object; main(){local v=[1,[a,'x'],nil];return v.length();}");
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
}

#[test]
fn dictionary_calls_bind_results_and_keep_ownership_restrictions() {
    let ast = parse(
        "dictionary d; property noun; a: object; q: object result=static (d.addWord(a,'lamp',&noun),d.findWord('lamp',&noun)); main(){d.removeWord(a,'lamp',&noun); return d.isWordDefined('lamp');}",
    );
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
    // A lookup in an ordinary function owns its result list.
    let ast = parse(
        "dictionary d; property noun; a: object; main(){d.addWord(a,'lamp',&noun); local owned m = d.findWord('lamp',&noun); return m.length();}",
    );
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
    for source in [
        "dictionary d; main(){return d.findWord('lamp');}",
        "dictionary d; main(){return d.addWord('lamp');}",
        "dictionary d; main(){return d.isWordDefined('lamp',nil);}",
    ] {
        assert!(flow::check(&parse(source)).is_err(), "{source}");
    }
}

#[test]
fn dynamic_index_kind_is_checked_by_the_runtime() {
    // Properties can contain tables, where nil keys and indexed writes are valid.
    // The runtime still rejects nil list indices and immutable list writes.
    for text in [
        "a: object v=static []; main(){return a.v[nil];}",
        "a: object v=static [1]; main(){a.v[1]=3;return nil;}",
    ] {
        let tree = parse(text);
        flow::check(&tree).unwrap();
        llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
    }
}
