#![forbid(unsafe_code)]
use zeb_frontend::{
    effects::{self, Effect},
    flow,
    ir::Terminator,
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
fn explicit_throw_terminates_flow_and_retains_propagation_checks() {
    let tree = ast("f(value){if(value==nil)return 1;throw value;} main(x){return f(x);}");
    let checked = flow::check(&tree).unwrap();
    assert!(
        checked.program.functions[0]
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Throw(..)))
    );
    let summaries = effects::analyze(&tree, &checked.program).unwrap();
    assert!(summaries.iter().all(|s| s.effects.contains(Effect::Throw)));
    let profiles = zeb_frontend::specialize::plan(&tree, &checked.program, None).unwrap();
    assert!(profiles.iter().all(|profile| profile.facts.is_none()));
    assert!(
        summaries
            .iter()
            .all(|summary| summary.effects.contains(Effect::SourceException))
    );
    let ir = llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert!(ir.contains("i32 9, 1"));
    assert!(ir.contains("callerr"));
    assert_eq!(
        llvm::emit(&tree, Target::MacX86_64).unwrap_err().code,
        "native-profile"
    );
}
#[test]
fn throw_operands_still_require_initialized_bindings() {
    assert_eq!(
        flow::check(&ast("f(){local x;throw x;}")).unwrap_err().code,
        "flow-uninitialized"
    );
    flow::check(&ast("failure:object;f(){throw failure;}")).unwrap();
}

#[test]
fn kind_queries_supply_logical_results_with_checked_arity() {
    let tree = ast(include_str!("../../../../tests/native/of-kind.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert_eq!(
        flow::check(&ast("f(x){return x.ofKind();}"))
            .unwrap_err()
            .code,
        "sem-arity"
    );
    assert_eq!(
        llvm::emit(&ast("f(x,y){return x.ofKind(y);}"), Target::MacX86_64)
            .unwrap_err()
            .code,
        "native-profile"
    );
}

#[test]
fn typed_catches_preserve_scope_and_partial_assignment_state() {
    flow::check(&ast(include_str!("../../../../tests/native/catch.t"))).unwrap();
    for source in [
        "class E:object;fail:E;raise(){throw fail;} f(){local x;try{x=raise();}catch(E e){return x;}return 0;}",
        "class E:object;fail:E;f(){local x;try{throw fail;x=1;}catch(E e){return x;}}",
    ] {
        assert_eq!(
            flow::check(&ast(source)).unwrap_err().code,
            "flow-uninitialized",
            "{source}"
        );
    }
    flow::check(&ast("class E:object;fail:E;raise(){throw fail;}f(){local x=3;try{x=raise();}catch(E e){return x;}return x;}")).unwrap();
    assert_eq!(
        flow::check(&ast(
            "class E:object;f(){try{return 0;}catch(E e){}return e;}"
        ))
        .unwrap_err()
        .code,
        "sem-name"
    );
}

#[test]
fn finally_control_flow_emits_with_pending_transfers() {
    let tree = ast(include_str!("../../../../tests/native/finally.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    let checked = flow::check(&tree).unwrap();
    let mut saves_owner = false;
    let mut restores_owner = false;
    for instruction in checked
        .program
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.instructions)
    {
        if let zeb_frontend::ir::Operation::Builtin { kind, arguments } = &instruction.operation {
            saves_owner |= *kind == zeb_frontend::sema::Builtin::PendingOwner;
            if *kind == zeb_frontend::sema::Builtin::RestorePending {
                assert_eq!(arguments.len(), 4);
                restores_owner = true;
            }
        }
    }
    assert!(saves_owner && restores_owner);
    let emitted = llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert!(emitted.contains("@zeb_objects_call(i32 79,"));
}

#[test]
fn local_vector_owners_lower_through_lexical_cleanup() {
    let tree = ast(include_str!("../../../../tests/native/local-vector.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    for text in [
        "f(){local owned v=new Vector(1);v=nil;return nil;}",
        "f(){local owned v=new Vector(1);return v;}",
        "f(){if(true)local owned v=new Vector(1);return nil;}",
    ] {
        assert_eq!(flow::check(&ast(text)).unwrap_err().code, "sem-owner");
    }
}

#[test]
fn labeled_loops_route_transfers_and_reject_non_enclosing_targets() {
    let tree = ast(include_str!("../../../../tests/native/labeled-loops.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    for text in [
        "f(){while(true){break missing;}return nil;}",
        "f(){done: while(nil){} continue done;return nil;}",
        "f(){same: while(true){same: while(true){break same;}}return nil;}",
        // `continue` needs a loop to go back to; a labelled block is not one.
        "f(){target: {continue target;}return nil;}",
    ] {
        assert_eq!(flow::check(&ast(text)).unwrap_err().code, "sem-label");
    }
    // Leaving a labelled block, however, is supported.
    flow::check(&ast("f(){target: {break target;}return nil;}")).unwrap();
}

#[test]
fn direct_rest_parameters_preserve_general_values_and_arity() {
    let tree = ast(include_str!("../../../../tests/native/rest-parameters.t"));
    let checked = flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    let profiles = zeb_frontend::specialize::plan(&tree, &checked.program, None).unwrap();
    for (i, id) in tree.functions.iter().enumerate() {
        if matches!(
            tree.nodes[id.0].syntax,
            parser::Syntax::Function { rest: true, .. }
        ) {
            assert!(profiles[i].facts.is_none());
        }
    }
    assert_eq!(
        flow::check(&ast("f(x,[args]){return x;}main(){return f();}"))
            .unwrap_err()
            .code,
        "sem-arity"
    );
    flow::check(&ast("f([args]){return 0;}main(){return f;}")).unwrap();
}

#[test]
fn expanded_function_values_lower_to_checked_native_dispatch() {
    let tree = ast(include_str!(
        "../../../../tests/native/argument-expansion.t"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    // Expanded method calls use the same exception outcome as direct calls.
    flow::check(&ast("f(o,args){return o.method(args...);}")).unwrap();
}

#[test]
fn constructed_throws_use_native_ownership_and_finalizer_transfer() {
    let tree = ast(include_str!(
        "../../../../tests/native/constructed-exceptions.t"
    ));
    flow::check(&tree).unwrap();
    let emitted = llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    for op in [76, 79, 80, 81] {
        assert!(emitted.contains(&format!("@zeb_objects_call(i32 {op},")));
    }
}

#[test]
fn variadic_methods_and_constructors_use_native_argument_lists() {
    let tree = ast(include_str!("../../../../tests/native/rest-methods.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}

#[test]
fn optional_parameters_pad_direct_calls_and_dynamic_dispatch() {
    let tree = ast(include_str!(
        "../../../../tests/native/optional-parameters.t"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert_eq!(
        flow::check(&ast("f(a,b?){return a;}main(){return f();}"))
            .unwrap_err()
            .code,
        "sem-arity"
    );
    assert_eq!(
        flow::check(&ast("f(a?){return a;}main(){return f(1,2);}"))
            .unwrap_err()
            .code,
        "sem-arity"
    );
    assert!(
        parser::parse_with(
            &Source::decode(b"f(a?,b){return a;}".to_vec(), Encoding::Utf8).unwrap(),
            parser::Model::Ownership
        )
        .is_err()
    );
}

#[test]
fn local_owned_objects_compile_but_do_not_return_ownership_implicitly() {
    let tree = ast(include_str!("../../../../tests/native/local-objects.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert_eq!(
        flow::check(&ast("class C:object;f(){local owned x=new C;return x;}"))
            .unwrap_err()
            .code,
        "sem-owner"
    );
}
