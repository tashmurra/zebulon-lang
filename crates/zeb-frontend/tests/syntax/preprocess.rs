use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use zeb_frontend::{preprocess, source::Encoding};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "zeb-preprocess-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn write(&self, path: &str, text: impl AsRef<[u8]>) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    fn run(&self) -> Result<preprocess::Preprocessed, preprocess::Error> {
        preprocess::read(
            &self.0.join("main.t"),
            &[self.0.join("include")],
            Encoding::Auto,
            1024 * 1024,
        )
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn nested_includes_guards_macros_and_multiline_calls_compile() {
    let t = Tree::new();
    t.write(
        "main.t",
        "#include <defs.h>\n#include <defs.h>\nmain(){return SUM(\nBASE, SUM(1,1));}\n",
    );
    t.write(
        "include/defs.h",
        "#pragma once\n#include \"nested.h\"\n#define SUM(a,b) ((a)+(b))\n",
    );
    t.write("include/nested.h","#ifndef NESTED\n#define NESTED\n#define BASE 40\n#else\n#error duplicate include\n#endif\n");
    let p = t.run().unwrap();
    let ast = zeb_frontend::parser::parse_with(&p.source, zeb_frontend::parser::Model::Ownership)
        .unwrap();
    zeb_frontend::flow::check(&ast).unwrap();
    let text = std::str::from_utf8(p.source.original_bytes()).unwrap();
    assert!(!text.contains("SUM"));
    assert!(text.contains("40"));
}
#[test]
fn quoted_search_walks_includers_and_angle_search_uses_configured_paths() {
    let t = Tree::new();
    t.write(
        "main.t",
        "#include \"sub/outer.h\"\nmain(){return WHICH;}\n",
    );
    t.write("sub/outer.h", "#include \"choice.h\"\n");
    t.write("choice.h", "#define WHICH 42\n");
    t.write("include/choice.h", "#define WHICH 99\n");
    assert!(
        std::str::from_utf8(t.run().unwrap().source.original_bytes())
            .unwrap()
            .contains("42")
    );
}
#[test]
fn comments_quotes_splices_undef_and_macro_recursion_are_distinct() {
    let t = Tree::new();
    t.write("main.t","#define A A\n#define X 9\n#define PLUS(a,b) a + \\\n b\n// #error ignored \\\n#error also ignored\n#undef X\n#ifdef X\n#error no\n#else\nvalue = 'X /*literal*/';\n#endif\nA PLUS(1,2)\n");
    let p = t.run().unwrap();
    let text = std::str::from_utf8(p.source.original_bytes()).unwrap();
    assert!(text.contains("'X /*literal*/'"));
    assert!(text.contains('A'));
    assert!(!text.contains("PLUS"));
}
#[test]
fn diagnostics_map_an_included_token_to_its_file_and_line() {
    let t = Tree::new();
    t.write("main.t", "#include <defs.h>\nmain(){return item.value;}\n");
    t.write(
        "include/defs.h",
        "#pragma once\nitem: object\n value = @;\n",
    );
    let p = t.run().unwrap();
    let e = zeb_frontend::parser::parse_with(&p.source, zeb_frontend::parser::Model::Ownership)
        .unwrap_err();
    let (path, line, column) = p.location(e.byte);
    assert!(path.ends_with("include/defs.h"));
    assert_eq!((line, column), (3, 10));
}
#[test]
fn declared_latin1_is_decoded_before_utf8_validation() {
    let t = Tree::new();
    t.write("main.t", b"#charset \"latin1\"\nmain(){return '\xe9';}\n");
    let p = t.run().unwrap();
    assert!(
        std::str::from_utf8(p.source.original_bytes())
            .unwrap()
            .contains('é')
    );
}
#[test]
fn malformed_and_unimplemented_directives_do_not_silently_pass() {
    for text in [
        "#else\n",
        "#ifdef X\n",
        "#include <missing.h>\n",
        "#if 1\n#endif\n",
        "#define F(x) x\nF(1,2)\n",
    ] {
        let t = Tree::new();
        t.write("main.t", text);
        assert!(t.run().is_err(), "{text}");
    }
}

#[test]
fn variadic_arguments_and_token_paste_keep_raw_paste_arguments() {
    let t = Tree::new();
    t.write("main.t", "#define ALIAS Wrong\n#define JOIN(x) x##Value\n#define FORWARD(first, rest...) call(first, ##rest)\nJOIN(ALIAS) FORWARD(1) FORWARD(1,2,3)\n");
    let p = t.run().unwrap();
    let text = std::str::from_utf8(p.source.original_bytes()).unwrap();
    assert!(text.contains("ALIASValue"));
    assert!(!text.contains("WrongValue"));
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("call(1)"));
    assert!(compact.contains("call(1,2,3)"));
}

#[test]
fn macros_expand_inside_nested_embeddings_but_not_literal_text() {
    let t = Tree::new();
    t.write("main.t", "#define VALUE 42\nmain(){\"VALUE <<VALUE>> <<true ? \"VALUE <<VALUE>>\" : \"no\">>\";return nil;}\n");
    let p = t.run().unwrap();
    let ast = zeb_frontend::parser::parse_with(&p.source, zeb_frontend::parser::Model::Ownership)
        .unwrap();
    zeb_frontend::flow::check(&ast).unwrap();
    let pool = zeb_frontend::string_pool::collect(&ast).unwrap();
    assert!(pool.text.contains(&"VALUE "));
    assert!(
        !ast.nodes
            .iter()
            .any(|n| matches!(&n.syntax,zeb_frontend::parser::Syntax::Name(name) if name=="VALUE"))
    );
}

#[test]
fn function_macro_names_without_parentheses_remain_identifiers() {
    let t = Tree::new();
    t.write("main.t", "#define newAction(x) ((x)+1)\nf(newAction) { return newAction; }\nmain() { return f(newAction(4)); }\n");
    let output = t.run().unwrap();
    let ast =
        zeb_frontend::parser::parse_with(&output.source, zeb_frontend::parser::Model::Ownership)
            .unwrap();
    zeb_frontend::flow::check(&ast).unwrap();
}

#[test]
fn stringizing_precedes_string_pasting_and_keeps_raw_arguments() {
    let t = Tree::new();
    t.write("main.t", r#"#define NAME expanded
#define STR(x) #@x
#define TEXT(x) #x
#define LABEL(a,b) '*' ## #@a ## #@b
main() { local a = STR(NAME); local b = LABEL(Dobj, Take); TEXT(hello world); local c = STR('a\\b'); }
"#);
    let p = t.run().unwrap();
    let text = std::str::from_utf8(p.source.original_bytes()).unwrap();
    assert!(text.contains("'NAME'"), "{text}");
    assert!(text.contains("'*DobjTake'"), "{text}");
    assert!(text.contains("\"hello world\""), "{text}");
    zeb_frontend::parser::parse_with(&p.source, zeb_frontend::parser::Model::Ownership).unwrap();
}

#[test]
fn invalid_macro_use_reports_invocation_instead_of_temporary_source_offset() {
    let t = Tree::new();
    t.write(
        "main.t",
        "#include <paste.h>\n\nmain() { return BAD(abc); }\n",
    );
    t.write("include/paste.h", "#define BAD(x, y) x ## y\n");
    let e = t.run().err().unwrap();
    assert_eq!(e.line, 3);
    assert_eq!(e.diagnostic.code, "preprocess");
    assert_eq!(e.diagnostic.message, "wrong macro argument count");
}

/// Pasting is textual as in the reference preprocessor, so a paste may produce
/// more than one token; adv3.h relies on this in `class name##Action: ##baseClass`.
#[test]
fn paste_may_produce_several_tokens() {
    let t = Tree::new();
    t.write(
        "main.t",
        "#define DEF(name, base...) class name##Cls: ##base\nDEF(Take, object) p = 1;\nmain() { return TakeCls; }\n",
    );
    let p = t.run().unwrap();
    let text = std::str::from_utf8(p.source.original_bytes()).unwrap();
    assert!(text.contains("class TakeCls:object"), "{text}");
    zeb_frontend::parser::parse_with(&p.source, zeb_frontend::parser::Model::Ownership).unwrap();
}
