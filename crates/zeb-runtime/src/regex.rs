//! TADS-dialect regular expressions.
//!
//! The pattern syntax follows the reference engine's documented dialect: `%`
//! escapes rather than backslashes, angle-bracket character classes and named
//! literals, `[]` ranges, groups with `(?:`/`(?=`/`(?!`, alternation, `*`/`+`/`?`
//! and `{n,m}` quantifiers, anchors, word-boundary assertions and `%1`-`%9` back
//! references. `<Case>`/`<NoCase>` and `<Min>`/`<Max>` set whole-pattern modes.
//!
//! Matching is a backtracking program over the subject's characters. Because the
//! reference selects the *longest* match of an ambiguous pattern (`<Max>`, the
//! default) rather than the first one a backtracking order finds, matching keeps
//! searching after a candidate and keeps the best one; `<Min>` keeps the shortest.
//! A step budget bounds pathological patterns instead of running unboundedly.

/// A compiled pattern.
#[derive(Debug)]
pub(crate) struct Pattern {
    program: Vec<Instruction>,
    classes: Vec<Class>,
    groups: usize,
    fold_case: bool,
    shortest: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Instruction {
    Char(char),
    Any,
    Class(usize),
    Split(usize, usize),
    Jump(usize),
    Save(usize),
    Backref(usize),
    Start,
    End,
    WordBegin,
    WordEnd,
    WordBoundary(bool),
    /// Zero-width sub-program; `negate` inverts success.
    Look {
        negate: bool,
        start: usize,
    },
    Match,
}

#[derive(Debug, Default, Clone)]
struct Class {
    negated: bool,
    chars: Vec<char>,
    ranges: Vec<(char, char)>,
    kinds: Vec<Kind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Alpha,
    Digit,
    Upper,
    Lower,
    AlphaNum,
    Space,
    VSpace,
    Punct,
    Newline,
    Word,
}

impl Kind {
    fn matches(self, ch: char) -> bool {
        match self {
            Self::Alpha => ch.is_alphabetic(),
            Self::Digit => ch.is_numeric(),
            Self::Upper => ch.is_uppercase(),
            Self::Lower => ch.is_lowercase(),
            Self::AlphaNum => ch.is_alphabetic() || ch.is_numeric(),
            Self::Space => ch.is_whitespace(),
            Self::VSpace => matches!(
                ch,
                '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{2028}' | '\u{2029}'
            ),
            Self::Punct => {
                ch.is_ascii_punctuation()
                    || (!ch.is_alphanumeric() && !ch.is_whitespace() && !ch.is_control())
            }
            Self::Newline => matches!(ch, '\n' | '\r' | '\u{8}' | '\u{2028}' | '\u{2029}'),
            Self::Word => ch.is_alphabetic() || ch.is_numeric(),
        }
    }
}

impl Class {
    fn matches(&self, ch: char, fold: bool) -> bool {
        let hit = |c: char| {
            self.chars.contains(&c)
                || self.ranges.iter().any(|(lo, hi)| *lo <= c && c <= *hi)
                || self.kinds.iter().any(|kind| kind.matches(c))
        };
        let mut found = hit(ch);
        if !found && fold {
            found = ch
                .to_lowercase()
                .chain(ch.to_uppercase())
                .any(|folded| folded != ch && hit(folded));
        }
        found != self.negated
    }
}

/// A successful match: subject character offsets, plus group captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Match {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) groups: Vec<Option<(usize, usize)>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Error {
    /// The pattern is not valid in this dialect.
    Pattern,
    /// Matching exceeded its step budget.
    Budget,
    /// Allocation failed.
    Memory,
}

const NAMED: &[(&str, char)] = &[
    ("langle", '<'),
    ("rangle", '>'),
    ("vbar", '|'),
    ("caret", '^'),
    ("squote", '\''),
    ("dquote", '"'),
    ("star", '*'),
    ("question", '?'),
    ("percent", '%'),
    ("dot", '.'),
    ("period", '.'),
    ("plus", '+'),
    ("lsquare", '['),
    ("rsquare", ']'),
    ("lparen", '('),
    ("rparen", ')'),
    ("lbrace", '{'),
    ("rbrace", '}'),
    ("dollar", '$'),
    ("backslash", '\\'),
    ("return", '\r'),
    ("linefeed", '\n'),
    ("tab", '\t'),
    ("nul", '\0'),
    ("null", '\0'),
];

const KINDS: &[(&str, Kind)] = &[
    ("alpha", Kind::Alpha),
    ("digit", Kind::Digit),
    ("upper", Kind::Upper),
    ("lower", Kind::Lower),
    ("alphanum", Kind::AlphaNum),
    ("space", Kind::Space),
    ("punct", Kind::Punct),
    ("newline", Kind::Newline),
    ("vspace", Kind::VSpace),
];

#[derive(Debug)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class(usize),
    Concat(Vec<Node>),
    Alternate(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: usize,
        max: Option<usize>,
        /// `x*?` and friends take the shortest working match.
        lazy: bool,
    },
    Group(Option<usize>, Box<Node>),
    Look {
        negate: bool,
        node: Box<Node>,
    },
    Backref(usize),
    Start,
    End,
    WordBegin,
    WordEnd,
    WordBoundary(bool),
}

struct Parser<'a> {
    text: &'a [char],
    at: usize,
    classes: Vec<Class>,
    groups: usize,
    fold_case: bool,
    shortest: bool,
}

fn push<T>(v: &mut Vec<T>, item: T) -> Result<(), Error> {
    v.try_reserve(1).map_err(|_| Error::Memory)?;
    v.push(item);
    Ok(())
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text.get(self.at).copied()
    }
    fn take(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.at += 1;
            return true;
        }
        false
    }

    /// alternation := concat ('|' concat)*
    fn alternation(&mut self) -> Result<Node, Error> {
        let mut branches = Vec::new();
        loop {
            let branch = self.concat()?;
            push(&mut branches, branch)?;
            if !self.take('|') {
                break;
            }
        }
        Ok(if branches.len() == 1 {
            branches.pop().expect("one branch")
        } else {
            Node::Alternate(branches)
        })
    }

    /// concat := repeat*
    fn concat(&mut self) -> Result<Node, Error> {
        let mut items = Vec::new();
        while let Some(ch) = self.peek() {
            if ch == '|' || ch == ')' {
                break;
            }
            if let Some(node) = self.repeat()? {
                push(&mut items, node)?;
            }
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().expect("one item"),
            _ => Node::Concat(items),
        })
    }

    /// repeat := atom ('*' | '+' | '?' | '{n,m}')?
    fn repeat(&mut self) -> Result<Option<Node>, Error> {
        let Some(node) = self.atom()? else {
            return Ok(None);
        };
        let (min, max) = match self.peek() {
            Some('*') => {
                self.at += 1;
                (0, None)
            }
            Some('+') => {
                self.at += 1;
                (1, None)
            }
            Some('?') => {
                self.at += 1;
                (0, Some(1))
            }
            Some('{') => {
                let save = self.at;
                self.at += 1;
                match self.bounds() {
                    Some(bounds) => bounds,
                    None => {
                        self.at = save;
                        return Ok(Some(node));
                    }
                }
            }
            _ => return Ok(Some(node)),
        };
        if max.is_some_and(|high| high < min) {
            return Err(Error::Pattern);
        }
        // A following '?' asks for the shortest working match of this quantifier.
        let lazy = self.take('?');
        Ok(Some(Node::Repeat {
            node: Box::new(node),
            min,
            max,
            lazy,
        }))
    }

    fn bounds(&mut self) -> Option<(usize, Option<usize>)> {
        let low = self.number()?;
        let high = if self.take(',') {
            if self.peek() == Some('}') {
                None
            } else {
                Some(self.number()?)
            }
        } else {
            Some(low)
        };
        self.take('}').then_some((low, high))
    }

    fn number(&mut self) -> Option<usize> {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.at += 1;
        }
        (self.at > start).then(|| {
            self.text[start..self.at].iter().fold(0usize, |n, c| {
                n.saturating_mul(10)
                    .saturating_add(*c as usize - '0' as usize)
            })
        })
    }

    /// Returns None for a construct that matches nothing, such as a mode flag.
    fn atom(&mut self) -> Result<Option<Node>, Error> {
        let Some(ch) = self.peek() else {
            return Ok(None);
        };
        self.at += 1;
        let node = match ch {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let (capture, look) = if self.take('?') {
                    match self.peek() {
                        Some(':') => {
                            self.at += 1;
                            (None, None)
                        }
                        Some('=') => {
                            self.at += 1;
                            (None, Some(false))
                        }
                        Some('!') => {
                            self.at += 1;
                            (None, Some(true))
                        }
                        _ => return Err(Error::Pattern),
                    }
                } else {
                    self.groups += 1;
                    (Some(self.groups), None)
                };
                let body = self.alternation()?;
                if !self.take(')') {
                    return Err(Error::Pattern);
                }
                match look {
                    Some(negate) => Node::Look {
                        negate,
                        node: Box::new(body),
                    },
                    None => Node::Group(capture, Box::new(body)),
                }
            }
            '[' => {
                let class = self.square_class()?;
                Node::Class(self.add_class(class)?)
            }
            '<' => match self.angle()? {
                Some(class) => Node::Class(self.add_class(class)?),
                None => return Ok(None),
            },
            '%' => {
                let Some(next) = self.peek() else {
                    return Err(Error::Pattern);
                };
                self.at += 1;
                match next {
                    '1'..='9' => Node::Backref(next as usize - '0' as usize),
                    '<' => Node::WordBegin,
                    '>' => Node::WordEnd,
                    'b' => Node::WordBoundary(false),
                    'B' => Node::WordBoundary(true),
                    'w' | 'W' | 's' | 'S' | 'd' | 'D' | 'v' | 'V' => {
                        let kind = match next.to_ascii_lowercase() {
                            'w' => Kind::Word,
                            's' => Kind::Space,
                            'v' => Kind::VSpace,
                            _ => Kind::Digit,
                        };
                        let mut kinds = Vec::new();
                        push(&mut kinds, kind)?;
                        let class = Class {
                            negated: next.is_uppercase(),
                            kinds,
                            ..Class::default()
                        };
                        Node::Class(self.add_class(class)?)
                    }
                    literal => Node::Char(literal),
                }
            }
            literal => Node::Char(literal),
        };
        Ok(Some(node))
    }

    fn add_class(&mut self, class: Class) -> Result<usize, Error> {
        push(&mut self.classes, class)?;
        Ok(self.classes.len() - 1)
    }

    /// `[abc]`, `[a-z]`, `[^...]`
    fn square_class(&mut self) -> Result<Class, Error> {
        let mut class = Class {
            negated: self.take('^'),
            ..Class::default()
        };
        let mut first = true;
        loop {
            let Some(ch) = self.peek() else {
                return Err(Error::Pattern);
            };
            if ch == ']' && !first {
                self.at += 1;
                return Ok(class);
            }
            first = false;
            self.at += 1;
            let ch = if ch == '%' {
                let escaped = self.peek().ok_or(Error::Pattern)?;
                self.at += 1;
                escaped
            } else {
                ch
            };
            if self.peek() == Some('-') && self.text.get(self.at + 1).is_some_and(|c| *c != ']') {
                self.at += 1;
                let hi = self.peek().ok_or(Error::Pattern)?;
                self.at += 1;
                push(&mut class.ranges, (ch, hi))?;
            } else {
                push(&mut class.chars, ch)?;
            }
        }
    }

    /// `<alpha|digit|a-m|squote>`, `<^space>`, and the whole-pattern mode flags.
    fn angle(&mut self) -> Result<Option<Class>, Error> {
        let start = self.at;
        let mut end = self.at;
        while self.text.get(end).is_some_and(|c| *c != '>') {
            end += 1;
        }
        if end >= self.text.len() {
            return Err(Error::Pattern);
        }
        let body: String = self.text[start..end].iter().collect();
        self.at = end + 1;
        let lowered = body.to_ascii_lowercase();
        match lowered.as_str() {
            "case" => {
                self.fold_case = false;
                return Ok(None);
            }
            "nocase" => {
                self.fold_case = true;
                return Ok(None);
            }
            "min" => {
                self.shortest = true;
                return Ok(None);
            }
            "max" => {
                self.shortest = false;
                return Ok(None);
            }
            "fb" | "firstbegin" | "fe" | "firstend" => return Ok(None),
            _ => {}
        }
        let mut class = Class::default();
        // Class names are case-insensitive, but literal characters keep their case.
        let mut body = body.as_str();
        if let Some(rest) = body.strip_prefix('^') {
            class.negated = true;
            body = rest;
        }
        for part in body.split('|') {
            if part.is_empty() {
                return Err(Error::Pattern);
            }
            let name = part.to_ascii_lowercase();
            if let Some((_, kind)) = KINDS.iter().find(|(known, _)| *known == name) {
                push(&mut class.kinds, *kind)?;
            } else if let Some((_, ch)) = NAMED.iter().find(|(known, _)| *known == name) {
                push(&mut class.chars, *ch)?;
            } else {
                let chars: Vec<char> = part.chars().collect();
                match chars[..] {
                    [ch] => push(&mut class.chars, ch)?,
                    [lo, '-', hi] => push(&mut class.ranges, (lo, hi))?,
                    _ => return Err(Error::Pattern),
                }
            }
        }
        Ok(Some(class))
    }
}

/// Compile a node tree into the backtracking program.
struct Compiler {
    program: Vec<Instruction>,
    greedy: bool,
}

impl Compiler {
    fn emit(&mut self, instruction: Instruction) -> Result<usize, Error> {
        push(&mut self.program, instruction)?;
        Ok(self.program.len() - 1)
    }

    fn node(&mut self, node: &Node) -> Result<(), Error> {
        match node {
            Node::Empty => {}
            Node::Char(ch) => {
                self.emit(Instruction::Char(*ch))?;
            }
            Node::Any => {
                self.emit(Instruction::Any)?;
            }
            Node::Class(index) => {
                self.emit(Instruction::Class(*index))?;
            }
            Node::Start => {
                self.emit(Instruction::Start)?;
            }
            Node::End => {
                self.emit(Instruction::End)?;
            }
            Node::WordBegin => {
                self.emit(Instruction::WordBegin)?;
            }
            Node::WordEnd => {
                self.emit(Instruction::WordEnd)?;
            }
            Node::WordBoundary(negate) => {
                self.emit(Instruction::WordBoundary(*negate))?;
            }
            Node::Backref(group) => {
                self.emit(Instruction::Backref(*group))?;
            }
            Node::Concat(items) => {
                for item in items {
                    self.node(item)?;
                }
            }
            Node::Alternate(branches) => {
                let mut jumps = Vec::new();
                for (index, branch) in branches.iter().enumerate() {
                    if index + 1 == branches.len() {
                        self.node(branch)?;
                        break;
                    }
                    let split = self.emit(Instruction::Split(0, 0))?;
                    let body = self.program.len();
                    self.node(branch)?;
                    let jump = self.emit(Instruction::Jump(0))?;
                    push(&mut jumps, jump)?;
                    let next = self.program.len();
                    self.program[split] = Instruction::Split(body, next);
                }
                let end = self.program.len();
                for jump in jumps {
                    self.program[jump] = Instruction::Jump(end);
                }
            }
            Node::Group(capture, body) => {
                if let Some(index) = capture {
                    self.emit(Instruction::Save(index * 2))?;
                }
                self.node(body)?;
                if let Some(index) = capture {
                    self.emit(Instruction::Save(index * 2 + 1))?;
                }
            }
            Node::Look { negate, node } => {
                let head = self.emit(Instruction::Look {
                    negate: *negate,
                    start: 0,
                })?;
                let jump = self.emit(Instruction::Jump(0))?;
                let start = self.program.len();
                self.node(node)?;
                self.emit(Instruction::Match)?;
                let after = self.program.len();
                self.program[head] = Instruction::Look {
                    negate: *negate,
                    start,
                };
                self.program[jump] = Instruction::Jump(after);
            }
            Node::Repeat {
                node,
                min,
                max,
                lazy,
            } => {
                let greedy = self.greedy && !lazy;
                for _ in 0..*min {
                    self.node(node)?;
                }
                match max {
                    Some(high) => {
                        let mut splits = Vec::new();
                        for _ in *min..*high {
                            let split = self.emit(Instruction::Split(0, 0))?;
                            push(&mut splits, split)?;
                            let body = self.program.len();
                            self.node(node)?;
                            let _ = body;
                        }
                        let end = self.program.len();
                        for split in splits {
                            let body = split + 1;
                            self.program[split] = if greedy {
                                Instruction::Split(body, end)
                            } else {
                                Instruction::Split(end, body)
                            };
                        }
                    }
                    None => {
                        let split = self.emit(Instruction::Split(0, 0))?;
                        let body = self.program.len();
                        self.node(node)?;
                        self.emit(Instruction::Jump(split))?;
                        let end = self.program.len();
                        self.program[split] = if greedy {
                            Instruction::Split(body, end)
                        } else {
                            Instruction::Split(end, body)
                        };
                    }
                }
            }
        }
        Ok(())
    }
}

fn is_word(ch: Option<char>) -> bool {
    ch.is_some_and(|c| c.is_alphabetic() || c.is_numeric())
}

impl Pattern {
    pub(crate) fn compile(text: &[char]) -> Result<Self, Error> {
        let mut parser = Parser {
            text,
            at: 0,
            classes: Vec::new(),
            groups: 0,
            fold_case: false,
            shortest: false,
        };
        let tree = parser.alternation()?;
        if parser.at != text.len() {
            return Err(Error::Pattern);
        }
        let mut compiler = Compiler {
            program: Vec::new(),
            greedy: !parser.shortest,
        };
        compiler.node(&tree)?;
        compiler.emit(Instruction::Match)?;
        Ok(Self {
            program: compiler.program,
            classes: parser.classes,
            groups: parser.groups,
            fold_case: parser.fold_case,
            shortest: parser.shortest,
        })
    }

    /// Match anchored at `at`, as `rexMatch` does.
    pub(crate) fn match_at(&self, subject: &[char], at: usize) -> Result<Option<Match>, Error> {
        let mut budget = 200_000usize;
        self.run(subject, at, 0, &mut budget)
    }

    /// Find the first match at or after `from`, as `rexSearch` does.
    pub(crate) fn search(&self, subject: &[char], from: usize) -> Result<Option<Match>, Error> {
        let mut budget = 1_000_000usize;
        for start in from..=subject.len() {
            if let Some(found) = self.run(subject, start, 0, &mut budget)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    fn equal(&self, a: char, b: char) -> bool {
        a == b || self.fold_case && a.to_lowercase().eq(b.to_lowercase())
    }

    /// Backtracking search from `pc`, keeping the best end position.
    fn run(
        &self,
        subject: &[char],
        start: usize,
        pc: usize,
        budget: &mut usize,
    ) -> Result<Option<Match>, Error> {
        let mut saves: Vec<Option<usize>> = vec![None; (self.groups + 1) * 2];
        let mut stack: Vec<(usize, usize, Vec<Option<usize>>)> = Vec::new();
        let mut best: Option<Match> = None;
        let (mut pc, mut pos) = (pc, start);
        loop {
            *budget = budget.checked_sub(1).ok_or(Error::Budget)?;
            let mut failed = false;
            match self.program[pc] {
                Instruction::Char(ch) => match subject.get(pos) {
                    Some(got) if self.equal(ch, *got) => {
                        pos += 1;
                        pc += 1;
                    }
                    _ => failed = true,
                },
                Instruction::Any => match subject.get(pos) {
                    Some(_) => {
                        pos += 1;
                        pc += 1;
                    }
                    None => failed = true,
                },
                Instruction::Class(index) => match subject.get(pos) {
                    Some(got) if self.classes[index].matches(*got, self.fold_case) => {
                        pos += 1;
                        pc += 1;
                    }
                    _ => failed = true,
                },
                Instruction::Split(a, b) => {
                    let mut alternative = saves.clone();
                    alternative.shrink_to_fit();
                    push(&mut stack, (b, pos, alternative))?;
                    pc = a;
                }
                Instruction::Jump(a) => pc = a,
                Instruction::Save(slot) => {
                    saves[slot] = Some(pos);
                    pc += 1;
                }
                Instruction::Backref(group) => {
                    let (from, to) = match (saves.get(group * 2), saves.get(group * 2 + 1)) {
                        (Some(Some(from)), Some(Some(to))) => (*from, *to),
                        _ => {
                            failed = true;
                            (0, 0)
                        }
                    };
                    if !failed {
                        let len = to - from;
                        let fits = pos + len <= subject.len()
                            && (0..len).all(|i| self.equal(subject[from + i], subject[pos + i]));
                        if fits {
                            pos += len;
                            pc += 1;
                        } else {
                            failed = true;
                        }
                    }
                }
                Instruction::Start => {
                    if pos == 0 {
                        pc += 1;
                    } else {
                        failed = true;
                    }
                }
                Instruction::End => {
                    if pos == subject.len() {
                        pc += 1;
                    } else {
                        failed = true;
                    }
                }
                Instruction::WordBegin => {
                    if is_word(subject.get(pos).copied())
                        && (pos == 0 || !is_word(subject.get(pos - 1).copied()))
                    {
                        pc += 1;
                    } else {
                        failed = true;
                    }
                }
                Instruction::WordEnd => {
                    if pos > 0
                        && is_word(subject.get(pos - 1).copied())
                        && !is_word(subject.get(pos).copied())
                    {
                        pc += 1;
                    } else {
                        failed = true;
                    }
                }
                Instruction::WordBoundary(negate) => {
                    let before = pos > 0 && is_word(subject.get(pos - 1).copied());
                    let after = is_word(subject.get(pos).copied());
                    if (before != after) != negate {
                        pc += 1;
                    } else {
                        failed = true;
                    }
                }
                Instruction::Look { negate, start } => {
                    let found = self.run(subject, pos, start, budget)?.is_some();
                    if found != negate {
                        pc += 1;
                    } else {
                        failed = true;
                    }
                }
                Instruction::Match => {
                    let candidate = Match {
                        start,
                        end: pos,
                        groups: (1..=self.groups)
                            .map(|group| match (saves[group * 2], saves[group * 2 + 1]) {
                                (Some(from), Some(to)) => Some((from, to)),
                                _ => None,
                            })
                            .collect(),
                    };
                    let better = match &best {
                        None => true,
                        Some(previous) if self.shortest => candidate.end < previous.end,
                        Some(previous) => candidate.end > previous.end,
                    };
                    if better {
                        best = Some(candidate);
                    }
                    failed = true; // keep searching for a better candidate
                }
            }
            if failed {
                match stack.pop() {
                    Some((next_pc, next_pos, next_saves)) => {
                        pc = next_pc;
                        pos = next_pos;
                        saves = next_saves;
                    }
                    None => return Ok(best),
                }
            }
        }
    }
}

#[cfg(test)]
mod parity {
    use super::*;

    /// Fixed compatibility expectations: pattern, subject, `rexMatch` length,
    /// `rexSearch` (zero-based start, length), and group (number, start, length).
    type Case = (
        &'static str,
        &'static str,
        Option<usize>,
        Option<(usize, usize)>,
        &'static [(usize, usize, usize)],
    );
    const CASES: &[Case] = &[
        ("<Space>+", "   x", Some(3), Some((0, 3)), &[]),
        ("[.,;:?!]", ",x", Some(1), Some((0, 1)), &[]),
        ("<alpha>+", "abc12", Some(3), Some((0, 3)), &[]),
        (
            "<NoCase>(twenty|thirty)-",
            "Thirty-four",
            Some(7),
            Some((0, 7)),
            &[(1, 0, 6)],
        ),
        (
            "<alphanum|-|squote>+",
            "o'clock-ish!",
            Some(11),
            Some((0, 11)),
            &[],
        ),
        (".*[^aeiou]y$", "happy", Some(5), Some((0, 5)), &[]),
        (
            ".*(o|ch|sh)$",
            "search",
            Some(6),
            Some((0, 6)),
            &[(1, 4, 2)],
        ),
        (
            "<alpha>(<^alpha>|$)",
            "a!",
            Some(2),
            Some((0, 2)),
            &[(1, 1, 1)],
        ),
        (
            "<nocase>[aefhilmnorsx]",
            "Hello",
            Some(1),
            Some((0, 1)),
            &[],
        ),
        (
            "1[18](<^digit>|$)",
            "18 ",
            Some(3),
            Some((0, 3)),
            &[(1, 2, 1)],
        ),
        (
            "<case><lower|A|E|I|M|U|V>",
            "Apple",
            Some(1),
            Some((0, 1)),
            &[],
        ),
        ("<upper|digit>", "7", Some(1), Some((0, 1)), &[]),
        ("[^aeoiu]y", "sky", None, Some((1, 2)), &[]),
        (
            "<alpha|-|&>+",
            "well-known&co",
            Some(13),
            Some((0, 13)),
            &[],
        ),
        ("(?!<AlphaNum>)", "!", Some(0), Some((0, 0)), &[]),
        ("a(?=b)", "ab", Some(1), Some((0, 1)), &[]),
        ("a(?!b)", "ac", Some(1), Some((0, 1)), &[]),
        ("(ab)+%1", "ababab", Some(6), Some((0, 6)), &[(1, 2, 2)]),
        ("%<word%>", "word", Some(4), Some((0, 4)), &[]),
        ("%w+", "hi there", Some(2), Some((0, 2)), &[]),
        ("%d+", "x42y", None, Some((1, 2)), &[]),
        ("<^space>+", "ab cd", Some(2), Some((0, 2)), &[]),
        ("<langle>%w+<rangle>", "<tag>", Some(5), Some((0, 5)), &[]),
        ("a{2,3}", "aaaa", Some(3), Some((0, 3)), &[]),
        ("a{2}", "aaa", Some(2), Some((0, 2)), &[]),
        ("a{2,}", "aaaa", Some(4), Some((0, 4)), &[]),
        ("<Min>a+", "aaa", Some(1), Some((0, 1)), &[]),
        ("<Max>a+", "aaa", Some(3), Some((0, 3)), &[]),
        ("colou?r", "color", Some(5), Some((0, 5)), &[]),
        ("(a|ab)c", "abc", Some(3), Some((0, 3)), &[(1, 0, 2)]),
        ("^$", "", Some(0), Some((0, 0)), &[]),
        ("x*", "yyy", Some(0), Some((0, 0)), &[]),
        (
            "(<alpha>+)<space>+(<alpha>+)",
            "hello   world",
            Some(13),
            Some((0, 13)),
            &[(1, 0, 5), (2, 8, 5)],
        ),
        ("<period>", "a.b", None, Some((1, 1)), &[]),
        ("%.", "a.b", None, Some((1, 1)), &[]),
        ("[a-m]+", "hijkz", Some(4), Some((0, 4)), &[]),
        ("<^lparen>+", "abc(d", Some(3), Some((0, 3)), &[]),
        ("<nocase>abc", "ABC", Some(3), Some((0, 3)), &[]),
        (
            "(a)(b)?(c)",
            "ac",
            Some(2),
            Some((0, 2)),
            &[(1, 0, 1), (3, 1, 1)],
        ),
    ];

    #[test]
    fn matches_the_reference_engine() {
        for (pattern, subject, length, search, groups) in CASES {
            let compiled: Vec<char> = pattern.chars().collect();
            let text: Vec<char> = subject.chars().collect();
            let pattern_compiled =
                Pattern::compile(&compiled).unwrap_or_else(|error| panic!("{pattern}: {error:?}"));
            let anchored = pattern_compiled
                .match_at(&text, 0)
                .unwrap_or_else(|error| panic!("{pattern}: {error:?}"));
            assert_eq!(
                anchored.as_ref().map(|found| found.end - found.start),
                *length,
                "rexMatch {pattern} on {subject}"
            );
            let found = pattern_compiled
                .search(&text, 0)
                .unwrap_or_else(|error| panic!("{pattern}: {error:?}"));
            assert_eq!(
                found.as_ref().map(|f| (f.start, f.end - f.start)),
                *search,
                "rexSearch {pattern} on {subject}"
            );
            for (number, start, len) in *groups {
                let capture = found
                    .as_ref()
                    .and_then(|f| f.groups.get(number - 1).copied().flatten());
                assert_eq!(
                    capture,
                    Some((*start, start + len)),
                    "group {number} of {pattern} on {subject}"
                );
            }
        }
    }
}
