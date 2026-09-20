use zeb_frontend::{
    flow, llvm,
    parser::{self, Declaration, Syntax},
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
fn contracts_preserve_optional_variadic_static_and_export_metadata() {
    let ast = parse(
        "intrinsic class Base 'base/1' { query(value?); static create(...); } intrinsic class Derived 'derived/1': Base { call(first, ...); } property p, q; export p 'external.p'; main(){return nil;}",
    );
    let Syntax::Declaration(Declaration::Intrinsic {
        class, signatures, ..
    }) = &ast.nodes[0].syntax
    else {
        panic!()
    };
    assert_eq!(class.as_deref(), Some("Base"));
    assert!(signatures[0].parameters[0].1);
    assert!(signatures[1].is_static && signatures[1].variadic);
    let Syntax::Declaration(Declaration::Intrinsic { parent, .. }) = &ast.nodes[1].syntax else {
        panic!()
    };
    assert_eq!(parent.as_deref(), Some("Base"));
    let Syntax::Declaration(Declaration::Export { external, .. }) = &ast.nodes[3].syntax else {
        panic!()
    };
    assert_eq!(external, "external.p");
    flow::check(&ast).unwrap();
}
#[test]
fn datatype_lowers_through_native_object_profile() {
    let ast =
        parse("intrinsic 'tads-gen/030008' { dataType(val); } main(){return dataType('hello');}");
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
}
#[test]
fn unsupported_operations_versions_signatures_and_collisions_do_not_compile() {
    for (source, code) in [
        (
            "intrinsic 'tads-gen/030008' { rand(...); } main(){return rand(9);}",
            "sem-intrinsic-unavailable",
        ),
        (
            "intrinsic 'other/1' { dataType(val); } main(){return dataType(9);}",
            "sem-intrinsic-unavailable",
        ),
        (
            "intrinsic 'tads-gen/030008' { dataType(val?); } main(){return dataType(9);}",
            "sem-intrinsic",
        ),
        (
            "intrinsic 'tads-gen/030008' { dataType(val); } main(){return dataType();}",
            "sem-arity",
        ),
        (
            "intrinsic 'tads-gen/030008' { dataType(val); } dataType(v){return 0;} main(){return dataType(9);}",
            "sem-intrinsic",
        ),
    ] {
        assert_eq!(
            flow::check(&parse(source)).unwrap_err().code,
            code,
            "{source}"
        );
    }
}
#[test]
fn malformed_signatures_are_rejected() {
    for text in [
        "intrinsic 'x' { f(a?, b); }",
        "intrinsic 'x' { f(a,a); }",
        "intrinsic 'x' { f(); f(); }",
        "intrinsic 'x' { static f(); }",
        "intrinsic 'x' { f(..., a); }",
    ] {
        assert!(
            parser::parse_with(
                &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
                parser::Model::Ownership
            )
            .is_err(),
            "{text}"
        );
    }
}

#[test]
fn dictionary_declarations_allocate_the_dictionary_runtime_kind() {
    let ast = parse("dictionary cmdDict; main(){return dataType(cmdDict);}");
    let Syntax::Object { is_dictionary, .. } = ast.nodes[ast.objects[0].0].syntax else {
        panic!()
    };
    assert!(is_dictionary);
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
}

#[test]
fn vocabulary_declarations_lower_lists_and_registration() {
    let ast = parse(
        "dictionary cmdDict; dictionary property noun, adjective; item: object noun = 'lamp'; main(){return nil;}",
    );
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
}

#[test]
fn property_addresses_are_distinct_values_and_resolve_forward_definitions() {
    let ast = parse(
        "property noun; entry: object category = &other; later: object other = 1; main(){ local p = &noun; entry.category = p; return dataType(entry.category) + 1; }",
    );
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
    let ast = parse("property noun; main(){return &noun + 1;}");
    assert!(flow::check(&ast).is_err());
    assert!(flow::check(&parse("main(){return &missing;}")).is_err());
    assert!(flow::check(&parse("main(){local x=1; return &x;}")).is_err());
    assert!(flow::check(&parse("property p; main(){&p = nil;}")).is_err());
}

#[test]
fn indirect_properties_lower_reads_writes_and_method_calls() {
    let ast = parse(
        "property p; a: object p = 1 calc(v){return v;}; main(){ local key=&p; a.(key)=3; a.(key)+=4; return a.(&calc)(a.(key)); }",
    );
    let checked = flow::check(&ast).unwrap();
    assert!(
        checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| i.operation.computed_property().is_some())
    );
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
    for source in [
        "a: object; main(){return a.(1);}",
        "property p; main(){return nil.(&p);}",
        "property p; a: object; main(){local key; return a.(key);}",
        "property p; a: object; main(){return a.(&p)++;}",
    ] {
        assert!(flow::check(&parse(source)).is_err(), "{source}");
    }
}

#[test]
fn vocabulary_switching_inheritance_and_explicit_limits() {
    let ast = parse(
        "dictionary d; dictionary property noun; class Base: object noun='base'; dictionary other; a: Base noun='child'; dictionary d; b: Base; main(){return a.noun.length();}",
    );
    assert_eq!(ast.objects.len(), 5);
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
    for source in [
        "dictionary property noun; a: object noun='lamp';",
        "dictionary d; dictionary property noun; class A: object noun='a'; class B: object; c: A,B;",
        "dictionary d; dictionary property noun; a: object noun=['a'];",
        "dictionary d; dictionary property noun; a: object noun(){return nil;};",
    ] {
        let result = parser::parse_with(
            &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            parser::Model::Ownership,
        );
        assert!(result.is_err(), "{source}");
    }
}

#[test]
fn enumerators_preserve_token_metadata_and_have_distinct_native_values() {
    let ast = parse(
        "enum colour; enum token wordToken; a: object p=colour values=static [wordToken]; main(){local v=colour; a.p=wordToken; return dataType(v);}",
    );
    assert!(ast.nodes.iter().any(|n| matches!(&n.syntax, Syntax::Declaration(Declaration::Enumerators { token: true, names, .. }) if names == &["wordToken"])));
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, llvm::Target::MacArm64).unwrap();
    for source in [
        "enum a,a; main(){return nil;}",
        "enum a; a: object; main(){return nil;}",
        "enum a; property a; main(){return nil;}",
        "enum a; a(){return nil;} main(){return nil;}",
        "enum a; b: object a=1; main(){return nil;}",
        "enum a; main(){return a+1;}",
        "enum a; main(){return a();}",
        "enum a; main(){a=nil;return nil;}",
        "enum a; main(){return &a;}",
    ] {
        assert!(flow::check(&parse(source)).is_err(), "{source}");
    }
}
