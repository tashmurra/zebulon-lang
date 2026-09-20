#![forbid(unsafe_code)]
use zeb_frontend::{
    parser::{Ast, Id, Node, Syntax, parse},
    sema::{Analysis, Scalar, analyze},
    source::{Encoding, Source},
};

fn tree(text: &str) -> Ast {
    parse(&Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap()).unwrap()
}
fn checked(text: &str) -> Analysis {
    analyze(&tree(text)).unwrap()
}
fn failure(text: &str, code: &str) {
    assert_eq!(analyze(&tree(text)).unwrap_err().code, code, "{text}");
}
fn result(text: &str) -> Option<Scalar> {
    let ast = tree(text);
    let analysis = analyze(&ast).unwrap();
    ast.nodes
        .iter()
        .find_map(|node| {
            if let Syntax::Return(Some(value)) = node.syntax {
                Some(analysis.constants[value.0])
            } else {
                None
            }
        })
        .unwrap()
}

#[test]
fn signed_limits_bit_patterns_and_remainder() {
    for (expr, value) in [
        ("-((2147483648))", i32::MIN),
        ("0xffffffff", -1),
        ("037777777777", -1),
        ("-2147483648 % -1", 0),
        ("-7 / 2", -3),
        ("-7 % 2", -1),
        ("1 << 31", i32::MIN),
        ("-1 >>> 1", i32::MAX),
        ("-4 >> 1", -2),
    ] {
        assert_eq!(
            result(&format!("f() {{ return {expr}; }}")),
            Some(Scalar::Integer(value)),
            "{expr}"
        );
    }
}

#[test]
fn arithmetic_failures_do_not_wrap_or_become_float() {
    for (expr, code) in [
        ("2147483648", "sem-literal-range"),
        ("0x100000000", "sem-literal-range"),
        ("2147483647+1", "sem-overflow"),
        ("-(-2147483648)", "sem-overflow"),
        ("-2147483648/-1", "sem-overflow"),
        ("1/0", "sem-divisor"),
        ("1%0", "sem-divisor"),
        ("1<<32", "sem-shift"),
        ("1>>-1", "sem-shift"),
        ("1.0", "sem-unavailable"),
        ("true+1", "sem-type"),
        ("!0", "sem-type"),
    ] {
        failure(&format!("f(){{return {expr};}}"), code);
    }
}

#[test]
fn logical_types_and_equality_do_not_coerce() {
    assert_eq!(result("f(){return nil==0;}"), Some(Scalar::Nil));
    assert_eq!(result("f(){return true==1;}"), Some(Scalar::Nil));
    assert_eq!(result("f(){return !nil && true;}"), Some(Scalar::True));
    assert_eq!(result("f(){return nil ?? 42;}"), Some(Scalar::Integer(42)));
    failure("f(){if(1) return 0; return 1;}", "sem-type");
    failure("f(x){return true*x;}", "sem-type");
}

#[test]
fn unselected_constant_failures_are_still_errors() {
    failure("f(){if(nil) return 1/0; return 0;}", "sem-divisor");
    failure("f(){return true ? 1 : 2147483647+1;}", "sem-overflow");
    failure("f(){return nil && (1/0==2);}", "sem-divisor");
    failure("f(){return -2147483648 + 2147483648;}", "sem-literal-range");
}

#[test]
fn forward_mutual_calls_and_parenthesized_callees_bind() {
    let analysis = checked("a(x){return (b)(x);} b(y){if(y==0)return 0; return a(y-1);}");
    assert_eq!(analysis.calls.iter().flatten().count(), 2);
    assert_eq!(analysis.locals.len(), 2);
    failure("f(){return g();} g(x){return x;}", "sem-arity");
    failure("f(){return missing();}", "sem-name");
    checked("f(){return f;}");
    checked("f(x){return x();}");
}

#[test]
fn names_have_lexical_scope_and_shadow_only_globals() {
    checked("f(x){{local y=x;} {local y=2;} return x;}");
    failure("f(x){local x=1;return x;}", "sem-shadowing");
    failure("f(x,x){return x;}", "sem-shadowing");
    failure("f(){return 1;} f(){return 2;}", "sem-name");
    // A local may shadow a global function or object.
    checked("f(){local f=1;return f;}");
    checked("a: object; f(){local a=1;return a;}");
    failure("f(){{local x=1;}return x;}", "sem-name");
    failure("f(){return x;local x=1;}", "sem-name");
    checked("f(){local x=(x=1);return x;}");
}

#[test]
fn mutation_and_loop_targets_are_validated() {
    checked("f(x){while(x>0){--x;if(x==2)break;continue;}return x;}");
    failure("f(){break;return 0;}", "sem-control-target");
    failure("f(){continue;return 0;}", "sem-control-target");
    failure("f(){1=2;return 0;}", "sem-destination");
    failure("f(){return ++(1+2);}", "sem-destination");
    checked("f(x){(x)+=2;return x++;}");
}

#[test]
fn dynamic_values_and_missing_flow_are_explicit() {
    let analysis = checked("f(x){local y; if(x)y=1;return y;}");
    assert!(!analysis.flow_complete);
    assert_eq!(result("f(x){return x+1;}"), None);
}

#[test]
fn malformed_ast_is_rejected_without_following_bad_ids() {
    let mut ast = tree("f(){return 1;}");
    ast.functions = vec![Id(usize::MAX)];
    assert_eq!(analyze(&ast).unwrap_err().code, "sem-ast");
    let ast = Ast {
        nodes: vec![Node {
            syntax: Syntax::Group(Id(0)),
            start: 0,
            end: 0,
        }],
        functions: vec![],
        objects: vec![],
        relations: vec![],
        declared_rows: vec![None],
        lifetimes: false,
    };
    assert_eq!(analyze(&ast).unwrap_err().code, "sem-ast");
    let ast = Ast {
        nodes: vec![Node {
            syntax: Syntax::Integer("1".to_owned(), 100),
            start: 0,
            end: 1,
        }],
        functions: vec![],
        objects: vec![],
        relations: vec![],
        declared_rows: vec![None],
        lifetimes: false,
    };
    assert_eq!(analyze(&ast).unwrap_err().code, "sem-ast");
}

#[test]
fn deep_scopes_and_groups_are_iterative() {
    let text = "f()".to_owned()
        + &"{".repeat(20_000)
        + "return "
        + &"(".repeat(20_000)
        + "1"
        + &")".repeat(20_000)
        + ";"
        + &"}".repeat(20_000);
    assert!(!checked(&text).flow_complete);
}

#[test]
fn external_functions_require_real_definitions_when_used() {
    use zeb_frontend::{
        flow, parser,
        source::{Encoding, Source},
    };
    let check = |text: &str| {
        let source = Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap();
        flow::check(&parser::parse_with(&source, parser::Model::Ownership).unwrap())
    };
    check(
        "extern function later(x);extern function later(value);main(){return later(4);}later(x){return x+1;}",
    )
    .unwrap();
    check("extern function unused;main(){return 0;}").unwrap();
    assert_eq!(
        check("extern function f;f(x){return x;}").unwrap_err().code,
        "sem-external"
    );
    assert_eq!(
        check("extern function f(x);extern function f(x,y);main(){return 0;}")
            .unwrap_err()
            .code,
        "sem-external"
    );
    for text in [
        "extern function missing;main(){return missing();}",
        "extern function missing;main(){return missing;}",
    ] {
        assert_eq!(check(text).unwrap_err().code, "sem-unresolved-external");
    }
    assert_eq!(
        check("extern function value;value:object;main(){return nil;}")
            .unwrap_err()
            .code,
        "sem-external"
    );
    assert_eq!(
        check("extern function later(x);main(){return later();}later(x){return x;}")
            .unwrap_err()
            .code,
        "sem-arity"
    );
}
