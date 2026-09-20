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

/// a callback captures the enclosing locals and `self` it uses, and
/// the captured environment belongs to the enclosing scope.
#[test]
fn callbacks_capture_locals_and_self() {
    for text in [
        "main(){ local base = 5; local owned add = {x: x + base}; return add(3); }",
        "o: object tag = 7 scale(n) { return tag * n; } \
         run(n) { local owned f = {x: scale(x) + tag}; return f(n); }; \
         main(){ return o.run(2); }",
        "apply(fn, v) { return fn(v); } \
         main(){ local n = 2; local owned f = {x: x * n}; return apply(f, 4); }",
        // a name the callback declares itself is not a capture
        "main(){ local x = 3; local f = {y: y + 1}; return f(x); }",
        // a nested callback captures through the one that encloses it
        "main(){ local v = [1, 2]; local owned m = v.mapAll({x: v.countWhich({g: g == x})}); \
         return m[1]; }",
        "main(){ local b = 1; local v = [1]; local owned m = v.mapAll({x: v.countWhich({g: g + b > x})}); \
         return m[1]; }",
    ] {
        let tree = parse(text);
        flow::check(&tree).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        llvm::emit_objects(&tree, llvm::Target::MacX86_64)
            .unwrap_or_else(|error| panic!("{text}: {error:?}"));
    }
}

#[test]
fn captured_environments_cannot_escape_or_go_stale() {
    for (text, code) in [
        // the environment dies with the scope, so the callback cannot leave it
        (
            "main(){ local x = 3; return ({: x}); }",
            "sem-capture-unavailable",
        ),
        (
            "o: object p = nil m() { local x = 3; p = {: x}; return nil; }; main(){ return o.m(); }",
            "sem-capture-unavailable",
        ),
        // a capture takes the value, so a later assignment is refused
        (
            "main(){ local x = 3; local owned f = {: x}; x = 4; return f(); }",
            "sem-capture-unavailable",
        ),
        // a callback that captures nothing allocates nothing to own
        (
            "main(){ local owned f = {x: x + 1}; return f(1); }",
            "sem-owner",
        ),
    ] {
        assert_eq!(flow::check(&parse(text)).unwrap_err().code, code, "{text}");
    }
}

/// callback list methods walk the collection and call the callback;
/// the ones that build a new list need an owned result.
#[test]
fn callback_list_methods_lower() {
    for text in [
        "main(){ local v = [1, 2, 3]; return v.indexWhich({x: x > 1}); }",
        "main(){ local v = [1, 2, 3]; return v.countWhich({x: x > 1}); }",
        "main(){ local v = [1, 2, 3]; return v.valWhich({x: x > 1}); }",
        "main(){ local v = [1, 2, 3]; return v.lastIndexWhich({x: x > 1}); }",
        "o: object n = 0 add(x) { n = n + x; return nil; }; \
         main(){ local v = [1, 2]; v.forEach({x: o.add(x)}); return o.n; }",
        "main(){ local v = [1, 2, 3]; local owned s = v.subset({x: x > 1}); return s.length(); }",
        "main(){ local v = [1, 2]; local owned m = v.mapAll({x: x + 1}); return m[1]; }",
    ] {
        let tree = parse(text);
        flow::check(&tree).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        llvm::emit_objects(&tree, llvm::Target::MacX86_64)
            .unwrap_or_else(|error| panic!("{text}: {error:?}"));
    }
}

#[test]
fn new_list_results_need_an_owned_local() {
    let tree = parse("main(){ local v = [1, 2]; local s = v.subset({x: x > 1}); return s; }");
    assert_eq!(flow::check(&tree).unwrap_err().code, "sem-owner");
}
