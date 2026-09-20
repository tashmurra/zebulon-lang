#![forbid(unsafe_code)]
use zeb_runtime::grammar::{
    Child, Error, Forest, Limits, Rule,
    Symbol::{Production as P, Star, Terminal as T},
    parse, recognize,
};
const LIMITS: Limits = Limits {
    states: 4096,
    work: 100_000,
    trees: 1024,
    depth: 64,
};
fn run(rules: &[Rule], tokens: &[u32]) -> bool {
    recognize(rules, 3, 0, tokens, LIMITS).unwrap()
}
fn trees(
    rules: &[Rule],
    productions: usize,
    tokens: &[u32],
    limits: Limits,
) -> Result<Forest, Error> {
    parse(rules, productions, 0, tokens.len(), limits, &mut |t, p| {
        Ok::<_, Error>(tokens[p] == t)
    })
}
#[test]
fn recursive_ambiguous_commands_accept_only_complete_inputs() {
    let rules = [
        Rule {
            production: 0,
            symbols: vec![P(0), T(2), P(0)],
        },
        Rule {
            production: 0,
            symbols: vec![T(1)],
        },
    ];
    for tokens in [&[1][..], &[1, 2, 1], &[1, 2, 1, 2, 1]] {
        assert!(run(&rules, tokens));
    }
    for tokens in [&[][..], &[2], &[1, 2], &[1, 1], &[1, 2, 3]] {
        assert!(!run(&rules, tokens));
    }
}
#[test]
fn late_nullable_waiters_and_cycles_reach_a_fixed_point() {
    let rules = [
        Rule {
            production: 0,
            symbols: vec![P(1), P(1)],
        },
        Rule {
            production: 1,
            symbols: vec![P(2)],
        },
        Rule {
            production: 2,
            symbols: vec![P(1)],
        },
        Rule {
            production: 2,
            symbols: vec![],
        },
    ];
    assert!(run(&rules, &[]));
    assert!(!run(&rules, &[1]));
}
#[test]
fn resource_failure_is_distinct_from_a_rejected_command() {
    let rules = [Rule {
        production: 0,
        symbols: vec![T(1)],
    }];
    for limits in [
        Limits {
            states: 1,
            work: 100,
            ..LIMITS
        },
        Limits {
            states: 10,
            work: 0,
            ..LIMITS
        },
    ] {
        assert_eq!(
            recognize(&rules, 1, 0, &[1], limits),
            Err(Error::ResourceLimit)
        );
    }
    assert_eq!(
        recognize(&rules, 0, 0, &[], LIMITS),
        Err(Error::InvalidGrammar)
    );
    assert!(run(&rules, &[1]));
}
/// Ordering observed from external TADS (`grammar-reference-001`): rule source
/// order, then shorter earlier child spans.
#[test]
fn ambiguous_trees_follow_rule_order_and_shorter_left_spans() {
    let rules = [
        Rule {
            production: 0,
            symbols: vec![T(1), P(1)],
        },
        Rule {
            production: 1,
            symbols: vec![P(2)],
        },
        Rule {
            production: 1,
            symbols: vec![P(1), T(3), P(1)],
        },
        Rule {
            production: 2,
            symbols: vec![T(2)],
        },
    ];
    let forest = trees(&rules, 3, &[1, 2, 3, 2, 3, 2], LIMITS).unwrap();
    assert_eq!(forest.roots.len(), 2);
    let left = |root: usize| {
        let Child::Tree(list) = forest.nodes[root].children[1] else {
            panic!("list child")
        };
        let Child::Tree(left) = forest.nodes[list].children[0] else {
            panic!("left child")
        };
        (forest.nodes[list].rule, forest.nodes[left].end)
    };
    assert_eq!(left(forest.roots[0]), (2, 2));
    assert_eq!(left(forest.roots[1]), (2, 4));
    assert!(trees(&rules, 3, &[1, 1], LIMITS).unwrap().roots.is_empty());
}
#[test]
fn star_and_nullable_children_keep_their_positions() {
    let rules = [
        Rule {
            production: 0,
            symbols: vec![T(1), P(1)],
        },
        Rule {
            production: 0,
            symbols: vec![T(2), Star],
        },
        Rule {
            production: 1,
            symbols: vec![],
        },
        Rule {
            production: 1,
            symbols: vec![T(3)],
        },
    ];
    let look = trees(&rules, 2, &[1], LIMITS).unwrap();
    assert_eq!(look.roots.len(), 1);
    let Child::Tree(opt) = look.nodes[look.roots[0]].children[1] else {
        panic!("optional child")
    };
    let node = &look.nodes[opt];
    assert_eq!((node.rule, node.start, node.end), (2, 1, 1));
    let say = trees(&rules, 2, &[2, 9, 9], LIMITS).unwrap();
    assert_eq!(
        say.nodes[say.roots[0]].children,
        vec![Child::Token(0), Child::Star { start: 1, end: 3 }]
    );
    let bare = trees(&rules, 2, &[2], LIMITS).unwrap();
    assert_eq!(
        bare.nodes[bare.roots[0]].children[1],
        Child::Star { start: 1, end: 1 }
    );
}
#[test]
fn unit_cycles_are_finite_and_ambiguity_is_bounded() {
    let cyclic = [
        Rule {
            production: 0,
            symbols: vec![P(1)],
        },
        Rule {
            production: 1,
            symbols: vec![P(0)],
        },
        Rule {
            production: 1,
            symbols: vec![T(1)],
        },
    ];
    assert_eq!(trees(&cyclic, 2, &[1], LIMITS).unwrap().roots.len(), 1);
    let ambiguous = [
        Rule {
            production: 0,
            symbols: vec![P(0), P(0)],
        },
        Rule {
            production: 0,
            symbols: vec![T(1)],
        },
    ];
    assert_eq!(
        trees(&ambiguous, 1, &[1; 4], LIMITS).unwrap().roots.len(),
        5
    );
    assert_eq!(
        trees(
            &ambiguous,
            1,
            &[1; 12],
            Limits {
                trees: 100,
                ..LIMITS
            }
        )
        .unwrap_err(),
        Error::ResourceLimit
    );
}
#[test]
fn matcher_errors_propagate_unchanged() {
    #[derive(Debug, PartialEq)]
    enum Failure {
        Grammar(Error),
        Lookup,
    }
    impl From<Error> for Failure {
        fn from(error: Error) -> Self {
            Self::Grammar(error)
        }
    }
    let rules = [Rule {
        production: 0,
        symbols: vec![T(1)],
    }];
    let result = parse(&rules, 1, 0, 1, LIMITS, &mut |_, _| {
        Err::<bool, _>(Failure::Lookup)
    });
    assert_eq!(result.unwrap_err(), Failure::Lookup);
    let result = parse(&rules, 0, 0, 1, LIMITS, &mut |_, _| Ok::<_, Failure>(true));
    assert_eq!(result.unwrap_err(), Failure::Grammar(Error::InvalidGrammar));
}
