#![forbid(unsafe_code)]
use std::collections::BTreeMap;
use zeb_frontend::llvm::StackCharge;
use zebc::stack_charges::{Refinement, refine_bounded};

fn charge(n: u64) -> StackCharge {
    StackCharge {
        general: n,
        integer: 16,
    }
}
fn report(n: u64, changed: bool, wrapper: u64) -> Refinement {
    Refinement {
        charges: vec![charge(n)],
        changed,
        wrappers: BTreeMap::from([("entry".into(), wrapper)]),
        detached_exports: BTreeMap::new(),
    }
}

#[test]
fn returns_only_rebuilt_artifact_and_fresh_wrapper() {
    let mut inputs = Vec::new();
    let stable = refine_bounded(&[charge(48)], |round, charges| {
        inputs.push(charges[0].general);
        Ok((
            report(64, round == 0, if round == 0 { 96 } else { 112 }),
            round,
        ))
    })
    .unwrap();
    assert_eq!(inputs, [48, 64]);
    assert_eq!(stable.round, 1);
    assert_eq!(stable.artifact, 1);
    assert_eq!(stable.refinement.wrappers["entry"], 112);
}

#[test]
fn unchanged_first_round_does_not_recompile() {
    let mut attempts = 0;
    let stable = refine_bounded(&[charge(64)], |_, _| {
        attempts += 1;
        Ok((report(64, false, 112), "measured"))
    })
    .unwrap();
    assert_eq!(attempts, 1);
    assert_eq!(stable.round, 0);
    assert_eq!(stable.artifact, "measured");
}

#[test]
fn nonconvergence_stops_at_four_without_returning_artifact() {
    let mut attempts = 0;
    let result = refine_bounded(&[charge(48)], |_, charges| {
        attempts += 1;
        Ok((report(charges[0].general + 16, true, 112), attempts))
    });
    assert_eq!(attempts, 4);
    assert!(result.err().unwrap().contains("four rounds"));
}

#[test]
fn tool_failure_stops_immediately() {
    let mut attempts = 0;
    let result = refine_bounded(&[charge(48)], |round, _| {
        attempts += 1;
        if round == 1 {
            return Err("measurement failed".into());
        }
        Ok((report(64, true, 96), round))
    });
    assert_eq!(attempts, 2);
    assert_eq!(result.err().unwrap(), "measurement failed");
}

#[test]
fn rejects_invalid_input_without_compiling() {
    for initial in [vec![], vec![charge(0)], vec![charge(17)]] {
        assert!(
            refine_bounded::<()>(&initial, |_, _| panic!("must reject before compile")).is_err()
        );
    }
}

#[test]
fn rejects_bad_reports_instead_of_trusting_change_flag() {
    for (n, changed) in [(64, false), (48, true), (32, true), (0, true), (49, true)] {
        assert!(refine_bounded(&[charge(48)], |_, _| Ok((report(n, changed, 96), ()))).is_err());
    }
    assert!(
        refine_bounded(&[charge(48)], |_, _| {
            let mut bad = report(48, false, 96);
            bad.charges.push(charge(48));
            Ok((bad, ()))
        })
        .is_err()
    );
    assert!(
        refine_bounded(&[charge(48)], |_, _| {
            let mut bad = report(48, true, 96);
            bad.charges[0].integer = 0;
            Ok((bad, ()))
        })
        .is_err()
    );
}

#[test]
fn rejects_decreased_specialization_reserve() {
    let initial = StackCharge {
        general: 48,
        integer: 32,
    };
    let result = refine_bounded(&[initial], |_, _| Ok((report(48, true, 96), ())));
    assert!(result.err().unwrap().contains("decreased"));
}
