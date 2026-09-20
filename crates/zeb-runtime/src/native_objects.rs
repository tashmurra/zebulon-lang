//! Pointer-free native helpers for the generated compiler/runtime boundary.
use crate::grammar::{self, Child, Forest, Rule, StaticGrammar, StaticTerm, Symbol};
use crate::objects::ValueIteration;
use crate::objects::{
    Capture, Construction, Error, ExceptionOwner, Limits, MemberCall, ObjectRef, Objects,
    PropertyId, PublishedConstruction, Store, Value,
};
use crate::regex::{Match, Pattern};
use crate::sort::{Exchange, QuickSort};
use std::cell::{Cell, RefCell};

enum Activation {
    Owned(Construction),
    Published(PublishedConstruction),
}
impl Activation {
    fn reference(&self) -> Option<ObjectRef> {
        match self {
            Self::Owned(value) => value.reference(),
            Self::Published(value) => value.reference(),
        }
    }
}

#[derive(Clone, Copy)]
struct Checkpoint {
    id: u64,
    iterations: usize,
    constructions: usize,
    members: usize,
    initializers: usize,
    locals: usize,
    parses: usize,
    sorts: usize,
}

#[derive(Default)]
struct State {
    store: Option<Store>,
    kind: u32,
    init_regions: Vec<ObjectRef>,
    constant_region: Option<ObjectRef>,
    /// Module constant lists in construction order; indices match compiler plan lists.
    constant_lists: Vec<ObjectRef>,
    local_owners: Vec<Option<ObjectRef>>,
    returned_owners: Vec<(ObjectRef, u64)>,
    constructions: Vec<Activation>,
    exceptions: Vec<(ExceptionOwner, Option<u64>)>,
    member_calls: Vec<(ObjectRef, Option<MemberCall>)>,
    iterations: Vec<(ValueIteration, Option<Value>)>,
    /// Grammar parses awaiting constructor calls, innermost last.
    parses: Vec<PendingParse>,
    /// Vector sorts suspended at a comparison, innermost last.
    sorts: Vec<PendingSort>,
    /// Compiled patterns, keyed by pattern text, and the last match.
    patterns: Vec<(Vec<char>, Pattern)>,
    /// `RexPattern` objects and the compiled pattern each one holds.
    pattern_objects: Vec<(ObjectRef, usize)>,
    last_match: Option<(Vec<char>, Match)>,
    /// Text objects made from literals, one per literal index. Literal text is
    /// immutable module data , so reading the same literal twice
    /// shares one object instead of allocating another.
    literal_texts: Vec<(u32, ObjectRef)>,
    /// The instant the first `getTime(GetTimeTicks)` call established.
    tick_origin: Option<std::time::Instant>,
    /// Whether this program uses World/Turn lifetimes. With them a
    /// value needs no owner scope: its class says when it goes.
    lifetimes: bool,
    /// Which handoff this session uses, so one process can run several
    /// independently.
    slot: u64,
    /// Completed cycles, oldest first, each replayable backwards.
    history: Vec<Cycle>,
    /// Entities the host's last action intent named.
    subjects: Vec<crate::session::Scalar>,
    /// Relations between entities, kept as tables beside the store.
    relations: crate::relations::Relations,
    label_families: std::collections::HashMap<usize, (u32, u32)>,
    output: crate::output::Output,
    checkpoints: Vec<Checkpoint>,
    next_checkpoint: u64,
    /// The grammar and literal tables arriving a word at a time.
    ///
    /// The boundary is pointer-free, so a program hands its tables over as
    /// words like everything else. They are accumulated here and decoded once,
    /// at the end; the decoded copy is process-wide, not session-wide, so it
    /// does not live in this struct.
    pending_tables: Vec<u8>,
    save_schema: [u8; 32],
    save_schema_words: u8,
}
thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static LAST_STATUS: Cell<u32> = const { Cell::new(0) };
    static LAST_KIND: Cell<u32> = const { Cell::new(0) };
}

/// The reference's portable names for keystrokes that are not characters, so a
/// host can deliver an arrow key or a function key and a game can recognise it.
/// Provenance: the name spellings are the interface `inputKey` has always
/// answered with (frobtads `tads3/vmbiftio.cpp`, design reference only); no code
/// is taken from it. A bracketed reply that is not one of these reads as the
/// reference's invalid-key name rather than as four separate characters.
const EXTENDED_KEY_NAMES: [&str; 31] = [
    "[up]",
    "[down]",
    "[right]",
    "[left]",
    "[end]",
    "[home]",
    "[del-eol]",
    "[del-line]",
    "[del]",
    "[scroll]",
    "[page up]",
    "[page down]",
    "[top]",
    "[bottom]",
    "[f1]",
    "[f2]",
    "[f3]",
    "[f4]",
    "[f5]",
    "[f6]",
    "[f7]",
    "[f8]",
    "[f9]",
    "[f10]",
    "[tab]",
    "[word-left]",
    "[word-right]",
    "[del-word]",
    "[eof]",
    "[break]",
    "[insert]",
];

/// What `inputKey` answers for a reply the host sent. A reply
/// naming an extended key is that key; any other reply is its first character,
/// because a host that can only read lines still presses one key at a time.
fn key_from_reply(text: &str) -> String {
    if text.starts_with('[') && text.trim_end_matches(['\r', '\n']).ends_with(']') {
        let name = text.trim_end_matches(['\r', '\n']);
        return match EXTENDED_KEY_NAMES.iter().find(|known| **known == name) {
            Some(known) => (*known).to_owned(),
            // The reference reports a keystroke it has no name for this way.
            None => "[?]".to_owned(),
        };
    }
    text.chars()
        .next()
        .map_or_else(|| "\n".to_owned(), |first| first.to_string())
}

/// The tokens a grammar production parses, from a list or a vector.
/// Under lifetimes a token sequence is built by appending and a vector is what
/// appending produces, so requiring a list meant copying one for no reason.
fn token_sequence<'a>(objects: &'a Objects<'_>, tokens: ObjectRef) -> Result<&'a [Value], Error> {
    match objects.list(tokens) {
        Ok(values) => Ok(values),
        // A vector reads the same way; anything else is neither.
        Err(Error::WrongType) => objects.vector(tokens),
        Err(error) => Err(error),
    }
}

fn error_code(error: Error) -> u32 {
    match error {
        Error::ForeignSession => 1,
        Error::Expired => 2,
        Error::NotOwned => 3,
        Error::OwnershipCycle => 4,
        Error::ResourceLimit => 5,
        Error::Allocation => 6,
        Error::IdentityExhausted => 7,
        Error::Terminal => 8,
        Error::InheritanceCycle => 14,
        Error::InitializationCycle => 15,
        Error::WrongType => 16,
        Error::InvalidArgument => 17,
        Error::NumericOverflow => 18,
    }
}

/// Replace substrings, as `String.findReplace` does. With parallel lists the
/// reference scans left to right and takes the earliest match, preferring the
/// earlier list entry on a tie; a replacement is never rescanned.
fn find_replace(
    text: &str,
    patterns: &[String],
    replacements: &[String],
    once: bool,
) -> Result<String, u32> {
    let characters: Vec<char> = text.chars().collect();
    let needles: Vec<Vec<char>> = patterns.iter().map(|p| p.chars().collect()).collect();
    let mut out = String::new();
    let mut at = 0usize;
    let mut replaced = false;
    while at < characters.len() {
        let mut hit = None;
        if !(once && replaced) {
            for (index, needle) in needles.iter().enumerate() {
                if !needle.is_empty()
                    && at + needle.len() <= characters.len()
                    && characters[at..at + needle.len()] == needle[..]
                {
                    hit = Some((index, needle.len()));
                    break;
                }
            }
        }
        match hit {
            Some((index, length)) => {
                let piece = &replacements[index];
                out.try_reserve(piece.len()).map_err(|_| 6u32)?;
                out.push_str(piece);
                at += length;
                replaced = true;
            }
            None => {
                let ch = characters[at];
                out.try_reserve(ch.len_utf8()).map_err(|_| 6u32)?;
                out.push(ch);
                at += 1;
            }
        }
    }
    Ok(out)
}

/// Escape markup, as `String.htmlify` does: `&`, `<` and `>` always, and spaces,
/// newlines and tabs under the systype.h flags.
fn htmlify(text: &str, flags: u32) -> Result<String, u32> {
    let mut out = String::new();
    let spaces = flags & 0x0001 != 0;
    let newlines = flags & 0x0002 != 0;
    let tabs = flags & 0x0004 != 0;
    for ch in text.chars() {
        let piece = match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            ' ' if spaces => "&nbsp;",
            '\n' if newlines => "<br>",
            '\t' if tabs => "<tab>",
            _ => {
                out.try_reserve(ch.len_utf8()).map_err(|_| 6u32)?;
                out.push(ch);
                continue;
            }
        };
        out.try_reserve(piece.len()).map_err(|_| 6u32)?;
        out.push_str(piece);
    }
    Ok(out)
}

/// One line from the console, without its line terminator. End of input is
/// reported as nil, which is what the reference's `inputLine` does when the
/// input stream ends.
/// Split a command line into the token list a grammar production parses: one
/// `[text, token, text]` per word, as the reference tokenizer produces for a
/// word. Tokenizing is the host's side of the boundary, not world modelling, so
/// it belongs here rather than in every game's source.
fn tokenize(
    objects: &mut Objects<'_>,
    line: &str,
    token: u32,
) -> Result<ObjectRef, crate::objects::Error> {
    let mut tokens: Vec<Value> = Vec::new();
    for word in split_words(line) {
        let text = objects.new_text(&word)?;
        let entry = objects.new_list(&[
            Value::Text(text),
            Value::Enumerator(token),
            Value::Text(text),
        ])?;
        tokens
            .try_reserve(1)
            .map_err(|_| crate::objects::Error::Allocation)?;
        tokens.push(Value::List(entry));
    }
    objects.new_list(&tokens)
}

/// Split a line into words and punctuation.
///
/// Whitespace alone was not enough. `KEEPER, OPEN THE DOOR` arrived as the word
/// `keeper,` and matched nothing, so an order to another actor could not be
/// typed — and every command ending in a full stop failed for the same reason.
///
/// The rule is the one TADS 3's tokenizer uses: a word is a run of letters and
/// digits, and punctuation **inside** a word stays part of it while punctuation
/// at either end becomes a token of its own. That keeps `91.7` one token, and
/// `actor/singer` and `p.o.` too, while `keeper,` becomes two.
fn split_words(line: &str) -> Vec<String> {
    let characters: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut at = 0;
    while at < characters.len() {
        let ch = characters[at];
        if ch.is_whitespace() {
            at += 1;
            continue;
        }
        // `#47 bus`: a hash immediately before a digit is part of the number,
        // which is one of the shapes `t3odd_noun.htm` says a parser should
        // already handle without the author doing anything.
        let numbered = ch == '#'
            && characters
                .get(at + 1)
                .is_some_and(|next| next.is_ascii_digit());
        if !ch.is_alphanumeric() && !numbered {
            words.push(ch.to_string());
            at += 1;
            continue;
        }
        let start = at;
        if numbered {
            at += 1;
        }
        while at < characters.len() {
            if characters[at].is_alphanumeric() {
                at += 1;
                continue;
            }
            // Punctuation continues the word only with a letter or digit on
            // both sides of it: `91.7` is one token and `91.` is two.
            let joins = matches!(characters[at], '.' | '-' | '\'' | '/' | '#' | '_')
                && characters
                    .get(at + 1)
                    .is_some_and(|next| next.is_alphanumeric());
            // `7% solution`: a percent sign ends a number and belongs to it.
            let percent =
                characters[at] == '%' && at > start && characters[at - 1].is_ascii_digit();
            if !joins && !percent {
                break;
            }
            at += 1;
        }
        words.push(characters[start..at].iter().collect());
    }
    words
}

#[cfg(test)]
mod tokenizer_tests {
    use super::split_words;

    #[test]
    fn punctuation_splits_at_the_edges_and_joins_in_the_middle() {
        // A comma separates the addressee from the command words.
        let words = |line: &str| split_words(line).join("|");
        assert_eq!(words("keeper, open the door"), "keeper|,|open|the|door");
        assert_eq!(words("set dial to 91.7"), "set|dial|to|91.7");
        assert_eq!(words("x actor/singer"), "x|actor/singer");
        assert_eq!(words("take it."), "take|it|.");
        assert_eq!(words("  north  "), "north");
        assert_eq!(words("say \"hello\""), "say|\"|hello|\"");
        assert_eq!(words("7% solution"), "7%|solution");
        assert_eq!(words("#47 bus"), "#47|bus");
        assert_eq!(words("button 7"), "button|7");
        assert_eq!(words("don't"), "don't");
        assert_eq!(words(""), "");
    }
}

fn regex_error(error: crate::regex::Error) -> u32 {
    match error {
        crate::regex::Error::Memory => 6,
        crate::regex::Error::Budget => 5,
        crate::regex::Error::Pattern => 17,
    }
}

/// Expand a replacement, where `%1`-`%9` are groups, `%*` the whole match and
/// `%%` a literal percent sign, as the reference does.
fn expand(
    replacement: &[char],
    subject: &[char],
    found: &Match,
    output: &mut Vec<char>,
) -> Result<(), u32> {
    let mut at = 0;
    while at < replacement.len() {
        let ch = replacement[at];
        at += 1;
        if ch != '%' || at >= replacement.len() {
            output.try_reserve(1).map_err(|_| 6u32)?;
            output.push(ch);
            continue;
        }
        let next = replacement[at];
        at += 1;
        let bounds = match next {
            '*' => Some((found.start, found.end)),
            '1'..='9' => found
                .groups
                .get(next as usize - '1' as usize)
                .copied()
                .flatten(),
            _ => {
                output.try_reserve(1).map_err(|_| 6u32)?;
                output.push(next);
                continue;
            }
        };
        if let Some((from, to)) = bounds {
            output.try_reserve(to - from).map_err(|_| 6u32)?;
            output.extend_from_slice(&subject[from..to]);
        }
    }
    Ok(())
}

/// A regular-expression request: `op` 0 matches at a position.
struct RegexRequest {
    op: u32,
    session: u64,
    pattern_kind: u64,
    pattern: u64,
    subject_kind: u64,
    subject: u64,
    index: u64,
}

struct DictionaryRequest {
    op: u32,
    session: u64,
    dictionary: u64,
    target: u64,
    flags: u64,
    word: u64,
    property: u64,
}

struct GrammarRequest {
    op: u32,
    session: u64,
    production: u64,
    tokens: u64,
    dictionary: u64,
    recipient: u64,
}

/// Command inputs are short; these bounds reject ambiguity explosions explicitly.
const GRAMMAR_LIMITS: grammar::Limits = grammar::Limits {
    states: 65_536,
    work: 4_000_000,
    trees: 4_096,
    depth: 256,
};

enum ParseFailure {
    Grammar(grammar::Error),
    Code(u32),
}
impl From<grammar::Error> for ParseFailure {
    fn from(error: grammar::Error) -> Self {
        Self::Grammar(error)
    }
}
impl ParseFailure {
    fn runtime(error: Error) -> Self {
        Self::Code(error_code(error))
    }
    fn code(self) -> u32 {
        match self {
            Self::Grammar(grammar::Error::ResourceLimit) => 5,
            Self::Grammar(grammar::Error::InvalidGrammar) => 11,
            Self::Code(code) => code,
        }
    }
}

fn token_text<'a>(objects: &'a Objects<'_>, value: Option<Value>) -> Result<&'a str, ParseFailure> {
    match value {
        Some(Value::String(literal)) => crate::tables::installed()
            .literals
            .get(literal.index() as usize)
            .copied()
            .ok_or(ParseFailure::Code(11)),
        Some(Value::Text(text)) => objects.text(text).map_err(ParseFailure::runtime),
        _ => Err(ParseFailure::Code(16)),
    }
}

/// Answer one bounded, read-only inspection. Kinds: 0 a relation
/// query, where `a` is the packed descriptor and `b` the subject; 1 an entity's
/// own property ids; 2 an entity's prototypes. Results are copied handles, and
/// nothing here evaluates game code.
/// Framed snapshots : [version, status, records], then length-prefixed
/// records. Errors discard the whole answer, never masquerade as a full scene.
const SNAPSHOT_ENTITIES: usize = 4096;
const SNAPSHOT_WORDS: usize = 65_536;

fn snapshot_record(answer: &mut Vec<u64>, record: &[u64], limit: usize) -> Result<(), u32> {
    if answer.len().saturating_add(record.len() + 1) > limit {
        return Err(5);
    }
    answer.try_reserve(record.len() + 1).map_err(|_| 6u32)?;
    answer.push((record.len() + 1) as u64);
    answer.extend_from_slice(record);
    answer[2] += 1;
    Ok(())
}

fn scene_snapshot(
    objects: &Objects<'_>,
    relations: &crate::relations::Relations,
    a: u64,
    root: u64,
    limit: usize,
) -> Result<Vec<u64>, u32> {
    // Upper 32 bits name the presentation property. Low bits select generic
    // containment and optional membership relations; no library names here.
    let descriptor = a as u32;
    let has_membership = descriptor & (1 << 24) != 0;
    let allowed = if has_membership { 0x03ff_ffff } else { 0xfff };
    if descriptor & !allowed != 0 {
        return Err(11);
    }
    let relation = (descriptor & 0xfff) as usize;
    let property = (a >> 32) as u32;
    let root = objects.reference(root).ok_or(2u32)?;
    let mut pending = Vec::new();
    pending.try_reserve(1).map_err(|_| 6u32)?;
    pending.push((root, true));
    // Optional second relation supplies scene members not held by containment,
    // e.g. a single door connected to both rooms. It is read only at the root.
    if has_membership {
        let membership = ((descriptor >> 12) & 0xfff) as usize;
        let reversed = descriptor & (1 << 25) != 0;
        for member in relations
            .all(membership, root, reversed)
            .map_err(error_code)?
        {
            if pending.iter().any(|(seen, _)| seen == member) {
                continue;
            }
            if pending.len() == SNAPSHOT_ENTITIES {
                return Err(5);
            }
            if objects.reference(member.handle()).is_none() {
                return Err(2);
            }
            pending.try_reserve(1).map_err(|_| 6u32)?;
            pending.push((*member, true));
        }
    }
    // First walk containment, so membership flags do not depend on whether a
    // presentation reference happened to encounter a resident first.
    let mut at = 0;
    while at < pending.len() {
        let entity = pending[at].0;
        for child in relations.all(relation, entity, false).map_err(error_code)? {
            if pending.iter().any(|(seen, _)| seen == child) {
                continue;
            }
            if pending.len() == SNAPSHOT_ENTITIES {
                return Err(5);
            }
            if objects.reference(child.handle()).is_none() {
                return Err(2);
            }
            pending.try_reserve(1).map_err(|_| 6u32)?;
            pending.push((*child, true));
        }
        at += 1;
    }
    let mut answer = vec![1, 0, 0];
    at = 0;
    while at < pending.len() {
        let (entity, resident) = pending[at];
        let properties = objects
            .presentation_checked(entity, property)
            .map_err(error_code)?;
        let parent = relations.get(relation, entity, true).map_err(error_code)?;
        let mut record = Vec::new();
        record.try_reserve(4 + properties.len()).map_err(|_| 6u32)?;
        record.extend_from_slice(&[
            entity.handle(),
            parent.map_or(0, |p| p.handle()),
            u64::from(resident),
            (properties.len() / 3) as u64,
        ]);
        record.extend_from_slice(&properties);
        snapshot_record(&mut answer, &record, limit)?;
        // Include presented entities (notably actor states and their leaders)
        // as reference records. Never evaluate methods or walk their contents.
        for triple in properties.as_chunks::<3>().0 {
            if triple[1] != 3 {
                continue;
            }
            let Some(linked) = objects.reference(triple[2]) else {
                continue;
            };
            if pending.iter().any(|(seen, _)| *seen == linked) {
                continue;
            }
            if pending.len() == SNAPSHOT_ENTITIES {
                return Err(5);
            }
            pending.try_reserve(1).map_err(|_| 6u32)?;
            pending.push((linked, false));
        }
        at += 1;
    }
    Ok(answer)
}

fn inspection_batch(
    objects: &Objects<'_>,
    relations: &crate::relations::Relations,
    kind: u32,
    a: u64,
    b: u64,
) -> Result<Vec<u64>, u32> {
    let first = b as u32;
    let count = ((b >> 32) as usize).max(1);
    if count > SNAPSHOT_ENTITIES {
        return Err(5);
    }
    let mut answer = vec![1, 0, 0];
    for step in 0..count {
        let index = first.checked_add(step as u32).ok_or(11u32)?;
        let handle = objects.declared(index).unwrap_or(0);
        let mut record = vec![u64::from(index), handle];
        if let Some(entity) = objects.reference(handle) {
            if kind == 7 {
                let property = u32::try_from(a).map_err(|_| 11u32)?;
                let properties = objects
                    .presentation_checked(entity, property)
                    .map_err(error_code)?;
                record.try_reserve(properties.len()).map_err(|_| 6u32)?;
                record.extend_from_slice(&properties);
            } else {
                let relation = (a & 0xfff) as usize;
                let reversed = (a >> 12) & 1 != 0;
                let partners = if a & (1 << 13) != 0 {
                    relations.all_labelled(relation, entity, reversed, ((a >> 16) & 0xfff) as u32)
                } else {
                    relations.all(relation, entity, reversed)
                }
                .map_err(error_code)?;
                if partners.len() > SNAPSHOT_WORDS {
                    return Err(5);
                }
                record.try_reserve(partners.len()).map_err(|_| 6u32)?;
                record.extend(partners.iter().map(|p| p.handle()));
            }
        }
        snapshot_record(&mut answer, &record, SNAPSHOT_WORDS)?;
    }
    Ok(answer)
}

fn inspect(
    objects: &mut Objects<'_>,
    relations: &crate::relations::Relations,
    kind: u32,
    a: u64,
    b: u64,
) -> Result<Vec<u64>, u32> {
    if (6..=8).contains(&kind) {
        let result = if kind == 6 {
            scene_snapshot(objects, relations, a, b, SNAPSHOT_WORDS)
        } else {
            inspection_batch(objects, relations, kind, a, b)
        };
        return Ok(result.unwrap_or_else(|code| vec![1, u64::from(code), 0]));
    }
    /// Inspection answers a bounded page, never a whole world at once.
    const LIMIT: usize = 4096;
    let mut values = Vec::new();
    match kind {
        0 => {
            let relation = usize::try_from(a & 0xfff).map_err(|_| 11u32)?;
            let reversed = (a >> 12) & 1 != 0;
            let subject = objects.reference(b).ok_or(3u32)?;
            // bit 13 says the descriptor carries a label in bits 16-27,
            // the same place a call site puts it. Without it a
            // labelled relation answered nothing at all, so a room's exits
            // could not be read and no host could draw a map or place a door.
            let labelled = (a >> 13) & 1 != 0;
            let partners = if labelled {
                let label = u32::try_from((a >> 16) & 0xfff).map_err(|_| 11u32)?;
                relations
                    .all_labelled(relation, subject, reversed, label)
                    .map_err(error_code)?
            } else {
                relations
                    .all(relation, subject, reversed)
                    .map_err(error_code)?
            };
            values
                .try_reserve(partners.len().min(LIMIT))
                .map_err(|_| 6u32)?;
            for partner in partners.iter().take(LIMIT) {
                values.push(partner.handle());
            }
        }
        1 | 2 => {
            let entity = objects.reference(b.max(a)).ok_or(3u32)?;
            let listed = if kind == 1 {
                objects.property_ids(entity).map_err(error_code)?
            } else {
                objects
                    .prototype_list(entity)
                    .map_err(error_code)?
                    .iter()
                    .map(|value| value.handle())
                    .collect()
            };
            values
                .try_reserve(listed.len().min(LIMIT))
                .map_err(|_| 6u32)?;
            values.extend(listed.into_iter().take(LIMIT));
        }
        // Where an inspector starts: every world entity, bounded.
        3 => {
            values = objects.world_entities(LIMIT);
        }
        // the handles of the entities declared at indices `a` to
        // `a + b`, which the manifest publishes beside the names their author
        // wrote. This is the join between an engine project — where a room is
        // a scene bound in the editor — and a running session, where it is a
        // handle that is different every time.
        //
        // **A range, because a turn answers one inspection**. An
        // engine binds every name it cares about before the first command; one
        // index per turn meant a game with fifty rooms spent two hundred turns
        // of nothing before it could recognise anything in its event stream.
        //
        // `b` is how many, and **0 means one**, so a host written against the
        // single-index form keeps working. An index nothing is declared at
        // contributes nothing, so the answer is **not** positional — a host
        // reading a range asks for one it knows is dense, which walking the
        // manifest in order gives it.
        5 => {
            let first = u32::try_from(a).map_err(|_| 11u32)?;
            let count = usize::try_from(b).map_err(|_| 11u32)?.clamp(1, LIMIT);
            values.try_reserve(count).map_err(|_| 6u32)?;
            for step in 0..count {
                let Some(index) = u32::try_from(step)
                    .ok()
                    .and_then(|step| first.checked_add(step))
                else {
                    break;
                };
                if let Some(handle) = objects.declared(index) {
                    values.push(handle);
                }
            }
        }
        // the declared presentation state of an entity — what a
        // renderer needs to draw it. `a` is the property id of the list that
        // declares which properties those are, so a game that spells it
        // differently is not shut out; `b` is the entity.
        //
        // Answered as triples: property id, value tag, payload. A literal is
        // read with the bundle's `text_byte` export, which a host already has.
        4 => {
            let property = u32::try_from(a).map_err(|_| 11u32)?;
            let entity = objects.reference(b).ok_or(3u32)?;
            let listed = objects.presentation(entity, property).map_err(error_code)?;
            values
                .try_reserve(listed.len().min(LIMIT * 3))
                .map_err(|_| 6u32)?;
            values.extend(listed.into_iter().take(LIMIT * 3));
        }
        _ => return Err(11),
    }
    Ok(values)
}

/// A token's text as an owned string, for a caller that needs the store back.
fn text_of(objects: &Objects<'_>, value: Value) -> Result<String, Error> {
    match value {
        Value::String(literal) => crate::tables::installed()
            .literals
            .get(literal.index() as usize)
            .map(|text| (*text).to_owned())
            .ok_or(Error::InvalidArgument),
        Value::Text(text) => Ok(objects.text(text)?.to_owned()),
        _ => Err(Error::WrongType),
    }
}

/// Inclusive one-based range of tokens consumed below a match. Star tokens do
/// not count; an empty match reports start+2..start+1, as external TADS does.
fn token_range(forest: &Forest, node: usize) -> Result<(i32, i32), Error> {
    let mut bounds: Option<(usize, usize)> = None;
    let mut pending = Vec::new();
    pending.try_reserve(1).map_err(|_| Error::Allocation)?;
    pending.push(node);
    while let Some(current) = pending.pop() {
        for child in &forest.nodes[current].children {
            match *child {
                Child::Token(position) => {
                    bounds = Some(bounds.map_or((position, position), |(low, high)| {
                        (low.min(position), high.max(position))
                    }));
                }
                Child::Tree(tree) => {
                    pending.try_reserve(1).map_err(|_| Error::Allocation)?;
                    pending.push(tree);
                }
                Child::Star { .. } => {}
            }
        }
    }
    let (first, last) = match bounds {
        Some((low, high)) => (low + 1, high + 1),
        None => (forest.nodes[node].start + 2, forest.nodes[node].start + 1),
    };
    Ok((
        i32::try_from(first).map_err(|_| Error::NumericOverflow)?,
        i32::try_from(last).map_err(|_| Error::NumericOverflow)?,
    ))
}

/// One step of the shared pre-order walk over match trees. Match objects are
/// numbered in visit order, which is also external TADS construction order.
enum Step {
    Match {
        node: usize,
        parent: Option<usize>,
        capture: Option<u32>,
    },
    Token {
        position: usize,
        target: usize,
        capture: u32,
        /// The part of speech, when the term came from a vocabulary relation:
        /// the capture then binds the entities the word names.
        vocabulary: Option<u32>,
    },
}

/// Roots in order, then each node before its children, left to right. Anonymous
/// groups create no object; their captures target the enclosing match object.
fn preorder(
    forest: &Forest,
    data: &StaticGrammar,
    mut visit: impl FnMut(Step) -> Result<(), Error>,
) -> Result<(), Error> {
    enum Entry {
        Tree(usize, Option<u32>),
        Token(usize, u32, Option<u32>),
    }
    let mut pending: Vec<(Entry, Option<usize>)> = Vec::new();
    pending
        .try_reserve(forest.roots.len())
        .map_err(|_| Error::Allocation)?;
    for &root in forest.roots.iter().rev() {
        pending.push((Entry::Tree(root, None), None));
    }
    let mut next = 0usize;
    while let Some((entry, target)) = pending.pop() {
        match entry {
            Entry::Token(position, capture, vocabulary) => visit(Step::Token {
                position,
                target: target.ok_or(Error::InvalidArgument)?,
                capture,
                vocabulary,
            })?,
            Entry::Tree(node, capture) => {
                let rule = &data.rules[forest.nodes[node].rule];
                let target = if rule.match_slot.is_some() {
                    visit(Step::Match {
                        node,
                        parent: target,
                        capture,
                    })?;
                    next += 1;
                    Some(next - 1)
                } else {
                    target
                };
                let children = &forest.nodes[node].children;
                pending
                    .try_reserve(children.len())
                    .map_err(|_| Error::Allocation)?;
                for (element, child) in rule.elements.iter().zip(children).rev() {
                    match *child {
                        Child::Tree(tree) => {
                            pending.push((Entry::Tree(tree, element.capture), target))
                        }
                        Child::Token(position) => {
                            if let Some(capture) = element.capture {
                                let vocabulary = match element.term {
                                    StaticTerm::Vocabulary(property) => Some(property),
                                    _ => None,
                                };
                                pending.push((Entry::Token(position, capture, vocabulary), target));
                            }
                        }
                        Child::Star { .. } => {}
                    }
                }
            }
        }
    }
    Ok(())
}

/// One completed command cycle, replayable backwards. Property
/// writes and relation changes are kept together because undoing a turn has to
/// put both back, and putting only one back would be worse than neither.
struct Cycle {
    containers: Vec<crate::objects::ContainerDelta>,
    properties: Vec<crate::objects::PropertyDelta>,
    relations: Vec<crate::relations::Delta>,
}

/// How many cycles stay undoable. A world's state is reverted by replay, so the
/// cost is the journal, not a snapshot; this bounds it.
const UNDO_DEPTH: usize = 32;

/// A `Vector.sort` suspended while generated code runs the comparator.
/// It owns nothing, so unwinding simply discards it.
struct PendingSort {
    vector: ObjectRef,
    sort: QuickSort,
    pair: (usize, usize),
}

struct VectorElements<'a, 'b> {
    objects: &'a mut Objects<'b>,
    vector: ObjectRef,
}

impl Exchange for VectorElements<'_, '_> {
    type Error = Error;
    fn exchange(&mut self, a: usize, b: usize) -> Result<(), Error> {
        let index = |n: usize| i32::try_from(n + 1).map_err(|_| Error::InvalidArgument);
        let (a, b) = (index(a)?, index(b)?);
        let left = self.objects.vector_get(self.vector, a)?;
        let right = self.objects.vector_get(self.vector, b)?;
        self.objects.vector_set(self.vector, b, left)?;
        self.objects.vector_set(self.vector, a, right)
    }
}

/// A parse between creation of bare match objects and property assignment, while
/// generated code runs their constructors. Storage owns everything.
struct PendingParse {
    storage: ObjectRef,
    holder: ObjectRef,
    order: ObjectRef,
    tokens: ObjectRef,
    /// Where the match trees go when the parse finishes. Without one they are
    /// ordinary turn records, freed with the rest of the cycle.
    recipient: Option<ObjectRef>,
    /// The dictionary the parse matched against, which a vocabulary capture
    /// looks its candidates up in.
    dictionary: Option<ObjectRef>,
    forest: Forest,
    objects: Vec<ObjectRef>,
}

/// Create bare match objects with their rule prototypes, in construction order.
/// Returns the objects, a private holder and the borrowed pre-order list.
fn begin_matches(
    objects: &mut Objects<'_>,
    data: &StaticGrammar,
    prototypes: &[Option<ObjectRef>],
    forest: &Forest,
    storage: ObjectRef,
) -> Result<(Vec<ObjectRef>, ObjectRef, ObjectRef), Error> {
    let mut created = Vec::new();
    preorder(forest, data, |step| {
        if let Step::Match { node, .. } = step {
            let prototype = prototypes[forest.nodes[node].rule].ok_or(Error::InvalidArgument)?;
            created.try_reserve(1).map_err(|_| Error::Allocation)?;
            let object = objects.create()?;
            if let Err(error) = objects.move_root_into_collection(storage, object) {
                let _ = objects.destroy(object);
                return Err(error);
            }
            objects.add_prototype(object, prototype)?;
            created.push(object);
        }
        Ok(())
    })?;
    let holder = objects.create()?;
    if let Err(error) = objects.move_root_into_collection(storage, holder) {
        let _ = objects.destroy(holder);
        return Err(error);
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(created.len())
        .map_err(|_| Error::Allocation)?;
    values.extend(created.iter().map(|object| Value::Reference(*object)));
    let order = objects.new_list(&values)?;
    if let Err(error) = objects.move_root_into_field(holder, PropertyId(0), order) {
        let _ = objects.destroy(order);
        return Err(error);
    }
    Ok((created, holder, order))
}

/// After construction, assign token ranges, token lists and captures in the same
/// pre-order, overwriting constructor values as external TADS does. Returns the
/// root list, held in the parse storage.
fn finish_matches(
    objects: &mut Objects<'_>,
    data: &StaticGrammar,
    pending: &PendingParse,
) -> Result<ObjectRef, Error> {
    let mut roots = Vec::new();
    let mut index = 0usize;
    preorder(&pending.forest, data, |step| {
        match step {
            Step::Match {
                node,
                parent,
                capture,
            } => {
                let object = *pending.objects.get(index).ok_or(Error::InvalidArgument)?;
                index += 1;
                let (first, last) = token_range(&pending.forest, node)?;
                // how bad a reading this rule is. The grammar has
                // carried it since the beginning and nothing read it, so every
                // parse that matched was equally good and a library had no way
                // to prefer one. Reported on the match so the **library**
                // decides, which is where ranking belongs — beside verify.
                if let Some(property) = data.badness {
                    objects.set(
                        object,
                        PropertyId(property),
                        Value::Int(data.rules[pending.forest.nodes[node].rule].badness),
                    )?;
                }
                if let Some(property) = data.first_token_index {
                    objects.set(object, PropertyId(property), Value::Int(first))?;
                }
                if let Some(property) = data.last_token_index {
                    objects.set(object, PropertyId(property), Value::Int(last))?;
                }
                if let Some(property) = data.token_list {
                    objects.set(object, PropertyId(property), Value::List(pending.tokens))?;
                }
                match (parent, capture) {
                    (None, _) => {
                        roots.try_reserve(1).map_err(|_| Error::Allocation)?;
                        roots.push(Value::Reference(object));
                    }
                    (Some(parent), Some(property)) => objects.set(
                        pending.objects[parent],
                        PropertyId(property),
                        Value::Reference(object),
                    )?,
                    (Some(_), None) => {}
                }
            }
            Step::Token {
                position,
                target,
                capture,
                vocabulary,
            } => {
                let Value::List(token) = objects.list(pending.tokens)?[position] else {
                    return Err(Error::WrongType);
                };
                let text = objects
                    .list(token)?
                    .first()
                    .copied()
                    .ok_or(Error::WrongType)?;
                let bound = match (vocabulary, pending.dictionary) {
                    // A vocabulary slot binds what the word names. The answer is
                    // a list because a word can name several things, which is
                    // exactly the set disambiguation would narrow.
                    (Some(property), Some(dictionary)) => {
                        let word = text_of(objects, text)?;
                        let candidates = objects.dictionary_candidates(
                            dictionary,
                            &word,
                            PropertyId(property),
                        )?;
                        Value::List(candidates)
                    }
                    _ => text,
                };
                objects.set(pending.objects[target], PropertyId(capture), bound)?;
            }
        }
        Ok(())
    })?;
    let list = objects.new_list(&roots)?;
    if let Err(error) = objects.move_root_into_field(pending.holder, PropertyId(1), list) {
        let _ = objects.destroy(list);
        return Err(error);
    }
    Ok(list)
}

struct LookupRequest {
    op: u32,
    session: u64,
    table: u64,
    flags: u64,
    key: u64,
    value: u64,
}

fn lookup_value(store: &Store, tag: u64, payload: u64) -> Result<Value, u32> {
    Ok(match tag {
        0 if payload == 0 => Value::Nil,
        1 if payload == 0 => Value::Bool(true),
        2 => Value::Int(u32::try_from(payload).map_err(|_| 11u32)? as i32),
        3 => Value::Reference(store.native_reference(payload)),
        4 => Value::String(
            store
                .literal(u32::try_from(payload).map_err(|_| 11u32)?)
                .map_err(error_code)?,
        ),
        7 => Value::Text(store.native_reference(payload)),
        8 => Value::Property(PropertyId(u32::try_from(payload).map_err(|_| 11u32)?)),
        9 => Value::List(store.native_reference(payload)),
        10 => Value::Enumerator(u32::try_from(payload).map_err(|_| 11u32)?),
        11 => Value::Function(u32::try_from(payload).map_err(|_| 11u32)?),
        _ => return Err(16),
    })
}

impl State {
    fn lookup(&mut self, request: LookupRequest) -> Result<u64, u32> {
        let LookupRequest {
            op,
            session,
            table,
            flags,
            key,
            value,
        } = request;
        if op > 8 {
            return Err(11);
        }
        let store = self.store.as_mut().ok_or(9u32)?;
        if session != store.session_id() {
            return Err(1);
        }
        store.ensure_active().map_err(error_code)?;
        let key = lookup_value(store, flags & 0xffff_ffff, key)?;
        let value = lookup_value(store, flags >> 32, value)?;
        if (((3..=5).contains(&op) || op == 8) && key != Value::Nil)
            || (!matches!(op, 1 | 5 | 7) && value != Value::Nil)
        {
            return Err(11);
        }
        let table = store.native_reference(table);
        let mut objects = store.objects();
        let result = match op {
            8 => objects.lookup_bucket_count(table).map(Value::Int),
            7 => objects.indexed_set(table, key, value).map(|()| value),
            6 => objects.indexed_get(table, key),
            0 => objects.lookup_get(table, key),
            1 => objects.lookup_set(table, key, value).map(|()| value),
            2 => objects.lookup_contains(table, key).map(Value::Bool),
            3 => objects.lookup_len(table).and_then(|n| {
                i32::try_from(n)
                    .map(Value::Int)
                    .map_err(|_| Error::NumericOverflow)
            }),
            4 => objects.lookup_default(table),
            5 => objects
                .lookup_set_default(table, value)
                .map(|()| Value::Reference(table)),
            _ => unreachable!(),
        }
        .map_err(error_code)?;
        let (kind, payload) = match result {
            Value::Nil => (0, 0),
            Value::Bool(value) => (u32::from(value), 0),
            Value::Int(value) => (2, u64::from(value as u32)),
            Value::Reference(value) => (3, value.handle()),
            Value::Text(value) => (7, value.handle()),
            Value::List(value) => (9, value.handle()),
            Value::String(value) => (4, u64::from(value.index())),
            Value::Property(value) => (8, u64::from(value.0)),
            Value::Enumerator(value) => (10, u64::from(value)),
            Value::Function(value) => (11, u64::from(value)),
            _ => return Err(16),
        };
        self.kind = kind;
        Ok(payload)
    }

    /// Parse from a compiled production ( two-phase contract).
    /// Ops 2/3 begin: create bare match objects and return them in construction
    /// order. Op 4 finishes the most recent begin (`production` carries its order
    /// list): assign properties, then move storage into the recipient and return
    /// the borrowed result list. Ops 0/1 begin and finish without constructors.
    /// Even ops pass a dictionary; odd begin ops pass nil.
    fn grammar(&mut self, request: GrammarRequest) -> Result<u64, u32> {
        let (op, session, order) = (request.op, request.session, request.production);
        match op {
            0..=3 => {
                let order = self.grammar_begin(request)?;
                if op <= 1 {
                    self.grammar_finish(session, order)
                } else {
                    self.kind = 9;
                    Ok(order)
                }
            }
            4 => self.grammar_finish(session, order),
            _ => Err(11),
        }
    }

    fn grammar_begin(&mut self, request: GrammarRequest) -> Result<u64, u32> {
        let GrammarRequest {
            op,
            session,
            production,
            tokens,
            dictionary,
            recipient,
        } = request;
        let store = self.store.as_mut().ok_or(9u32)?;
        if store.session_id() != session {
            return Err(1);
        }
        store.ensure_active().map_err(error_code)?;
        let data = crate::tables::installed().grammar;
        let production = store.native_reference(production);
        let mut start = None;
        for (index, slot) in data.productions.iter().enumerate() {
            if let Some(slot) = slot
                && store.static_object(*slot).map_err(error_code)? == production
            {
                start = Some(index);
                break;
            }
        }
        let start = start.ok_or(16u32)?;
        // Rule workspace is call-scoped Rust storage, not session-owned game state.
        let mut terminals = Vec::new();
        let mut rules = Vec::new();
        let mut prototypes = Vec::new();
        rules
            .try_reserve_exact(data.rules.len())
            .and_then(|()| prototypes.try_reserve_exact(data.rules.len()))
            .map_err(|_| 6u32)?;
        for rule in data.rules {
            let mut symbols = Vec::new();
            symbols
                .try_reserve_exact(rule.elements.len())
                .map_err(|_| 6u32)?;
            for element in rule.elements {
                symbols.push(match element.term {
                    StaticTerm::Production(child) => Symbol::Production(child as usize),
                    StaticTerm::Star => Symbol::Star,
                    term => {
                        terminals.try_reserve(1).map_err(|_| 6u32)?;
                        terminals.push(term);
                        Symbol::Terminal(u32::try_from(terminals.len() - 1).map_err(|_| 6u32)?)
                    }
                });
            }
            rules.push(Rule {
                production: rule.production as usize,
                symbols,
            });
            prototypes.push(match rule.match_slot {
                Some(slot) => Some(store.static_object(slot).map_err(error_code)?),
                None => None,
            });
        }
        let tokens = store.native_reference(tokens);
        let dictionary = (op % 2 == 0).then(|| store.native_reference(dictionary));
        let recipient = (recipient != 0).then(|| store.native_reference(recipient));
        let mut objects = store.objects();
        let forest = {
            let values = token_sequence(&objects, tokens).map_err(error_code)?;
            let objects = &objects;
            let mut matches = |terminal: u32, position: usize| -> Result<bool, ParseFailure> {
                let Value::List(token) = values[position] else {
                    return Err(ParseFailure::Code(16));
                };
                let token = objects.list(token).map_err(ParseFailure::runtime)?;
                let text = token_text(objects, token.first().copied())?;
                Ok(match terminals[terminal as usize] {
                    StaticTerm::Literal(literal) => text == literal,
                    StaticTerm::Token(value) => token.get(1) == Some(&Value::Enumerator(value)),
                    // A vocabulary relation matches exactly as a part of
                    // speech does; only its capture differs.
                    StaticTerm::Speech(property) | StaticTerm::Vocabulary(property) => {
                        match dictionary {
                            Some(dictionary) => objects
                                .dictionary_has(dictionary, text, PropertyId(property))
                                .map_err(ParseFailure::runtime)?,
                            None => false,
                        }
                    }
                    _ => return Err(ParseFailure::Code(11)),
                })
            };
            grammar::parse(
                &rules,
                data.productions.len(),
                start,
                values.len(),
                GRAMMAR_LIMITS,
                &mut matches,
            )
            .map_err(ParseFailure::code)?
        };
        let storage = objects.new_owned_collection().map_err(error_code)?;
        match begin_matches(&mut objects, data, &prototypes, &forest, storage) {
            Ok((created, holder, order)) => {
                if self.parses.try_reserve(1).is_err() {
                    let _ = objects.destroy(storage);
                    return Err(6);
                }
                self.parses.push(PendingParse {
                    storage,
                    holder,
                    order,
                    tokens,
                    recipient,
                    dictionary,
                    forest,
                    objects: created,
                });
                Ok(order.handle())
            }
            Err(error) => {
                let _ = objects.destroy(storage);
                Err(error_code(error))
            }
        }
    }

    /// Finish only the innermost pending parse; a nested parse inside a
    /// constructor must finish before its enclosing parse.
    fn grammar_finish(&mut self, session: u64, order: u64) -> Result<u64, u32> {
        let store = self.store.as_mut().ok_or(9u32)?;
        if store.session_id() != session {
            return Err(1);
        }
        store.ensure_active().map_err(error_code)?;
        let order = store.native_reference(order);
        if self.parses.last().map(|pending| pending.order) != Some(order) {
            return Err(11);
        }
        let pending = self.parses.pop().expect("checked pending parse");
        let data = crate::tables::installed().grammar;
        let mut objects = store.objects();
        let built =
            finish_matches(&mut objects, data, &pending).and_then(|list| match pending.recipient {
                Some(recipient) => objects
                    .move_root_into_collection(recipient, pending.storage)
                    .map(|()| list),
                None => Ok(list),
            });
        match built {
            Ok(list) => {
                self.kind = 9;
                Ok(list.handle())
            }
            Err(error) => {
                let _ = objects.destroy(pending.storage);
                Err(error_code(error))
            }
        }
    }

    /// Read a source text value (string literal or text object) as characters.
    /// One text value, or every element of a list of text values.
    fn text_arguments(&mut self, kind: u64, value: u64) -> Result<Vec<String>, u32> {
        let mut out = Vec::new();
        if kind == 9 {
            let store = self.store.as_mut().ok_or(9u32)?;
            let list = store.native_reference(value);
            let elements = store.objects().list(list).map_err(error_code)?.to_vec();
            out.try_reserve_exact(elements.len()).map_err(|_| 6u32)?;
            for element in elements {
                let (kind, payload) = match element {
                    Value::String(literal) => (4, u64::from(literal.index())),
                    Value::Text(reference) => (7, reference.handle()),
                    _ => return Err(16),
                };
                out.push(self.characters(kind, payload)?.into_iter().collect());
            }
            return Ok(out);
        }
        out.try_reserve_exact(1).map_err(|_| 6u32)?;
        out.push(self.characters(kind, value)?.into_iter().collect());
        Ok(out)
    }

    /// Register newly built text with the calling scope.
    fn own_text(&mut self, text: &str) -> Result<u64, u32> {
        if !self.lifetimes && self.checkpoints.is_empty() {
            return Err(11);
        }
        let store = self.store.as_mut().ok_or(9u32)?;
        store.ensure_active().map_err(error_code)?;
        if self.local_owners.try_reserve(1).is_err() {
            return store
                .terminate(Error::Allocation)
                .map(|()| 0)
                .map_err(error_code);
        }
        let value = store.objects().new_text(text).map_err(error_code)?;
        // With lifetimes the record's class decides when it goes, so there is
        // no scope entry to make.
        if !self.lifetimes {
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(value));
            }
        }
        self.kind = 7;
        Ok(value.handle())
    }

    fn characters(&mut self, kind: u64, value: u64) -> Result<Vec<char>, u32> {
        let store = self.store.as_mut().ok_or(9u32)?;
        let text = match kind {
            4 => {
                let literal = store
                    .literal(u32::try_from(value).map_err(|_| 11u32)?)
                    .map_err(error_code)?;
                crate::tables::installed().literals[literal.index() as usize].to_owned()
            }
            7 => {
                let reference = store.native_reference(value);
                store
                    .objects()
                    .copy_text_buffer(reference)
                    .map_err(error_code)?
            }
            _ => return Err(16),
        };
        let mut chars = Vec::new();
        chars
            .try_reserve_exact(text.chars().count())
            .map_err(|_| 6u32)?;
        chars.extend(text.chars());
        Ok(chars)
    }

    /// Compile and cache a pattern, returning its cache index.
    fn pattern(&mut self, text: Vec<char>) -> Result<usize, u32> {
        if let Some(index) = self.patterns.iter().position(|(cached, _)| *cached == text) {
            return Ok(index);
        }
        let compiled = Pattern::compile(&text).map_err(|error| match error {
            crate::regex::Error::Memory => 6u32,
            crate::regex::Error::Budget => 5,
            crate::regex::Error::Pattern => 17,
        })?;
        // Bound the cache; patterns are small and reused heavily.
        if self.patterns.len() >= 64 {
            self.patterns.remove(0);
        }
        self.patterns.try_reserve(1).map_err(|_| 6u32)?;
        self.patterns.push((text, compiled));
        Ok(self.patterns.len() - 1)
    }

    /// Build the reference's `[start, length, text]` result, owned by the
    /// calling scope like any other local allocation.
    fn match_list(&mut self, matched: String, from: usize, to: usize) -> Result<u64, u32> {
        if self.checkpoints.is_empty() {
            return Err(11);
        }
        let store = self.store.as_mut().ok_or(9u32)?;
        store.ensure_active().map_err(error_code)?;
        if self.local_owners.try_reserve(2).is_err() {
            return store
                .terminate(Error::Allocation)
                .map(|()| 0)
                .map_err(error_code);
        }
        let mut objects = store.objects();
        let value = objects.new_text(&matched).map_err(error_code)?;
        // A turn record needs no scope entry.
        if !self.lifetimes {
            self.local_owners.push(Some(value));
        }
        let start = i32::try_from(from + 1).map_err(|_| 18u32)?;
        let length = i32::try_from(to - from).map_err(|_| 18u32)?;
        let list = objects
            .new_list(&[Value::Int(start), Value::Int(length), Value::Text(value)])
            .map_err(error_code)?;
        // A turn record needs no scope entry.
        if !self.lifetimes {
            self.local_owners.push(Some(list));
        }
        self.kind = 9;
        Ok(list.handle())
    }

    /// Resolve a pattern argument: a compiled `RexPattern` object, or text.
    fn compiled(&mut self, kind: u64, value: u64) -> Result<usize, u32> {
        if kind == 3 {
            let store = self.store.as_ref().ok_or(9u32)?;
            let object = store.native_reference(value);
            return self
                .pattern_objects
                .iter()
                .find(|(known, _)| *known == object)
                .map(|(_, index)| *index)
                .ok_or(16u32);
        }
        let text = self.characters(kind, value)?;
        self.pattern(text)
    }

    fn regex(&mut self, request: RegexRequest) -> Result<u64, u32> {
        let RegexRequest {
            op,
            session,
            pattern_kind,
            pattern,
            subject_kind,
            subject,
            index,
        } = request;
        if op > 8 {
            return Err(11);
        }
        // Op 8 is String.findReplace: plain substring replacement, where the
        // pattern and replacement are each one text or parallel lists of text.
        // `pattern_kind` packs both value tags.
        if op == 8 {
            let store = self.store.as_mut().ok_or(9u32)?;
            if store.session_id() != session {
                return Err(1);
            }
            store.ensure_active().map_err(error_code)?;
            let subject_ref = store.native_reference(subject_kind);
            let text = store
                .objects()
                .text(subject_ref)
                .map_err(error_code)?
                .to_owned();
            let patterns = self.text_arguments(pattern_kind >> 32, pattern)?;
            let replacements = self.text_arguments(pattern_kind & 0xffff_ffff, subject)?;
            if patterns.len() != replacements.len() {
                return Err(17);
            }
            // ReplaceOnce is 0x0010; every other flag combination replaces all.
            let once = index & 0x0010 != 0 && index & 0x0001 == 0;
            let result = find_replace(&text, &patterns, &replacements, once)?;
            return self.own_text(&result);
        }
        // Op 7 replaces matches, returning owned text. `index` carries the
        // replacement text (kind in its high bits) and the reference's flags.
        if op == 7 {
            let replacement_kind = index >> 32;
            let replacement = index & 0xffff_ffff;
            let flags = subject >> 32;
            let subject = subject & 0xffff_ffff;
            let subject_text = self.characters(subject_kind, subject)?;
            let replacement_text = self.characters(replacement_kind, replacement)?;
            let cached = self.compiled(pattern_kind, pattern)?;
            // ReplaceAll is 0x0001 and ReplaceOnce is 0x0010; all is the default.
            let once = flags & 0x0010 != 0 && flags & 0x0001 == 0;
            let mut output: Vec<char> = Vec::new();
            let mut at = 0usize;
            loop {
                let found = self.patterns[cached]
                    .1
                    .search(&subject_text, at)
                    .map_err(regex_error)?;
                let Some(found) = found else { break };
                output.try_reserve(found.start - at).map_err(|_| 6u32)?;
                output.extend_from_slice(&subject_text[at..found.start]);
                expand(&replacement_text, &subject_text, &found, &mut output)?;
                let next = if found.end > found.start {
                    found.end
                } else {
                    // an empty match still advances, as the reference does
                    if found.end < subject_text.len() {
                        output.try_reserve(1).map_err(|_| 6u32)?;
                        output.push(subject_text[found.end]);
                    }
                    found.end + 1
                };
                self.last_match = Some((subject_text.clone(), found));
                at = next;
                if once || at > subject_text.len() {
                    break;
                }
            }
            if at <= subject_text.len() {
                output
                    .try_reserve(subject_text.len() - at)
                    .map_err(|_| 6u32)?;
                output.extend_from_slice(&subject_text[at..]);
            }
            let replaced: String = output.into_iter().collect();
            if self.checkpoints.is_empty() {
                return Err(11);
            }
            let store = self.store.as_mut().ok_or(9u32)?;
            store.ensure_active().map_err(error_code)?;
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let value = store.objects().new_text(&replaced).map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(value));
            }
            self.kind = 7;
            return Ok(value.handle());
        }
        // Op 6 compiles a pattern into an object owned by the given field.
        if op == 6 {
            let text = self.characters(pattern_kind, pattern)?;
            let cached = self.pattern(text)?;
            let store = self.store.as_mut().ok_or(9u32)?;
            if store.session_id() != session {
                return Err(1);
            }
            store.ensure_active().map_err(error_code)?;
            let owner = store.native_reference(subject_kind);
            let property = PropertyId(u32::try_from(subject).map_err(|_| 11u32)?);
            let mut objects = store.objects();
            let object = objects.create().map_err(error_code)?;
            if let Err(error) = objects.move_root_into_field(owner, property, object) {
                let _ = objects.destroy(object);
                return Err(error_code(error));
            }
            self.pattern_objects.try_reserve(1).map_err(|_| 6u32)?;
            self.pattern_objects.push((object, cached));
            self.kind = 3;
            return Ok(object.handle());
        }
        // Op 5 returns the reference's result list for a group of the last
        // match, owned by the calling scope like any other local allocation.
        if op == 5 {
            if self.checkpoints.is_empty() {
                return Err(11);
            }
            let group = usize::try_from(index).map_err(|_| 11u32)?;
            let found = self.last_match.as_ref().and_then(|(text, found)| {
                let bounds = if group == 0 {
                    Some((found.start, found.end))
                } else {
                    found.groups.get(group - 1).copied().flatten()
                };
                bounds.map(|(from, to)| (text[from..to].iter().collect::<String>(), from, to))
            });
            let Some((matched, from, to)) = found else {
                self.kind = 0;
                return Ok(0);
            };
            return self.match_list(matched, from, to);
        }
        // Ops 2 and 3 report the last match's group bounds; they need no inputs.
        if op == 2 || op == 3 {
            let group = usize::try_from(index).map_err(|_| 11u32)?;
            let bounds = self.last_match.as_ref().and_then(|(_, found)| {
                if group == 0 {
                    Some((found.start, found.end))
                } else {
                    found.groups.get(group - 1).copied().flatten()
                }
            });
            let Some((from, to)) = bounds else {
                self.kind = 0;
                return Ok(0);
            };
            self.kind = 2;
            // A source index is one-based.
            let value = if op == 2 { from + 1 } else { to - from };
            return u64::try_from(value).map_err(|_| 18u32);
        }
        {
            let store = self.store.as_ref().ok_or(9u32)?;
            if store.session_id() != session {
                return Err(1);
            }
        }
        let subject_text = self.characters(subject_kind, subject)?;
        let cached = self.compiled(pattern_kind, pattern)?;
        // A source index is one-based; 0 means "from the start".
        let start = match usize::try_from(index).map_err(|_| 11u32)? {
            0 => 0,
            n => n - 1,
        };
        if start > subject_text.len() {
            self.kind = 0;
            return Ok(0);
        }
        let pattern = &self.patterns[cached].1;
        let found = if op == 0 {
            pattern.match_at(&subject_text, start)
        } else {
            pattern.search(&subject_text, start)
        }
        .map_err(|error| match error {
            crate::regex::Error::Memory => 6u32,
            crate::regex::Error::Budget => 5,
            crate::regex::Error::Pattern => 17,
        })?;
        match found {
            Some(found) => {
                // op 0 reports the match length, op 1 its one-based start, and
                // op 4 the reference's result list.
                let value = if op == 0 {
                    found.end - found.start
                } else {
                    found.start + 1
                };
                let bounds = (found.start, found.end);
                let matched: String = subject_text[bounds.0..bounds.1].iter().collect();
                self.last_match = Some((subject_text, found));
                if op == 4 {
                    return self.match_list(matched, bounds.0, bounds.1);
                }
                self.kind = 2;
                u64::try_from(value).map_err(|_| 18u32)
            }
            None => {
                self.last_match = None;
                self.kind = 0;
                Ok(0)
            }
        }
    }

    fn dictionary(&mut self, request: DictionaryRequest) -> Result<u64, u32> {
        let DictionaryRequest {
            op,
            session,
            dictionary,
            target,
            flags,
            word,
            property,
        } = request;
        if op > 5 {
            return Err(11);
        }
        let store = self.store.as_mut().ok_or(9u32)?;
        if store.session_id() != session {
            return Err(1);
        }
        store.ensure_active().map_err(error_code)?;
        let property = match flags >> 32 {
            // find and isWordDefined may leave the part of speech out; the
            // vocabulary-relation operations always name one.
            0 if property == 0 && (op == 2 || op == 3) => None,
            8 if op != 3 => Some(PropertyId(u32::try_from(property).map_err(|_| 11u32)?)),
            _ => return Err(16),
        };
        let region = if op == 2 {
            self.init_regions.last().copied()
        } else {
            None
        };
        let word = match flags as u32 {
            // reading the words that name an entity asks about no word,
            // so there is none to decode.
            0 if op == 4 => std::borrow::Cow::Borrowed(""),
            4 => {
                let literal = store
                    .literal(u32::try_from(word).map_err(|_| 11u32)?)
                    .map_err(error_code)?;
                std::borrow::Cow::Borrowed(
                    crate::tables::installed().literals[literal.index() as usize],
                )
            }
            7 => {
                let reference = store.native_reference(word);
                std::borrow::Cow::Owned(
                    store
                        .objects()
                        .copy_text_buffer(reference)
                        .map_err(error_code)?,
                )
            }
            _ => return Err(16),
        };
        let dictionary = store.native_reference(dictionary);
        let target = store.native_reference(target);
        let mut objects = store.objects();
        match op {
            0 => objects
                .dictionary_add(dictionary, target, &word, property.ok_or(16u32)?)
                .map(|()| 0)
                .map_err(error_code),
            1 => objects
                .dictionary_remove(dictionary, target, &word, property.ok_or(16u32)?)
                .map(|()| 0)
                .map_err(error_code),
            2 => {
                let result = objects
                    .dictionary_find_list(dictionary, &word, property)
                    .map_err(error_code)?;
                match region {
                    Some(region) => objects.adopt(region, result).map_err(error_code)?,
                    None => {
                        // Outside a static initializer the match list belongs to
                        // the calling scope, like any other allocation.
                        if self.checkpoints.is_empty() {
                            let _ = objects.destroy(result);
                            return Err(11);
                        }
                        if self.local_owners.try_reserve(1).is_err() {
                            let _ = objects.destroy(result);
                            return Err(6);
                        }
                        // A turn record needs no scope entry.
                        if !self.lifetimes {
                            self.local_owners.push(Some(result));
                        }
                    }
                }
                self.kind = 9;
                Ok(result.handle())
            }
            // a vocabulary relation read forward answers the words
            // that name an entity. The list belongs where an ordinary relation's
            // `all` puts one, because that is the operation this is: only the
            // storage behind it differs.
            4 => {
                let result = objects
                    .dictionary_words(dictionary, target, property.ok_or(16u32)?)
                    .map_err(error_code)?;
                self.kind = 9;
                Ok(result.handle())
            }
            // Whether this entity answers to this word.
            5 => {
                self.kind = u32::from(
                    objects
                        .dictionary_names(dictionary, target, &word, property.ok_or(16u32)?)
                        .map_err(error_code)?,
                );
                Ok(0)
            }
            _ => {
                self.kind = u32::from(
                    objects
                        .dictionary_defined(dictionary, &word)
                        .map_err(error_code)?,
                );
                Ok(0)
            }
        }
    }

    fn perform(&mut self, op: u32, session: u64, a: u64, b: u64, c: u64) -> Result<u64, u32> {
        // Reject operation codes beyond the implemented dispatch table.
        if op > 153 {
            return Err(11);
        }
        /*
         * The tables, a word at a time . 148 says how many bytes are
         * coming, 149 carries eight of them, and 150 decodes and installs.
         *
         * They arrive this way because the boundary is pointer-free and stays
         * that way : a program's tables are data it owns, and the
         * runtime copies them rather than borrowing them.
         */
        if (148..=150).contains(&op) {
            let store = self.store.as_ref().ok_or(9u32)?;
            if store.session_id() != session {
                return Err(1);
            }
            return match op {
                148 => {
                    let len = usize::try_from(a).map_err(|_| 11u32)?;
                    self.pending_tables.clear();
                    self.pending_tables.try_reserve(len).map_err(|_| 4u32)?;
                    Ok(0)
                }
                149 => {
                    self.pending_tables.try_reserve(8).map_err(|_| 4u32)?;
                    self.pending_tables.extend_from_slice(&a.to_le_bytes());
                    Ok(0)
                }
                _ => {
                    // 148 declared the true length; the last word is padded, so
                    // the declared length is what is read.
                    let len = usize::try_from(a).map_err(|_| 11u32)?;
                    let bytes = self.pending_tables.get(..len).ok_or(11u32)?;
                    crate::tables::install(bytes).map_err(|_| 11u32)?;
                    self.pending_tables = Vec::new();
                    Ok(0)
                }
            };
        }
        if op == 0 {
            if session != 0 {
                return Err(11);
            }
            if self.store.is_some() {
                return Err(10);
            }
            let limits = Limits {
                objects: usize::try_from(a).map_err(|_| 11u32)?,
                properties: usize::try_from(b).map_err(|_| 11u32)?,
                string_bytes: 8 * 1024 * 1024,
            };
            let output_limit = usize::try_from(c).map_err(|_| 11u32)?;
            let store = Store::new(limits).map_err(error_code)?;
            let id = store.session_id();
            self.output.reset(output_limit);
            self.store = Some(store);
            self.lifetimes = false;
            return Ok(id);
        }
        // Restore each lexical checkpoint separately, innermost first. This also
        // lets exception owners be released after their handler's local resources.
        if op == 65 {
            let store = self.store.as_ref().ok_or(9u32)?;
            if store.session_id() != session {
                return Err(1);
            }
            let index = self
                .checkpoints
                .iter()
                .position(|mark| mark.id == a)
                .ok_or(11u32)?;
            while self.checkpoints.len() > index {
                let id = self.checkpoints.last().ok_or(11u32)?.id;
                self.perform(64, session, id, 0, 0)?;
            }
            return Ok(0);
        }
        let store = self.store.as_mut().ok_or(9u32)?;
        if store.session_id() != session {
            return Err(1);
        }
        if op == 151 {
            if a >= 4 || c != 0 {
                return Err(11);
            }
            let offset = a as usize * 8;
            self.save_schema[offset..offset + 8].copy_from_slice(&b.to_le_bytes());
            self.save_schema_words |= 1 << a;
            return Ok(0);
        }
        if op == 9 {
            self.save_schema_words = 0;
            // Hand over whatever the session gathered but never flushed, so a
            // program that finishes without ever asking for input still has its
            // output reach the host. The bytes stay readable through
            // the session-local `output_byte` as well, which is what a direct
            // caller of the operation ABI reads.
            crate::session::publish(self.slot, self.output.bytes())?;
            self.store = None;
            self.init_regions.clear();
            self.constant_region = None;
            self.constant_lists.clear();
            self.parses.clear();
            self.sorts.clear();
            self.patterns.clear();
            self.pattern_objects.clear();
            self.last_match = None;
            self.relations.reset();
            self.label_families.clear();
            self.history.clear();
            self.literal_texts.clear();
            self.local_owners.clear();
            self.returned_owners.clear();
            self.constructions.clear();
            self.exceptions.clear();
            self.member_calls.clear();
            self.iterations.clear();
            self.checkpoints.clear();
            return Ok(0);
        }
        if op == 76 {
            // Transfer only a completed top owned constructor, never a borrowed
            // object or published activation. Reserve inventory before mutation.
            if b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            if self
                .checkpoints
                .last()
                .is_none_or(|mark| mark.constructions >= self.constructions.len())
            {
                return Err(11);
            }
            let object = store.native_reference(a);
            let Some(Activation::Owned(construction)) = self.constructions.last_mut() else {
                return Err(11);
            };
            if construction.reference() != Some(object) {
                return Err(11);
            }
            if self.exceptions.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let owner = store
                .objects()
                .finish_exception(construction)
                .map_err(error_code)?;
            let token = owner.token().ok_or(11u32)?;
            self.constructions.pop();
            self.exceptions.push((owner, None));
            return Ok(token);
        }
        if op == 77 || op == 78 {
            if b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            let index = self
                .exceptions
                .iter()
                .position(|(owner, _)| owner.token() == Some(a))
                .ok_or(11u32)?;
            if op == 77 {
                let object = self.exceptions[index].0.reference().ok_or(11u32)?;
                // Validate identity even though the private token is still present.
                store
                    .objects()
                    .of_kind(object, object)
                    .map_err(error_code)?;
                self.kind = 3;
                return Ok(object.handle());
            }
            store
                .objects()
                .release_exception(&mut self.exceptions[index].0)
                .map_err(error_code)?;
            self.exceptions.swap_remove(index);
            return Ok(0);
        }
        if op == 96 || op == 99 {
            if op == 99 && c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            // a is a local root, b the recipient, c the property id.
            let root = store.native_reference(a);
            let owner = store.native_reference(b);
            let property = PropertyId(u32::try_from(c).map_err(|_| 11u32)?);
            let index = self
                .local_owners
                .iter()
                .rposition(|value| *value == Some(root))
                .ok_or(3u32)?;
            if op == 99 {
                store.objects().move_root_into_collection(owner, root)
            } else {
                store.objects().move_root_into_field(owner, property, root)
            }
            .map_err(error_code)?;
            self.local_owners[index] = None;
            self.kind = 0;
            return Ok(0);
        }
        if op == 83 {
            if c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            let frame = self
                .checkpoints
                .iter()
                .find(|mark| mark.id == b)
                .ok_or(11u32)?;
            let object = store.native_reference(a);
            let index = self
                .local_owners
                .iter()
                .rposition(|owner| *owner == Some(object))
                .filter(|index| *index >= frame.locals)
                .ok_or(3u32)?;
            // Reserve the receiving inventory before consuming the local slot.
            if self.returned_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            store
                .objects()
                .of_kind(object, object)
                .map_err(error_code)?;
            self.local_owners[index] = None;
            self.returned_owners.push((object, b));
            return Ok(object.handle());
        }
        if op == 84 {
            if c > 1 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            if a == 0 {
                return if c == 0 { Ok(0) } else { Err(17) };
            }
            let index = self
                .returned_owners
                .iter()
                .position(|(owner, _)| owner.handle() == a)
                .ok_or(11u32)?;
            let (object, origin) = self.returned_owners[index];
            if self.checkpoints.iter().any(|mark| mark.id == origin) {
                return Err(11);
            }
            if c == 0 {
                store.objects().destroy(object).map_err(error_code)?;
                self.returned_owners.swap_remove(index);
                return Err(17);
            }
            if b != object.handle() || self.checkpoints.is_empty() {
                return Err(11);
            }
            store
                .objects()
                .of_kind(object, object)
                .map_err(error_code)?;
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            self.returned_owners.swap_remove(index);
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(object));
            }
            self.kind = 3;
            return Ok(object.handle());
        }
        if op == 82 {
            if b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            if self
                .checkpoints
                .last()
                .is_none_or(|mark| mark.constructions >= self.constructions.len())
            {
                return Err(11);
            }
            let object = store.native_reference(a);
            let Some(Activation::Owned(construction)) = self.constructions.last_mut() else {
                return Err(11);
            };
            if construction.reference() != Some(object) {
                return Err(11);
            }
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let result = store
                .objects()
                .finish_local_construction(construction)
                .map_err(error_code)?;
            self.constructions.pop();
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(result));
            }
            self.kind = 3;
            return Ok(result.handle());
        }
        if op == 81 {
            if b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            let object = store.native_reference(a);
            store
                .objects()
                .of_kind(object, object)
                .map_err(error_code)?;
            // Explicit throw transfers only an existing exception owner. It never
            // takes ownership from a field, local object or published recipient.
            if let Some((owner, scope)) = self
                .exceptions
                .iter_mut()
                .find(|(owner, _)| owner.reference() == Some(object))
            {
                *scope = None;
                return owner.token().ok_or(11u32);
            }
            return Ok(0);
        }
        if op == 79 || op == 80 {
            if b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            // Zero is the non-owning source-exception outcome.
            if a == 0 {
                return Ok(0);
            }
            let index = self
                .exceptions
                .iter()
                .position(|(owner, _)| owner.token() == Some(a))
                .ok_or(11u32)?;
            if op == 80 {
                if self.exceptions[index].1.take().is_none() {
                    return Err(11);
                }
                return Ok(a);
            }
            let mark = self.checkpoints.last().ok_or(11u32)?;
            // A handler acquires its exception first, in a fresh lexical scope.
            if self.exceptions[index].1.is_some()
                || self
                    .exceptions
                    .iter()
                    .any(|(_, scope)| *scope == Some(mark.id))
                || self.iterations.len() != mark.iterations
                || self.constructions.len() != mark.constructions
                || self.member_calls.len() != mark.members
                || self.init_regions.len() != mark.initializers
                || self.local_owners.len() != mark.locals
            {
                return Err(11);
            }
            self.exceptions[index].1 = Some(mark.id);
            return Ok(a);
        }
        if op == 75 {
            if self.checkpoints.is_empty() || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            let object = store.native_reference(a);
            let skip = usize::try_from(b).map_err(|_| 11u32)?;
            if skip > store.objects().list(object).map_err(error_code)?.len() {
                return Err(17);
            }
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let list = store
                .objects()
                .argument_tail(object, skip)
                .map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(list));
            }
            self.kind = 9;
            return Ok(list.handle());
        }
        if op == 66 || op == 73 || op == 85 || op == 86 {
            // Local allocation requires an enclosing cleanup checkpoint. Reserve
            // the owner inventory before allocating the source object.
            if self.checkpoints.is_empty() || b != 0 || c != 0 || ((op == 85 || op == 86) && a != 0)
            {
                return Err(11);
            }
            let capacity = u32::try_from(a).map_err(|_| 11u32)? as i32;
            if capacity < 0 {
                return Err(17);
            }
            store.ensure_active().map_err(error_code)?;
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let object = if op == 66 {
                store.objects().new_vector(capacity)
            } else if op == 85 {
                store.objects().new_lookup()
            } else if op == 86 {
                store.objects().new_string_buffer()
            } else {
                store.objects().begin_list(capacity as usize)
            }
            .map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(object));
            }
            self.kind = if op == 73 { 9 } else { 3 };
            return Ok(object.handle());
        }
        if op == 107 {
            if self.checkpoints.is_empty() {
                return Err(11);
            }
            let function = u32::try_from(a).map_err(|_| 11u32)?;
            let modes = store.native_reference(b);
            let values = store.native_reference(c);
            let mut objects = store.objects();
            let modes = objects.list(modes).map_err(error_code)?;
            let values = objects.list(values).map_err(error_code)?;
            if modes.len() != values.len() {
                return Err(11);
            }
            let mut captures = Vec::new();
            if captures.try_reserve_exact(values.len()).is_err()
                || self.local_owners.try_reserve(1).is_err()
            {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            for (mode, value) in modes.iter().zip(values) {
                captures.push(match mode {
                    Value::Int(0) => Capture::Copy(*value),
                    Value::Int(1) => Capture::Reference(*value),
                    // Mode 2 lets the compiler capture without a static type:
                    // referenced values by reference, everything else by copy.
                    Value::Int(2) => match value {
                        Value::Reference(_) | Value::Text(_) | Value::List(_) => {
                            Capture::Reference(*value)
                        }
                        _ => Capture::Copy(*value),
                    },
                    _ => return Err(11),
                });
            }
            let closure = objects
                .new_closure(function, &captures)
                .map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(closure));
            }
            self.kind = 3;
            return Ok(closure.handle());
        }
        if op == 108 {
            if b != 0 || c != 0 {
                return Err(11);
            }
            let closure = store.native_reference(a);
            return store
                .objects()
                .closure_function(closure)
                .map(u64::from)
                .map_err(error_code);
        }
        if op == 109 {
            if c != 0 {
                return Err(11);
            }
            let closure = store.native_reference(a);
            let value = store
                .objects()
                .closure_capture(closure, usize::try_from(b).map_err(|_| 11u32)?)
                .map_err(error_code)?;
            let (kind, payload) = match value {
                Value::Nil => (0, 0),
                Value::Bool(v) => (u32::from(v), 0),
                Value::Int(v) => (2, u64::from(v as u32)),
                Value::Reference(v) => (3, v.handle()),
                Value::String(v) => (4, u64::from(v.index())),
                Value::Text(v) => (7, v.handle()),
                Value::Property(v) => (8, u64::from(v.0)),
                Value::List(v) => (9, v.handle()),
                Value::Enumerator(v) => (10, u64::from(v)),
                Value::Function(v) => (11, u64::from(v)),
                _ => return Err(16),
            };
            self.kind = kind;
            return Ok(payload);
        }
        if op == 106 {
            // the flag names which records the walk takes. Only 2,
            // classes, was implemented; 1 and 3 were refused at this line, so a
            // program that asked for instances compiled and died on its first
            // call.
            let want = crate::objects::Derived::from_flags(c).ok_or(11u32)?;
            let after = (a != 0).then(|| store.native_reference(a));
            let ancestor = store.native_reference(b);
            let found = store
                .objects()
                .next_derived(after, ancestor, want)
                .map_err(error_code)?;
            self.kind = if found.is_some() { 3 } else { 0 };
            return Ok(found.map_or(0, |object| object.handle()));
        }
        if op == 105 {
            // c selects the reference's PropDef mode: 0 or 1 ask whether the
            // property is defined at all, 2 directly, 3 by inheritance, and
            // 4 names the defining object.
            let object = store.native_reference(a);
            let property = PropertyId(u32::try_from(b).map_err(|_| 11u32)?);
            if c > 4 {
                return Err(11);
            }
            if c <= 1 {
                let found = store
                    .objects()
                    .property_defined(object, property)
                    .map_err(error_code)?;
                self.kind = u32::from(found);
                return Ok(u64::from(found));
            }
            let definer = store
                .objects()
                .property_definer(object, property)
                .map_err(error_code)?;
            return match c {
                2 => {
                    let direct = definer == Some(object);
                    self.kind = u32::from(direct);
                    Ok(u64::from(direct))
                }
                3 => {
                    let inherited = definer.is_some_and(|definer| definer != object);
                    self.kind = u32::from(inherited);
                    Ok(u64::from(inherited))
                }
                _ => match definer {
                    Some(definer) => {
                        self.kind = 3;
                        Ok(definer.handle())
                    }
                    None => {
                        self.kind = 0;
                        Ok(0)
                    }
                },
            };
        }
        if op == 104 {
            let vector = store.native_reference(a);
            let index = u32::try_from(b).map_err(|_| 11u32)? as i32;
            let values = store.native_reference(c);
            store
                .objects()
                .vector_insert_values(vector, index, values)
                .map_err(error_code)?;
            self.kind = 3;
            return Ok(a);
        }
        if op == 103 {
            let vector = store.native_reference(a);
            let value = lookup_value(store, b, c)?;
            let index = store
                .objects()
                .vector_index_of(vector, value)
                .map_err(error_code)?;
            self.kind = if index.is_some() { 2 } else { 0 };
            return Ok(index.unwrap_or(0) as u64);
        }
        if op == 102 {
            let source = store.native_reference(a);
            let destination = store.native_reference(b);
            let source_property = PropertyId(c as u32);
            let destination_property = PropertyId((c >> 32) as u32);
            let moved = store
                .objects()
                .move_field_owner(source, source_property, destination, destination_property)
                .map_err(error_code)?;
            self.kind = if moved { 1 } else { 0 };
            return Ok(u64::from(moved));
        }
        if op == 101 {
            let source = store.native_reference(a);
            let object = store.native_reference(b);
            let destination = store.native_reference(c);
            let moved = store
                .objects()
                .move_owned_member(source, object, destination)
                .map_err(error_code)?;
            self.kind = if moved { 1 } else { 0 };
            return Ok(u64::from(moved));
        }
        if op == 100 {
            if self.checkpoints.is_empty() || b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let vector = store.native_reference(a);
            let list = store
                .objects()
                .vector_snapshot(vector)
                .map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(list));
            }
            self.kind = 9;
            return Ok(list.handle());
        }
        if op == 98 {
            if self.checkpoints.is_empty() || a != 0 || b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let collection = store.objects().new_owned_collection().map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(collection));
            }
            self.kind = 3;
            return Ok(collection.handle());
        }
        if op == 97 {
            if self.checkpoints.is_empty() || c != 0 {
                return Err(11);
            }
            let buckets = u32::try_from(a).map_err(|_| 11u32)? as i32;
            let capacity = u32::try_from(b).map_err(|_| 11u32)? as i32;
            store.ensure_active().map_err(error_code)?;
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let table = store
                .objects()
                .new_lookup_sized(buckets, capacity)
                .map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(table));
            }
            self.kind = 3;
            return Ok(table.handle());
        }
        if op == 93 {
            if b != 0 || c != 0 {
                return Err(11);
            }
            let mut objects = store.objects();
            let region = if let Some(region) = self.constant_region {
                region
            } else {
                let region = objects.create().map_err(error_code)?;
                self.constant_region = Some(region);
                region
            };
            let list = objects
                .begin_list(usize::try_from(a).map_err(|_| 11u32)?)
                .map_err(error_code)?;
            objects.adopt(region, list).map_err(error_code)?;
            self.constant_lists.try_reserve(1).map_err(|_| 6u32)?;
            self.constant_lists.push(list);
            self.kind = 9;
            return Ok(list.handle());
        }
        if (112..=114).contains(&op) {
            //  resumable Vector.sort; a is the vector handle. 112 begins
            // (b: descending), 113 steps, 114 reads element b of the pending pair.
            let vector = store.native_reference(a);
            let mut objects = store.objects();
            if op == 112 {
                if b > 1 || c != 0 {
                    return Err(11);
                }
                let len = objects.vector(vector).map_err(error_code)?.len();
                let sort = QuickSort::new(len, b == 1).ok_or(6u32)?;
                self.sorts.try_reserve(1).map_err(|_| 6u32)?;
                self.sorts.push(PendingSort {
                    vector,
                    sort,
                    pair: (0, 0),
                });
                self.kind = 0;
                return Ok(0);
            }
            let pending = self
                .sorts
                .last_mut()
                .filter(|pending| pending.vector == vector)
                .ok_or(11u32)?;
            if op == 114 {
                if b > 1 || c != 0 || !pending.sort.awaiting() {
                    return Err(11);
                }
                let index = if b == 0 {
                    pending.pair.0
                } else {
                    pending.pair.1
                };
                let index = i32::try_from(index + 1).map_err(|_| 11u32)?;
                let value = objects.vector_get(vector, index).map_err(error_code)?;
                let (kind, payload) = match value {
                    Value::Nil => (0, 0),
                    Value::Bool(value) => (u32::from(value), 0),
                    Value::Int(value) => (2, u64::from(value as u32)),
                    Value::Reference(value) => (3, value.handle()),
                    Value::String(value) => (4, u64::from(value.index())),
                    Value::Text(value) => (7, value.handle()),
                    Value::Property(value) => (8, u64::from(value.0)),
                    Value::List(value) => (9, value.handle()),
                    Value::Enumerator(value) => (10, u64::from(value)),
                    Value::Function(value) => (11, u64::from(value)),
                    _ => return Err(16),
                };
                self.kind = kind;
                return Ok(payload);
            }
            // b and c are the comparator result's tag and payload; the first
            // step, before any comparison, ignores them.
            let result = if pending.sort.awaiting() {
                if b != 2 {
                    self.sorts.pop();
                    return Err(error_code(Error::WrongType));
                }
                Some(c as u32 as i32)
            } else {
                None
            };
            let mut elements = VectorElements {
                objects: &mut objects,
                vector,
            };
            return match pending.sort.resume(&mut elements, result) {
                Ok(Some(pair)) => {
                    pending.pair = pair;
                    self.kind = 1;
                    Ok(1)
                }
                Ok(None) => {
                    self.sorts.pop();
                    self.kind = 0;
                    Ok(0)
                }
                Err(error) => {
                    self.sorts.pop();
                    Err(error_code(error))
                }
            };
        }
        if op == 116 {
            // join two lists; the result belongs to the calling scope.
            if c != 0 {
                return Err(11);
            }
            if self.checkpoints.is_empty() {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            let (left, right) = (store.native_reference(a), store.native_reference(b));
            let objects = store.objects();
            let mut values = Vec::new();
            {
                let head = objects.list(left).map_err(error_code)?;
                values.try_reserve(head.len()).map_err(|_| 6u32)?;
                values.extend_from_slice(head);
            }
            {
                let tail = objects.list(right).map_err(error_code)?;
                values.try_reserve(tail.len()).map_err(|_| 6u32)?;
                values.extend_from_slice(tail);
            }
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let joined = store.objects().new_list(&values).map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(joined));
            }
            self.kind = 9;
            return Ok(joined.handle());
        }
        if (119..=124).contains(&op) {
            // systype.h String methods. `a` is the subject text.
            store.ensure_active().map_err(error_code)?;
            let subject = store.native_reference(a);
            let text = store
                .objects()
                .text(subject)
                .map_err(error_code)?
                .to_owned();
            if (119..=121).contains(&op) {
                let other = store.native_reference(b);
                let needle = store.objects().text(other).map_err(error_code)?.to_owned();
                if op == 119 || op == 120 {
                    if c != 0 {
                        return Err(11);
                    }
                    let hit = if op == 119 {
                        text.starts_with(&needle)
                    } else {
                        text.ends_with(&needle)
                    };
                    self.kind = u32::from(hit);
                    return Ok(0);
                }
                // find(str, index?): a one-based character index, or nil.
                let characters: Vec<char> = text.chars().collect();
                let start = match c {
                    0 => 0usize,
                    _ => {
                        let index = u32::try_from(c).map_err(|_| 11u32)? as i32;
                        if index > 0 {
                            usize::try_from(index - 1).map_err(|_| 11u32)?
                        } else {
                            // A negative index counts back from the end.
                            let back = usize::try_from(-i64::from(index)).map_err(|_| 11u32)?;
                            characters.len().saturating_sub(back)
                        }
                    }
                };
                let pattern: Vec<char> = needle.chars().collect();
                let mut at = start;
                while at + pattern.len() <= characters.len() {
                    if characters[at..at + pattern.len()] == pattern[..] {
                        self.kind = 2;
                        return i32::try_from(at + 1).map(|n| n as u64).map_err(|_| 18u32);
                    }
                    at += 1;
                }
                self.kind = 0;
                return Ok(0);
            }
            if c != 0 {
                return Err(11);
            }
            let result = match op {
                122 => text.to_lowercase(),
                123 => text.to_uppercase(),
                _ => htmlify(&text, u32::try_from(b).map_err(|_| 11u32)?)?,
            };
            return self.own_text(&result);
        }
        if op == 142 {
            // The entities the host's action intent named.
            if a != 0 || b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            let mut values = Vec::new();
            values.try_reserve(self.subjects.len()).map_err(|_| 6u32)?;
            for scalar in std::mem::take(&mut self.subjects) {
                use crate::session::Scalar;
                values.push(match scalar {
                    Scalar::Nil => Value::Nil,
                    Scalar::True => Value::Bool(true),
                    Scalar::Integer(n) => Value::Int(n),
                    Scalar::Entity(h) => Value::Reference(store.native_reference(h)),
                    Scalar::Text(bytes) => Value::Text(
                        store
                            .objects()
                            .new_text(std::str::from_utf8(&bytes).map_err(|_| 17u32)?)
                            .map_err(error_code)?,
                    ),
                });
            }
            let list = store.objects().new_list(&values).map_err(error_code)?;
            self.kind = 9;
            return Ok(list.handle());
        }
        if op == 143 || op == 144 {
            // ask the host to do something, or to answer something.
            // The game proposes and waits; the world advances only on what
            // comes back, which is what the embedding contract requires of a
            // movement the host animates.
            if b != 0 || c != 0 {
                return Err(11);
            }
            let request = if op == 143 {
                crate::session::Request::HostAction
            } else {
                crate::session::Request::EngineQuery
            };
            let bytes = self.output.take();
            let reply = crate::session::suspend(self.slot, request, &bytes)?;
            return match reply {
                crate::session::Reply::Accepted(accepted) => {
                    self.kind = u32::from(accepted);
                    Ok(u64::from(accepted))
                }
                crate::session::Reply::Value(value) => {
                    let value = i32::try_from(value).map_err(|_| 18u32)?;
                    self.kind = 2;
                    Ok(value as u32 as u64)
                }
                // The host closed instead of answering.
                _ => {
                    self.kind = 0;
                    Ok(0)
                }
            };
        }
        if op == 141 {
            // despawn. Every row naming the entity is retracted
            // first, so no relation answers for something that is gone, and
            // then the record itself goes. Handles to it read as expired, which
            // is what generational identity is for.
            if b != 0 || c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            let entity = store.native_reference(a);
            store.objects().record_exists(entity).map_err(error_code)?;
            let rows = self.relations.forget(entity).map_err(error_code)?;
            store.objects().despawn(entity).map_err(error_code)?;
            self.kind = 2;
            return i32::try_from(rows).map(|n| n as u64).map_err(|_| 18u32);
        }
        if op == 140 {
            // ask the host for the next command. The cycle's output
            // goes with the request, the host answers when it is ready, and the
            // reply comes back as the token list a grammar production parses.
            // `a` is the word-token enumerator, which only the compiler knows.
            if b != 0 || c != 0 {
                return Err(11);
            }
            let token = u32::try_from(a).map_err(|_| 11u32)?;
            store.ensure_active().map_err(error_code)?;
            // Between cycles is the safe boundary: no cycle is open, so an
            // inspector reads a world that is not part-way through a command.
            // Reading never calls into game code.
            if let Some((kind, x, y)) = crate::session::take_inspection(self.slot) {
                // An inspection that cannot be answered answers nothing. Reading
                // the world must never be able to stop the game.
                let values =
                    inspect(&mut store.objects(), &self.relations, kind, x, y).unwrap_or_default();
                crate::session::publish_results(self.slot, &values)?;
            }
            let bytes = self.output.take();
            let mut output = bytes;
            let reply = loop {
                let reply =
                    crate::session::suspend(self.slot, crate::session::Request::Command, &output)?;
                output.clear();
                let crate::session::Reply::Persistence { restore, bytes } = reply else {
                    break reply;
                };
                let result = (|| {
                    if !self.lifetimes
                        || self.save_schema_words != 15
                        || !self.parses.is_empty()
                        || !self.sorts.is_empty()
                        || !self.constructions.is_empty()
                        || !self.exceptions.is_empty()
                        || !self.iterations.is_empty()
                        || self.pattern_objects.iter().any(|(r, _)| {
                            store.is_valid(*r) && !store.objects().is_transient(*r).unwrap_or(false)
                        })
                    {
                        return Err(11);
                    }
                    if restore {
                        let (mut restored, relations, literal_texts) = crate::save::decode(
                            &bytes,
                            store,
                            &self.save_schema,
                            store.save_row_limit(),
                        )
                        .map_err(error_code)?;
                        // Code's module constants are logical records too. Restore their
                        // references before replacing the store; no saved token survives.
                        let remap = |r: ObjectRef| {
                            restored
                                .saved_reference(r.handle() & 0xffff_ffff)
                                .ok_or(11u32)
                        };
                        let mut lists = Vec::new();
                        lists
                            .try_reserve(self.constant_lists.len())
                            .map_err(|_| 6u32)?;
                        for r in &self.constant_lists {
                            lists.push(remap(*r)?);
                        }
                        let mut regions = Vec::new();
                        regions
                            .try_reserve(self.init_regions.len())
                            .map_err(|_| 6u32)?;
                        for r in &self.init_regions {
                            regions.push(remap(*r)?);
                        }
                        let constant_region = self.constant_region.map(remap).transpose()?;
                        restored
                            .objects()
                            .set_default_lifetime(crate::objects::Lifetime::Turn);
                        restored.objects().set_journalling(true);
                        *store = restored;
                        self.relations = relations;
                        self.relations.set_journalling(true);
                        self.constant_lists = lists;
                        self.init_regions = regions;
                        self.constant_region = constant_region;
                        self.literal_texts = literal_texts;
                        self.patterns.clear();
                        self.pattern_objects.clear();
                        self.last_match = None;
                        self.history.clear();
                        self.subjects.clear();
                        self.tick_origin = None;
                        crate::session::restored(self.slot)?;
                        Ok(Vec::new())
                    } else {
                        crate::save::encode(
                            store,
                            &self.relations,
                            &self.save_schema,
                            &self.literal_texts,
                        )
                        .map_err(error_code)
                    }
                })();
                crate::session::saved(self.slot, result)?;
            };
            return match reply {
                // A parser front end's intent: the line, tokenized.
                crate::session::Reply::Line(Some(line)) => {
                    let list = tokenize(&mut store.objects(), &line, token).map_err(error_code)?;
                    self.kind = 9;
                    Ok(list.handle())
                }
                // A host that chose the action itself. The verb answers here and
                // the entities it names are read with op 142.
                crate::session::Reply::Action { verb, subjects } => {
                    self.subjects = subjects;
                    self.kind = 2;
                    Ok(u64::from(verb))
                }
                // No more input: the host has closed the session.
                _ => {
                    self.kind = 0;
                    Ok(0)
                }
            };
        }
        if (129..=139).contains(&op) {
            // /: relation tables and the two lifetimes.
            store.ensure_active().map_err(error_code)?;
            if op == 139 {
                if c != 0 || (b != 0 && a != 8) {
                    return Err(11);
                }
                return match a {
                    // Bind this session to a handoff, so one process can run
                    // several independently.
                    8 => {
                        self.slot = b;
                        self.kind = 0;
                        Ok(0)
                    }
                    // Select the lifetime model for this program.
                    3 => {
                        self.lifetimes = true;
                        store.objects().set_lifetime_model(true);
                        self.kind = 0;
                        Ok(0)
                    }
                    // Begin a command cycle: new records belong to the turn,
                    // and every change from here is recorded so the cycle can
                    // be put back. Starting one also selects the
                    // lifetime model.
                    0 => {
                        self.lifetimes = true;
                        {
                            let mut objects = store.objects();
                            objects.set_lifetime_model(true);
                            objects.set_default_lifetime(crate::objects::Lifetime::Turn);
                            objects.set_journalling(true);
                        }
                        self.relations.set_journalling(true);
                        self.kind = 0;
                        Ok(0)
                    }
                    // End it: every turn record goes at once, and the cycle's
                    // changes move to the undo history.
                    1 | 9 => {
                        let containers = store.objects().take_container_journal();
                        let properties = {
                            let mut objects = store.objects();
                            let journal = objects.take_property_journal();
                            objects.set_journalling(false);
                            journal
                        };
                        self.relations.set_journalling(false);
                        let relations = self.relations.take_journal();
                        // A cycle that changed nothing is not worth undoing,
                        // and recording it would make UNDO appear to do nothing
                        // when the player asked to undo the turn before it.
                        if !properties.is_empty() || !relations.is_empty() || !containers.is_empty()
                        {
                            if self.history.len() >= UNDO_DEPTH {
                                self.history.remove(0);
                            }
                            self.history.try_reserve(1).map_err(|_| 6u32)?;
                            self.history.push(Cycle {
                                containers,
                                properties,
                                relations,
                            });
                        }
                        if a == 9 {
                            store.objects().set_journalling(true);
                            self.relations.set_journalling(true);
                            self.kind = 0;
                            return Ok(0);
                        }
                        let released =
                            store.objects().release_turn_records().map_err(error_code)?;
                        store
                            .objects()
                            .set_default_lifetime(crate::objects::Lifetime::World);
                        self.kind = 2;
                        i32::try_from(released).map(|n| n as u64).map_err(|_| 18u32)
                    }
                    // How many changes this cycle has recorded so far.
                    2 => {
                        let changes = self.relations.journal().len()
                            + store.objects().property_journal_len()
                            + store.objects().container_journal_len();
                        self.kind = 2;
                        i32::try_from(changes).map(|n| n as u64).map_err(|_| 18u32)
                    }
                    // Put every change this cycle made back, leaving the
                    // cycle open so the game can still report what happened.
                    // This is what a failed turn runs, so a turn that dies
                    // half-done leaves no half-done world.
                    4 => {
                        let containers = store.objects().take_container_journal();
                        let containers = store.objects().revert_containers(containers);
                        let properties = store.objects().take_property_journal();
                        let relations = self.relations.take_journal();
                        let rows = self.relations.revert(&relations);
                        let fields = store.objects().revert_properties(&properties);
                        if self.slot != 0 {
                            crate::session::resync(self.slot)?;
                        }
                        self.kind = 2;
                        i32::try_from(rows + fields + containers)
                            .map(|n| n as u64)
                            .map_err(|_| 18u32)
                    }
                    // Undo the most recently completed cycle. Answers true when
                    // one was put back, nil when the history is empty, which is
                    // what the reference's `undo` reports.
                    5 => {
                        // Undo is history navigation. Discard this invocation's
                        // preparatory writes too (e.g. library command state),
                        // and do not manufacture an undo-of-undo cycle.
                        let pending_containers = store.objects().take_container_journal();
                        store.objects().revert_containers(pending_containers);
                        let pending_properties = store.objects().take_property_journal();
                        let pending_relations = self.relations.take_journal();
                        self.relations.revert(&pending_relations);
                        store.objects().revert_properties(&pending_properties);
                        self.relations.set_journalling(false);
                        store.objects().set_journalling(false);
                        let Some(cycle) = self.history.pop() else {
                            self.kind = 0;
                            return Ok(0);
                        };
                        store.objects().revert_containers(cycle.containers);
                        self.relations.revert(&cycle.relations);
                        store.objects().revert_properties(&cycle.properties);
                        if self.slot != 0 {
                            crate::session::resync(self.slot)?;
                        }
                        self.kind = 1;
                        Ok(1)
                    }
                    // Say that the turn failed, for a game that declares no
                    // `recover` of its own. Better a plain line than silence.
                    7 => {
                        self.output
                            .append("[the turn could not be completed]\n")
                            .map_err(error_code)?;
                        self.kind = 0;
                        Ok(0)
                    }
                    // How many cycles can still be undone.
                    6 => {
                        self.kind = 2;
                        i32::try_from(self.history.len())
                            .map(|n| n as u64)
                            .map_err(|_| 18u32)
                    }
                    _ => Err(11),
                };
            }
            // reserve a declared relation's table. Every declared
            // relation is reserved at initialization, so a labelled relation's
            // tables — which are allocated after the ones in use — can never be
            // handed an index another relation was going to claim.
            if op == 129 {
                let index = usize::try_from(a).map_err(|_| 11u32)?;
                let cardinality = crate::relations::Cardinality::from_code(b).ok_or(11u32)?;
                self.relations
                    .ensure(index, cardinality)
                    .map_err(error_code)?;
                if c != 0 {
                    let start = ((c >> 1) & 0x1fff) as u32;
                    let end = (c >> 14) as u32;
                    if c & 1 == 0 || start >= end || end > 4096 {
                        return Err(11);
                    }
                    self.label_families.try_reserve(1).map_err(|_| 6u32)?;
                    self.label_families.insert(index, (start, end));
                }
                self.kind = 0;
                return Ok(0);
            }
            if op == 130 {
                if b != 0 || c != 0 {
                    return Err(11);
                }
                let cardinality = crate::relations::Cardinality::from_code(a).ok_or(11u32)?;
                let id = self.relations.declare(cardinality).map_err(error_code)?;
                self.kind = 2;
                return i32::try_from(id).map(|n| n as u64).map_err(|_| 18u32);
            }
            // The relation identifier carries the direction in its high bits, so
            // a reverse name reads the same table the other way.
            let declared = usize::try_from(a & 0xfff).map_err(|_| 11u32)?;
            let reversed = (a >> 12) & 1 != 0;
            let cardinality =
                crate::relations::Cardinality::from_code((a >> 13) & 3).ok_or(11u32)?;
            // a labelled relation carries its label in the descriptor
            // and answers from the table that label owns, so every query below
            // is the query an unlabelled relation answers.
            // `all` without a label reads the whole family instead of
            // one table, which is how a room lists the ways out of it.
            let every_label = (a >> 28) & 1 != 0;
            if every_label && op != 134 {
                return Err(11);
            }
            let relation = if (a >> 15) & 1 != 0 && !every_label {
                let label = u32::try_from((a >> 16) & 0xfff).map_err(|_| 11u32)?;
                if self
                    .label_families
                    .get(&declared)
                    .is_some_and(|(start, end)| label < *start || label >= *end)
                {
                    return Err(11);
                }
                self.relations
                    .ensure_labelled(declared, label, cardinality)
                    .map_err(error_code)?
            } else {
                declared
            };
            self.relations
                .ensure(relation, cardinality)
                .map_err(error_code)?;
            let key = store.native_reference(b);
            store.objects().of_kind(key, key).map_err(error_code)?;
            if matches!(op, 131 | 132 | 135) {
                let other = store.native_reference(c);
                store.objects().of_kind(other, other).map_err(error_code)?;
                // a reverse name reads the table right to left, so a row
                // it writes goes in the same way round as a row it reads.
                // `location.set(coin, box)` puts the coin in the box, exactly as
                // `location.get(coin)` answers the box.
                let (key, right) = if reversed { (other, key) } else { (key, other) };
                if op == 135 {
                    let held = self
                        .relations
                        .contains(relation, key, right)
                        .map_err(error_code)?;
                    self.kind = u32::from(held);
                    return Ok(0);
                }
                if op == 132 {
                    let removed = self
                        .relations
                        .unset(relation, key, right)
                        .map_err(error_code)?;
                    self.kind = u32::from(removed);
                    return Ok(0);
                }
                // relating a turn value to world state promotes it,
                // and the author writes nothing to say so.
                let mut objects = store.objects();
                let world = crate::objects::Lifetime::World;
                if objects.lifetime(key).map_err(error_code)? == world {
                    objects.promote(right).map_err(error_code)?;
                }
                if objects.lifetime(right).map_err(error_code)? == world {
                    objects.promote(key).map_err(error_code)?;
                }
                self.relations
                    .set(relation, key, right)
                    .map_err(error_code)?;
                self.kind = 0;
                return Ok(0);
            }
            if op == 133 {
                let found = self
                    .relations
                    .get(relation, key, reversed)
                    .map_err(error_code)?;
                return match found {
                    Some(partner) => {
                        self.kind = 3;
                        Ok(partner.handle())
                    }
                    None => {
                        self.kind = 0;
                        Ok(0)
                    }
                };
            }
            if op == 136 {
                let root = self
                    .relations
                    .outermost(relation, key, reversed)
                    .map_err(error_code)?;
                self.kind = 3;
                return Ok(root.handle());
            }
            let partners = match op {
                134 if every_label => {
                    // One table per label, read in label order. A partner reached
                    // by two labels is listed once: this answers which entities the
                    // family relates, not how many ways it does so.
                    let mut found: Vec<ObjectRef> = Vec::new();
                    let mut failure = None;
                    for (_, table) in self.relations.tables_of(declared) {
                        match self.relations.all(table, key, reversed) {
                            Ok(partners) => {
                                for partner in partners {
                                    if found.contains(partner) {
                                        continue;
                                    }
                                    if found.try_reserve(1).is_err() {
                                        failure = Some(Error::Allocation);
                                        break;
                                    }
                                    found.push(*partner);
                                }
                            }
                            Err(error) => failure = Some(error),
                        }
                        if failure.is_some() {
                            break;
                        }
                    }
                    match failure {
                        Some(error) => Err(error),
                        None => Ok(found),
                    }
                }
                134 => self
                    .relations
                    .all(relation, key, reversed)
                    .map(<[_]>::to_vec),
                137 => self.relations.descendants(relation, key, reversed),
                _ => self.relations.ancestors(relation, key, reversed),
            }
            .map_err(error_code)?;
            let mut values = Vec::new();
            values.try_reserve(partners.len()).map_err(|_| 6u32)?;
            values.extend(partners.into_iter().map(Value::Reference));
            let list = store.objects().new_list(&values).map_err(error_code)?;
            self.kind = 9;
            return Ok(list.handle());
        }
        if op == 127 {
            // own a copy of text for an accumulator's slot. `a` is the
            // value kind (4 literal, 7 text) and `b` its payload. The copy is
            // registered with the scope that declares the local.
            if c != 0 {
                return Err(11);
            }
            let text: String = self.characters(a, b)?.into_iter().collect();
            return self.own_text(&text);
        }
        if op == 128 {
            // an accumulator's slot keeps its owner entry. `a` is the
            // value the slot held and `b` the value replacing it: the new text
            // takes over the old entry, wherever that scope is, and the old text
            // is released. The new text's own entry, made where it was built, is
            // given up.
            if c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            let old = store.native_reference(a);
            let new = store.native_reference(b);
            if old == new {
                self.kind = 7;
                return Ok(b);
            }
            let slot = self
                .local_owners
                .iter()
                .rposition(|owner| *owner == Some(old))
                .ok_or(3u32)?;
            let built = self
                .local_owners
                .iter()
                .rposition(|owner| *owner == Some(new))
                .ok_or(3u32)?;
            self.local_owners[built] = None;
            self.local_owners[slot] = Some(new);
            store.objects().destroy(old).map_err(error_code)?;
            self.kind = 7;
            return Ok(b);
        }
        if op == 126 {
            // Hand the gathered output to the host, which owns the console
            //.
            if a != 0 || b != 0 || c != 0 {
                return Err(11);
            }
            let bytes = self.output.take();
            crate::session::publish(self.slot, &bytes)?;
            self.kind = 0;
            return Ok(0);
        }
        if op == 118 || op == 125 {
            // inputKey/inputLine: the game suspends and the host supplies
            // the input on its own schedule.
            // The output gathered so far goes with the request, so a prompt
            // reaches the player before the game waits for an answer.
            if a != 0 || b != 0 || c != 0 {
                return Err(11);
            }
            let bytes = self.output.take();
            let request = if op == 118 {
                crate::session::Request::Key
            } else {
                crate::session::Request::Line
            };
            let reply = crate::session::suspend(self.slot, request, &bytes)?;
            let reply = match reply {
                crate::session::Reply::Line(text) => text,
                _ => None,
            };
            // A key is one character of the reply, or the portable name of a
            // keystroke that is not a character at all ; the reference
            // reads one keystroke without waiting for a newline.
            let key = match (op, reply) {
                (118, Some(text)) => Some(key_from_reply(&text)),
                // inputKey reports the end of input as the reference's key name.
                (118, None) => Some("[eof]".to_owned()),
                (_, reply) => reply,
            };
            let Some(key) = key else {
                // inputLine reports the end of input as nil.
                self.kind = 0;
                return Ok(0);
            };
            let mut objects = store.objects();
            let result = objects.new_text(&key).map_err(error_code)?;
            match self.init_regions.last().copied() {
                Some(region) => objects.adopt(region, result).map_err(error_code)?,
                None => {
                    if self.checkpoints.is_empty() {
                        let _ = objects.destroy(result);
                        return Err(11);
                    }
                    if self.local_owners.try_reserve(1).is_err() {
                        let _ = objects.destroy(result);
                        return Err(6);
                    }
                    // A turn record needs no scope entry.
                    if !self.lifetimes {
                        self.local_owners.push(Some(result));
                    }
                }
            }
            self.kind = 7;
            return Ok(result.handle());
        }
        if op == 117 {
            // getTime(GetTimeTicks): milliseconds since the first such call,
            // which returns zero, wrapping like the reference's 32-bit counter.
            // The calendar mode is not implemented.
            if b != 0 || c != 0 {
                return Err(11);
            }
            if a != 2 {
                return Err(17);
            }
            let now = std::time::Instant::now();
            let origin = *self.tick_origin.get_or_insert(now);
            let elapsed = now.saturating_duration_since(origin).as_millis();
            self.kind = 2;
            return Ok(u64::from((elapsed as u32) & 0x7fff_ffff));
        }
        if op == 110 {
            // Content equality of two list values; a and b are list handles.
            if c != 0 {
                return Err(11);
            }
            let (left, right) = (store.native_reference(a), store.native_reference(b));
            let equal = store
                .objects()
                .list_equal(left, right)
                .map_err(error_code)?;
            self.kind = u32::from(equal);
            return Ok(u64::from(equal));
        }
        if op == 111 {
            // Borrow a module constant list by compiler plan index.
            if b != 0 || c != 0 {
                return Err(11);
            }
            let index = usize::try_from(a).map_err(|_| 11u32)?;
            let list = *self.constant_lists.get(index).ok_or(11u32)?;
            store.objects().list(list).map_err(error_code)?;
            self.kind = 9;
            return Ok(list.handle());
        }
        if op == 92 || op == 95 {
            if op == 95 && c != 0 {
                return Err(11);
            }
            let owner = store.native_reference(a);
            let property = PropertyId(u32::try_from(b).map_err(|_| 11u32)?);
            let capacity = u32::try_from(c).map_err(|_| 11u32)? as i32;
            let region = self.init_regions.last().copied().ok_or(11u32)?;
            let mut objects = store.objects();
            objects.validate_object(owner).map_err(error_code)?;
            objects.validate_object(region).map_err(error_code)?;
            let value = if op == 95 {
                objects.new_lookup()
            } else {
                objects.new_vector(capacity)
            }
            .map_err(error_code)?;
            objects.adopt(region, value).map_err(error_code)?;
            if op == 95 {
                objects.assign_lookup(owner, property, value, region)
            } else {
                objects.assign_vector(owner, property, value, region)
            }
            .map_err(error_code)?;
            self.kind = 3;
            return Ok(value.handle());
        }
        if op == 90 || op == 91 {
            let object = store.native_reference(a);
            if c != 0 || (op == 90 && b > 1) || (op == 91 && b != 0) {
                return Err(11);
            }
            if op == 90 {
                store
                    .objects()
                    .set_transient(object, b == 1)
                    .map_err(error_code)?;
                return Ok(0);
            }
            let flag = store.objects().is_transient(object).map_err(error_code)?;
            self.kind = u32::from(flag);
            return Ok(0);
        }
        if (87..=89).contains(&op) {
            store.ensure_active().map_err(error_code)?;
            let buffer = store.native_reference(a);
            if op == 87 {
                let value = lookup_value(store, b, c)?;
                let result = store
                    .objects()
                    .string_buffer_append_value(buffer, value)
                    .map_err(error_code)?;
                self.kind = 3;
                return Ok(result.handle());
            }
            if b != 0 || c != 0 {
                return Err(11);
            }
            if op == 89 {
                let length = store
                    .objects()
                    .string_buffer(buffer)
                    .map_err(error_code)?
                    .chars()
                    .count();
                self.kind = 2;
                return i32::try_from(length).map(|n| n as u64).map_err(|_| 18);
            }
            if self.checkpoints.is_empty() {
                return Err(11);
            }
            if self.local_owners.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let snapshot = store
                .objects()
                .string_buffer_snapshot(buffer)
                .map_err(error_code)?;
            // A turn record needs no scope entry.
            if !self.lifetimes {
                self.local_owners.push(Some(snapshot));
            }
            self.kind = 7;
            return Ok(snapshot.handle());
        }
        if (67..=72).contains(&op) {
            let vector = store.native_reference(a);
            // Appending uses b=tag, c=payload; indexed replacement packs its
            // positive index in b's high word and the ordinary value tag below.
            let value = if op == 69 || op == 72 {
                Some(match b & 0xffff_ffff {
                    0 if c == 0 => Value::Nil,
                    1 if c == 0 => Value::Bool(true),
                    2 => Value::Int(u32::try_from(c).map_err(|_| 11u32)? as i32),
                    3 => Value::Reference(store.native_reference(c)),
                    4 => Value::String(
                        store
                            .literal(u32::try_from(c).map_err(|_| 11u32)?)
                            .map_err(error_code)?,
                    ),
                    7 => Value::Text(store.native_reference(c)),
                    8 => Value::Property(PropertyId(u32::try_from(c).map_err(|_| 11u32)?)),
                    9 => Value::List(store.native_reference(c)),
                    10 => Value::Enumerator(u32::try_from(c).map_err(|_| 11u32)?),
                    11 => Value::Function(u32::try_from(c).map_err(|_| 11u32)?),
                    _ => return Err(16),
                })
            } else {
                None
            };
            let signed = |n| u32::try_from(n).map(|n| n as i32).map_err(|_| 11u32);
            let mut objects = store.objects();
            match op {
                67 => {
                    if b != 0 || c != 0 {
                        return Err(11);
                    }
                    let length = objects.vector(vector).map_err(error_code)?.len();
                    self.kind = 2;
                    return i32::try_from(length).map(|n| n as u64).map_err(|_| 18);
                }
                68 => {
                    if c != 0 {
                        return Err(11);
                    }
                    let value = objects.vector_get(vector, signed(b)?).map_err(error_code)?;
                    let (kind, payload) = match value {
                        Value::Nil => (0, 0),
                        Value::Bool(value) => (u32::from(value), 0),
                        Value::Int(value) => (2, u64::from(value as u32)),
                        Value::Reference(value) => (3, value.handle()),
                        Value::String(value) => (4, u64::from(value.index())),
                        Value::Text(value) => (7, value.handle()),
                        Value::Property(value) => (8, u64::from(value.0)),
                        Value::List(value) => (9, value.handle()),
                        Value::Enumerator(value) => (10, u64::from(value)),
                        Value::Function(value) => (11, u64::from(value)),
                        _ => return Err(16),
                    };
                    self.kind = kind;
                    return Ok(payload);
                }
                69 => {
                    if b >> 32 != 0 {
                        return Err(11);
                    }
                    objects
                        .append_value(vector, value.ok_or(11u32)?)
                        .map_err(error_code)?;
                }
                70 => {
                    if c != 0 {
                        return Err(11);
                    }
                    objects
                        .vector_set_length(vector, signed(b)?)
                        .map_err(error_code)?;
                }
                71 => {
                    objects
                        .vector_remove_range(vector, signed(b)?, signed(c)?)
                        .map_err(error_code)?;
                }
                72 => {
                    objects
                        .vector_set(vector, signed(b >> 32)?, value.ok_or(11u32)?)
                        .map_err(error_code)?;
                }
                _ => return Err(11),
            }
            self.kind = 3;
            return Ok(a);
        }
        if op == 63 {
            let Some(id) = self.next_checkpoint.checked_add(1) else {
                return store
                    .terminate(Error::IdentityExhausted)
                    .map(|()| 0)
                    .map_err(error_code);
            };
            if self.checkpoints.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            self.checkpoints.push(Checkpoint {
                id,
                iterations: self.iterations.len(),
                constructions: self.constructions.len(),
                members: self.member_calls.len(),
                initializers: self.init_regions.len(),
                locals: self.local_owners.len(),
                parses: self.parses.len(),
                sorts: self.sorts.len(),
            });
            self.next_checkpoint = id;
            return Ok(id);
        }
        if op == 64 || op == 65 {
            let index = if op == 64 {
                self.checkpoints
                    .len()
                    .checked_sub(1)
                    .filter(|index| self.checkpoints[*index].id == a)
            } else {
                self.checkpoints.iter().position(|mark| mark.id == a)
            }
            .ok_or(11u32)?;
            let mark = self.checkpoints[index];
            if self.iterations.len() < mark.iterations
                || self.constructions.len() < mark.constructions
                || self.member_calls.len() < mark.members
                || self.init_regions.len() < mark.initializers
                || self.local_owners.len() < mark.locals
                || self.parses.len() < mark.parses
                || self.sorts.len() < mark.sorts
            {
                return Err(11);
            }
            // A comparator threw: abandon sorts begun inside this scope.
            self.sorts.truncate(mark.sorts);
            loop {
                // Nested ordinary method calls have no additional owner to release.
                while self.member_calls.len() > mark.members
                    && self
                        .member_calls
                        .last()
                        .is_some_and(|(_, call)| call.is_none())
                {
                    self.member_calls.pop();
                }
                while self.local_owners.len() > mark.locals
                    && self.local_owners.last() == Some(&None)
                {
                    self.local_owners.pop();
                }
                let mut latest = None;
                let mut choose = |kind, identity| {
                    if let Some(identity) = identity
                        && latest.is_none_or(|(_, old)| identity > old)
                    {
                        latest = Some((kind, identity));
                    }
                };
                if self.iterations.len() > mark.iterations {
                    choose(
                        0,
                        self.iterations
                            .last()
                            .and_then(|(cursor, _)| cursor.cleanup_identity()),
                    );
                }
                if self.constructions.len() > mark.constructions {
                    choose(
                        1,
                        self.constructions
                            .last()
                            .and_then(Activation::reference)
                            .map(|value| value.handle()),
                    );
                }
                if self.member_calls.len() > mark.members {
                    choose(
                        2,
                        self.member_calls
                            .last()
                            .and_then(|(_, call)| call.as_ref())
                            .and_then(MemberCall::cleanup_identity),
                    );
                }
                if self.init_regions.len() > mark.initializers {
                    choose(3, self.init_regions.last().map(|value| value.handle()));
                }
                if self.local_owners.len() > mark.locals {
                    choose(
                        4,
                        self.local_owners
                            .last()
                            .and_then(|value| value.map(|value| value.handle())),
                    );
                }
                if self.parses.len() > mark.parses {
                    // A constructor threw before finish: release the whole parse.
                    choose(
                        5,
                        self.parses.last().map(|pending| pending.storage.handle()),
                    );
                }
                let Some((kind, _)) = latest else {
                    break;
                };
                let result = match kind {
                    0 => {
                        let (mut cursor, _) = self.iterations.pop().ok_or(11u32)?;
                        store.objects().end_iteration(&mut cursor)
                    }
                    1 => match self.constructions.pop().ok_or(11u32)? {
                        Activation::Owned(mut construction) => {
                            store.objects().abort_construction(&mut construction)
                        }
                        Activation::Published(mut construction) => store
                            .objects()
                            .finish_published_construction(&mut construction)
                            .map(|_| ()),
                    },
                    2 => {
                        let (_, mut call) = self.member_calls.pop().ok_or(11u32)?;
                        store
                            .objects()
                            .finish_member_call(call.as_mut().ok_or(11u32)?)
                            .map(|_| ())
                    }
                    3 => store
                        .objects()
                        .destroy(self.init_regions.pop().ok_or(11u32)?),
                    5 => store
                        .objects()
                        .destroy(self.parses.pop().ok_or(11u32)?.storage),
                    _ => store
                        .objects()
                        .destroy(self.local_owners.pop().flatten().ok_or(11u32)?),
                };
                if let Err(error) = result {
                    let _ = store.terminate(error);
                    // The terminal store has reclaimed every root; discard
                    // only this frame's now-invalid bookkeeping, never caller marks.
                    self.iterations.truncate(mark.iterations);
                    self.constructions.truncate(mark.constructions);
                    self.member_calls.truncate(mark.members);
                    self.init_regions.truncate(mark.initializers);
                    self.local_owners.truncate(mark.locals);
                    self.parses.truncate(mark.parses);
                    self.checkpoints.truncate(index);
                    return Err(error_code(error));
                }
            }
            if let Some(owner) = self
                .exceptions
                .iter()
                .position(|(_, scope)| *scope == Some(mark.id))
            {
                if let Err(error) = store
                    .objects()
                    .release_exception(&mut self.exceptions[owner].0)
                {
                    let _ = store.terminate(error);
                    self.checkpoints.truncate(index);
                    return Err(error_code(error));
                }
                self.exceptions.swap_remove(owner);
            }
            self.checkpoints.truncate(index);
            return Ok(0);
        }
        if op == 62 {
            let object = store.native_reference(a);
            let ancestor = store.native_reference(b);
            return store
                .objects()
                .of_kind(object, ancestor)
                .map(u64::from)
                .map_err(error_code);
        }
        if op == 61 {
            let object = store.native_reference(a);
            return store
                .objects()
                .validate_object(object)
                .map(|()| 0)
                .map_err(error_code);
        }
        if op == 57 {
            let source = match a {
                3 => Value::Reference(store.native_reference(b)),
                9 => Value::List(store.native_reference(b)),
                _ => return Err(16),
            };
            if self.iterations.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let cursor = store
                .objects()
                .begin_iteration(source)
                .map_err(error_code)?;
            self.iterations.push((cursor, None));
            return Ok(0);
        }
        if op == 58 {
            let (cursor, current) = self.iterations.last_mut().ok_or(11u32)?;
            *current = store.objects().iteration_next(cursor).map_err(error_code)?;
            return Ok(u64::from(current.is_some()));
        }
        if op == 59 {
            let value = self
                .iterations
                .last()
                .and_then(|(_, value)| *value)
                .ok_or(11u32)?;
            let (kind, payload) = match value {
                Value::Nil => (0, 0),
                Value::Bool(value) => (u32::from(value), 0),
                Value::Int(value) => (2, u64::from(value as u32)),
                Value::Reference(value) => (3, value.handle()),
                Value::String(value) => (4, u64::from(value.index())),
                Value::Text(value) => (7, value.handle()),
                Value::Property(value) => (8, u64::from(value.0)),
                Value::List(value) => (9, value.handle()),
                Value::Enumerator(value) => (10, u64::from(value)),
                Value::Function(value) => (11, u64::from(value)),
                _ => return Err(16),
            };
            self.kind = kind;
            return Ok(payload);
        }
        if op == 60 {
            let (mut cursor, _) = self.iterations.pop().ok_or(11u32)?;
            store
                .objects()
                .end_iteration(&mut cursor)
                .map_err(error_code)?;
            return Ok(0);
        }
        if op == 55 {
            if self.member_calls.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let object = store.native_reference(a);
            let call = store
                .objects()
                .begin_member_call(object)
                .map_err(error_code)?;
            self.member_calls.push((object, call));
            return Ok(0);
        }
        if op == 56 {
            let object = store.native_reference(a);
            if self.member_calls.last().map(|(member, _)| *member) != Some(object) {
                return Err(11);
            }
            let (_, call) = self.member_calls.pop().ok_or(11u32)?;
            if let Some(mut call) = call {
                store
                    .objects()
                    .finish_member_call(&mut call)
                    .map_err(error_code)?;
            }
            return Ok(0);
        }
        if op == 43 || op == 46 {
            if self.constructions.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let prototype = store.native_reference(a);
            let construction = if op == 43 {
                Activation::Owned(
                    store
                        .objects()
                        .begin_construction(prototype)
                        .map_err(error_code)?,
                )
            } else {
                Activation::Published(
                    store
                        .objects()
                        .begin_published_construction(prototype)
                        .map_err(error_code)?,
                )
            };
            let object = construction.reference().ok_or(11u32)?;
            self.constructions.push(construction);
            self.kind = 3;
            return Ok(object.handle());
        }
        if op == 44 || op == 45 {
            let object = store.native_reference(a);
            if self.constructions.last().and_then(Activation::reference) != Some(object) {
                return Err(11);
            }
            // Validate the wire property before consuming any activation.
            let property = PropertyId(u32::try_from(c).map_err(|_| 11u32)?);
            if !matches!(self.constructions.last(), Some(Activation::Owned(_))) {
                return Err(11);
            }
            let Some(Activation::Owned(mut construction)) = self.constructions.pop() else {
                return Err(11);
            };
            if op == 45 {
                return store
                    .objects()
                    .abort_construction(&mut construction)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let destination = store.native_reference(b);
            let object = store
                .objects()
                .finish_construction(&mut construction, destination, property)
                .map_err(error_code)?;
            self.kind = 3;
            return Ok(object.handle());
        }
        if op == 47 || op == 51 {
            let object = store.native_reference(a);
            let recipient = store.native_reference(b);
            let property = PropertyId(u32::try_from(c).map_err(|_| 11u32)?);
            if op == 51
                && let Some(call) = self
                    .member_calls
                    .iter_mut()
                    .rev()
                    .filter_map(|(_, call)| call.as_mut())
                    .find(|call| call.reference() == Some(object))
            {
                return store
                    .objects()
                    .register_executing_member(recipient, call)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let Some(Activation::Published(construction)) = self
                .constructions
                .iter_mut()
                .rev()
                .find(|value| value.reference() == Some(object))
            else {
                return Err(11);
            };
            return if op == 51 {
                store
                    .objects()
                    .register_owned_construction(recipient, construction)
            } else {
                store
                    .objects()
                    .reserve_construction(construction, recipient, property)
            }
            .map(|()| 0)
            .map_err(error_code);
        }
        if op == 50 {
            let region = *self.init_regions.last().ok_or(11u32)?;
            let mut objects = store.objects();
            let collection = objects.new_owned_collection().map_err(error_code)?;
            objects.adopt(region, collection).map_err(error_code)?;
            self.kind = 3;
            return Ok(collection.handle());
        }
        if (52..=54).contains(&op) {
            let collection = store.native_reference(a);
            let object = store.native_reference(b);
            let mut objects = store.objects();
            return match op {
                52 => objects
                    .remove_owned_member(collection, object)
                    .map(u64::from)
                    .map_err(error_code),
                53 => objects
                    .owned_collection_len(collection)
                    .map_err(error_code)
                    .and_then(|len| i32::try_from(len).map(|len| len as u64).map_err(|_| 18u32)),
                _ => objects
                    .owned_collection_get(collection, u32::try_from(b).map_err(|_| 11u32)? as i32)
                    .map(|object| object.handle())
                    .map_err(error_code),
            };
        }
        if op == 48 {
            let object = store.native_reference(a);
            if self.constructions.last().and_then(Activation::reference) != Some(object)
                || !matches!(self.constructions.last(), Some(Activation::Published(_)))
            {
                return Err(11);
            }
            let Some(Activation::Published(mut construction)) = self.constructions.pop() else {
                return Err(11);
            };
            let result = store
                .objects()
                .finish_published_construction(&mut construction)
                .map_err(error_code)?;
            self.kind = 3;
            return Ok(result.handle());
        }
        if op == 10 {
            let slot = u32::try_from(a).map_err(|_| 11u32)?;
            let object = store.native_reference(b);
            store.bind_static(slot, object).map_err(error_code)?;
            return Ok(0);
        }
        if op == 11 {
            let slot = u32::try_from(a).map_err(|_| 11u32)?;
            let object = store.static_object(slot).map_err(error_code)?;
            self.kind = 3;
            return Ok(object.handle());
        }
        if op == 21 {
            let index = u32::try_from(a).map_err(|_| 11u32)?;
            let literal = store.literal(index).map_err(error_code)?;
            // Inside a static initializer the region owns its own copy; outside
            // one, the same literal shares a single object, so reading it in a
            // loop allocates nothing after the first turn.
            if let Some(region) = self.init_regions.last().copied() {
                let object = store
                    .objects()
                    .new_text(crate::tables::installed().literals[literal.index() as usize])
                    .map_err(error_code)?;
                store.objects().adopt(region, object).map_err(error_code)?;
                self.kind = 7;
                return Ok(object.handle());
            }
            if let Some((_, object)) = self.literal_texts.iter().find(|(i, _)| *i == index) {
                let object = *object;
                // A closed and reopened store leaves stale entries behind.
                if store.objects().text(object).is_ok() {
                    self.kind = 7;
                    return Ok(object.handle());
                }
                self.literal_texts.retain(|(i, _)| *i != index);
            }
            let object = store
                .objects()
                .new_text(crate::tables::installed().literals[literal.index() as usize])
                .map_err(error_code)?;
            // The text a literal stands for is module data, so it outlives any
            // turn that happens to read it first.
            store
                .objects()
                .set_lifetime(object, crate::objects::Lifetime::World)
                .map_err(error_code)?;
            if self.literal_texts.try_reserve(1).is_err() {
                let _ = store.objects().destroy(object);
                return Err(6);
            }
            self.literal_texts.push((index, object));
            self.kind = 7;
            return Ok(object.handle());
        }
        if op == 32 {
            if c > 3 {
                return Err(11);
            }
            if c & 1 != 0 {
                store
                    .literal(u32::try_from(a).map_err(|_| 11u32)?)
                    .map_err(error_code)?;
            }
            if c & 2 != 0 {
                store
                    .literal(u32::try_from(b).map_err(|_| 11u32)?)
                    .map_err(error_code)?;
            }
            let left = store.native_reference(a);
            let right = store.native_reference(b);
            let objects = store.objects();
            let text = |value, reference, literal| -> Result<&str, u32> {
                if literal {
                    crate::tables::installed()
                        .literals
                        .get(usize::try_from(value).map_err(|_| 11u32)?)
                        .copied()
                        .ok_or(11u32)
                } else {
                    objects.text(reference).map_err(error_code)
                }
            };
            self.kind = 1;
            return Ok(u64::from(
                text(a, left, c & 1 != 0)? == text(b, right, c & 2 != 0)?,
            ));
        }
        if op == 31 {
            let object = store.native_reference(a);
            store.objects().text(object).map_err(error_code)?;
            self.kind = 7;
            return Ok(a);
        }
        if op == 20 {
            let object = store.native_reference(a);
            let property = PropertyId(u32::try_from(b).map_err(|_| 11u32)?);
            store.objects().get(object, property).map_err(error_code)?;
            if self.init_regions.try_reserve(1).is_err() {
                return store
                    .terminate(Error::Allocation)
                    .map(|()| 0)
                    .map_err(error_code);
            }
            let region = store.objects().create().map_err(error_code)?;
            store
                .objects()
                .set(object, property, Value::Initializing)
                .map_err(error_code)?;
            self.init_regions.push(region);
            return Ok(0);
        }
        if op == 29 || op == 41 {
            let object = store.native_reference(a);
            let property = PropertyId(u32::try_from(b).map_err(|_| 11u32)?);
            let value = store.native_reference(c);
            let Some(region) = self.init_regions.last().copied() else {
                if op == 41 {
                    store.objects().list(value).map_err(error_code)?;
                } else {
                    store.objects().text(value).map_err(error_code)?;
                }
                return store
                    .objects()
                    .set(
                        object,
                        property,
                        if op == 41 {
                            Value::List(value)
                        } else {
                            Value::Text(value)
                        },
                    )
                    .map(|()| 0)
                    .map_err(error_code);
            };
            return if op == 41 {
                store.objects().assign_list(object, property, value, region)
            } else {
                store.objects().assign_text(object, property, value, region)
            }
            .map(|()| 0)
            .map_err(error_code);
        }
        if (36..=40).contains(&op) || op == 74 || op == 94 {
            store.ensure_active().map_err(error_code)?;
            let object = store.native_reference(a);
            let reference = store.native_reference(c);
            let literal = if (op == 37 || op == 74 || op == 94) && b == 4 {
                Some(
                    store
                        .literal(u32::try_from(c).map_err(|_| 11u32)?)
                        .map_err(error_code)?,
                )
            } else {
                None
            };
            let mut objects = store.objects();
            match op {
                36 => {
                    let list = objects
                        .begin_list(usize::try_from(a).map_err(|_| 11u32)?)
                        .map_err(error_code)?;
                    match self.init_regions.last().copied() {
                        Some(region) => objects.adopt(region, list).map_err(error_code)?,
                        None => {
                            // Outside a static initializer the list belongs to the
                            // calling scope, like any other allocation.
                            // With lifetimes it needs no scope at all.
                            if self.lifetimes {
                                return Ok(list.handle());
                            }
                            if self.checkpoints.is_empty() {
                                let _ = objects.destroy(list);
                                return Err(11);
                            }
                            if self.local_owners.try_reserve(1).is_err() {
                                let _ = objects.destroy(list);
                                return Err(6);
                            }
                            // A turn record needs no scope entry.
                            if !self.lifetimes {
                                self.local_owners.push(Some(list));
                            }
                        }
                    }
                    return Ok(list.handle());
                }
                37 | 74 | 94 => {
                    let value = match b {
                        0 if c == 0 => Value::Nil,
                        1 if c == 0 => Value::Bool(true),
                        2 => Value::Int(u32::try_from(c).map_err(|_| 11u32)? as i32),
                        3 => Value::Reference(reference),
                        4 => Value::String(literal.ok_or(11u32)?),
                        7 => Value::Text(reference),
                        11 => Value::Function(u32::try_from(c).map_err(|_| 11u32)?),
                        10 => Value::Enumerator(u32::try_from(c).map_err(|_| 11u32)?),
                        8 => Value::Property(PropertyId(u32::try_from(c).map_err(|_| 11u32)?)),
                        9 => Value::List(reference),
                        _ => return Err(16),
                    };
                    if op == 94 {
                        let region = self.constant_region.ok_or(11u32)?;
                        return objects
                            .push_constant_list(object, value, region)
                            .map(|()| 0)
                            .map_err(error_code);
                    }
                    if op == 74 {
                        if !self.local_owners.contains(&Some(object)) {
                            return Err(11);
                        }
                        return objects
                            .push_borrowed_list(object, value)
                            .map(|()| 0)
                            .map_err(error_code);
                    }
                    let Some(region) = self.init_regions.last().copied() else {
                        // Outside a static initializer the builder belongs to the
                        // calling scope. An element this scope owns moves
                        // into the builder; anything else is borrowed as usual.
                        // With lifetimes the builder is an ordinary turn record
                        // and its elements are borrowed.
                        if self.lifetimes {
                            return objects
                                .push_borrowed_list(object, value)
                                .map(|()| 0)
                                .map_err(error_code);
                        }
                        if !self.local_owners.contains(&Some(object)) {
                            return Err(11);
                        }
                        let child = match value {
                            Value::Text(v) | Value::List(v) => Some(v),
                            _ => None,
                        };
                        if let Some(child) = child
                            && let Some(index) = self
                                .local_owners
                                .iter()
                                .rposition(|owner| *owner == Some(child))
                        {
                            objects.adopt(object, child).map_err(error_code)?;
                            self.local_owners[index] = None;
                        }
                        return objects
                            .push_borrowed_list(object, value)
                            .map(|()| 0)
                            .map_err(error_code);
                    };
                    return objects
                        .push_list(object, value, region)
                        .map(|()| 0)
                        .map_err(error_code);
                }
                38 => {
                    objects.finish_list(object).map_err(error_code)?;
                    self.kind = 9;
                    return Ok(a);
                }
                39 => {
                    let length = match b {
                        4 => crate::tables::installed()
                            .literals
                            .get(usize::try_from(a).map_err(|_| 11u32)?)
                            .ok_or(2u32)?
                            .chars()
                            .count(),
                        7 => objects.text(object).map_err(error_code)?.chars().count(),
                        9 => objects.list(object).map_err(error_code)?.len(),
                        3 => objects.object_length(object).map_err(error_code)?,
                        _ => return Err(16),
                    };
                    self.kind = 2;
                    return i32::try_from(length).map(|n| n as u64).map_err(|_| 18u32);
                }
                _ => {
                    let value = objects
                        .list_get(object, u32::try_from(b).map_err(|_| 11u32)? as i32)
                        .map_err(error_code)?;
                    let (kind, payload) = match value {
                        Value::Nil | Value::Bool(false) => (0, 0),
                        Value::Bool(true) => (1, 0),
                        Value::Int(v) => (2, u64::from(v as u32)),
                        Value::Reference(v) => (3, v.handle()),
                        Value::String(v) => (4, u64::from(v.index())),
                        Value::Text(v) => (7, v.handle()),
                        Value::Function(v) => (11, u64::from(v)),
                        Value::Enumerator(v) => (10, u64::from(v)),
                        Value::Property(v) => (8, u64::from(v.0)),
                        Value::List(v) => (9, v.handle()),
                        _ => return Err(16),
                    };
                    self.kind = kind;
                    return Ok(payload);
                }
            }
        }
        if op == 30 {
            let region = self.init_regions.pop().ok_or(11u32)?;
            return store
                .objects()
                .destroy(region)
                .map(|()| 0)
                .map_err(error_code);
        }
        if op == 18 {
            let object = store.native_reference(a);
            return store.mark_class(object).map(|()| 0).map_err(error_code);
        }
        if op == 19 {
            let property = PropertyId(u32::try_from(a).map_err(|_| 11u32)?);
            return store
                .register_class_property(property)
                .map(|()| 0)
                .map_err(error_code);
        }
        if op == 16 {
            let object = store.native_reference(a);
            let prototype = store.native_reference(b);
            return store
                .objects()
                .add_prototype(object, prototype)
                .map(|()| 0)
                .map_err(error_code);
        }
        if op == 15 {
            let object = store.native_reference(a);
            let property = PropertyId(u32::try_from(b).map_err(|_| 11u32)?);
            let method = u32::try_from(c).map_err(|_| 11u32)?;
            return store
                .objects()
                .set(object, property, Value::Method(method))
                .map(|()| 0)
                .map_err(error_code);
        }
        if op == 33 {
            store.ensure_active().map_err(error_code)?;
            if c != 0 {
                return Err(11);
            }
            let result = match a {
                0 if b == 0 => Ok(()),
                1 => return Err(16),
                2 => {
                    let value = u32::try_from(b).map_err(|_| 11u32)? as i32;
                    let mut buffer = [0u8; 11];
                    let mut index = buffer.len();
                    let mut magnitude = value.unsigned_abs();
                    loop {
                        index -= 1;
                        buffer[index] = b'0' + (magnitude % 10) as u8;
                        magnitude /= 10;
                        if magnitude == 0 {
                            break;
                        }
                    }
                    if value < 0 {
                        index -= 1;
                        buffer[index] = b'-';
                    }
                    self.output
                        .append(std::str::from_utf8(&buffer[index..]).expect("decimal ASCII"))
                }
                4 => {
                    let literal = store
                        .literal(u32::try_from(b).map_err(|_| 11u32)?)
                        .map_err(error_code)?;
                    self.output
                        .append(crate::tables::installed().literals[literal.index() as usize])
                }
                7 => {
                    let value = store.native_reference(b);
                    self.output
                        .append(store.objects().text(value).map_err(error_code)?)
                }
                _ => return Err(16),
            };
            if let Err(error) = result {
                return store.terminate(error).map(|()| 0).map_err(error_code);
            }
            return Ok(0);
        }
        // a semantic event. 146 starts one with an id, 147 names an
        // entity it concerns. Two operations rather than one because the call
        // boundary has three payload slots and an id costs two of them — and
        // because it is the same shape the host assembles an intent with
        // (`reply_action` then `reply_subject`), read the other way round.
        if op == 146 {
            store.ensure_active().map_err(error_code)?;
            if c != 0 {
                return Err(11);
            }
            // Tag 4 is a string literal and 7 a text object, the same two
            // shapes op 33 prints. An id is text; anything else is a mistake
            // worth reporting rather than rendering.
            match a {
                4 => {
                    let literal = store
                        .literal(u32::try_from(b).map_err(|_| 11u32)?)
                        .map_err(error_code)?;
                    crate::session::event(
                        self.slot,
                        crate::tables::installed().literals[literal.index() as usize].as_bytes(),
                    )?;
                }
                7 => {
                    let value = store.native_reference(b);
                    crate::session::event(
                        self.slot,
                        store.objects().text(value).map_err(error_code)?.as_bytes(),
                    )?;
                }
                _ => return Err(16),
            }
            return Ok(0);
        }
        if op == 152 {
            if c != 0 {
                return Err(11);
            }
            store.ensure_active().map_err(error_code)?;
            match a {
                0 | 1 if b == 0 => crate::session::event_value(self.slot, a, &[])?,
                2 => {
                    let n = u32::try_from(b).map_err(|_| 18u32)? as i32;
                    crate::session::event_value(self.slot, 2, &(i64::from(n)).to_le_bytes())?;
                }
                3 => {
                    if store.objects().reference(b).is_none() {
                        return Err(3);
                    }
                    crate::session::event_value(self.slot, 3, &b.to_le_bytes())?;
                }
                4 => {
                    let literal = store
                        .literal(u32::try_from(b).map_err(|_| 11u32)?)
                        .map_err(error_code)?;
                    crate::session::event_value(
                        self.slot,
                        4,
                        crate::tables::installed().literals[literal.index() as usize].as_bytes(),
                    )?;
                }
                7 => {
                    let r = store.native_reference(b);
                    crate::session::event_value(
                        self.slot,
                        4,
                        store.objects().text(r).map_err(error_code)?.as_bytes(),
                    )?;
                }
                _ => return Err(16),
            }
            self.kind = 0;
            return Ok(0);
        }
        if op == 153 {
            if b != 0 || c != 0 {
                return Err(11);
            }
            let token = u32::try_from(a).map_err(|_| 18u32)?;
            if token > i32::MAX as u32 {
                return Err(18);
            }
            let result = crate::session::snapshot_token(self.slot, token)?;
            self.kind = if token == 0 { 2 } else { result };
            return Ok(if token == 0 { u64::from(result) } else { 0 });
        }
        if op == 147 {
            store.ensure_active().map_err(error_code)?;
            if b != 0 || c != 0 {
                return Err(11);
            }
            // An entity, named by the handle it crosses the boundary as
            // , so a host can match it against an inspection. A
            // handle that names nothing is refused: an event about an entity
            // that is not there would be worse than no event.
            let value = store.native_reference(a);
            if store.objects().reference(value.handle()).is_none() {
                return Err(3);
            }
            crate::session::event_subject(self.slot, value.handle())?;
            return Ok(0);
        }
        // the same formatting as op 33, answered as text instead of
        // written to the output. A value string's interpolation needs the text,
        // not the side effect. (34 was taken: it makes a dictionary.)
        if op == 145 {
            store.ensure_active().map_err(error_code)?;
            if c != 0 {
                return Err(11);
            }
            // The text is copied out before a new one is created, because
            // creating one borrows the store — the same shape dictionary_words
            // uses.
            let mut buffer = [0u8; 11];
            let mut owned = String::new();
            let rendered: &str = match a {
                0 if b == 0 => "nil",
                1 => {
                    if b == 0 {
                        "nil"
                    } else {
                        "true"
                    }
                }
                2 => {
                    let value = u32::try_from(b).map_err(|_| 11u32)? as i32;
                    let mut index = buffer.len();
                    let mut magnitude = value.unsigned_abs();
                    loop {
                        index -= 1;
                        buffer[index] = b'0' + (magnitude % 10) as u8;
                        magnitude /= 10;
                        if magnitude == 0 {
                            break;
                        }
                    }
                    if value < 0 {
                        index -= 1;
                        buffer[index] = b'-';
                    }
                    std::str::from_utf8(&buffer[index..]).expect("decimal ASCII")
                }
                4 => {
                    let literal = store
                        .literal(u32::try_from(b).map_err(|_| 11u32)?)
                        .map_err(error_code)?;
                    crate::tables::installed().literals[literal.index() as usize]
                }
                7 => {
                    let value = store.native_reference(b);
                    let scope = store.objects();
                    let text = scope.text(value).map_err(error_code)?;
                    owned.try_reserve(text.len()).map_err(|_| 9u32)?;
                    owned.push_str(text);
                    owned.as_str()
                }
                _ => return Err(16),
            };
            let text = store.objects().new_text(rendered).map_err(error_code)?;
            self.kind = 7;
            return Ok(text.handle());
        }
        if op == 14 {
            let literal = store
                .literal(u32::try_from(a).map_err(|_| 11u32)?)
                .map_err(error_code)?;
            let text = crate::tables::installed().literals[literal.index() as usize];
            if let Err(error) = self.output.append(text) {
                return store.terminate(error).map(|()| 0).map_err(error_code);
            }
            return Ok(0);
        }
        if op == 12 {
            let literal = store
                .literal(u32::try_from(a).map_err(|_| 11u32)?)
                .map_err(error_code)?;
            self.kind = 4;
            return Ok(u64::from(literal.index()));
        }
        if op == 13 {
            let object = store.native_reference(a);
            let property = PropertyId(u32::try_from(b).map_err(|_| 11u32)?);
            let literal = store
                .literal(u32::try_from(c).map_err(|_| 11u32)?)
                .map_err(error_code)?;
            return store
                .objects()
                .set(object, property, Value::String(literal))
                .map(|()| 0)
                .map_err(error_code);
        }
        let object = store.native_reference(a);
        let child = store.native_reference(b);
        let reference = store.native_reference(c);
        let property = if (4..=8).contains(&op) || matches!(op, 17 | 35 | 42 | 49) {
            PropertyId(u32::try_from(b).map_err(|_| 11u32)?)
        } else {
            PropertyId(0)
        };
        let definer = store.native_reference(c);
        let mut objects = store.objects();
        if op == 8
            && let Some(region) = self.init_regions.last().copied()
            && objects.owned_collection_len(reference).is_ok()
        {
            return objects
                .assign_owned_collection(object, property, reference, region)
                .map(|()| 0)
                .map_err(error_code);
        }
        match op {
            22 | 23 | 25 => {
                let result = match op {
                    22 => objects.concat_text(object, child),
                    23 => {
                        let start = u32::try_from(b).map_err(|_| 11u32)? as i32;
                        let length = if c == u64::MAX {
                            None
                        } else {
                            Some(u32::try_from(c).map_err(|_| 11u32)? as i32)
                        };
                        objects.substring_text(object, start, length)
                    }
                    _ => {
                        if c > 1 {
                            return Err(11);
                        }
                        objects.integer_text(
                            u32::try_from(a).map_err(|_| 11u32)? as i32,
                            u32::try_from(b).map_err(|_| 11u32)?,
                            c != 0,
                        )
                    }
                }
                .map_err(error_code)?;
                if let Some(region) = self.init_regions.last() {
                    objects.adopt(*region, result).map_err(error_code)?;
                } else if !self.lifetimes {
                    // Outside a static initializer the new text belongs to the
                    // calling scope, like any other local allocation.
                    if self.checkpoints.is_empty() {
                        let _ = objects.destroy(result);
                        return Err(11);
                    }
                    if self.local_owners.try_reserve(1).is_err() {
                        let _ = objects.destroy(result);
                        return Err(6);
                    }
                    // A turn record needs no scope entry.
                    if !self.lifetimes {
                        self.local_owners.push(Some(result));
                    }
                }
                self.kind = 7;
                Ok(result.handle())
            }
            24 => {
                let value = objects
                    .text_integer(object, u32::try_from(b).map_err(|_| 11u32)?)
                    .map_err(error_code)?;
                self.kind = 2;
                Ok(u64::from(value as u32))
            }
            26 => {
                let offset = usize::try_from(b).map_err(|_| 11u32)?;
                Ok(objects
                    .text(object)
                    .map_err(error_code)?
                    .as_bytes()
                    .get(offset)
                    .map_or(256, |byte| u64::from(*byte)))
            }
            27 => {
                let equal = objects.text_equal(object, child).map_err(error_code)?;
                self.kind = u32::from(equal);
                Ok(u64::from(equal))
            }
            28 => {
                let result = self
                    .output
                    .append(objects.text(object).map_err(error_code)?);
                if let Err(error) = result {
                    return store.terminate(error).map(|()| 0).map_err(error_code);
                }
                Ok(0)
            }
            1 | 34 => {
                let object = if op == 34 {
                    objects.new_dictionary()
                } else {
                    objects.create()
                }
                .map_err(error_code)?;
                self.kind = 3;
                Ok(object.handle())
            }
            2 => objects.destroy(object).map(|()| 0).map_err(error_code),
            3 => objects.adopt(object, child).map(|()| 0).map_err(error_code),
            4 | 17 => {
                let value = if op == 17 {
                    objects.get_inherited(object, property, definer)
                } else {
                    objects.get(object, property).map(Some)
                }
                .map_err(error_code)?;
                let Some(value) = value else {
                    self.kind = 6;
                    return Ok(0);
                };
                let (kind, payload) = match value {
                    Value::Nil | Value::Bool(false) => (0, 0),
                    Value::Bool(true) => (1, 1),
                    Value::Int(value) => (2, u64::from(value as u32)),
                    Value::Reference(value) => (3, value.handle()),
                    Value::String(value) => (4, u64::from(value.index())),
                    Value::Text(value) => (7, value.handle()),
                    Value::List(value) => (9, value.handle()),
                    Value::Method(value) => (5, u64::from(value)),
                    Value::Function(value) => (11, u64::from(value)),
                    Value::Enumerator(value) => (10, u64::from(value)),
                    Value::Property(value) => (8, u64::from(value.0)),
                    Value::Initializing => return Err(15),
                };
                self.kind = kind;
                Ok(payload)
            }
            49 => objects
                .set(
                    object,
                    property,
                    Value::Function(u32::try_from(c).map_err(|_| 11u32)?),
                )
                .map(|()| 0)
                .map_err(error_code),
            42 => objects
                .set(
                    object,
                    property,
                    Value::Enumerator(u32::try_from(c).map_err(|_| 11u32)?),
                )
                .map(|()| 0)
                .map_err(error_code),
            35 => objects
                .set(
                    object,
                    property,
                    Value::Property(PropertyId(u32::try_from(c).map_err(|_| 11u32)?)),
                )
                .map(|()| 0)
                .map_err(error_code),
            5..=8 => {
                let value = match op {
                    5 => Value::Nil,
                    6 => Value::Bool(true),
                    7 => Value::Int(u32::try_from(c).map_err(|_| 11u32)? as i32),
                    _ if c != 0 => Value::Reference(reference),
                    _ => return Err(11),
                };
                objects
                    .set(object, property, value)
                    .map(|()| 0)
                    .map_err(error_code)
            }
            _ => Err(11),
        }
    }
}

/// The host side of the command cycle. These run on the consumer's
/// own thread while the game runs on its session worker, so they touch only the
/// handoff in `session`, never the thread-local object store. Every one names
/// its session, so a process may run several. Each returns 0 on
/// success and a failure code otherwise.
pub extern "C" fn host_reset(slot: u64) -> u32 {
    crate::session::reset(slot).err().unwrap_or(0)
}

/// Forget a session's handoff once its worker has finished.
pub extern "C" fn host_discard(slot: u64) -> u32 {
    crate::session::discard(slot).err().unwrap_or(0)
}

/// Wait for the game to suspend or finish: 0 wants a line, 1 wants a key, 2 is
/// the end of the session, 3 wants the host to act, 4 asks the host a question.
/// Higher values are failures.
pub extern "C" fn host_poll(slot: u64) -> u32 {
    crate::session::poll(slot).unwrap_or_else(|code| code.max(5))
}

/// One byte of the output the game handed over, or 256 past the end.
pub extern "C" fn host_output_byte(slot: u64, offset: u64) -> u32 {
    crate::session::output_byte(slot, offset)
}

/// How many bytes are waiting, and eight of them at a time.
pub extern "C" fn host_output_len(slot: u64) -> u64 {
    crate::session::output_len(slot)
}

pub extern "C" fn host_output_word(slot: u64, offset: u64) -> u64 {
    crate::session::output_word(slot, offset)
}

/// How many bytes of semantic events are waiting, and eight of them at a time.
/// The same shape as the output stream, drained with it.
pub extern "C" fn host_event_len(slot: u64) -> u64 {
    crate::session::event_len(slot)
}

pub extern "C" fn host_event_word(slot: u64, offset: u64) -> u64 {
    crate::session::event_word(slot, offset)
}

/// Forget the output the host has printed, so the next handover starts at zero.
pub extern "C" fn host_drained(slot: u64) -> u32 {
    crate::session::drained(slot).err().unwrap_or(0)
}

/// Add one byte to the reply being assembled for the pending request.
pub extern "C" fn host_reply_byte(slot: u64, byte: u32) -> u32 {
    match u8::try_from(byte) {
        Ok(byte) => crate::session::reply_byte(slot, byte).err().unwrap_or(0),
        Err(_) => 11,
    }
}

/// Submit an action rather than a line, with the entities it names.
pub extern "C" fn host_reply_action(slot: u64, verb: u32) -> u32 {
    crate::session::reply_action(slot, verb).err().unwrap_or(0)
}

pub extern "C" fn host_reply_value(slot: u64, tag: u32, payload: u64) -> u32 {
    crate::session::reply_value(slot, tag, payload)
        .err()
        .unwrap_or(0)
}

pub extern "C" fn host_reply_subject(slot: u64, handle: u64) -> u32 {
    crate::session::reply_subject(slot, handle)
        .err()
        .unwrap_or(0)
}

/// Deliver the assembled reply and let the game run on.
pub extern "C" fn host_resume(slot: u64) -> u32 {
    crate::session::resume(slot).err().unwrap_or(0)
}

/// Report that no more input will come; every request from now ends the input.
pub extern "C" fn host_close(slot: u64) -> u32 {
    crate::session::close(slot).err().unwrap_or(0)
}

/// Record the session's outcome once the game entry has returned.
pub extern "C" fn host_finish(slot: u64, outcome: u64) -> u32 {
    crate::session::finish(slot, outcome).err().unwrap_or(0)
}

/// The finished session's outcome.
pub extern "C" fn host_outcome(slot: u64) -> u64 {
    crate::session::outcome(slot)
}

/// Ask for a bounded, read-only look at stored state. The answer is
/// ready after the game reaches its next safe boundary, which is between
/// commands; read it with `host_result_len` and `host_result`.
pub extern "C" fn host_inspect(slot: u64, kind: u32, a: u64, b: u64) -> u32 {
    crate::session::inspect(slot, kind, a, b).err().unwrap_or(0)
}

/// Number of values produced by the last inspection query.
pub extern "C" fn host_result_len(slot: u64) -> u64 {
    crate::session::result_len(slot)
}

pub extern "C" fn host_result(slot: u64, index: u64) -> u64 {
    crate::session::result(slot, index)
}

/// Execute one operation. Read status before interpreting the returned payload.
pub extern "C" fn call(op: u32, session: u64, a: u64, b: u64, c: u64) -> u64 {
    invoke(|state| state.perform(op, session, a, b, c))
}

/// Private exact-bundle ABI: low flags encode word kind; high flags property kind.
pub extern "C" fn dictionary_call(
    op: u32,
    session: u64,
    dictionary: u64,
    target: u64,
    flags: u64,
    word: u64,
    property: u64,
) -> u64 {
    invoke(|state| {
        state.dictionary(DictionaryRequest {
            op,
            session,
            dictionary,
            target,
            flags,
            word,
            property,
        })
    })
}

/// Private exact-bundle regular expressions: op 0 matches at a position.
pub extern "C" fn regex_call(
    op: u32,
    session: u64,
    pattern_kind: u64,
    pattern: u64,
    subject_kind: u64,
    subject: u64,
    index: u64,
) -> u64 {
    invoke(|state| {
        state.regex(RegexRequest {
            op,
            session,
            pattern_kind,
            pattern,
            subject_kind,
            subject,
            index,
        })
    })
}

/// Private exact-bundle grammar parse: op 0 uses `dictionary`, op 1 passes nil.
pub extern "C" fn grammar_call(
    op: u32,
    session: u64,
    production: u64,
    tokens: u64,
    dictionary: u64,
    recipient: u64,
) -> u64 {
    invoke(|state| {
        state.grammar(GrammarRequest {
            op,
            session,
            production,
            tokens,
            dictionary,
            recipient,
        })
    })
}

/// Private exact-bundle table operations: low flags are key kind, high flags value kind.
pub extern "C" fn lookup_call(
    op: u32,
    session: u64,
    table: u64,
    flags: u64,
    key: u64,
    value: u64,
) -> u64 {
    invoke(|state| {
        state.lookup(LookupRequest {
            op,
            session,
            table,
            flags,
            key,
            value,
        })
    })
}

fn invoke(run: impl FnOnce(&mut State) -> Result<u64, u32>) -> u64 {
    let (payload, status, kind) = STATE
        .try_with(|cell| {
            let Ok(mut state) = cell.try_borrow_mut() else {
                return (0, 12, 0);
            };
            state.kind = 0;
            match run(&mut state) {
                Ok(value) => (value, 0, state.kind),
                Err(error) => (0, error, 0),
            }
        })
        .unwrap_or((0, 13, 0));
    let _ = LAST_STATUS.try_with(|cell| cell.set(status));
    let _ = LAST_KIND.try_with(|cell| cell.set(kind));
    payload
}

/// Read immutable module data after session close. 256 is end/invalid, not NUL.
pub extern "C" fn text_byte(literal: u32, offset: u64) -> u32 {
    let Ok(offset) = usize::try_from(offset) else {
        return 256;
    };
    crate::tables::installed()
        .literals
        .get(literal as usize)
        .and_then(|text| text.as_bytes().get(offset))
        .map_or(256, |byte| u32::from(*byte))
}

/// Bytes from the most recently accepted invocation, including a failed prefix.
pub extern "C" fn output_byte(offset: u64) -> u32 {
    STATE
        .try_with(|cell| {
            cell.try_borrow()
                .map_or(256, |state| state.output.byte(offset))
        })
        .unwrap_or(256)
}

/// Current thread's session for generated source calls; zero means unavailable.
pub extern "C" fn session() -> u64 {
    STATE
        .try_with(|cell| {
            cell.try_borrow()
                .ok()
                .and_then(|state| state.store.as_ref().map(Store::session_id))
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

pub extern "C" fn status() -> u32 {
    LAST_STATUS.try_with(Cell::get).unwrap_or(13)
}
pub extern "C" fn kind() -> u32 {
    LAST_KIND.try_with(Cell::get).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a host that reads lines can still name a keystroke that is not
    /// a character. Self-authored; the name spellings follow the reference.
    #[test]
    fn a_key_reply_is_one_character_or_the_name_of_an_extended_key() {
        assert_eq!(key_from_reply("x"), "x");
        // The rest of a line is not a second keystroke.
        assert_eq!(key_from_reply("xyz"), "x");
        // An empty line is the return key, as it was before.
        assert_eq!(key_from_reply(""), "\n");
        assert_eq!(key_from_reply("[up]"), "[up]");
        assert_eq!(key_from_reply("[page down]"), "[page down]");
        // A console hands the name over with the newline still attached.
        assert_eq!(key_from_reply("[f10]\n"), "[f10]");
        // A name the reference has no keystroke for is not four characters.
        assert_eq!(key_from_reply("[nonesuch]"), "[?]");
        // A bracket the host meant literally is still one character.
        assert_eq!(key_from_reply("[a"), "[");
    }

    /// a token sequence may be a list or a vector, and nothing else.
    /// Self-authored: TADS has no vector-shaped token list.
    #[test]
    fn a_token_sequence_reads_from_a_list_or_a_vector() {
        let mut state = State::default();
        state.perform(0, 0, 64, 256, 4096).unwrap();
        let store = state.store.as_mut().unwrap();
        let mut objects = store.objects();
        let word = token(&mut objects, "look", 1);
        let list = objects.new_list(&[word]).unwrap();
        let vector = objects.new_vector(1).unwrap();
        objects.vector_append(vector, word).unwrap();
        let text = objects.new_text("look").unwrap();

        assert_eq!(token_sequence(&objects, list).unwrap(), &[word]);
        assert_eq!(token_sequence(&objects, vector).unwrap(), &[word]);
        // Text is not a sequence of tokens, and the error still says so.
        assert_eq!(token_sequence(&objects, text), Err(Error::WrongType));
    }

    fn token(objects: &mut Objects<'_>, text: &str, kind: u32) -> Value {
        let text = objects.new_text(text).unwrap();
        Value::List(
            objects
                .new_list(&[Value::Text(text), Value::Enumerator(kind)])
                .unwrap(),
        )
    }

    /// op 0 matches at a position, op 1 searches, and ops 2 and 3
    /// report the last match's group bounds, all one-based like the reference.
    #[test]
    fn regex_ops_match_search_and_report_groups() {
        let mut state = State::default();
        let session = state.perform(0, 0, 64, 256, 4096).unwrap();
        let text = |state: &mut State, value: &str| {
            let store = state.store.as_mut().unwrap();
            let mut objects = store.objects();
            objects.new_text(value).unwrap().handle()
        };
        let pattern = text(&mut state, "(<alpha>+)<space>+(<alpha>+)");
        let subject = text(&mut state, "  hello   world");
        let request = |op, index| RegexRequest {
            op,
            session,
            pattern_kind: 7,
            pattern,
            subject_kind: 7,
            subject,
            index,
        };
        // anchored at the start: no match, because of the leading spaces
        assert_eq!(state.regex(request(0, 0)), Ok(0));
        assert_eq!(state.kind, 0);
        // searching finds it at one-based index 3, and the groups follow
        assert_eq!(state.regex(request(1, 0)), Ok(3));
        assert_eq!(state.kind, 2);
        assert_eq!(state.regex(request(3, 0)), Ok(13)); // whole match length
        assert_eq!(state.regex(request(2, 1)), Ok(3)); // group 1 start
        assert_eq!(state.regex(request(3, 1)), Ok(5)); // group 1 length
        assert_eq!(state.regex(request(2, 2)), Ok(11)); // group 2 start
        assert_eq!(state.regex(request(3, 2)), Ok(5));
        // a group the pattern never captured reports nil
        assert_eq!(state.regex(request(2, 3)), Ok(0));
        assert_eq!(state.kind, 0);
        // an invalid pattern is a source failure, not a panic
        let bad = text(&mut state, "(unclosed");
        assert_eq!(
            state.regex(RegexRequest {
                pattern: bad,
                ..request(0, 0)
            }),
            Err(17)
        );
    }

    /// Two-phase contract : begin exposes bare objects in pre-order,
    /// finish assigns properties, and checkpoint unwinding releases an unfinished parse.
    #[test]
    fn grammar_begin_orders_bare_objects_and_unwinding_releases_them() {
        let mut state = State::default();
        let session = state.perform(0, 0, 64, 256, 4096).unwrap();
        let store = state.store.as_mut().unwrap();
        let slots: Vec<ObjectRef> = (0..5).map(|_| store.objects().create().unwrap()).collect();
        for (slot, object) in slots.iter().enumerate() {
            store.bind_static(slot as u32, *object).unwrap();
        }
        let mut objects = store.objects();
        let get = token(&mut objects, "get", 5);
        let lamp = token(&mut objects, "lamp", 5);
        let get_lamp = objects.new_list(&[get, lamp]).unwrap();
        let recipient = objects.new_owned_collection().unwrap();
        let begin = GrammarRequest {
            op: 3,
            session,
            production: slots[0].handle(),
            tokens: get_lamp.handle(),
            dictionary: 0,
            recipient: recipient.handle(),
        };
        let order = state.grammar(GrammarRequest { op: 3, ..begin }).unwrap();
        assert_eq!(state.kind, 9);
        let store = state.store.as_mut().unwrap();
        let order_ref = store.native_reference(order);
        let mut objects = store.objects();
        let bare = objects.list(order_ref).unwrap().to_vec();
        let [Value::Reference(take), Value::Reference(noun)] = bare[..] else {
            panic!("pre-order take then noun: {bare:?}")
        };
        assert_eq!(objects.of_kind(take, slots[2]), Ok(true));
        assert_eq!(objects.of_kind(noun, slots[3]), Ok(true));
        assert_eq!(objects.get(take, PropertyId(20)), Ok(Value::Nil));
        assert_eq!(objects.get(take, PropertyId(31)), Ok(Value::Nil));
        let finish = |order: u64| GrammarRequest {
            op: 4,
            session,
            production: order,
            tokens: 0,
            dictionary: 0,
            recipient: 0,
        };
        assert_eq!(state.grammar(finish(get_lamp.handle())), Err(11));
        let list = state.grammar(finish(order)).unwrap();
        assert!(state.parses.is_empty());
        let store = state.store.as_mut().unwrap();
        let list = store.native_reference(list);
        let mut objects = store.objects();
        assert_eq!(objects.list(list).unwrap(), &[Value::Reference(take)]);
        assert_eq!(objects.get(take, PropertyId(20)), Ok(Value::Int(1)));
        assert_eq!(
            objects.get(take, PropertyId(31)),
            Ok(Value::Reference(noun))
        );
        assert_eq!(objects.owned_collection_len(recipient), Ok(1));

        let mark = state.perform(63, session, 0, 0, 0).unwrap();
        let live = state.store.as_ref().unwrap().live_objects();
        state.grammar(begin).unwrap();
        assert_eq!(state.parses.len(), 1);
        state.perform(65, session, mark, 0, 0).unwrap();
        assert!(state.parses.is_empty());
        let store = state.store.as_mut().unwrap();
        assert_eq!(store.live_objects(), live);
        assert_eq!(store.objects().owned_collection_len(recipient), Ok(1));
    }

    /// Uses the test tables in grammar_data.rs.
    #[test]
    fn grammar_matches_capture_groups_and_belong_to_one_recipient() {
        let mut state = State::default();
        let session = state.perform(0, 0, 64, 256, 4096).unwrap();
        let store = state.store.as_mut().unwrap();
        let slots: Vec<ObjectRef> = (0..5).map(|_| store.objects().create().unwrap()).collect();
        for (slot, object) in slots.iter().enumerate() {
            store.bind_static(slot as u32, *object).unwrap();
        }
        let mut objects = store.objects();
        let get = token(&mut objects, "get", 5);
        let lamp = token(&mut objects, "lamp", 5);
        let say = token(&mut objects, "say", 5);
        let number = token(&mut objects, "7", 6);
        let get_lamp = objects.new_list(&[get, lamp]).unwrap();
        let say_more = objects.new_list(&[say, lamp, number]).unwrap();
        let wrong = objects.new_list(&[get, number]).unwrap();
        let recipient = objects.new_owned_collection().unwrap();
        let plain = objects.create().unwrap();
        let request =
            |production: ObjectRef, tokens: ObjectRef, recipient: ObjectRef| GrammarRequest {
                op: 1,
                session,
                production: production.handle(),
                tokens: tokens.handle(),
                dictionary: 0,
                recipient: recipient.handle(),
            };

        let list = state
            .grammar(request(slots[0], get_lamp, recipient))
            .unwrap();
        assert_eq!(state.kind, 9);
        let store = state.store.as_mut().unwrap();
        let list = store.native_reference(list);
        let mut objects = store.objects();
        assert_eq!(objects.owned_collection_len(recipient), Ok(1));
        let matches = objects.list(list).unwrap().to_vec();
        let [Value::Reference(take)] = matches[..] else {
            panic!("one match expected: {matches:?}")
        };
        assert_eq!(objects.of_kind(take, slots[2]), Ok(true));
        assert_eq!(objects.get(take, PropertyId(20)), Ok(Value::Int(1)));
        assert_eq!(objects.get(take, PropertyId(21)), Ok(Value::Int(2)));
        assert_eq!(objects.get(take, PropertyId(22)), Ok(Value::List(get_lamp)));
        let Ok(Value::Text(verb)) = objects.get(take, PropertyId(30)) else {
            panic!("group capture belongs to the enclosing match")
        };
        assert_eq!(objects.text(verb), Ok("get"));
        let Ok(Value::Reference(noun)) = objects.get(take, PropertyId(31)) else {
            panic!("production capture")
        };
        assert_eq!(objects.of_kind(noun, slots[3]), Ok(true));
        assert_eq!(objects.get(noun, PropertyId(20)), Ok(Value::Int(2)));
        let Ok(Value::Text(word)) = objects.get(noun, PropertyId(32)) else {
            panic!("token capture")
        };
        assert_eq!(objects.text(word), Ok("lamp"));

        let list = state
            .grammar(request(slots[0], say_more, recipient))
            .unwrap();
        let store = state.store.as_mut().unwrap();
        let list = store.native_reference(list);
        let mut objects = store.objects();
        let [Value::Reference(said)] = objects.list(list).unwrap().to_vec()[..] else {
            panic!("star match")
        };
        assert_eq!(objects.of_kind(said, slots[4]), Ok(true));
        assert_eq!(objects.get(said, PropertyId(21)), Ok(Value::Int(1)));

        let list = state.grammar(request(slots[0], wrong, recipient)).unwrap();
        let store = state.store.as_mut().unwrap();
        let list = store.native_reference(list);
        assert_eq!(store.objects().list(list).map(<[Value]>::len), Ok(0));
        assert_eq!(store.objects().owned_collection_len(recipient), Ok(3));

        let live = store.live_objects();
        assert_eq!(
            state.grammar(request(slots[2], get_lamp, recipient)),
            Err(16)
        );
        assert_eq!(state.grammar(request(slots[0], get_lamp, plain)), Err(16));
        let store = state.store.as_mut().unwrap();
        assert_eq!(store.live_objects(), live);
        store.objects().destroy(recipient).unwrap();
        assert!(!store.is_valid(take) && !store.is_valid(noun) && !store.is_valid(said));
    }

    #[test]
    fn captured_callback_bridge_checks_modes_indices_and_scope_expiry() {
        let mut state = State::default();
        let session = state.perform(0, 0, 16, 64, 4096).unwrap();
        let scope = state.perform(63, session, 0, 0, 0).unwrap();
        let modes = state.perform(73, session, 1, 0, 0).unwrap();
        state.perform(74, session, modes, 2, 0).unwrap();
        state.perform(38, session, modes, 0, 0).unwrap();
        let values = state.perform(73, session, 1, 0, 0).unwrap();
        state.perform(74, session, values, 2, 7).unwrap();
        state.perform(38, session, values, 0, 0).unwrap();
        let closure = state.perform(107, session, 42, modes, values).unwrap();
        assert_eq!(state.perform(108, session, closure, 0, 0), Ok(42));
        assert_eq!(state.perform(109, session, closure, 0, 0), Ok(7));
        assert_eq!(state.kind, 2);
        assert!(state.perform(109, session, closure, 1, 0).is_err());
        assert_eq!(state.perform(107, session, 42, values, values), Err(11));
        state.perform(65, session, scope, 0, 0).unwrap();
        assert!(state.perform(108, session, closure, 0, 0).is_err());
        assert_eq!(state.store.as_ref().unwrap().live_objects(), 0);
    }

    #[test]
    fn returned_local_owner_survives_callee_cleanup_and_is_claimed_once() {
        let mut state = State::default();
        let session = state.perform(0, 0, 32, 64, 4096).unwrap();
        let prototype = state
            .store
            .as_mut()
            .unwrap()
            .objects()
            .create()
            .unwrap()
            .handle();
        let caller = state.perform(63, session, 0, 0, 0).unwrap();
        let borrowed = state.perform(66, session, 1, 0, 0).unwrap();
        let callee = state.perform(63, session, 0, 0, 0).unwrap();
        assert_eq!(state.perform(83, session, borrowed, callee, 0), Err(3));
        let object = state.perform(43, session, prototype, 0, 0).unwrap();
        state.perform(82, session, object, 0, 0).unwrap();
        state.perform(63, session, 0, 0, 0).unwrap();
        let temporary = state.perform(66, session, 2, 0, 0).unwrap();
        let token = state.perform(83, session, object, callee, 0).unwrap();
        assert_eq!(state.perform(83, session, object, callee, 0), Err(3));
        assert_eq!(state.perform(84, session, token, object, 1), Err(11));
        state.perform(65, session, callee, 0, 0).unwrap();
        assert!(state.perform(67, session, temporary, 0, 0).is_err());
        assert_eq!(state.perform(67, session, borrowed, 0, 0), Ok(0));
        assert_eq!(state.perform(62, session, object, prototype, 0), Ok(1));
        assert_eq!(state.perform(84, session, token, borrowed, 1), Err(11));
        assert_eq!(state.perform(84, session, token, object, 1), Ok(object));
        assert_eq!(state.perform(84, session, token, object, 1), Err(11));
        state.perform(65, session, caller, 0, 0).unwrap();
        assert!(state.perform(62, session, object, prototype, 0).is_err());
        assert!(state.returned_owners.is_empty());
        assert!(state.local_owners.is_empty());
        assert_eq!(state.store.as_ref().unwrap().live_objects(), 1);
    }

    #[test]
    fn unconsumed_return_is_released_and_borrowed_results_cannot_be_adopted() {
        let mut state = State::default();
        let session = state.perform(0, 0, 16, 32, 4096).unwrap();
        let caller = state.perform(63, session, 0, 0, 0).unwrap();
        assert_eq!(state.perform(84, session, 0, 0, 1), Err(17));
        assert_eq!(state.perform(84, session, 0, 42, 0), Ok(0));
        for _ in 0..32 {
            let callee = state.perform(63, session, 0, 0, 0).unwrap();
            let value = state.perform(66, session, 1, 0, 0).unwrap();
            let token = state.perform(83, session, value, callee, 0).unwrap();
            state.perform(65, session, callee, 0, 0).unwrap();
            assert_eq!(state.perform(84, session, token, value, 0), Err(17));
            assert_eq!(state.store.as_ref().unwrap().live_objects(), 0);
            assert!(state.returned_owners.is_empty());
        }
        state.perform(65, session, caller, 0, 0).unwrap();
    }

    #[test]
    fn handled_exception_scope_releases_or_transfers_its_owner() {
        let mut state = State::default();
        let session = state.perform(0, 0, 32, 64, 4096).unwrap();
        let prototype = state
            .store
            .as_mut()
            .unwrap()
            .objects()
            .create()
            .unwrap()
            .handle();
        let outer = state.perform(63, session, 0, 0, 0).unwrap();
        let object = state.perform(43, session, prototype, 0, 0).unwrap();
        let token = state.perform(76, session, object, 0, 0).unwrap();
        let handler = state.perform(63, session, 0, 0, 0).unwrap();
        assert_eq!(state.perform(79, session, token, 0, 0), Ok(token));
        assert_eq!(state.perform(79, session, token, 0, 0), Err(11));
        let local = state.perform(66, session, 2, 0, 0).unwrap();
        // Moving into a rethrow preserves ownership across handler cleanup.
        assert_eq!(state.perform(80, session, token, 0, 0), Ok(token));
        assert_eq!(state.perform(80, session, token, 0, 0), Err(11));
        state.perform(65, session, handler, 0, 0).unwrap();
        assert_eq!(state.perform(77, session, token, 0, 0), Ok(object));
        assert!(state.perform(67, session, local, 0, 0).is_err());
        let next = state.perform(63, session, 0, 0, 0).unwrap();
        state.perform(79, session, token, 0, 0).unwrap();
        state.perform(63, session, 0, 0, 0).unwrap();
        state.perform(66, session, 1, 0, 0).unwrap();
        state.perform(65, session, next, 0, 0).unwrap();
        assert_eq!(state.perform(77, session, token, 0, 0), Err(11));
        assert!(state.exceptions.is_empty());
        assert_eq!(state.store.as_ref().unwrap().live_objects(), 1);
        assert_eq!(state.perform(79, session, 0, 0, 0), Ok(0));
        assert_eq!(state.perform(80, session, 0, 0, 0), Ok(0));
        state.perform(65, session, outer, 0, 0).unwrap();
    }

    #[test]
    fn exception_tokens_survive_unwind_and_release_independently() {
        let mut state = State::default();
        let session = state.perform(0, 0, 16, 32, 4096).unwrap();
        let prototype = state
            .store
            .as_mut()
            .unwrap()
            .objects()
            .create()
            .unwrap()
            .handle();
        let outer = state.perform(63, session, 0, 0, 0).unwrap();
        let inner = state.perform(63, session, 0, 0, 0).unwrap();
        let object = state.perform(43, session, prototype, 0, 0).unwrap();
        // A nested callee cannot consume a caller's construction activation.
        let callee = state.perform(63, session, 0, 0, 0).unwrap();
        assert_eq!(state.perform(76, session, object, 0, 0), Err(11));
        state.perform(65, session, callee, 0, 0).unwrap();
        let token = state.perform(76, session, object, 0, 0).unwrap();
        assert_ne!(token, object);
        assert_eq!(state.perform(76, session, object, 0, 0), Err(11));
        state.perform(65, session, inner, 0, 0).unwrap();
        assert_eq!(state.perform(77, session, token, 0, 0), Ok(object));
        // A nested catch/finalizer can carry another exception without replacing
        // the first pending owner. Each outcome decides which token to release.
        let second = state.perform(43, session, prototype, 0, 0).unwrap();
        let other = state.perform(76, session, second, 0, 0).unwrap();
        state.perform(78, session, other, 0, 0).unwrap();
        assert_eq!(state.perform(77, session, other, 0, 0), Err(11));
        assert_eq!(state.perform(77, session, token, 0, 0), Ok(object));
        state.perform(65, session, outer, 0, 0).unwrap();
        assert_eq!(state.perform(77, session, token, 0, 0), Ok(object));
        state.perform(78, session, token, 0, 0).unwrap();
        assert_eq!(state.perform(78, session, token, 0, 0), Err(11));
        assert_eq!(state.store.as_ref().unwrap().live_objects(), 1);
        assert!(state.exceptions.is_empty());
    }

    #[test]
    fn argument_lists_preserve_order_and_borrow_caller_heap_values() {
        let mut state = State::default();
        let session = state.perform(0, 0, 32, 64, 4096).unwrap();
        let text = state
            .store
            .as_mut()
            .unwrap()
            .objects()
            .new_text("caller")
            .unwrap();
        let nested = state
            .store
            .as_mut()
            .unwrap()
            .objects()
            .new_list(&[Value::Int(8)])
            .unwrap();
        assert_eq!(state.perform(73, session, 4, 0, 0), Err(11));
        let frame = state.perform(63, session, 0, 0, 0).unwrap();
        let args = state.perform(73, session, 4, 0, 0).unwrap();
        assert_eq!(state.kind, 9);
        assert_eq!(state.perform(39, session, args, 9, 0), Err(15));
        assert_eq!(state.perform(38, session, args, 0, 0), Err(17));
        for (tag, value) in [(2, 42), (7, text.handle()), (9, nested.handle()), (11, 6)] {
            state.perform(74, session, args, tag, value).unwrap();
        }
        assert_eq!(state.perform(74, session, args, 0, 0), Err(17));
        state.perform(38, session, args, 0, 0).unwrap();
        assert_eq!(state.perform(39, session, args, 9, 0), Ok(4));
        for (index, (tag, value)) in [(2, 42), (7, text.handle()), (9, nested.handle()), (11, 6)]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                state.perform(40, session, args, index as u64 + 1, 0),
                Ok(value)
            );
            assert_eq!(state.kind, tag);
        }
        assert_eq!(state.perform(74, session, args, 0, 0), Err(17));
        let tail = state.perform(75, session, args, 2, 0).unwrap();
        assert_eq!(state.perform(39, session, tail, 9, 0), Ok(2));
        assert_eq!(state.perform(40, session, tail, 1, 0), Ok(nested.handle()));
        assert_eq!(state.kind, 9);
        let empty = state.perform(75, session, args, 4, 0).unwrap();
        assert_eq!(state.perform(39, session, empty, 9, 0), Ok(0));
        assert_eq!(state.perform(75, session, args, 5, 0), Err(17));
        state.perform(65, session, frame, 0, 0).unwrap();
        assert_eq!(state.perform(39, session, args, 9, 0), Err(2));
        let objects = state.store.as_mut().unwrap().objects();
        assert_eq!(objects.text(text), Ok("caller"));
        assert_eq!(objects.list_get(nested, 1), Ok(Value::Int(8)));
        assert!(state.local_owners.is_empty());
    }

    #[test]
    fn an_accumulator_slot_keeps_one_owner_entry_however_often_it_grows() {
        // each append hands the slot's entry to the new text and
        // releases the old, so a long accumulation holds one text at a time.
        let mut state = State::default();
        let session = state.perform(0, 0, 8, 400, 1024).unwrap();
        let frame = state.perform(63, session, 0, 0, 0).unwrap();
        // The slot owns a copy of the starting text.
        let piece = state.perform(127, session, 4, 0, 0).unwrap();
        let mut current = state.perform(127, session, 4, 0, 0).unwrap();
        assert_eq!(state.kind, 7);
        let owners = state.local_owners.len();
        for _ in 0..50 {
            let built = state.perform(22, session, current, piece, 0).unwrap();
            let previous = current;
            current = state.perform(128, session, previous, built, 0).unwrap();
            assert_eq!(current, built);
            // The old text is gone and the slot holds exactly one entry.
            assert_eq!(state.perform(39, session, previous, 7, 0), Err(2));
            assert_eq!(
                state.local_owners.iter().filter(|o| o.is_some()).count(),
                owners
            );
        }
        state.perform(65, session, frame, 0, 0).unwrap();
        assert!(state.local_owners.is_empty());
    }

    #[test]
    fn scope_owned_list_literals_adopt_their_scope_owned_elements() {
        // outside a static initializer a list literal belongs to the
        // calling scope, and an element that scope owns moves into it.
        let mut state = State::default();
        let session = state.perform(0, 0, 16, 64, 1024).unwrap();
        // Without an owner scope there is nothing to register the list with.
        assert_eq!(state.perform(36, session, 1, 0, 0), Err(11));
        let frame = state.perform(63, session, 0, 0, 0).unwrap();
        let inner = state.perform(36, session, 1, 0, 0).unwrap();
        let inner_ref = state.store.as_ref().unwrap().native_reference(inner);
        state.perform(37, session, inner, 2, 8).unwrap();
        state.perform(38, session, inner, 0, 0).unwrap();
        assert!(state.local_owners.contains(&Some(inner_ref)));
        let outer = state.perform(36, session, 2, 0, 0).unwrap();
        state.perform(37, session, outer, 2, 5).unwrap();
        state.perform(37, session, outer, 9, inner).unwrap();
        state.perform(38, session, outer, 0, 0).unwrap();
        // The nested list now belongs to the new list, not to the scope.
        assert!(!state.local_owners.contains(&Some(inner_ref)));
        assert_eq!(state.perform(39, session, outer, 9, 0), Ok(2));
        assert_eq!(state.perform(40, session, outer, 2, 0), Ok(inner));
        assert_eq!(state.kind, 9);
        // A builder this scope does not own cannot be filled this way.
        assert_eq!(state.perform(37, session, session, 2, 1), Err(11));
        state.perform(65, session, frame, 0, 0).unwrap();
        assert_eq!(state.perform(39, session, outer, 9, 0), Err(2));
        assert_eq!(state.perform(39, session, inner, 9, 0), Err(2));
        assert!(state.local_owners.is_empty());
    }

    #[test]
    fn partial_argument_lists_are_reclaimed_and_admission_is_fallible() {
        let mut state = State::default();
        let session = state.perform(0, 0, 16, 4, 1024).unwrap();
        let outer = state.perform(63, session, 0, 0, 0).unwrap();
        let empty = state.perform(73, session, 0, 0, 0).unwrap();
        state.perform(38, session, empty, 0, 0).unwrap();
        let inner = state.perform(63, session, 0, 0, 0).unwrap();
        let partial = state.perform(73, session, 4, 0, 0).unwrap();
        assert_eq!(state.perform(74, session, partial, 6, 0), Err(16));
        state.perform(74, session, partial, 0, 0).unwrap();
        state.perform(65, session, inner, 0, 0).unwrap();
        assert_eq!(state.perform(39, session, empty, 9, 0), Ok(0));
        let replacement = state.perform(73, session, 4, 0, 0).unwrap();
        assert_eq!(state.perform(39, session, replacement, 9, 0), Err(15));
        assert_eq!(state.perform(73, session, 1, 0, 0), Err(5));
        assert_eq!(state.store.as_ref().unwrap().live_objects(), 0);
        let _ = state.perform(65, session, outer, 0, 0);
        assert!(state.local_owners.is_empty());
    }

    #[test]
    fn native_transient_metadata_is_checked_and_expires_with_its_object() {
        let mut state = State::default();
        let session = state.perform(0, 0, 32, 64, 4096).unwrap();
        let scope = state.perform(63, session, 0, 0, 0).unwrap();
        let object = state.perform(86, session, 0, 0, 0).unwrap();
        assert_eq!(state.perform(91, session, object, 0, 0), Ok(0));
        assert_eq!(state.kind, 0);
        assert_eq!(state.perform(90, session, object, 2, 0), Err(11));
        state.perform(90, session, object, 1, 0).unwrap();
        state.perform(91, session, object, 0, 0).unwrap();
        assert_eq!(state.kind, 1);
        state.perform(65, session, scope, 0, 0).unwrap();
        assert_eq!(state.perform(91, session, object, 0, 0), Err(2));
    }

    #[test]
    fn native_string_buffers_and_snapshots_have_scoped_owners() {
        let mut state = State::default();
        let session = state.perform(0, 0, 32, 64, 4096).unwrap();
        assert_eq!(state.perform(86, session, 0, 0, 0), Err(11));
        let caller = state.perform(63, session, 0, 0, 0).unwrap();
        let buffer = state.perform(86, session, 0, 0, 0).unwrap();
        assert_eq!(state.perform(87, session, buffer, 4, 0), Ok(buffer));
        assert_eq!(state.perform(89, session, buffer, 0, 0), Ok(7));
        let inner = state.perform(63, session, 0, 0, 0).unwrap();
        let snapshot = state.perform(88, session, buffer, 0, 0).unwrap();
        assert_eq!(state.kind, 7);
        assert_eq!(state.perform(87, session, buffer, 3, buffer), Ok(buffer));
        assert_eq!(state.perform(89, session, buffer, 0, 0), Ok(14));
        let object = state.store.as_ref().unwrap().native_reference(snapshot);
        assert_eq!(
            state.store.as_mut().unwrap().objects().text(object),
            Ok("literal")
        );
        state.perform(65, session, inner, 0, 0).unwrap();
        assert_eq!(state.perform(87, session, buffer, 7, snapshot), Err(2));
        assert_eq!(state.perform(89, session, buffer, 0, 0), Ok(14));
        state.perform(65, session, caller, 0, 0).unwrap();
        assert_eq!(state.perform(89, session, buffer, 0, 0), Err(2));
        assert_eq!(state.store.as_ref().unwrap().live_objects(), 0);
    }

    #[test]
    fn native_lookup_returns_values_and_transfers_its_single_owner() {
        let mut state = State::default();
        let session = state.perform(0, 0, 32, 64, 4096).unwrap();
        assert_eq!(state.perform(85, session, 0, 0, 0), Err(11));
        let caller = state.perform(63, session, 0, 0, 0).unwrap();
        let callee = state.perform(63, session, 0, 0, 0).unwrap();
        let table = state.perform(85, session, 0, 0, 0).unwrap();
        let request = |op, flags, key, value| LookupRequest {
            op,
            session,
            table,
            flags,
            key,
            value,
        };
        assert_eq!(
            state.lookup(request(1, (2u64 << 32) | 2, u64::from((-1i32) as u32), 42)),
            Ok(42)
        );
        assert_eq!(
            state.lookup(request(0, 2, u64::from((-1i32) as u32), 0)),
            Ok(42)
        );
        assert_eq!(state.kind, 2);
        assert_eq!(state.lookup(request(5, 2u64 << 32, 0, 99)), Ok(table));
        assert_eq!(state.lookup(request(0, 2, 100, 0)), Ok(99));
        assert_eq!(state.lookup(request(2, 2, 100, 0)), Ok(0));
        assert_eq!(state.kind, 0);
        assert_eq!(state.lookup(request(3, 0, 0, 0)), Ok(1));
        assert_eq!(state.lookup(request(0, 2, 1u64 << 32, 0)), Err(11));
        assert_eq!(state.lookup(request(1, (6u64 << 32) | 2, 1, 0)), Err(16));
        assert_eq!(state.lookup(request(3, 0, 0, 0)), Ok(1));
        let token = state.perform(83, session, table, callee, 0).unwrap();
        state.perform(65, session, callee, 0, 0).unwrap();
        state.perform(84, session, token, table, 1).unwrap();
        assert_eq!(
            state.lookup(request(0, 2, u64::from((-1i32) as u32), 0)),
            Ok(42)
        );
        state.perform(65, session, caller, 0, 0).unwrap();
        assert_eq!(state.lookup(request(3, 0, 0, 0)), Err(2));
        assert_eq!(state.store.as_ref().unwrap().live_objects(), 0);
    }

    #[test]
    fn native_vectors_belong_to_their_checkpoint_and_borrow_their_entries() {
        let mut state = State::default();
        let session = state.perform(0, 0, 32, 64, 4096).unwrap();
        assert_eq!(state.perform(66, session, 3, 0, 0), Err(11));
        let outer = state.perform(63, session, 0, 0, 0).unwrap();
        let vector = state.perform(66, session, 3, 0, 0).unwrap();
        assert_eq!(state.kind, 3);
        for n in [10, 20, 30] {
            assert_eq!(state.perform(69, session, vector, 2, n), Ok(vector));
        }
        assert_eq!(state.perform(68, session, vector, 3, 0), Ok(30));
        assert_eq!(state.kind, 2);
        state
            .perform(
                71,
                session,
                vector,
                u64::from((-2i32) as u32),
                u64::from((-1i32) as u32),
            )
            .unwrap();
        state.perform(70, session, vector, 3, 0).unwrap();
        assert_eq!(state.perform(68, session, vector, 2, 0), Ok(0));
        assert_eq!(state.kind, 0);
        state
            .perform(72, session, vector, (2u64 << 32) | 2, 40)
            .unwrap();
        let inner = state.perform(63, session, 0, 0, 0).unwrap();
        let child = state.perform(66, session, 0, 0, 0).unwrap();
        state.perform(69, session, child, 3, vector).unwrap();
        state.perform(57, session, 3, vector, 0).unwrap();
        state.perform(70, session, vector, 0, 0).unwrap();
        for (tag, payload) in [(2, 10), (2, 40), (0, 0)] {
            assert_eq!(state.perform(58, session, 0, 0, 0), Ok(1));
            assert_eq!(state.perform(59, session, 0, 0, 0), Ok(payload));
            assert_eq!(state.kind, tag);
        }
        state.perform(64, session, inner, 0, 0).unwrap();
        assert!(state.iterations.is_empty());
        assert_eq!(state.perform(67, session, child, 0, 0), Err(2));
        assert_eq!(state.perform(67, session, vector, 0, 0), Ok(0));
        state.perform(65, session, outer, 0, 0).unwrap();
        assert!(state.local_owners.is_empty());
        assert_eq!(state.perform(67, session, vector, 0, 0), Err(2));
        assert_eq!(state.store.as_ref().unwrap().live_objects(), 0);
    }

    #[test]
    fn native_vector_arguments_fail_without_publishing_or_mutating() {
        let mut state = State::default();
        let session = state.perform(0, 0, 16, 16, 1024).unwrap();
        let frame = state.perform(63, session, 0, 0, 0).unwrap();
        assert_eq!(state.perform(66, session, u64::MAX, 0, 0), Err(11));
        assert_eq!(
            state.perform(66, session, u64::from(u32::MAX), 0, 0),
            Err(17)
        );
        assert!(state.local_owners.is_empty());
        let vector = state.perform(66, session, 1, 0, 0).unwrap();
        assert_eq!(state.perform(69, session, vector, 1u64 << 32, 0), Err(11));
        assert_eq!(state.perform(69, session, vector, 6, 0), Err(16));
        assert_eq!(state.perform(69, session, vector, 2, 1u64 << 32), Err(11));
        assert_eq!(state.perform(67, session, vector, 0, 0), Ok(0));
        state.perform(69, session, vector, 2, 7).unwrap();
        assert_eq!(state.perform(72, session, vector, 2, 8), Err(17));
        assert_eq!(state.perform(68, session, vector, 1, 0), Ok(7));
        state.perform(65, session, frame, 0, 0).unwrap();
        assert_eq!(state.perform(64, session, frame, 0, 0), Err(11));
    }

    #[test]
    fn checkpoint_unwinds_callee_work_and_preserves_caller_state() {
        let mut state = State::default();
        let session = state.perform(0, 0, 30, 60, 0).unwrap();
        let prototype = state.perform(1, session, 0, 0, 0).unwrap();
        let recipient = state.perform(1, session, 0, 0, 0).unwrap();
        let list = state
            .store
            .as_mut()
            .unwrap()
            .objects()
            .new_list(&[Value::Int(7)])
            .unwrap()
            .handle();
        state.perform(57, session, 9, list, 0).unwrap();
        let outer = state.perform(63, session, 0, 0, 0).unwrap();
        let owned = state.perform(43, session, prototype, 0, 0).unwrap();
        let inner = state.perform(63, session, 0, 0, 0).unwrap();
        let published = state.perform(46, session, prototype, 0, 0).unwrap();
        state.perform(47, session, published, recipient, 1).unwrap();
        state.perform(57, session, 9, list, 0).unwrap();
        assert_eq!(state.perform(64, session, outer, 0, 0), Err(11));
        state.perform(64, session, inner, 0, 0).unwrap();
        assert_eq!(state.perform(4, session, recipient, 1, 0), Ok(published));
        assert_eq!(state.constructions.len(), 1);
        assert_eq!(state.iterations.len(), 1);
        state.perform(64, session, outer, 0, 0).unwrap();
        assert_eq!(state.perform(61, session, owned, 0, 0), Err(2));
        assert_eq!(state.perform(58, session, 0, 0, 0), Ok(1));
        assert_eq!(state.perform(59, session, 0, 0, 0), Ok(7));
        assert_eq!(state.perform(64, session, outer, 0, 0), Err(11));
        state.perform(60, session, 0, 0, 0).unwrap();
        assert!(state.checkpoints.is_empty());
        assert!(state.constructions.is_empty());
        state.perform(9, session, 0, 0, 0).unwrap();
    }

    #[test]
    fn checkpoint_restores_active_member_then_cancels_withdrawn_ownership() {
        let mut state = State::default();
        let session = state.perform(0, 0, 30, 60, 0).unwrap();
        let prototype = state.perform(1, session, 0, 0, 0).unwrap();
        let collection = state
            .store
            .as_mut()
            .unwrap()
            .objects()
            .new_owned_collection()
            .unwrap()
            .handle();
        let member = state.perform(46, session, prototype, 0, 0).unwrap();
        state.perform(51, session, member, collection, 0).unwrap();
        state.perform(48, session, member, 0, 0).unwrap();
        let caller = state.perform(63, session, 0, 0, 0).unwrap();
        state.perform(55, session, member, 0, 0).unwrap();
        let callee = state.perform(63, session, 0, 0, 0).unwrap();
        state.perform(52, session, collection, member, 0).unwrap();
        state.perform(64, session, callee, 0, 0).unwrap();
        assert_eq!(state.perform(61, session, member, 0, 0), Ok(0));
        state.perform(64, session, caller, 0, 0).unwrap();
        assert_eq!(state.perform(61, session, member, 0, 0), Err(2));
        assert!(state.member_calls.is_empty());
        state.perform(9, session, 0, 0, 0).unwrap();
    }

    #[test]
    fn output_failure_terminates_store_but_preserves_accepted_prefix() {
        let current = call(0, 0, 1, 1, 7);
        call(14, current, 0, 0, 0);
        assert_eq!(status(), 0);
        call(14, current, 1, 0, 0);
        assert_eq!(status(), 5);
        call(1, current, 0, 0, 0);
        assert_eq!(status(), 8);
        call(9, current, 0, 0, 0);
        assert_eq!(output_byte(0), u32::from(b'l'));
        assert_eq!(output_byte(7), 256);
        let next = call(0, 0, 1, 1, 0);
        assert_eq!(output_byte(0), 256);
        call(9, next, 0, 0, 0);
    }

    #[test]
    fn literal_metadata_is_checked_and_survives_session_close() {
        let session = call(0, 0, 1, 1, 0);
        let object = call(1, session, 0, 0, 0);
        assert_eq!(call(12, session, 1, 0, 0), 1);
        assert_eq!((status(), kind()), (0, 4));
        call(13, session, object, 0, 1);
        assert_eq!(call(4, session, object, 0, 0), 1);
        assert_eq!((status(), kind()), (0, 4));
        call(13, session, object, 0, u32::MAX as u64);
        assert_eq!(status(), 2);
        assert_eq!(call(4, session, object, 0, 0), 1);
        call(9, session, 0, 0, 0);
        assert_eq!(text_byte(1, 0), 97);
        assert_eq!(text_byte(1, 1), 0);
        assert_eq!(text_byte(1, 2), 195);
        assert_eq!(text_byte(1, 3), 169);
        assert_eq!(text_byte(1, 4), 256);
        assert_eq!(text_byte(u32::MAX, 0), 256);
    }

    #[test]
    fn dictionary_native_calls_preserve_filters_results_and_checked_inputs() {
        let current = call(0, 0, 20, 40, 100);
        let dictionary = call(34, current, 0, 0, 0);
        let target = call(1, current, 0, 0, 0);
        let flags = (8u64 << 32) | 4;
        dictionary_call(0, current, dictionary, target, flags, 0, 7);
        dictionary_call(0, current, dictionary, target, flags, 0, 7);
        assert_eq!((status(), kind()), (0, 0));
        dictionary_call(3, current, dictionary, 0, 4, 0, 0);
        assert_eq!((status(), kind()), (0, 1));
        dictionary_call(0, current, dictionary, target, (8u64 << 32) | 1, 0, 7);
        assert_eq!(status(), 16);
        dictionary_call(3, current, dictionary, 0, 4, u64::from(u32::MAX), 0);
        assert_eq!(status(), 2);
        call(20, current, target, 9, 0);
        let matches = dictionary_call(2, current, dictionary, 0, flags, 0, 7);
        assert_eq!((status(), kind()), (0, 9));
        assert_eq!(call(39, current, matches, 9, 0), 2);
        assert_eq!(call(40, current, matches, 1, 0), target);
        assert_eq!((status(), kind()), (0, 3));
        call(41, current, target, 9, matches);
        let word = call(21, current, 0, 0, 0);
        dictionary_call(0, current, dictionary, target, (8u64 << 32) | 7, word, 8);
        assert_eq!(status(), 0);
        dictionary_call(1, current, dictionary, target, flags, 0, 7);
        call(30, current, 0, 0, 0);
        dictionary_call(3, current, dictionary, 0, 4, 0, 0);
        assert_eq!((status(), kind()), (0, 1));
        dictionary_call(3, current, dictionary, 0, 7, word, 0);
        assert_eq!(status(), 2);
        dictionary_call(2, current, dictionary, 0, 4, 0, 0);
        assert_eq!(status(), 11);
        dictionary_call(1, current, dictionary, target, flags, 0, 8);
        dictionary_call(3, current, dictionary, 0, 4, 0, 0);
        assert_eq!((status(), kind()), (0, 0));
        assert_eq!(call(39, current, matches, 9, 0), 2);
        call(9, current, 0, 0, 0);
    }

    #[test]
    fn list_helpers_keep_typed_values_and_require_active_sessions() {
        let current = call(0, 0, 10, 20, 100);
        let owner = call(1, current, 0, 0, 0);
        call(20, current, owner, 5, 0);
        let list = call(36, current, 2, 0, 0);
        call(37, current, list, 2, 42);
        call(37, current, list, 1, 0);
        assert_eq!(call(38, current, list, 0, 0), list);
        assert_eq!((status(), kind()), (0, 9));
        call(41, current, owner, 5, list);
        call(30, current, 0, 0, 0);
        assert_eq!(call(39, current, list, 9, 0), 2);
        assert_eq!(call(40, current, list, 1, 0), 42);
        assert_eq!((status(), kind()), (0, 2));
        assert_eq!(call(40, current, list, 2, 0), 0);
        assert_eq!((status(), kind()), (0, 1));
        call(40, current, list, 0, 0);
        assert_eq!(status(), 17);
        call(20, current, owner, 6, 0);
        call(36, current, 100, 0, 0);
        assert_eq!(status(), 5);
        call(39, current, 0, 4, 0);
        assert_eq!(status(), 8);
        call(9, current, 0, 0, 0);
    }

    #[test]
    fn native_published_construction_keeps_modes_and_nesting_distinct() {
        let current = call(0, 0, 20, 30, 0);
        let prototype = call(1, current, 0, 0, 0);
        let recipient = call(1, current, 0, 0, 0);
        let outer = call(43, current, prototype, 0, 0);
        let inner = call(46, current, prototype, 0, 0);
        call(47, current, outer, recipient, 1);
        assert_eq!(status(), 11);
        call(44, current, inner, recipient, 1);
        assert_eq!(status(), 11);
        call(47, current, inner, recipient, 2);
        assert_eq!(status(), 0);
        call(48, current, outer, 0, 0);
        assert_eq!(status(), 11);
        assert_eq!(call(48, current, inner, 0, 0), inner);
        assert_eq!((status(), kind()), (0, 3));
        assert_eq!(call(4, current, recipient, 2, 0), inner);
        call(45, current, outer, 0, 0);
        assert_eq!(status(), 0);
        call(47, current, inner, recipient, 3);
        assert_eq!(status(), 11);
        call(9, current, 0, 0, 0);
    }

    #[test]
    fn native_owned_construction_checks_nesting_and_field_handoff() {
        let current = call(0, 0, 12, 20, 0);
        let prototype = call(1, current, 0, 0, 0);
        let holder = call(1, current, 0, 0, 0);
        let outer = call(43, current, prototype, 0, 0);
        assert_eq!((status(), kind()), (0, 3));
        call(7, current, outer, 1, 12);
        let inner = call(43, current, prototype, 0, 0);
        call(44, current, outer, holder, 2);
        assert_eq!(status(), 11);
        call(45, current, inner, 0, 0);
        assert_eq!(status(), 0);
        call(4, current, inner, 1, 0);
        assert_eq!(status(), 2);
        assert_eq!(call(44, current, outer, holder, 2), outer);
        assert_eq!((status(), kind()), (0, 3));
        assert_eq!(call(4, current, outer, 1, 0), 12);
        call(5, current, holder, 2, 0);
        call(4, current, outer, 1, 0);
        assert_eq!(status(), 2);
        call(9, current, 0, 0, 0);
    }

    #[test]
    fn enumerators_preserve_kind_payload_and_destination() {
        let current = call(0, 0, 2, 2, 0);
        let object = call(1, current, 0, 0, 0);
        call(42, current, object, 7, u64::from(u32::MAX));
        assert_eq!(status(), 0);
        assert_eq!(call(4, current, object, 7, 0), u64::from(u32::MAX));
        assert_eq!((status(), kind()), (0, 10));
        call(42, current, object, 7, u64::MAX);
        assert_eq!(status(), 11);
        assert_eq!(call(4, current, object, 7, 0), u64::from(u32::MAX));
        call(9, current, 0, 0, 0);
    }

    #[test]
    fn property_values_roundtrip_without_becoming_methods_or_objects() {
        let current = call(0, 0, 2, 2, 0);
        let object = call(1, current, 0, 0, 0);
        call(35, current, object, 9, 42);
        assert_eq!(status(), 0);
        assert_eq!(call(4, current, object, 9, 0), 42);
        assert_eq!((status(), kind()), (0, 8));
        call(35, current, object, 9, u64::MAX);
        assert_eq!(status(), 11);
        assert_eq!(call(4, current, object, 9, 0), 42);
        assert_eq!((status(), kind()), (0, 8));
        call(2, current, object, 9, 0);
        call(35, current, object, 9, 1);
        assert_eq!(status(), 2);
        call(9, current, 0, 0, 0);
    }

    #[test]
    fn session_properties_and_cleanup() {
        let session = call(0, 0, 8, 8, 0);
        assert_eq!(status(), 0);
        let a = call(1, session, 0, 0, 0);
        let b = call(1, session, 0, 0, 0);
        call(7, session, a, 17, u64::from((-42i32) as u32));
        assert_eq!(status(), 0);
        assert_eq!(call(4, session, a, 17, 0), u64::from((-42i32) as u32));
        assert_eq!((status(), kind()), (0, 2));
        call(8, session, b, 1, a);
        call(3, session, a, b, 0);
        call(3, session, b, a, 0);
        assert_eq!(status(), 4);
        call(2, session, a, 0, 0);
        call(4, session, b, 1, 0);
        assert_eq!((status(), kind()), (2, 0));
        call(9, session, 0, 0, 0);
        assert_eq!(status(), 0);
        call(1, session, 0, 0, 0);
        assert_eq!(status(), 9);
    }

    #[test]
    fn invalid_arguments_and_terminal_close() {
        let session = call(0, 0, 1, 1, 0);
        call(0, 0, 1, 1, 0);
        assert_eq!(status(), 10);
        call(1, session + 1, 0, 0, 0);
        assert_eq!(status(), 1);
        let a = call(1, session, 0, 0, 0);
        call(7, session, a, 1, u64::MAX);
        assert_eq!(status(), 11);
        call(4, session, a, 1, 0);
        assert_eq!((status(), kind()), (0, 0));
        call(1, session, 0, 0, 0);
        assert_eq!(status(), 5);
        call(1, session, 0, 0, 0);
        assert_eq!(status(), 8);
        call(9, session, 0, 0, 0);
        assert_eq!(status(), 0);
    }

    #[test]
    fn owned_text_helpers_expose_bytes_and_require_live_ownership() {
        let session = call(0, 0, 8, 8, 100);
        // Text built outside a static initializer belongs to a scope.
        let frame = call(63, session, 0, 0, 0);
        let source = call(21, session, 0, 0, 0);
        assert_eq!((status(), kind()), (0, 7));
        let number = call(25, session, 214, 10, 1);
        assert_eq!(call(24, session, number, 10, 0), 214);
        let joined = call(22, session, source, number, 0);
        let suffix = call(23, session, joined, 8, u64::MAX);
        assert_eq!(call(27, session, number, suffix, 0), 1);
        call(28, session, joined, 0, 0);
        assert_eq!(status(), 0);
        for (offset, byte) in b"literal214".iter().enumerate() {
            assert_eq!(output_byte(offset as u64), u32::from(*byte));
        }
        call(2, session, joined, 0, 0);
        call(26, session, joined, 0, 0);
        assert_eq!(status(), 2);
        assert_eq!(call(26, session, suffix, 0, 0), u64::from(b'2'));
        call(65, session, frame, 0, 0);
        call(9, session, 0, 0, 0);
        assert_eq!(status(), 0);
    }

    #[test]
    fn embedded_output_uses_bounded_decimal_bytes_and_rejects_boolean_values() {
        let session = call(0, 0, 1, 1, 12);
        call(33, session, 0, 0, 0);
        assert_eq!(status(), 0);
        call(33, session, 2, u64::from(i32::MIN as u32), 0);
        assert_eq!(status(), 0);
        let text: Vec<u8> = (0..11).map(|i| output_byte(i) as u8).collect();
        assert_eq!(text, b"-2147483648");
        call(33, session, 1, 0, 0);
        assert_eq!(status(), 16);
        assert_eq!(output_byte(11), 256);
        call(33, session, 2, 42, 0);
        assert_eq!(status(), 5);
        assert_eq!(output_byte(11), 256);
        call(9, session, 0, 0, 0);
        assert_eq!(output_byte(0), u32::from(b'-'));
        let session = call(0, 0, 1, 1, 16);
        let text = call(21, session, 0, 0, 0);
        call(33, session, 7, text, 0);
        assert_eq!(status(), 0);
        assert_eq!(output_byte(0), u32::from(b'l'));
        call(2, session, text, 0, 0);
        call(33, session, 7, text, 0);
        assert_eq!(status(), 2);
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn mixed_string_equality_validates_borrows_and_terminal_sessions() {
        let session = call(0, 0, 3, 3, 0);
        let text = call(21, session, 0, 0, 0);
        assert_eq!(call(32, session, 0, text, 1), 1);
        assert_eq!(status(), 0);
        assert_eq!(call(32, session, text, 0, 2), 1);
        assert_eq!(status(), 0);
        call(2, session, text, 0, 0);
        call(32, session, 0, text, 1);
        assert_eq!(status(), 2);
        call(1, session, 0, 0, 0);
        call(1, session, 0, 0, 0);
        call(1, session, 0, 0, 0);
        call(1, session, 0, 0, 0);
        assert_eq!(status(), 5);
        call(32, session, 0, 0, 3);
        assert_eq!(status(), 8);
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn initializer_strings_transfer_results_and_release_temporaries() {
        let session = call(0, 0, 6, 4, 0);
        let owner = call(1, session, 0, 0, 0);
        let borrower = call(1, session, 0, 0, 0);
        for _ in 0..20 {
            call(20, session, owner, 1, 0);
            assert_eq!(status(), 0);
            let temporary = call(25, session, 12, 10, 1);
            let result = call(22, session, temporary, temporary, 0);
            call(29, session, owner, 1, result);
            assert_eq!(status(), 0);
            call(29, session, borrower, 2, result);
            assert_eq!(status(), 0);
            call(30, session, 0, 0, 0);
            assert_eq!(status(), 0);
            call(26, session, temporary, 0, 0);
            assert_eq!(status(), 2);
            assert_eq!(call(4, session, owner, 1, 0), result);
            assert_eq!((status(), kind()), (0, 7));
            assert_eq!(call(26, session, result, 0, 0), u64::from(b'1'));
            call(5, session, owner, 1, 0);
            assert_eq!(status(), 0);
            assert_eq!(call(4, session, borrower, 2, 0), result);
            call(26, session, result, 0, 0);
            assert_eq!(status(), 2);
        }
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn nested_initializers_keep_outer_temporaries_and_borrow_inner_results() {
        let session = call(0, 0, 8, 4, 0);
        let owner = call(1, session, 0, 0, 0);
        call(20, session, owner, 1, 0);
        let outer = call(25, session, 12, 10, 1);
        call(20, session, owner, 2, 0);
        let inner = call(25, session, 34, 10, 1);
        call(29, session, owner, 2, inner);
        call(30, session, 0, 0, 0);
        assert_eq!(status(), 0);
        assert_eq!(call(26, session, outer, 0, 0), u64::from(b'1'));
        call(29, session, owner, 1, inner);
        call(30, session, 0, 0, 0);
        assert_eq!(status(), 0);
        call(26, session, outer, 0, 0);
        assert_eq!(status(), 2);
        assert_eq!(call(26, session, inner, 0, 0), u64::from(b'3'));
        call(5, session, owner, 2, 0);
        call(26, session, inner, 0, 0);
        assert_eq!(status(), 2);
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn static_marker_detects_cycles_and_can_publish_a_completed_value() {
        let session = call(0, 0, 2, 2, 0);
        let object = call(1, session, 0, 0, 0);
        call(15, session, object, 7, 0);
        call(20, session, object, 7, 0);
        call(4, session, object, 7, 0);
        assert_eq!(status(), 15);
        call(7, session, object, 7, 42);
        assert_eq!(call(4, session, object, 7, 0), 42);
        assert_eq!((status(), kind()), (0, 2));
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn inherited_lookup_preserves_property_and_defining_object() {
        let session = call(0, 0, 3, 10, 0);
        let base = call(1, session, 0, 0, 0);
        let child = call(1, session, 0, 0, 0);
        call(7, session, base, 7, 99);
        call(7, session, child, 7, 12);
        call(16, session, child, base, 0);
        assert_eq!(status(), 0);
        assert_eq!(call(17, session, child, 7, child), 99);
        assert_eq!((status(), kind()), (0, 2));
        call(5, session, base, 7, 0);
        assert_eq!(call(17, session, child, 7, child), 0);
        assert_eq!((status(), kind()), (0, 0));
        assert_eq!(call(17, session, child, 7, base), 0);
        assert_eq!((status(), kind()), (0, 6));
        call(17, session, child, u64::MAX, child);
        assert_eq!(status(), 11);
        call(2, session, base, 0, 0);
        call(17, session, child, 7, base);
        assert_eq!(status(), 2);
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn method_descriptors_are_checked_data_not_callbacks() {
        let session = call(0, 0, 1, 1, 0);
        let object = call(1, session, 0, 0, 0);
        call(15, session, object, 7, u64::MAX);
        assert_eq!(status(), 11);
        call(15, session, object, 7, u64::from(u32::MAX));
        assert_eq!(status(), 0);
        assert_eq!(call(4, session, object, 7, 0), u64::from(u32::MAX));
        assert_eq!((status(), kind()), (0, 5));
        call(2, session, object, 0, 0);
        call(4, session, object, 7, 0);
        assert_eq!(status(), 2);
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn local_owner_moves_to_field_once_and_survives_checkpoint_cleanup() {
        let session = call(0, 0, 10, 30, 0);
        let recipient = call(1, session, 0, 0, 0);
        let checkpoint = call(63, session, 0, 0, 0);
        let table = call(85, session, 0, 0, 0);
        call(96, session, table, recipient, 1);
        assert_eq!(status(), 0);
        call(96, session, table, recipient, 2);
        assert_eq!(status(), 3);
        call(64, session, checkpoint, 0, 0);
        assert_eq!(status(), 0);
        assert_eq!(call(4, session, recipient, 1, 0), table);
        assert_eq!((status(), kind()), (0, 3));
        call(5, session, recipient, 1, 0);
        call(4, session, table, 1, 0);
        assert_eq!(status(), 2);
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn constant_list_admission_failure_terminates_the_session() {
        let session = call(0, 0, 1, 20, 0);
        call(93, session, 0, 0, 0); // Region fits; list does not.
        assert_eq!(status(), 5);
        call(1, session, 0, 0, 0);
        assert_eq!(status(), 8);
        call(9, session, 0, 0, 0);
    }

    #[test]
    fn constant_lists_survive_field_replacement_and_expire_with_session() {
        let session = call(0, 0, 8, 20, 0);
        let owner = call(1, session, 0, 0, 0);
        let child = call(93, session, 1, 0, 0);
        call(94, session, child, 2, 7);
        call(38, session, child, 0, 0);
        let parent = call(93, session, 1, 0, 0);
        call(94, session, parent, 9, child);
        call(38, session, parent, 0, 0);
        assert_eq!(status(), 0);
        call(41, session, owner, 1, parent);
        call(5, session, owner, 1, 0);
        assert_eq!(call(40, session, parent, 1, 0), child);
        assert_eq!((status(), kind()), (0, 9));
        assert_eq!(call(40, session, child, 1, 0), 7);
        assert_eq!((status(), kind()), (0, 2));
        call(94, session, parent, 2, 9);
        assert_eq!(status(), 17); // Finished constants cannot be extended.
        call(9, session, 0, 0, 0);
        let next = call(0, 0, 8, 20, 0);
        call(40, session, child, 1, 0);
        assert_eq!(status(), 1);
        let empty = call(93, next, 0, 0, 0);
        call(38, next, empty, 0, 0);
        assert_eq!(status(), 0); // The old region was not reused.
        call(9, next, 0, 0, 0);
    }

    #[test]
    fn busy_call_does_not_publish_a_stale_success() {
        STATE.with(|cell| {
            let _borrow = cell.borrow_mut();
            assert_eq!(call(0, 0, 1, 1, 0), 0);
        });
        assert_eq!((status(), kind()), (12, 0));
    }

    #[test]
    fn independent_threads_and_foreign_tokens() {
        let session = call(0, 0, 2, 2, 0);
        let other = std::thread::spawn(move || {
            let own = call(0, 0, 2, 2, 0);
            call(1, session, 0, 0, 0);
            assert_eq!(status(), 1);
            call(1, own, 0, 0, 0);
            assert_eq!(status(), 0);
            own // Thread exit drops the live forest.
        })
        .join()
        .unwrap();
        assert_ne!(session, other);
        call(9, session, 0, 0, 0);
    }
}

#[cfg(test)]
mod scene_tests {
    use super::*;
    use crate::relations::{Cardinality, Relations};

    #[test]
    fn scene_contains_nested_locations_and_linked_states_without_evaluation() {
        let mut store = Store::new(Limits {
            objects: 32,
            properties: 128,
            string_bytes: 1024,
        })
        .unwrap();
        let mut objects = store.scope().unwrap();
        let root = objects.create().unwrap();
        let box_ = objects.create().unwrap();
        let actor = objects.create().unwrap();
        let state = objects.create().unwrap();
        let leader = objects.create().unwrap();
        let props = objects
            .new_list(&[
                Value::Property(PropertyId(2)),
                Value::Property(PropertyId(3)),
            ])
            .unwrap();
        objects
            .set(actor, PropertyId(1), Value::List(props))
            .unwrap();
        objects
            .set(actor, PropertyId(2), Value::Reference(state))
            .unwrap();
        objects
            .set(actor, PropertyId(3), Value::Function(999))
            .unwrap();
        objects
            .set(state, PropertyId(1), Value::List(props))
            .unwrap();
        objects
            .set(state, PropertyId(2), Value::Reference(leader))
            .unwrap();
        objects
            .set(leader, PropertyId(1), Value::List(props))
            .unwrap();
        objects
            .set(leader, PropertyId(2), Value::Reference(actor))
            .unwrap();
        let mut relations = Relations::default();
        let rel = relations.declare(Cardinality::OneToMany).unwrap();
        relations.set(rel, root, box_).unwrap();
        relations.set(rel, box_, actor).unwrap();
        let before = relations.all(rel, root, false).unwrap().to_vec();
        objects.set_journalling(true);
        let answer = inspect(&mut objects, &relations, 6, 1 << 32, root.handle()).unwrap();
        assert_eq!(&answer[..3], &[1, 0, 5]);
        let mut at = 3;
        let mut records = Vec::new();
        while at < answer.len() {
            let end = at + answer[at] as usize;
            records.push(&answer[at + 1..end]);
            at = end;
        }
        assert_eq!(&records[2][..4], &[actor.handle(), box_.handle(), 1, 2]);
        assert_eq!(&records[2][4..], &[2, 3, state.handle(), 3, 15, 0]);
        assert_eq!(&records[3][..4], &[state.handle(), 0, 0, 2]);
        assert_eq!(&records[3][4..7], &[2, 3, leader.handle()]);
        assert_eq!(objects.property_journal_len(), 0);
        assert_eq!(relations.all(rel, root, false).unwrap(), before);
        assert_eq!(
            scene_snapshot(&objects, &relations, 1 << 32, root.handle(), 8),
            Err(5)
        );
        assert_eq!(
            inspect(&mut objects, &relations, 6, 1 << 32, u64::MAX).unwrap(),
            [1, 2, 0]
        );
        assert_eq!(
            inspect(&mut objects, &relations, 6, (1 << 32) | 4096, root.handle()).unwrap(),
            [1, 11, 0]
        );
        // Existing kind 4 still returns exactly the same unframed triples.
        assert_eq!(
            inspect(&mut objects, &relations, 4, 1, actor.handle()).unwrap(),
            records[2][4..]
        );
    }

    #[test]
    fn scene_includes_connected_members_without_inventing_a_container() {
        let mut store = Store::new(Limits {
            objects: 8,
            properties: 16,
            string_bytes: 128,
        })
        .unwrap();
        let mut objects = store.objects();
        let room = objects.create().unwrap();
        let door = objects.create().unwrap();
        let other = objects.create().unwrap();
        let mut relations = Relations::default();
        relations.declare(Cardinality::OneToMany).unwrap();
        let connected = relations.declare(Cardinality::ManyToMany).unwrap();
        relations.set(connected, door, room).unwrap();
        relations.set(connected, door, other).unwrap();
        let descriptor = (1 << 32) | (1 << 24) | (1 << 25) | ((connected as u64) << 12);
        assert_eq!(
            inspect(&mut objects, &relations, 6, descriptor, room.handle()).unwrap(),
            [
                1,
                0,
                2,
                5,
                room.handle(),
                0,
                1,
                0,
                5,
                door.handle(),
                0,
                1,
                0
            ]
        );
        assert_eq!(
            inspect(&mut objects, &relations, 6, descriptor, other.handle()).unwrap(),
            [
                1,
                0,
                2,
                5,
                other.handle(),
                0,
                1,
                0,
                5,
                door.handle(),
                0,
                1,
                0
            ]
        );
        assert_eq!(
            inspect(
                &mut objects,
                &relations,
                6,
                descriptor | (1 << 26),
                room.handle()
            )
            .unwrap(),
            [1, 11, 0]
        );
    }

    #[test]
    fn ranges_frame_holes_and_reverse_relations_without_changing_legacy_queries() {
        let mut store = Store::new(Limits {
            objects: 8,
            properties: 16,
            string_bytes: 128,
        })
        .unwrap();
        let root = store.objects().create().unwrap();
        let actor = store.objects().create().unwrap();
        store.bind_static(5, root).unwrap();
        store.bind_static(7, actor).unwrap();
        let mut objects = store.objects();
        let props = objects.new_list(&[Value::Property(PropertyId(2))]).unwrap();
        objects
            .set(actor, PropertyId(1), Value::List(props))
            .unwrap();
        objects.set(actor, PropertyId(2), Value::Int(42)).unwrap();
        let mut relations = Relations::default();
        let rel = relations.declare(Cardinality::OneToMany).unwrap();
        relations.set(rel, root, actor).unwrap();
        let range = (3 << 32) | 5;
        assert_eq!(
            inspect(&mut objects, &relations, 7, 1, range).unwrap(),
            [
                1,
                0,
                3,
                3,
                5,
                root.handle(),
                3,
                6,
                0,
                6,
                7,
                actor.handle(),
                2,
                2,
                42
            ]
        );
        assert_eq!(
            inspect(&mut objects, &relations, 8, 0, range).unwrap(),
            [
                1,
                0,
                3,
                4,
                5,
                root.handle(),
                actor.handle(),
                3,
                6,
                0,
                3,
                7,
                actor.handle()
            ]
        );
        assert_eq!(
            inspect(&mut objects, &relations, 8, 1 << 12, 7).unwrap(),
            [1, 0, 1, 4, 7, actor.handle(), root.handle()]
        );
        assert_eq!(
            inspect(&mut objects, &relations, 0, 1 << 12, actor.handle()).unwrap(),
            [root.handle()]
        );
        assert_eq!(
            inspect(
                &mut objects,
                &relations,
                7,
                1,
                (2 << 32) | u64::from(u32::MAX)
            )
            .unwrap(),
            [1, 11, 0]
        );
    }

    #[test]
    fn scene_reports_empty_and_oversized_presentation_explicitly() {
        let mut store = Store::new(Limits {
            objects: 4,
            properties: 300,
            string_bytes: 1024,
        })
        .unwrap();
        let mut objects = store.scope().unwrap();
        let root = objects.create().unwrap();
        let mut relations = Relations::default();
        relations.declare(Cardinality::OneToMany).unwrap();
        assert_eq!(
            inspect(&mut objects, &relations, 6, 1 << 32, root.handle()).unwrap(),
            [1, 0, 1, 5, root.handle(), 0, 1, 0]
        );
        let list = objects
            .new_list(&vec![Value::Property(PropertyId(2)); 257])
            .unwrap();
        objects.set(root, PropertyId(1), Value::List(list)).unwrap();
        assert_eq!(
            inspect(&mut objects, &relations, 6, 1 << 32, root.handle()).unwrap(),
            [1, 5, 0]
        );
        assert_eq!(
            inspect(&mut objects, &relations, 7, 1, 4097 << 32).unwrap(),
            [1, 5, 0]
        );
    }
}

/// Pointer-free host save transport; see session::persistence.
pub extern "C" fn host_persistence(slot: u64, op: u32, value: u64) -> u64 {
    crate::session::persistence(slot, op, value).unwrap_or_else(u64::from)
}
