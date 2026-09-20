#![forbid(unsafe_code)]

use zeb_frontend::{
    lexer::{Kind, lex},
    source::{Encoding, Source},
};

fn source(text: &str) -> Source {
    Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap()
}

#[test]
fn encodings_preserve_original_byte_coordinates() {
    let text = "// é😀\r\nmain() { return 42; }";
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        let (bytes, start) = match encoding {
            Encoding::Utf8 => (text.as_bytes().to_vec(), 11),
            Encoding::Utf16Le => (text.encode_utf16().flat_map(u16::to_le_bytes).collect(), 16),
            Encoding::Utf16Be => (text.encode_utf16().flat_map(u16::to_be_bytes).collect(), 16),
            _ => unreachable!(),
        };
        let decoded = Source::decode(bytes, encoding).unwrap();
        let tokens = lex(&decoded).unwrap();
        assert_eq!(tokens[0].spelling(&decoded).collect::<String>(), "main");
        assert_eq!(tokens[0].byte_start, start);
        assert_eq!(decoded.location(start), (2, 1));
        let return_token = tokens
            .iter()
            .find(|token| token.spelling(&decoded).collect::<String>() == "return")
            .unwrap();
        let column = if encoding == Encoding::Utf8 { 10 } else { 19 };
        assert_eq!(decoded.location(return_token.byte_start), (2, column));
    }
    let latin = Source::decode(b"// \xe9\nalpha".to_vec(), Encoding::Latin1).unwrap();
    assert_eq!(lex(&latin).unwrap()[0].byte_start, 5);
    assert_eq!(latin.location(5), (2, 1));
    assert!(Source::decode(b"alpha".to_vec(), Encoding::Ascii).is_ok());
}

#[test]
fn bom_selection_and_mismatch() {
    let utf8 = Source::decode(b"\xef\xbb\xbfmain".to_vec(), Encoding::Auto).unwrap();
    assert_eq!(utf8.encoding, Encoding::Utf8);
    assert_eq!(lex(&utf8).unwrap()[0].byte_start, 3);
    assert_eq!(utf8.location(3), (1, 1));
    let utf16 = Source::decode(vec![0xff, 0xfe, b'x', 0], Encoding::Auto).unwrap();
    assert_eq!(utf16.encoding, Encoding::Utf16Le);
    assert_eq!(lex(&utf16).unwrap()[0].byte_start, 2);
    assert_eq!(
        Source::decode(vec![0xff, 0xfe, b'x', 0], Encoding::Utf8)
            .unwrap_err()
            .code,
        "source-bom"
    );
}

#[test]
fn malformed_encodings_and_nul_fail_at_original_offset() {
    for (bytes, encoding, offset) in [
        (vec![b'a', 0xff], Encoding::Utf8, 1),
        (vec![b'a', 0xe9], Encoding::Ascii, 1),
        (vec![0x00, 0xd8], Encoding::Utf16Le, 0),
        (vec![0x00, 0xdc], Encoding::Utf16Le, 0),
        (vec![0x00, 0xd8, b'a', 0], Encoding::Utf16Le, 0),
        (vec![b'a', 0, 1], Encoding::Utf16Le, 2),
        (vec![b'a', 0], Encoding::Utf8, 1),
    ] {
        assert_eq!(Source::decode(bytes, encoding).unwrap_err().byte, offset);
    }
}

#[test]
fn numeric_spellings_preserve_radix_and_do_not_narrow() {
    let src = source("0 012 0xff 2147483648 .5 1. 1.e2 012.3 1..2 0xffffffff");
    let tokens = lex(&src).unwrap();
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind).collect();
    assert_eq!(
        kinds,
        vec![
            Kind::Integer(10),
            Kind::Integer(8),
            Kind::Integer(16),
            Kind::Integer(10),
            Kind::BigNumber,
            Kind::BigNumber,
            Kind::BigNumber,
            Kind::BigNumber,
            Kind::Integer(10),
            Kind::Symbol(".."),
            Kind::Integer(10),
            Kind::Integer(16),
            Kind::End
        ]
    );
    assert_eq!(tokens[3].spelling(&src).collect::<String>(), "2147483648");
    for text in ["08", "0x", "0xg", "1e+", "12abc"] {
        assert_eq!(lex(&source(text)).unwrap_err().code, "lex-number");
    }
}

#[test]
fn maximal_operators_and_comments() {
    let src = source("a>>>=1; /* // not a line */ b<<=2 // /* not a block\r\nc??d...e");
    let tokens = lex(&src).unwrap();
    let spellings: Vec<String> = tokens
        .iter()
        .map(|token| token.spelling(&src).collect())
        .collect();
    assert_eq!(
        spellings,
        [
            "a", ">>>=", "1", ";", "b", "<<=", "2", "c", "??", "d", "...", "e", ""
        ]
    );
    assert_eq!(lex(&source("/*")).unwrap_err().code, "lex-comment");
    assert_eq!(src.location(tokens[7].byte_start), (2, 1));
}

#[test]
fn unavailable_and_invalid_are_distinct() {
    assert_eq!(
        lex(&source("#include <adv3.h>")).unwrap_err().code,
        "lex-unavailable"
    );
    for text in ["é", "`"] {
        assert_eq!(lex(&source(text)).unwrap_err().code, "lex-character");
    }
}

#[test]
fn deep_lexical_input_is_iterative_and_empty_input_has_eof() {
    let text = "(".repeat(100_000) + &")".repeat(100_000);
    let src = source(&text);
    let tokens = lex(&src).unwrap();
    assert_eq!(tokens.len(), 200_001);
    assert_eq!(tokens.last().unwrap().byte_start, text.len());
    assert_eq!(lex(&source("")).unwrap()[0].kind, Kind::End);
}
