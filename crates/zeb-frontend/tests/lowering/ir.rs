#![forbid(unsafe_code)]
use zeb_frontend::{
    ir::{self, Binary, BranchMode, Failure, Operation, Program, Terminator},
    parser::{self, Model},
    sema::Scalar,
    source::{Encoding, Source},
};

fn lower(text: &str) -> Program {
    let source = Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap();
    ir::lower(&parser::parse_with(&source, Model::Ownership).unwrap()).unwrap()
}

#[test]
fn calls_evaluate_right_to_left_but_pass_positional_arguments() {
    let p = lower("g(a,b){return a;} f(x){return g(x=1,x=2);}");
    let instructions = &p.functions[1].blocks[0].instructions;
    let constants: Vec<_> = instructions
        .iter()
        .filter_map(|i| match i.operation {
            Operation::Constant(v) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(constants, [Scalar::Integer(2), Scalar::Integer(1)]);
    let call = instructions.last().unwrap();
    let Operation::Call {
        function,
        arguments,
    } = &call.operation
    else {
        panic!("missing call")
    };
    assert_eq!(*function, 0);
    assert_eq!(
        arguments,
        &[
            instructions[2].result.unwrap(),
            instructions[0].result.unwrap()
        ]
    );
    assert_eq!(call.failure, Failure::Propagate);
}

#[test]
fn simple_assignment_evaluates_rhs_without_loading_destination() {
    let p = lower("f(){local x;return x=7;}");
    let instructions = &p.functions[0].blocks[0].instructions;
    assert!(matches!(instructions[0].operation, Operation::Reset(_)));
    assert!(matches!(
        instructions[1].operation,
        Operation::Constant(Scalar::Integer(7))
    ));
    assert!(matches!(instructions[2].operation, Operation::Store(..)));
    assert!(
        !instructions
            .iter()
            .any(|i| matches!(i.operation, Operation::Load(_)))
    );
}

#[test]
fn compound_assignment_captures_old_value_before_rhs_mutation() {
    let p = lower("f(x){return x+=(x=2);}");
    let i = &p.functions[0].blocks[0].instructions;
    assert!(matches!(i[0].operation, Operation::Load(_)));
    assert!(matches!(i[2].operation, Operation::Store(..)));
    let Operation::Binary(Binary::Add, left, right) = i[3].operation else {
        panic!("missing addition")
    };
    assert_eq!(Some(left), i[0].result);
    assert_eq!(Some(right), i[1].result);
    assert!(matches!(i[4].operation, Operation::Store(..)));
}

#[test]
fn prefix_and_postfix_keep_distinct_results_and_left_first_operands() {
    let p = lower("f(x){return x++ + ++x;}");
    let i = &p.functions[0].blocks[0].instructions;
    let Operation::Binary(Binary::Add, left, right) = i[8].operation else {
        panic!("missing addition")
    };
    assert_eq!(Some(left), i[0].result);
    assert_eq!(Some(right), i[6].result);
    assert!(matches!(i[3].operation, Operation::Store(..)));
    assert!(matches!(i[4].operation, Operation::Load(_)));
}

#[test]
fn short_circuit_routes_effects_through_only_the_selected_edge() {
    for (op, mode, rhs_on_yes) in [
        ("&&", BranchMode::Logical, true),
        ("||", BranchMode::Logical, false),
        ("??", BranchMode::IsNil, true),
    ] {
        let p = lower(&format!("f(x){{return x {op} (x=true);}}"));
        let f = &p.functions[0];
        let Terminator::Branch {
            mode: actual,
            yes,
            no,
            ..
        } = f.blocks[0].terminator
        else {
            panic!("missing branch")
        };
        assert_eq!(actual, mode);
        let rhs = if rhs_on_yes { yes } else { no };
        let join = if rhs_on_yes { no } else { yes };
        assert!(
            f.blocks[rhs.0]
                .instructions
                .iter()
                .any(|i| matches!(i.operation, Operation::Store(ir::Slot(0), _)))
        );
        assert!(
            !f.blocks[0]
                .instructions
                .iter()
                .any(|i| matches!(i.operation, Operation::Store(ir::Slot(0), _)))
        );
        assert!(matches!(f.blocks[rhs.0].terminator,Terminator::Jump(target) if target==join));
        assert!(matches!(f.blocks[join.0].terminator, Terminator::Return(_)));
    }
}

#[test]
fn conditional_arms_store_into_one_join_slot() {
    let p = lower("f(x){return x ? (x=1) : (x=2);}");
    let f = &p.functions[0];
    let Terminator::Branch { yes, no, .. } = f.blocks[0].terminator else {
        panic!("missing branch")
    };
    let Terminator::Jump(join) = f.blocks[yes.0].terminator else {
        panic!("missing join")
    };
    assert!(matches!(f.blocks[no.0].terminator,Terminator::Jump(other) if other==join));
    for arm in [yes, no] {
        assert!(matches!(
            f.blocks[arm.0].instructions.last().unwrap().operation,
            Operation::Store(ir::Slot(1), _)
        ));
    }
    assert!(matches!(
        f.blocks[join.0].instructions[0].operation,
        Operation::Load(ir::Slot(1))
    ));
}

#[test]
fn nested_loop_break_and_continue_retain_their_own_targets() {
    let p = lower("f(x){while(x){while(x){break;}continue;}return x;}");
    let f = &p.functions[0];
    let Terminator::Jump(outer_test) = f.blocks[0].terminator else {
        panic!()
    };
    let Terminator::Branch {
        yes: outer_body,
        no: outer_exit,
        ..
    } = f.blocks[outer_test.0].terminator
    else {
        panic!()
    };
    let Terminator::Jump(inner_test) = f.blocks[outer_body.0].terminator else {
        panic!()
    };
    let Terminator::Branch {
        yes: inner_body,
        no: inner_exit,
        ..
    } = f.blocks[inner_test.0].terminator
    else {
        panic!()
    };
    assert!(
        matches!(f.blocks[inner_body.0].terminator,Terminator::Jump(target) if target==inner_exit)
    );
    assert!(
        matches!(f.blocks[inner_exit.0].terminator,Terminator::Jump(target) if target==outer_test)
    );
    assert!(matches!(
        f.blocks[outer_exit.0].terminator,
        Terminator::Return(_)
    ));
}

#[test]
fn locals_are_function_scoped_in_storage_and_reinitialized_in_loops() {
    let p = lower("f(a){while(a){local b=1;return b;}return a;} g(c){local d=c;return d;}");
    assert_eq!(p.functions[0].slots.len(), 2);
    assert_eq!(p.functions[1].slots.len(), 2);
    for f in &p.functions {
        assert!(f.slots[0].parameter);
        assert!(!f.slots[1].parameter);
    }
    let f = &p.functions[0];
    let Terminator::Jump(test) = f.blocks[0].terminator else {
        panic!()
    };
    let Terminator::Branch { yes, .. } = f.blocks[test.0].terminator else {
        panic!()
    };
    assert!(matches!(
        f.blocks[yes.0].instructions[0].operation,
        Operation::Reset(ir::Slot(1))
    ));
}

#[test]
fn dynamic_failures_are_explicit_and_deep_expressions_do_not_recurse() {
    let p = lower("f(x){return -(x/2);}");
    for i in &p.functions[0].blocks[0].instructions {
        let checked = matches!(i.operation, Operation::Unary(..) | Operation::Binary(..));
        assert_eq!(
            i.failure,
            if checked {
                Failure::Propagate
            } else {
                Failure::None
            }
        );
    }
    let text = "f(x){return ".to_owned() + &"+".repeat(20_000) + "x;}";
    // Separate adjacent unary operators so they cannot lex as increments.
    let text = text.replace("++", "+ + ");
    assert!(lower(&text).functions[0].values > 10_000);
}
