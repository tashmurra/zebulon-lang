//! Independent textual preprocessing. Expansion keeps original diagnostic sites.
use crate::{
    Diagnostic,
    source::{Encoding, Source},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
struct Site {
    file: usize,
    byte: usize,
}
#[derive(Clone, Debug)]
struct Token {
    text: String,
    site: Site,
}
impl Token {
    fn space(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }
    fn word(&self) -> bool {
        self.text
            .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
    }
}
struct Input {
    path: PathBuf,
    source: Source,
}
struct Macro {
    parameters: Option<Vec<String>>,
    body: Vec<Token>,
    variadic: bool,
}
struct Frame {
    file: usize,
    tokens: Vec<Token>,
    cursor: usize,
    conditions: Vec<Condition>,
}
struct Condition {
    parent: bool,
    active: bool,
    taken: bool,
    had_else: bool,
}
struct Mapping {
    output: usize,
    site: Site,
}
pub struct Preprocessed {
    pub source: Source,
    files: Vec<Input>,
    mappings: Vec<Mapping>,
}
impl Preprocessed {
    pub fn location(&self, byte: usize) -> (&Path, usize, usize) {
        let i = self
            .mappings
            .partition_point(|m| m.output <= byte)
            .saturating_sub(1);
        let site = self
            .mappings
            .get(i)
            .map_or(Site { file: 0, byte: 0 }, |m| m.site);
        let input = &self.files[site.file];
        let (line, column) = input.source.location(site.byte);
        (&input.path, line, column)
    }
}
#[derive(Debug)]
pub struct Error {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub diagnostic: Diagnostic,
}
fn push<T>(v: &mut Vec<T>, value: T) -> Result<(), Diagnostic> {
    v.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
    v.push(value);
    Ok(())
}
fn append(s: &mut String, text: &str) -> Result<(), Diagnostic> {
    s.try_reserve(text.len())
        .map_err(|_| Diagnostic::resource(0))?;
    s.push_str(text);
    Ok(())
}
fn token(text: &str, site: Site) -> Result<Token, Diagnostic> {
    let mut value = String::new();
    append(&mut value, text)?;
    Ok(Token { text: value, site })
}
fn fail(site: Site, message: &'static str) -> Diagnostic {
    Diagnostic::new("preprocess", message, site.byte)
}

// Preserve whitespace inside literals; comments become spaces. Source-language
// interpolation is still rejected by the parser, not interpreted here.
fn string_end(
    u: &[crate::source::Unit],
    mut i: usize,
    quote: char,
    triple: bool,
) -> Result<(usize, bool), Diagnostic> {
    let start = u.get(i).map_or(0, |u| u.start);
    while i < u.len() {
        if u[i].ch == '\\' {
            i += 1;
            let escaped = u.get(i).map(|u| u.ch);
            i = (i + 1).min(u.len());
            if triple && escaped == Some(quote) {
                while i < u.len() && u[i].ch == quote {
                    i += 1;
                }
            }
            continue;
        }
        if u[i].ch == '<' && u.get(i + 1).is_some_and(|u| u.ch == '<') {
            return Ok((i + 2, true));
        }
        if u[i].ch == quote {
            if !triple {
                return Ok((i + 1, false));
            }
            let first = i;
            while i < u.len() && u[i].ch == quote {
                i += 1;
            }
            if i - first >= 3 {
                return Ok((i, false));
            }
            continue;
        }
        i += 1;
    }
    Err(Diagnostic::new("preprocess", "unterminated literal", start))
}
fn tokenize(source: &Source, file: usize) -> Result<Vec<Token>, Diagnostic> {
    let original = source.units();
    let mut joined = Vec::new();
    let mut cursor = 0;
    while cursor < original.len() {
        let mut unit = original[cursor];
        cursor += 1;
        if unit.ch == '\\'
            && original
                .get(cursor)
                .is_some_and(|u| matches!(u.ch, '\r' | '\n'))
        {
            let cr = original[cursor].ch == '\r';
            cursor += 1;
            if cr && original.get(cursor).is_some_and(|u| u.ch == '\n') {
                cursor += 1;
            }
            unit.ch = ' ';
        }
        push(&mut joined, unit)?;
    }
    let u = &joined;
    let mut i = 0;
    let mut out = Vec::new();
    let mut embeddings: Vec<(char, bool, usize)> = Vec::new();
    while i < u.len() {
        let site = Site {
            file,
            byte: u[i].start,
        };
        let c = u[i].ch;
        let next = u.get(i + 1).map(|u| u.ch);
        if let Some(&(quote, triple, depth)) = embeddings.last()
            && c == '>'
            && next == Some('>')
            && depth == 0
        {
            let start = i;
            let (end, more) = string_end(u, i + 2, quote, triple)?;
            i = end;
            let mut text = String::new();
            for unit in &u[start..i] {
                let mut bytes = [0; 4];
                append(&mut text, unit.ch.encode_utf8(&mut bytes))?;
            }
            push(&mut out, Token { text, site })?;
            if !more {
                embeddings.pop();
            }
            continue;
        }
        if c == '\\' && matches!(next, Some('\r' | '\n')) {
            i += 2;
            if next == Some('\r') && u.get(i).is_some_and(|u| u.ch == '\n') {
                i += 1;
            }
            push(&mut out, token(" ", site)?)?;
            continue;
        }
        if c == '/' && matches!(next, Some('/' | '*')) {
            let block = next == Some('*');
            i += 2;
            let mut closed = !block;
            push(&mut out, token(" ", site)?)?;
            while i < u.len() {
                if block && u[i].ch == '*' && u.get(i + 1).is_some_and(|u| u.ch == '/') {
                    i += 2;
                    closed = true;
                    break;
                }
                if matches!(u[i].ch, '\r' | '\n') {
                    if !block {
                        break;
                    }
                    push(
                        &mut out,
                        token(
                            "\n",
                            Site {
                                file,
                                byte: u[i].start,
                            },
                        )?,
                    )?;
                }
                i += 1;
            }
            if !closed {
                return Err(fail(site, "unterminated comment"));
            }
            continue;
        }
        let start = i;
        i += 1;
        if matches!(c, '\'' | '"') {
            let triple =
                u.get(i).is_some_and(|u| u.ch == c) && u.get(i + 1).is_some_and(|u| u.ch == c);
            if triple {
                i += 2;
            }
            let (end, more) = string_end(u, i, c, triple)?;
            i = end;
            if more {
                push(&mut embeddings, (c, triple, 0))?;
            }
        } else if c.is_ascii_alphanumeric() || c == '_' {
            while i < u.len() && (u[i].ch.is_ascii_alphanumeric() || u[i].ch == '_') {
                i += 1;
            }
        } else if c.is_whitespace() && !matches!(c, '\r' | '\n') {
            while i < u.len() && u[i].ch.is_whitespace() && !matches!(u[i].ch, '\r' | '\n') {
                i += 1;
            }
        } else if c == '\r' && next == Some('\n') {
            i += 1;
        }
        if !matches!(c, '\'' | '"')
            && let Some((_, _, depth)) = embeddings.last_mut()
        {
            match c {
                '(' | '[' | '{' => {
                    *depth = depth
                        .checked_add(1)
                        .ok_or(Diagnostic::resource(site.byte))?
                }
                ')' | ']' | '}' => *depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let mut text = String::new();
        for unit in &u[start..i] {
            let mut bytes = [0; 4];
            append(&mut text, unit.ch.encode_utf8(&mut bytes))?;
        }
        push(&mut out, Token { text, site })?;
    }
    Ok(out)
}

struct Processor {
    files: Vec<Input>,
    frames: Vec<Frame>,
    macros: HashMap<String, Macro>,
    once: HashSet<PathBuf>,
    paths: Vec<PathBuf>,
    encoding: Encoding,
    limit: usize,
    read_bytes: usize,
    output: String,
    mappings: Vec<Mapping>,
    current: Site,
    loading: Option<PathBuf>,
}
impl Processor {
    fn load(&mut self, path: PathBuf) -> Result<(), Diagnostic> {
        if self.once.contains(&path) {
            return Ok(());
        }
        if self.frames.len() >= 256 {
            return Err(fail(self.current, "include nesting exceeds 256"));
        }
        let remaining = self.limit.saturating_sub(self.read_bytes);
        self.loading = Some(path.clone());
        let source = Source::read_declared(&path, self.encoding, remaining)?;
        self.loading = None;
        self.read_bytes = self
            .read_bytes
            .checked_add(source.byte_len())
            .ok_or(Diagnostic::resource(0))?;
        let file = self.files.len();
        push(&mut self.files, Input { path, source })?;
        self.current = Site { file, byte: 0 };
        let tokens = tokenize(&self.files[file].source, file)?;
        push(
            &mut self.frames,
            Frame {
                file,
                tokens,
                cursor: 0,
                conditions: Vec::new(),
            },
        )
    }
    fn emit(&mut self, t: &Token) -> Result<(), Diagnostic> {
        if t.text.len() > self.limit.saturating_sub(self.output.len()) {
            return Err(fail(t.site, "expanded source exceeds byte limit"));
        }
        push(
            &mut self.mappings,
            Mapping {
                output: self.output.len(),
                site: t.site,
            },
        )?;
        append(&mut self.output, &t.text)
    }
    fn expand(&self, input: &[Token]) -> Result<Vec<Token>, Diagnostic> {
        self.expand_inner(input, 0)
    }
    fn expand_inner(&self, input: &[Token], depth: usize) -> Result<Vec<Token>, Diagnostic> {
        if depth >= 128 {
            return Err(fail(self.current, "macro argument nesting exceeds 128"));
        }
        // Explicit expansion frames avoid Rust recursion; disabled names implement
        // ordinary self/mutual macro suppression during rescanning.
        let mut stack: Vec<(Vec<Token>, usize, Option<String>)> = Vec::new();
        let mut copy = Vec::new();
        copy.try_reserve(input.len())
            .map_err(|_| Diagnostic::resource(0))?;
        for t in input {
            copy.push(token(&t.text, t.site)?);
        }
        push(&mut stack, (copy, 0, None))?;
        let mut output = Vec::new();
        let mut work = 0usize;
        while !stack.is_empty() {
            work += 1;
            if work > self.limit {
                return Err(fail(self.current, "macro expansion exceeds work limit"));
            }
            let last = stack.len() - 1;
            if stack[last].1 == stack[last].0.len() {
                stack.pop();
                continue;
            }
            let t = &stack[last].0[stack[last].1];
            let site = t.site;
            let name = &t.text;
            let definition = if t.word() && !stack.iter().any(|f| f.2.as_ref() == Some(name)) {
                self.macros.get(name)
            } else {
                None
            };
            if let Some(definition) = definition {
                let mut consumed = stack[last].1 + 1;
                let mut arguments: Vec<Vec<Token>> = Vec::new();
                if let Some(params) = &definition.parameters {
                    while consumed < stack[last].0.len() && stack[last].0[consumed].space() {
                        consumed += 1;
                    }
                    if stack[last].0.get(consumed).is_none_or(|t| t.text != "(") {
                        // A function-like macro name without an invocation remains
                        // an ordinary token (for example, a parameter named newAction).
                        push(&mut output, token(name, site)?)?;
                        stack[last].1 += 1;
                        continue;
                    }
                    consumed += 1;
                    let mut nesting = 0usize;
                    let mut brackets = 0usize;
                    let mut braces = 0usize;
                    let mut argument = Vec::new();
                    let mut closed = false;
                    while consumed < stack[last].0.len() {
                        let t = &stack[last].0[consumed];
                        consumed += 1;
                        if t.text == ")" && nesting == 0 && brackets == 0 && braces == 0 {
                            if !params.is_empty() || argument.iter().any(|t: &Token| !t.space()) {
                                push(&mut arguments, argument)?;
                            }
                            closed = true;
                            break;
                        }
                        if t.text == "," && nesting == 0 && brackets == 0 && braces == 0 {
                            push(&mut arguments, argument)?;
                            argument = Vec::new();
                            continue;
                        }
                        // As in the reference preprocessor, commas inside parentheses,
                        // square brackets or braces belong to the argument text.
                        match t.text.as_str() {
                            "(" => nesting += 1,
                            ")" => nesting -= 1,
                            "[" => brackets += 1,
                            "]" => brackets = brackets.saturating_sub(1),
                            "{" => braces += 1,
                            "}" => braces = braces.saturating_sub(1),
                            _ => {}
                        }
                        push(&mut argument, token(&t.text, t.site)?)?;
                    }
                    if !closed {
                        return Err(fail(site, "unterminated macro invocation"));
                    }
                    if definition.variadic {
                        if arguments.len() < params.len().saturating_sub(1) {
                            return Err(fail(site, "too few macro arguments"));
                        }
                        let mut tail = Vec::new();
                        let mut first = true;
                        while arguments.len() >= params.len() {
                            let arg = arguments.remove(params.len() - 1);
                            if !first {
                                push(&mut tail, token(",", site)?)?;
                            }
                            first = false;
                            for t in arg {
                                push(&mut tail, t)?;
                            }
                        }
                        push(&mut arguments, tail)?;
                    }
                    if arguments.len() != params.len() {
                        return Err(fail(site, "wrong macro argument count"));
                    }
                }
                // Stringize formal arguments before applying token pastes.
                let mut replacement = Vec::new();
                let mut at = 0;
                while at < definition.body.len() {
                    let t = &definition.body[at];
                    let is_paste = t.text == "#"
                        && (definition.body.get(at + 1).is_some_and(|t| t.text == "#")
                            || at > 0 && definition.body[at - 1].text == "#");
                    if t.text == "#" && !is_paste {
                        at += 1;
                        let quote = if definition.body.get(at).is_some_and(|t| t.text == "@") {
                            at += 1;
                            '\''
                        } else {
                            '"'
                        };
                        while definition.body.get(at).is_some_and(Token::space) {
                            at += 1;
                        }
                        let parameter = definition
                            .body
                            .get(at)
                            .and_then(|t| {
                                definition
                                    .parameters
                                    .as_ref()
                                    .and_then(|p| p.iter().position(|p| p == &t.text))
                            })
                            .ok_or(fail(site, "stringizing requires a macro parameter"))?;
                        let mut text = String::new();
                        append(&mut text, if quote == '\'' { "'" } else { "\"" })?;
                        let mut space = false;
                        for t in &arguments[parameter] {
                            if t.space() {
                                space = text.len() > 1;
                                continue;
                            }
                            if space {
                                append(&mut text, " ")?;
                                space = false;
                            }
                            for c in t.text.chars() {
                                if c == quote || c == '\\' {
                                    append(&mut text, "\\")?;
                                }
                                let mut bytes = [0; 4];
                                append(&mut text, c.encode_utf8(&mut bytes))?;
                            }
                        }
                        append(&mut text, if quote == '\'' { "'" } else { "\"" })?;
                        push(&mut replacement, Token { text, site })?;
                    } else {
                        push(&mut replacement, token(&t.text, t.site)?)?;
                    }
                    at += 1;
                }
                let mut body = Vec::new();
                push(&mut body, token(" ", site)?)?;
                for (index, t) in replacement.iter().enumerate() {
                    let parameter = definition
                        .parameters
                        .as_ref()
                        .and_then(|p| p.iter().position(|p| p == &t.text));
                    if let Some(parameter) = parameter {
                        let before = replacement[..index].iter().rev().find(|t| !t.space());
                        let after = replacement[index + 1..].iter().find(|t| !t.space());
                        let pasted = before.is_some_and(|t| t.text == "#")
                            || after.is_some_and(|t| t.text == "#");
                        let expanded = if pasted {
                            None
                        } else {
                            Some(self.expand_inner(&arguments[parameter], depth + 1)?)
                        };
                        let argument = expanded.as_ref().unwrap_or(&arguments[parameter]);
                        if argument.iter().all(Token::space) && pasted {
                            push(&mut body, token("", site)?)?;
                        }
                        for t in argument {
                            if body.len() >= self.limit {
                                return Err(fail(site, "macro substitution exceeds limit"));
                            }
                            push(&mut body, token(&t.text, t.site)?)?;
                        }
                    } else {
                        push(&mut body, token(&t.text, site)?)?;
                    }
                }
                let mut pasted: Vec<Token> = Vec::new();
                let mut at = 0;
                while at < body.len() {
                    if body[at].text == "#" {
                        if body.get(at + 1).is_none_or(|t| t.text != "#") {
                            return Err(fail(
                                site,
                                "macro stringification and foreach are not implemented",
                            ));
                        }
                        while pasted
                            .last()
                            .is_some_and(|t| t.space() && !t.text.is_empty())
                        {
                            pasted.pop();
                        }
                        let mut left = pasted
                            .pop()
                            .ok_or(fail(site, "token paste has no left operand"))?;
                        at += 2;
                        while at < body.len() && body[at].space() && !body[at].text.is_empty() {
                            at += 1;
                        }
                        let right = body
                            .get(at)
                            .ok_or(fail(site, "token paste has no right operand"))?;
                        if left.text == "," && definition.variadic {
                            if !right.text.is_empty() {
                                push(&mut pasted, left)?;
                                push(&mut pasted, token(&right.text, site)?)?;
                            }
                        } else {
                            let quote = left.text.chars().next();
                            if matches!(quote, Some('\'' | '"'))
                                && right.text.starts_with(quote.expect("quote"))
                                && left.text.ends_with(quote.expect("quote"))
                                && right.text.ends_with(quote.expect("quote"))
                                && !left.text.starts_with("\"\"\"")
                                && !left.text.starts_with("'''")
                            {
                                left.text.pop();
                                append(&mut left.text, &right.text[1..])?;
                            } else {
                                append(&mut left.text, &right.text)?;
                            }
                            if !left.text.is_empty() {
                                let source = Source::decode(
                                    token(&left.text, left.site)?.text.into_bytes(),
                                    Encoding::Utf8,
                                )?;
                                let lexed = crate::lexer::lex(&source).map_err(|_| {
                                    fail(site, "token paste does not form valid tokens")
                                })?;
                                // Reference pasting is textual, so the result may re-lex
                                // as several tokens; adv3.h relies on this in
                                // `class name##Action: ##baseClass`.
                                for piece in &lexed[..lexed.len().saturating_sub(1)] {
                                    let text: String = piece.spelling(&source).collect();
                                    push(&mut pasted, token(&text, left.site)?)?;
                                }
                            }
                        }
                        at += 1;
                    } else {
                        push(&mut pasted, token(&body[at].text, body[at].site)?)?;
                        at += 1;
                    }
                }
                body = pasted;
                push(&mut body, token(" ", site)?)?;
                let name = token(name, site)?.text;
                stack[last].1 = consumed;
                push(&mut stack, (body, 0, Some(name)))?;
            } else {
                push(&mut output, token(&t.text, site)?)?;
                stack[last].1 += 1;
            }
        }
        Ok(output)
    }
    fn process(mut self, path: PathBuf) -> Result<Preprocessed, Error> {
        let result = (|| -> Result<(), Diagnostic> {
            self.load(path.clone())?;
            while !self.frames.is_empty() {
                let f = self.frames.len() - 1;
                if self.frames[f].cursor == self.frames[f].tokens.len() {
                    self.current = Site {
                        file: self.frames[f].file,
                        byte: self.files[self.frames[f].file].source.byte_len(),
                    };
                    if !self.frames[f].conditions.is_empty() {
                        return Err(fail(self.current, "unterminated conditional"));
                    }
                    self.frames.pop();
                    continue;
                }
                let start = self.frames[f].cursor;
                let mut end = start;
                while end < self.frames[f].tokens.len() {
                    end += 1;
                    if matches!(
                        self.frames[f].tokens[end - 1].text.as_str(),
                        "\n" | "\r" | "\r\n"
                    ) {
                        break;
                    }
                }
                self.frames[f].cursor = end;
                let mut line = Vec::new();
                for t in &self.frames[f].tokens[start..end] {
                    push(&mut line, token(&t.text, t.site)?)?;
                }
                self.current = line[0].site;
                let first = line.iter().position(|t| !t.space());
                if first.is_none_or(|i| line[i].text != "#") {
                    loop {
                        let start = self.frames[f].cursor;
                        if start == self.frames[f].tokens.len() {
                            break;
                        }
                        let mut end = start;
                        while end < self.frames[f].tokens.len() {
                            end += 1;
                            if matches!(
                                self.frames[f].tokens[end - 1].text.as_str(),
                                "\n" | "\r" | "\r\n"
                            ) {
                                break;
                            }
                        }
                        let next = &self.frames[f].tokens[start..end];
                        if next
                            .iter()
                            .find(|t| !t.space())
                            .is_some_and(|t| t.text == "#")
                        {
                            break;
                        }
                        for t in next {
                            push(&mut line, token(&t.text, t.site)?)?;
                        }
                        self.frames[f].cursor = end;
                    }
                }
                let active = self.frames[f].conditions.last().is_none_or(|c| c.active);
                if let Some(first) = first.filter(|i| line[*i].text == "#") {
                    let tokens = &line[first + 1..];
                    let mut significant = Vec::new();
                    for (i, t) in tokens.iter().enumerate() {
                        if !t.space() {
                            push(&mut significant, i)?;
                        }
                    }
                    let Some(&name_i) = significant.first() else {
                        continue;
                    };
                    let name = tokens[name_i].text.as_str();
                    let args = &tokens[name_i + 1..];
                    let mut words = Vec::new();
                    for t in args {
                        if !t.space() {
                            push(&mut words, t)?;
                        }
                    }
                    match name {
                        "ifdef" | "ifndef" => {
                            if words.len() != 1 || !words[0].word() {
                                return Err(fail(
                                    self.current,
                                    "conditional requires a macro name",
                                ));
                            }
                            let exists = self.macros.contains_key(&words[0].text);
                            let selected = active && (exists == (name == "ifdef"));
                            push(
                                &mut self.frames[f].conditions,
                                Condition {
                                    parent: active,
                                    active: selected,
                                    taken: selected,
                                    had_else: false,
                                },
                            )?;
                        }
                        "else" => {
                            let c = self.frames[f]
                                .conditions
                                .last_mut()
                                .ok_or(fail(self.current, "unmatched else"))?;
                            if c.had_else || !words.is_empty() {
                                return Err(fail(self.current, "invalid else"));
                            }
                            c.active = c.parent && !c.taken;
                            c.taken = true;
                            c.had_else = true;
                        }
                        "endif" => {
                            if !words.is_empty() || self.frames[f].conditions.pop().is_none() {
                                return Err(fail(self.current, "unmatched endif"));
                            }
                        }
                        "if" | "elif" => {
                            return Err(fail(
                                self.current,
                                "conditional expressions are not implemented",
                            ));
                        }
                        _ if !active => {}
                        "charset" => {
                            if self.current.byte != 0 {
                                return Err(fail(
                                    self.current,
                                    "charset must be first in its file",
                                ));
                            }
                        }
                        "pragma" if words.len() == 1 && words[0].text == "once" => {
                            self.once
                                .try_reserve(1)
                                .map_err(|_| Diagnostic::resource(0))?;
                            self.once
                                .insert(self.files[self.frames[f].file].path.clone());
                        }
                        "define" => {
                            let Some(name) = words.first() else {
                                return Err(fail(self.current, "missing macro name"));
                            };
                            if !name.word() {
                                return Err(fail(self.current, "invalid macro name"));
                            }
                            let index = args
                                .iter()
                                .position(|t| t.site.byte == name.site.byte)
                                .expect("macro name");
                            let rest = &args[index + 1..];
                            let mut parameters = None;
                            let mut variadic = false;
                            let mut body_start = 0;
                            if rest.first().is_some_and(|t| t.text == "(") {
                                let mut params = Vec::new();
                                let mut expect_name = true;
                                body_start = 1;
                                while body_start < rest.len() && rest[body_start].text != ")" {
                                    let t = &rest[body_start];
                                    body_start += 1;
                                    if t.space() {
                                        continue;
                                    }
                                    if expect_name && t.word() {
                                        if params.contains(&t.text) {
                                            return Err(fail(t.site, "duplicate macro parameter"));
                                        }
                                        push(&mut params, token(&t.text, t.site)?.text)?;
                                        expect_name = false;
                                    } else if !expect_name && t.text == "," {
                                        expect_name = true;
                                    } else if !expect_name
                                        && t.text == "."
                                        && rest.get(body_start).is_some_and(|t| t.text == ".")
                                        && rest.get(body_start + 1).is_some_and(|t| t.text == ".")
                                    {
                                        body_start += 2;
                                        variadic = true;
                                        while body_start < rest.len() && rest[body_start].space() {
                                            body_start += 1;
                                        }
                                        if rest.get(body_start).is_none_or(|t| t.text != ")") {
                                            return Err(fail(
                                                t.site,
                                                "variadic parameter must be last",
                                            ));
                                        }
                                    } else {
                                        return Err(fail(
                                            t.site,
                                            "unsupported macro parameter list",
                                        ));
                                    }
                                }
                                if body_start == rest.len() {
                                    return Err(fail(
                                        self.current,
                                        "unterminated macro parameters",
                                    ));
                                }
                                body_start += 1;
                                if expect_name && !params.is_empty() {
                                    return Err(fail(
                                        self.current,
                                        "trailing macro parameter comma",
                                    ));
                                }
                                parameters = Some(params);
                            }
                            let mut body = Vec::new();
                            for t in &rest[body_start..] {
                                if !matches!(t.text.as_str(), "\n" | "\r" | "\r\n") {
                                    push(&mut body, token(&t.text, t.site)?)?;
                                }
                            }
                            self.macros
                                .try_reserve(1)
                                .map_err(|_| Diagnostic::resource(0))?;
                            self.macros.insert(
                                token(&name.text, name.site)?.text,
                                Macro {
                                    parameters,
                                    body,
                                    variadic,
                                },
                            );
                        }
                        "undef" => {
                            if words.len() != 1 {
                                return Err(fail(self.current, "undef requires a macro name"));
                            }
                            self.macros.remove(&words[0].text);
                        }
                        "include" => {
                            let expanded = self.expand(args)?;
                            let mut text = String::new();
                            for t in &expanded {
                                append(&mut text, &t.text)?;
                            }
                            let text = text.trim();
                            let quoted = text.starts_with('"') && text.ends_with('"');
                            if !quoted && !(text.starts_with('<') && text.ends_with('>')) {
                                return Err(fail(self.current, "invalid include filename"));
                            }
                            let name = &text[1..text.len() - 1];
                            let mut found = None;
                            if quoted {
                                for frame in self.frames.iter().rev() {
                                    let candidate = self.files[frame.file]
                                        .path
                                        .parent()
                                        .unwrap_or(Path::new("."))
                                        .join(name);
                                    if candidate.is_file() {
                                        found = Some(candidate);
                                        break;
                                    }
                                }
                            }
                            if found.is_none() {
                                for dir in &self.paths {
                                    let candidate = dir.join(name);
                                    if candidate.is_file() {
                                        found = Some(candidate);
                                        break;
                                    }
                                }
                            }
                            if found.is_none()
                                && Path::new(name).is_absolute()
                                && Path::new(name).is_file()
                            {
                                found = Some(PathBuf::from(name));
                            }
                            let found =
                                found.ok_or(fail(self.current, "include file not found"))?;
                            let found = found
                                .canonicalize()
                                .map_err(|_| fail(self.current, "cannot resolve include file"))?;
                            self.load(found)?;
                        }
                        "error" => return Err(fail(self.current, "source error directive")),
                        _ => return Err(fail(self.current, "unsupported preprocessing directive")),
                    }
                    self.emit(&token("\n", self.current)?)?;
                } else if active {
                    for t in self.expand(&line)? {
                        self.emit(&t)?;
                    }
                } else {
                    self.emit(&token("\n", self.current)?)?;
                }
            }
            Ok(())
        })();
        if let Err(diagnostic) = result {
            let input = self.files.get(self.current.file);
            let (line, column) = if self.loading.is_some() {
                (0, diagnostic.byte)
            } else {
                input.map_or((1, 1), |i| i.source.location(diagnostic.byte))
            };
            return Err(Error {
                path: self
                    .loading
                    .unwrap_or_else(|| input.map_or(path, |i| i.path.clone())),
                line,
                column,
                diagnostic,
            });
        }
        let source =
            Source::decode(self.output.into_bytes(), Encoding::Utf8).map_err(|diagnostic| {
                Error {
                    path: path.clone(),
                    line: 1,
                    column: 1,
                    diagnostic,
                }
            })?;
        Ok(Preprocessed {
            source,
            files: self.files,
            mappings: self.mappings,
        })
    }
}
pub fn read(
    path: &Path,
    paths: &[PathBuf],
    encoding: Encoding,
    limit: usize,
) -> Result<Preprocessed, Error> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Processor {
        files: Vec::new(),
        frames: Vec::new(),
        macros: HashMap::new(),
        once: HashSet::new(),
        paths: paths.to_vec(),
        encoding,
        limit,
        read_bytes: 0,
        output: String::new(),
        mappings: Vec::new(),
        current: Site { file: 0, byte: 0 },
        loading: None,
    }
    .process(path)
}

/// Keep the original unexpanded source pipeline when there are no directives.
pub fn has_directives(source: &Source) -> bool {
    if !source.units().iter().any(|u| u.ch == '#') {
        return false;
    }
    let Ok(tokens) = tokenize(source, 0) else {
        return true;
    };
    let mut start = true;
    for t in tokens {
        if matches!(t.text.as_str(), "\n" | "\r" | "\r\n") {
            start = true;
            continue;
        }
        if t.space() {
            continue;
        }
        if start && t.text == "#" {
            return true;
        }
        start = false;
    }
    false
}
