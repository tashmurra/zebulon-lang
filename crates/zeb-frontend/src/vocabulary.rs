//! Source vocabulary expands into owned static lists and native dictionary calls.
use super::{Id, Parser, Syntax, push};
use crate::Diagnostic;

fn copy(text: &str, byte: usize) -> Result<String, Diagnostic> {
    let mut value = String::new();
    value
        .try_reserve(text.len())
        .map_err(|_| Diagnostic::resource(byte))?;
    value.push_str(text);
    Ok(value)
}

impl Parser<'_> {
    fn vocabulary_word(&mut self, id: Id) -> Result<Id, Diagnostic> {
        let node = &self.nodes[id.0];
        let Syntax::String(literal) = &node.syntax else {
            unreachable!()
        };
        let mut text = String::new();
        text.try_reserve(literal.text.len())
            .map_err(|_| Diagnostic::resource(node.start))?;
        text.push_str(&literal.text);
        let literal = crate::strings::Literal {
            text,
            emitting: literal.emitting,
            triple: literal.triple,
        };
        self.node(Syntax::String(literal), node.start, node.end)
    }
    pub(super) fn lower_vocabulary(
        &mut self,
        objects: &mut [Id],
        dictionaries: &[Option<String>],
        vocabulary_values: &[Id],
        functions: &mut Vec<Id>,
    ) -> Result<Vec<Id>, Diagnostic> {
        if vocabulary_values.is_empty() {
            return Ok(Vec::new());
        }
        let mut definitions = Vec::new();
        for id in objects.iter() {
            let Syntax::Object {
                name,
                parents,
                properties,
                is_class,
                ..
            } = &self.nodes[id.0].syntax
            else {
                unreachable!()
            };
            let mut vocab = Vec::new();
            for (property, value) in properties {
                if vocabulary_values.contains(value) {
                    let Syntax::List(words) = &self.nodes[value.0].syntax else {
                        unreachable!()
                    };
                    let start = self.nodes[id.0].start;
                    let mut word_ids = Vec::new();
                    word_ids
                        .try_reserve(words.len())
                        .map_err(|_| Diagnostic::resource(start))?;
                    word_ids.extend_from_slice(words);
                    push(&mut vocab, (copy(property, start)?, word_ids), start)?;
                }
            }
            let start = self.nodes[id.0].start;
            let mut parent_names = Vec::new();
            for parent in parents {
                push(&mut parent_names, copy(parent, start)?, start)?;
            }
            push(
                &mut definitions,
                (copy(name, start)?, parent_names, *is_class, vocab),
                start,
            )?;
        }
        let mut registrations = Vec::new();
        for (index, id) in objects.iter_mut().enumerate() {
            let start = self.nodes[id.0].start;
            let end = self.nodes[id.0].end;
            let mut pending = Vec::new();
            push(&mut pending, index, start)?;
            let mut visited = Vec::new();
            let mut merged: Vec<(String, Vec<Id>)> = Vec::new();
            let mut multiple = false;
            while let Some(current) = pending.pop() {
                if visited.contains(&current) {
                    continue;
                }
                push(&mut visited, current, start)?;
                let (_, parents, _, vocab) = &definitions[current];
                multiple |= parents.len() > 1;
                for (property, words) in vocab {
                    let entry =
                        if let Some(entry) = merged.iter_mut().find(|(name, _)| name == property) {
                            entry
                        } else {
                            push(&mut merged, (copy(property, start)?, Vec::new()), start)?;
                            merged.last_mut().unwrap()
                        };
                    entry
                        .1
                        .try_reserve(words.len())
                        .map_err(|_| Diagnostic::resource(start))?;
                    entry.1.splice(0..0, words.iter().copied());
                }
                for parent in parents.iter().rev() {
                    if let Some(parent) = definitions.iter().position(|(name, ..)| name == parent) {
                        push(&mut pending, parent, start)?;
                    }
                }
            }
            if merged.is_empty() {
                continue;
            }
            if multiple {
                return Err(Diagnostic::new(
                    "dictionary-vocabulary-unavailable",
                    "multiple-inheritance vocabulary ordering is not implemented",
                    start,
                ));
            }
            let (name, _, is_class, _) = &definitions[index];
            for (property, words) in merged {
                if !is_class {
                    let dictionary = dictionaries[index].as_ref().ok_or_else(|| {
                        Diagnostic::new(
                            "sem-dictionary",
                            "vocabulary instance requires an active dictionary",
                            start,
                        )
                    })?;
                    for word in &words {
                        let receiver =
                            self.node(Syntax::Name(copy(dictionary, start)?), start, end)?;
                        let method = self.node(
                            Syntax::Property(receiver, "addWord".to_owned()),
                            start,
                            end,
                        )?;
                        let target = self.node(Syntax::Name(copy(name, start)?), start, end)?;
                        let key = self.node(
                            Syntax::PropertyAddress(copy(&property, start)?),
                            start,
                            end,
                        )?;
                        let mut args = Vec::new();
                        let word = self.vocabulary_word(*word)?;
                        for arg in [target, word, key] {
                            push(&mut args, arg, start)?;
                        }
                        let call = self.node(Syntax::Call(method, args), start, end)?;
                        let statement = self.node(Syntax::Expression(call), start, end)?;
                        push(&mut registrations, statement, start)?;
                    }
                }
                let mut literals = Vec::new();
                for word in words {
                    let literal = self.vocabulary_word(word)?;
                    push(&mut literals, literal, start)?;
                }
                let value = self.node(Syntax::List(literals), start, end)?;
                let receiver = self.node(Syntax::Name("self".to_owned()), start, end)?;
                let target = self.node(
                    Syntax::Property(receiver, copy(&property, start)?),
                    start,
                    end,
                )?;
                let assign = self.node(Syntax::Binary("=", target, value), start, end)?;
                let body = self.node(Syntax::Return(Some(assign)), start, end)?;
                let mut parameters = Vec::new();
                push(&mut parameters, "self".to_owned(), start)?;
                let function = self.node(
                    Syntax::Function {
                        returns_owned: false,
                        optional: 0,
                        rest: false,
                        name: format!("${name}.{property}"),
                        is_static: true,
                        parameters,
                        body,
                    },
                    start,
                    end,
                )?;
                push(functions, function, start)?;
                let Syntax::Object { properties, .. } = &mut self.nodes[id.0].syntax else {
                    unreachable!()
                };
                if let Some((_, old)) = properties.iter_mut().find(|(name, _)| name == &property) {
                    if !vocabulary_values.contains(old) {
                        return Err(Diagnostic::new(
                            "dictionary-vocabulary-unavailable",
                            "non-vocabulary override of inherited vocabulary is not implemented",
                            start,
                        ));
                    }
                    *old = function;
                } else {
                    push(properties, (property, function), start)?;
                }
            }
            let syntax = std::mem::replace(&mut self.nodes[id.0].syntax, Syntax::Empty);
            *id = self.node(syntax, start, end)?;
        }
        Ok(registrations)
    }
}

/// the parts of speech an adv3Lite vocab string's sections file under.
/// These are TADS dictionary property names, and the compiler uses them by name
/// for the same reason it knows `PreinitObject` by name : a tutorial's
/// spelling has to work.
pub(crate) const NOUN: &str = "noun";
pub(crate) const ADJECTIVE: &str = "adjective";

/// One section's worth of words, already assigned a part of speech.
#[derive(Debug)]
pub(crate) struct Sectioned {
    /// The short name, with a leading article removed. Empty when section one is.
    pub name: String,
    /// `(part of speech, word)` in the order the string wrote them.
    pub words: Vec<(&'static str, String)>,
}

/// An article at the head of the short name is a flag rather than a word, and
/// adv3Lite reads `some` and `the` as more than that. Only the skip is taken
/// here; what they additionally mean is library policy.
fn article(word: &str) -> bool {
    matches!(word, "a" | "an" | "some" | "the")
}

/// Split an adv3Lite vocab string into words with parts of speech.
///
/// `'oak desk; large oak wooden; table counter'` is short name; adjectives;
/// nouns. In the short name every word is an adjective except the last, which
/// is the noun — `initVocab` in `english.t` does the same, and that is the rule
/// that makes `'oak desk'` mean a desk rather than an oak.
///
/// The English rules this does **not** take are recorded in : a
/// preposition starting a sub-phrase, weak tokens, the pronoun section, and the
/// `massNoun`/`qualified` flags an article implies. Each needs a decision, and
/// each belongs to the library rather than to the compiler.
pub(crate) fn sections(text: &str, byte: usize) -> Result<Sectioned, Diagnostic> {
    let parts: Vec<&str> = text.split(';').map(str::trim).collect();
    if parts.len() > 3 {
        return Err(Diagnostic::new(
            "sem-vocabulary",
            "a vocab string has a short name, adjectives and nouns; the pronoun section is not implemented",
            byte,
        ));
    }
    let mut words: Vec<(&'static str, String)> = Vec::new();
    let mut name = String::new();
    if let Some(short) = parts.first() {
        let mut spelled: Vec<&str> = short.split_whitespace().collect();
        if spelled.first().is_some_and(|first| article(first)) {
            spelled.remove(0);
        }
        let mut rebuilt = String::new();
        for (at, word) in spelled.iter().enumerate() {
            if at > 0 {
                rebuilt
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(byte))?;
                rebuilt.push(' ');
            }
            rebuilt
                .try_reserve(word.len())
                .map_err(|_| Diagnostic::resource(byte))?;
            rebuilt.push_str(word);
            // Every word is an adjective except the last, which is the noun.
            let part = if at + 1 == spelled.len() {
                NOUN
            } else {
                ADJECTIVE
            };
            push(&mut words, (part, copy(word, byte)?), byte)?;
        }
        name = rebuilt;
    }
    for (at, part) in [ADJECTIVE, NOUN].into_iter().enumerate() {
        let Some(section) = parts.get(at + 1) else {
            continue;
        };
        for word in section.split_whitespace() {
            push(&mut words, (part, copy(word, byte)?), byte)?;
        }
    }
    if words.is_empty() {
        return Err(Diagnostic::new(
            "sem-vocabulary",
            "a vocab string names at least one word",
            byte,
        ));
    }
    Ok(Sectioned { name, words })
}

impl Parser<'_> {
    /// A fresh string literal node holding `word`, for words the compiler
    /// derived rather than read.
    pub(super) fn string_word(
        &mut self,
        word: &str,
        start: usize,
        end: usize,
    ) -> Result<Id, Diagnostic> {
        let literal = crate::strings::Literal {
            text: copy(word, start)?,
            emitting: false,
            triple: false,
        };
        self.node(Syntax::String(literal), start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str) -> Vec<(&'static str, String)> {
        sections(text, 0).unwrap().words
    }

    #[test]
    fn the_last_word_of_the_short_name_is_the_noun() {
        assert_eq!(
            parts("oak desk"),
            vec![(ADJECTIVE, "oak".to_owned()), (NOUN, "desk".to_owned())]
        );
    }

    #[test]
    fn a_single_word_short_name_is_a_noun() {
        assert_eq!(parts("desk"), vec![(NOUN, "desk".to_owned())]);
    }

    #[test]
    fn the_second_section_is_adjectives_and_the_third_is_nouns() {
        assert_eq!(
            parts("desk; large oak; table counter"),
            vec![
                (NOUN, "desk".to_owned()),
                (ADJECTIVE, "large".to_owned()),
                (ADJECTIVE, "oak".to_owned()),
                (NOUN, "table".to_owned()),
                (NOUN, "counter".to_owned()),
            ]
        );
    }

    #[test]
    fn a_leading_article_is_a_flag_not_a_word() {
        assert_eq!(parts("the desk"), vec![(NOUN, "desk".to_owned())]);
        assert_eq!(parts("a desk"), vec![(NOUN, "desk".to_owned())]);
        assert_eq!(sections("some water", 0).unwrap().name, "water");
    }

    #[test]
    fn an_empty_section_contributes_nothing() {
        assert_eq!(
            parts("desk;; counter"),
            vec![(NOUN, "desk".to_owned()), (NOUN, "counter".to_owned())]
        );
    }

    #[test]
    fn the_name_is_the_short_name_without_its_article() {
        assert_eq!(sections("the oak desk; big", 0).unwrap().name, "oak desk");
    }

    #[test]
    fn a_pronoun_section_is_refused_rather_than_ignored() {
        let error = sections("desk; big; table; it", 0).unwrap_err();
        assert_eq!(error.code, "sem-vocabulary");
        assert!(error.message.contains("pronoun"), "{error:?}");
    }

    #[test]
    fn a_string_with_no_words_is_refused() {
        assert!(sections("  ", 0).is_err());
        assert!(sections(";;", 0).is_err());
    }
}
