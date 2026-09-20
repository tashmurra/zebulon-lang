#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    ir::Operation,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn ast(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn object_values_use_existing_functions_slots_and_checked_properties() {
    let tree = ast(include_str!("../../../../tests/native/object-values.t"));
    let checked = flow::check(&tree).unwrap();
    assert!(
        checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(i.operation, Operation::GetProperty(..)))
    );
    let text = llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert!(text.contains("%out = type { i128, i32, i64, i64 }"));
    assert!(text.contains("alloca i128"));
    assert!(text.contains("shl i128"));
    assert_eq!(
        llvm::emit(&tree, Target::MacX86_64).unwrap_err().code,
        "native-profile"
    );
}
#[test]
fn property_receiver_must_allow_object_and_locals_may_shadow_globals() {
    assert_eq!(
        flow::check(&ast("main(){return 1 .count;}"))
            .unwrap_err()
            .code,
        "flow-type"
    );
    // A local or parameter may shadow a global object, as the reference
    // allows, and the local wins inside its scope.
    for text in [
        "a: object; main(a){return a;}",
        "a: object; main(){local a=nil;return a;}",
    ] {
        flow::check(&ast(text)).unwrap();
    }
    // An enclosing local or parameter still may not be shadowed.
    for text in [
        "main(a){local a=nil;return a;}",
        "main(){local a=nil;local a=1;return a;}",
    ] {
        assert_eq!(flow::check(&ast(text)).unwrap_err().code, "sem-shadowing");
    }
}

#[test]
fn property_mutations_are_checked_effects_in_shared_ir() {
    let tree = ast(include_str!("../../../../tests/native/object-mutation.t"));
    let checked = flow::check(&tree).unwrap();
    assert!(
        checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(i.operation, Operation::SetProperty(..))
                && i.failure == zeb_frontend::ir::Failure::Propagate)
    );
    let effects = zeb_frontend::effects::analyze(&tree, &checked.program).unwrap();
    assert!(
        effects[2]
            .effects
            .contains(zeb_frontend::effects::Effect::WriteState)
    );
    assert!(
        llvm::emit_objects(&tree, Target::MacArm64)
            .unwrap()
            .contains("add i32")
    );
}

#[test]
fn methods_share_checked_ir_and_cannot_be_assumed_pure() {
    let tree = ast(include_str!("../../../../tests/native/object-methods.t"));
    let checked = flow::check(&tree).unwrap();
    let summaries = zeb_frontend::effects::analyze(&tree, &checked.program).unwrap();
    for (index, function) in checked.program.functions.iter().enumerate() {
        if function
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| {
                matches!(
                    i.operation,
                    Operation::CallMethod { .. } | Operation::GetProperty(..)
                )
            })
        {
            assert_eq!(
                summaries[index].effects,
                zeb_frontend::effects::Effects::unknown()
            );
        }
    }
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    assert_eq!(
        flow::check(&ast("main(){return 1 .method();}"))
            .unwrap_err()
            .code,
        "flow-type"
    );
    assert!(flow::check(&ast("o: object method() { } ; main(){return o.method();}")).is_ok());
    assert_eq!(
        flow::check(&ast("o: object method(self) { } ; main(){return nil;}"))
            .unwrap_err()
            .code,
        "sem-shadowing"
    );
}

#[test]
fn implicit_self_preserves_locals_and_requires_method_context() {
    let tree = ast(include_str!("../../../../tests/native/implicit-self.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    let analysis = zeb_frontend::sema::analyze(&tree).unwrap();
    assert!(
        analysis
            .bindings
            .iter()
            .any(|b| matches!(b, Some(zeb_frontend::sema::Binding::SelfProperty(..))))
    );
    assert_eq!(
        flow::check(&ast("o: object count=1; main(){return count;}"))
            .unwrap_err()
            .code,
        "sem-name"
    );
    assert_eq!(
        flow::check(&ast(
            "o: object run(){return unknown;} ; main(){return o.run();}"
        ))
        .unwrap_err()
        .code,
        "sem-name"
    );
}

#[test]
fn inherited_calls_require_a_method_and_are_not_destinations() {
    let tree = ast(include_str!("../../../../tests/native/inherited-calls.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    assert_eq!(
        flow::check(&ast("main(){return inherited;}"))
            .unwrap_err()
            .code,
        "sem-context"
    );
    for source in [
        "base: object p=1; child: base f(){ inherited.p = 2; } ;",
        "base: object p=1; child: base f(){ inherited.p++; } ;",
    ] {
        assert_eq!(
            flow::check(&ast(source)).unwrap_err().code,
            "sem-destination"
        );
    }
}

#[test]
fn class_declarations_preserve_metadata_and_use_native_methods() {
    let tree = ast(include_str!(
        "../../../../tests/native/class-declarations.t"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    let flags: Vec<_> = tree
        .objects
        .iter()
        .map(|id| {
            let parser::Syntax::Object { is_class, .. } = tree.nodes[id.0].syntax else {
                panic!("object expected")
            };
            is_class
        })
        .collect();
    assert_eq!(flags, [true, true, false, false, true, false]);
    assert!(
        parser::parse_with(
            &Source::decode(b"class f(){return nil;}".to_vec(), Encoding::Utf8).unwrap(),
            parser::Model::Ownership
        )
        .is_err()
    );
}

#[test]
fn static_initializers_have_native_guards_and_a_startup_root() {
    let tree = ast(include_str!(
        "../../../../tests/native/static-initializers.t"
    ));
    let checked = flow::check(&tree).unwrap();
    assert!(
        checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(i.operation, Operation::BeginStatic(..))
                && i.failure == zeb_frontend::ir::Failure::Propagate)
    );
    let module = llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    assert!(module.contains("define %out @zeb_run_static_initializers()"));
}

#[test]
fn static_string_intrinsics_preserve_binding_and_owned_result_types() {
    let tree = ast(include_str!("../../../../tests/native/static-strings.t"));
    let checked = flow::check(&tree).unwrap();
    assert!(
        checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(i.operation, Operation::Builtin { .. }))
    );
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    let author = ast(
        "toString(value) { return value; } item: object value = static toString(42); main() {return item.value;}",
    );
    let checked = flow::check(&author).unwrap();
    assert!(
        !checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(i.operation, Operation::Builtin { .. }))
    );
    let method = ast(
        "item: object substr(n) {return n;} value = static item.substr(42); main() {return item.value;}",
    );
    flow::check(&method).unwrap();
    // Text built at run time belongs to its scope, so it cannot be returned
    // directly; an owning function moves an owned local instead.
    let escaping = ast("main() { return toString(42); }");
    assert_eq!(flow::check(&escaping).unwrap_err().code, "sem-owner");
    let owning = ast(
        "owned make() { local owned s = toString(42); return move s; }          main() { local owned t = make(); return t.length(); }",
    );
    flow::check(&owning).unwrap();
}

#[test]
fn owned_static_construction_lowers_activation_dispatch_and_completion() {
    let tree = ast(
        "class C: object x=0 construct(a){x=a;}; holder: object owned child=static new C(42); main(){return holder.child.x;}",
    );
    let checked = flow::check(&tree).unwrap();
    assert!(
        checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(
                i.operation,
                Operation::Builtin {
                    kind: zeb_frontend::sema::Builtin::FinishConstruction,
                    ..
                }
            ))
    );
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    for source in [
        "class C: object; main(){local x=new C();return nil;}",
        "class C: object; h: object owned x=new C();",
        "class C: object; h: object owned x=static 42;",
        "h: object owned x=static new Unknown(); main(){return nil;}",
    ] {
        let parsed = parser::parse_with(
            &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            parser::Model::Ownership,
        );
        assert!(
            parsed.is_err() || flow::check(&parsed.unwrap()).is_err(),
            "{source}"
        );
    }
}

#[test]
fn class_enumeration_intrinsics_have_object_or_nil_results() {
    let tree = ast(
        "intrinsic 'tads-gen/030008' {firstObj(cls?,flags?);nextObj(obj,cls?,flags?);} class C: object; main(){local cur=firstObj(C,2);if(cur!=nil)cur=nextObj(cur,C,2);return cur;}",
    );
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
}

#[test]
fn local_owned_construction_accepts_a_resolved_prototype_binding() {
    let tree = ast(
        "class C: object construct(n){value=n;} value=0; owned make(selected,args){local owned result=new selected(args...);return move result;} inputs: object args=[7]; main(){local owned result=make(C,inputs.args);return result.value;}",
    );
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    let unknown = ast("main(){local owned result=new Unknown();return nil;}");
    assert!(flow::check(&unknown).is_err());
}

#[test]
fn published_construction_has_explicit_reservation_and_no_implicit_local_owner() {
    let tree = ast(
        "queue: object slot=nil; class C: object construct(){self.reserveConstruction(queue,&slot);}; main(){new published C();return nil;}",
    );
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacArm64).unwrap();
    assert!(
        flow::check(&ast(
            "class C: object; main(){new published Unknown();return nil;}"
        ))
        .is_err()
    );
    assert!(
        flow::check(&ast(
            "a: object; main(){a.reserveConstruction(a);return nil;}"
        ))
        .is_err()
    );
}
