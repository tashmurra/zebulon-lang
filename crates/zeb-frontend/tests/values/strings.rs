#![forbid(unsafe_code)]
use zeb_frontend::{
    lexer::{self, Kind},
    parser::{self, Syntax},
    sema,
    source::{Encoding, Source},
};
fn source(text: &str) -> Source {
    Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap()
}
fn literal(text: &str) -> String {
    let source = source(text);
    lexer::lex(&source).unwrap()[0]
        .literal(&source)
        .unwrap()
        .text
}
#[test]
fn escapes_are_unicode_scalars_not_reinterpreted_source() {
    assert_eq!(
        literal(r"'a\n\r\t\b\v\^\ z'"),
        "a\n\r\t\u{b}\u{e}\u{f}\u{15}z"
    );
    assert_eq!(literal(r"'\x41F\u0042C\000\377\400\777'"), "AFBC\0ÿ 0?7");
    assert_eq!(literal(r"'\q\8\9\U0001F600'"), r"\q\8\9\U0001F600");
    assert_eq!(literal(r"'\<<x>\>'"), "<<x>>");
    assert_eq!(literal("'é😀'"), "é😀");
}
#[test]
fn newline_spelling_controls_indentation() {
    assert_eq!(literal("'a \r\n  b\nc'"), "a  b c");
    assert_eq!(literal("'a\\n\n  b'"), "a\n  b");
    assert_eq!(literal("'a\\x0a\n  b'"), "a\n b");
    assert_eq!(literal("'a\\\r\n  b'"), "a  b");
}
#[test]
fn triple_quote_runs_and_output_identity() {
    assert_eq!(literal("'''a''b''''"), "a''b'");
    assert_eq!(literal("'''a\\''''b'''"), "a''''b");
    assert_eq!(literal("''''''"), "");
    let source = source("\"\"\"some 'text'\"\"\"");
    let token = lexer::lex(&source).unwrap()[0];
    assert_eq!(
        token.kind,
        Kind::Quoted {
            emitting: true,
            triple: true
        }
    );
    let text = token.literal(&source).unwrap();
    assert!(text.emitting && text.triple);
    assert_eq!(text.text, "some 'text'");
}
#[test]
fn errors_retain_physical_byte_locations() {
    for (text, code, byte) in [
        ("'open", "lex-string", 0),
        ("'end\\", "lex-string", 4),
        ("'é\\xZ'", "lex-escape", 3),
        (r"'\uD800'", "lex-escape", 1),
        (r"'\u'", "lex-escape", 1),
    ] {
        let error = lexer::lex(&source(text)).unwrap_err();
        assert_eq!((error.code, error.byte), (code, byte), "{text}");
    }
}
#[test]
fn original_spans_survive_all_source_encodings() {
    for (bytes, encoding) in [
        (b"'text'".to_vec(), Encoding::Ascii),
        (b"'\xe9'".to_vec(), Encoding::Latin1),
        ("'é'".as_bytes().to_vec(), Encoding::Utf8),
        (
            "'é'".encode_utf16().flat_map(u16::to_le_bytes).collect(),
            Encoding::Utf16Le,
        ),
        (
            "'é'".encode_utf16().flat_map(u16::to_be_bytes).collect(),
            Encoding::Utf16Be,
        ),
    ] {
        let length = bytes.len();
        let source = Source::decode(bytes, encoding).unwrap();
        let token = lexer::lex(&source).unwrap()[0];
        assert_eq!((token.byte_start, token.byte_end), (0, length));
        assert_eq!(
            token.literal(&source).unwrap().text,
            if encoding == Encoding::Ascii {
                "text"
            } else {
                "é"
            }
        );
    }
}
#[test]
fn ast_keeps_text_and_output_is_not_faked() {
    let tree = parser::parse_with(
        &source("main(){local x=\"Welcome!\";return nil;}"),
        parser::Model::Ownership,
    )
    .unwrap();
    assert!(
        tree.nodes
            .iter()
            .any(|node| matches!(node.syntax, Syntax::String(_)))
    );
    assert_eq!(sema::analyze(&tree).unwrap_err().code, "sem-output-context");
}

#[test]
fn equal_literal_text_is_shared_and_rust_data_is_escaped() {
    let tree = parser::parse_with(
        &source("main(){local a='same';local b='s\\x61me';return a==b;}"),
        parser::Model::Ownership,
    )
    .unwrap();
    let pool = zeb_frontend::string_pool::collect(&tree).unwrap();
    assert_eq!(pool.text, ["same"]);
    assert_eq!(
        pool.ids.iter().flatten().copied().collect::<Vec<_>>(),
        [0, 0]
    );
    let tree = parser::parse_with(
        &source("main(){return '\"; unsafe { x } \\0';}"),
        parser::Model::Ownership,
    )
    .unwrap();
    let rust = zeb_frontend::string_pool::rust_data(&tree).unwrap();
    assert!(rust.contains("\\\""));
    assert!(rust.contains("\\u{0}"));
    assert!(sema::analyze(&tree).is_ok());
}

#[test]
fn output_statements_lower_as_ordered_effects_without_string_values() {
    let tree = parser::parse_with(
        &source(include_str!("../../../../tests/native/output-text.t")),
        parser::Model::Ownership,
    )
    .unwrap();
    let checked = zeb_frontend::flow::check(&tree).unwrap();
    assert!(
        checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(
                |i| matches!(i.operation, zeb_frontend::ir::Operation::EmitLiteral(_))
                    && i.result.is_none()
            )
    );
    let effects = zeb_frontend::effects::analyze(&tree, &checked.program).unwrap();
    assert!(
        effects[0]
            .effects
            .contains(zeb_frontend::effects::Effect::Io)
    );
    assert!(zeb_frontend::llvm::emit_objects(&tree, zeb_frontend::llvm::Target::MacX86_64).is_ok());
}
