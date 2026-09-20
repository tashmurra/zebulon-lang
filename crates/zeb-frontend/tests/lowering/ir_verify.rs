#![forbid(unsafe_code)]
use zeb_frontend::{
    ir::{self, BlockId, Failure, Operation, Program, Slot, Terminator, Unary, Value},
    ir_verify::verify,
    parser::{Ast, Id, parse},
    source::{Encoding, Source},
};
fn fixture(text: &str) -> (Ast, Program) {
    let ast = parse(&Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap()).unwrap();
    let ir = ir::lower(&ast).unwrap();
    (ast, ir)
}
fn rejected(ast: &Ast, p: &Program, expected: &str) {
    let e = verify(ast, p).unwrap_err();
    assert_eq!(e.code, "ir-invalid");
    assert!(e.message.contains(expected), "{e:?}");
}
#[test]
fn verifies_nested_control_flow_and_recursive_calls() {
    for text in [
        "f(x){return x? (x?x+1:x+2) : x+3;}",
        "f(x){while(x){while(x){if(x)continue;break;}x=nil;}return x;}",
        "f(x){if(x)return f(nil);return 0;}",
        "f(x){return x;return x+1;}",
    ] {
        let (ast, p) = fixture(text);
        verify(&ast, &p).unwrap();
    }
}
#[test]
fn invalid_targets_sites_and_slots_fail_before_indexing() {
    let (ast, mut p) = fixture("f(x){return x;}");
    p.functions[0].blocks[0].terminator = Terminator::Jump(BlockId(usize::MAX));
    rejected(&ast, &p, "target is out of range");
    let (ast, mut p) = fixture("f(x){return x;}");
    p.functions[0].blocks[0].site = Id(usize::MAX);
    rejected(&ast, &p, "source site");
    let (ast, mut p) = fixture("f(x){return x;}");
    p.functions[0].blocks[0].instructions[0].operation = Operation::Load(Slot(usize::MAX));
    rejected(&ast, &p, "slot is out of range");
}
#[test]
fn duplicate_missing_and_out_of_range_values_are_rejected() {
    let (ast, mut p) = fixture("f(x){return x+1;}");
    p.functions[0].blocks[0].instructions[1].result = Some(Value(0));
    rejected(&ast, &p, "duplicate");
    let (ast, mut p) = fixture("f(x){return x;}");
    p.functions[0].values += 1;
    rejected(&ast, &p, "no definition");
    let (ast, mut p) = fixture("f(x){return x;}");
    p.functions[0].blocks[0].terminator = Terminator::Return(Value(usize::MAX));
    rejected(&ast, &p, "no definition");
}
#[test]
fn use_before_definition_and_non_dominating_arm_are_rejected() {
    let (ast, mut p) = fixture("f(x){return x+1;}");
    p.functions[0].blocks[0].instructions.swap(0, 2);
    rejected(&ast, &p, "before definition");
    let (ast, mut p) = fixture("f(x){return x ? x+1 : x+2;}");
    let f = &mut p.functions[0];
    let Terminator::Branch { yes, .. } = f.blocks[0].terminator else {
        panic!()
    };
    let value = f.blocks[yes.0]
        .instructions
        .iter()
        .filter_map(|i| i.result)
        .next_back()
        .unwrap();
    let Terminator::Jump(join) = f.blocks[yes.0].terminator else {
        panic!()
    };
    f.blocks[join.0].instructions[0].operation = Operation::Unary(Unary::Positive, value);
    f.blocks[join.0].instructions[0].failure = Failure::Propagate;
    rejected(&ast, &p, "does not dominate");
}
#[test]
fn failure_and_guard_contracts_cannot_be_removed() {
    let (ast, mut p) = fixture("f(x){return x+1;}");
    p.functions[0].blocks[0].instructions[2].failure = Failure::None;
    rejected(&ast, &p, "failure edge");
    let (ast, mut p) = fixture("f(x){if(x)return 1;return 2;}");
    p.functions[0].blocks[0]
        .instructions
        .retain(|i| !matches!(i.operation, Operation::LogicalGuard(_)));
    rejected(&ast, &p, "lacks its guard");
    let (ast, mut p) = fixture("f(x){return x;}");
    p.functions[0].blocks[0].instructions[0].result = None;
    rejected(&ast, &p, "result shape");
}
#[test]
fn call_arity_and_parameter_inventory_are_checked() {
    let (ast, mut p) = fixture("f(x){return f(x);}");
    let Operation::Call { arguments, .. } = &mut p.functions[0].blocks[0].instructions[1].operation
    else {
        panic!()
    };
    arguments.clear();
    rejected(&ast, &p, "arity");
    let (ast, mut p) = fixture("f(x){return x;}");
    p.functions[0].slots[0].parameter = false;
    rejected(&ast, &p, "parameter slots");
}

#[test]
fn parameter_slots_cannot_swap_source_order() {
    let (ast, mut p) = fixture("f(a,b){return a;}");
    p.functions[0].slots.swap(0, 1);
    rejected(&ast, &p, "parameter source order");
}

#[test]
fn source_declarations_cannot_cross_function_boundaries() {
    let (ast, mut p) = fixture("f(){local x=1;return x;}g(){local y=2;return y;}");
    p.functions[0].slots[0].declaration = p.functions[1].slots[0].declaration;
    rejected(&ast, &p, "another function");
}

#[test]
fn source_sites_cannot_cross_functions_or_use_orphan_nodes() {
    let (ast, mut p) = fixture("f(){return 1;}g(){return 2;}");
    p.functions[0].blocks[0].site = p.functions[1].blocks[0].site;
    rejected(&ast, &p, "block source belongs to another function");
    let (ast, mut p) = fixture("f(){return 1;}g(){return 2;}");
    p.functions[0].blocks[0].instructions[0].site = p.functions[1].blocks[0].instructions[0].site;
    rejected(&ast, &p, "instruction source belongs to another function");
    let (mut ast, mut p) = fixture("f(){return 1;}");
    let orphan = Id(ast.nodes.len());
    ast.nodes.push(zeb_frontend::parser::Node {
        syntax: zeb_frontend::parser::Syntax::Nil,
        start: 0,
        end: 0,
    });
    p.functions[0].blocks[0].instructions[0].site = orphan;
    rejected(&ast, &p, "instruction source belongs to another function");
}

#[test]
fn malformed_source_edges_fail_without_recursion() {
    let (mut ast, p) = fixture("f(){return 1;}");
    let root = ast.functions[0];
    let zeb_frontend::parser::Syntax::Function { body, .. } = &mut ast.nodes[root.0].syntax else {
        panic!()
    };
    *body = root;
    rejected(&ast, &p, "non-topological source tree");
}
