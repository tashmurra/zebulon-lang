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

/// gotos to labels in an enclosing block or switch body lower through
/// the same scope, iteration and finalizer exits as break.
#[test]
fn enclosing_gotos_check_and_lower() {
    for text in [
        "f(){local n=0; again: n+=1; if(n<3) goto again; return n;} main(){return f();}",
        "f(){local n=1; goto skip; n=2; skip: return n;} main(){return f();}",
        "f(){local c=0; for(local i=0;i<3;++i){ while(true){ ++c; goto done; } } done: return c;} main(){return f();}",
        "f(){local n=0; try { goto out; } finally { n=1; } out: return n;} main(){return f();}",
        "f(x){local n=0; switch(x){ case 1: n=1; goto two; case 2: n=5; two: n+=2; break; } return n;} main(){return f(1);}",
        "f(){local n=0; retry: { local owned c = new OwnedCollection(); ++n; if(n<3) goto retry; } return n;} main(){return f();}",
        "f(){local k=0; top: while(k<2){ ++k; goto top; } return k;} main(){return f();}",
    ] {
        let tree = parse(text);
        flow::check(&tree).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        llvm::emit_objects(&tree, llvm::Target::MacX86_64)
            .unwrap_or_else(|error| panic!("{text}: {error:?}"));
    }
}

#[test]
fn undefined_entering_and_owner_crossing_gotos_are_rejected() {
    for (text, code) in [
        (
            "f(){goto nowhere; return 1;} main(){return f();}",
            "sem-goto",
        ),
        (
            "f(){local n=0; a: n=1; a: n=2; return n;} main(){return f();}",
            "sem-label",
        ),
        (
            "f(x){ if(x) goto inner; { inner: return 1; } return 2;} main(){return f(1);}",
            "sem-goto",
        ),
        (
            "f(){ goto body; while(true){ body: return 1; } } main(){return f();}",
            "sem-goto",
        ),
        (
            "f(){ local n=0; retry: n+=1; local owned c = new OwnedCollection(); if(n<3) goto retry; return n; } main(){return f();}",
            "sem-goto",
        ),
        (
            "f(){ local n=0; goto after; local owned c = new OwnedCollection(); after: return n; } main(){return f();}",
            "sem-goto",
        ),
        (
            // `break a` leaves the labelled `if` ; `continue` has no loop.
            "f(){ local n=0; a: if(n==0) continue a; return n;} main(){return f();}",
            "sem-label",
        ),
    ] {
        assert_eq!(flow::check(&parse(text)).unwrap_err().code, code, "{text}");
    }
}
