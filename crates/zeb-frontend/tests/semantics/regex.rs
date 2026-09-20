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
const GEN: &str = "intrinsic 'tads-gen/030008' { rexMatch(pat, str, index?); \
                   rexSearch(pat, str, index?); rexGroup(groupNum); \
                   rexReplace(pat, str, replacement, flags?, index?, limit?); } ";

/// `rexMatch` takes a literal or owned-text pattern and subject, with
/// an optional one-based index, and returns the match length or nil.
#[test]
fn rex_match_lowers_with_and_without_an_index() {
    let tree = parse(&format!(
        "{GEN} main(){{ local n = rexMatch('<alpha>+', 'abc'); \
         return n + rexMatch('a', 'xa', 2); }}"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
}

#[test]
fn rex_match_arity_is_checked() {
    let tree = parse(&format!("{GEN} main(){{ return rexMatch('a'); }}"));
    assert_eq!(flow::check(&tree).unwrap_err().code, "sem-arity");
}

/// Search and group results are lists, so they need an owned local, as a
/// vector snapshot does.
#[test]
fn search_and_group_results_require_an_owned_local() {
    let tree = parse(&format!(
        "{GEN} main(){{ local owned found = rexSearch('a', 'xa'); \
         local owned first = rexGroup(1); return found[1] + first[2]; }}"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
    let borrowed = parse(&format!(
        "{GEN} main(){{ local f = rexSearch('a', 'xa'); return f[1]; }}"
    ));
    assert_eq!(flow::check(&borrowed).unwrap_err().code, "sem-owner");
}

/// A compiled `RexPattern` field is constructed once and matched against; the
/// replaced text is owned like a search result.
#[test]
fn compiled_patterns_and_replacement_lower() {
    let tree = parse(&format!(
        "{GEN} patterns: object owned word = static new RexPattern('<alpha>+'); \
         main(){{ local n = rexMatch(patterns.word, 'abc'); \
         local owned changed = rexReplace('a', 'banana', '-', 1); \
         return n + changed.length(); }}"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
}

#[test]
fn a_static_pattern_needs_a_field_and_one_argument() {
    let loose = Source::decode(
        b"main(){ local owned p = static new RexPattern('a'); return p; }".to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    assert!(parser::parse_with(&loose, parser::Model::Ownership).is_err());
    let source = Source::decode(
        b"patterns: object owned word = static new RexPattern('a', 'b');".to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    assert_eq!(
        parser::parse_with(&source, parser::Model::Ownership)
            .unwrap_err()
            .code,
        "parse-expected"
    );
}
