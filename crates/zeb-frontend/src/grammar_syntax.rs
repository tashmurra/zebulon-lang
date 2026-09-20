//! Grammar-rule metadata. Match-object bodies use the ordinary object parser.
use super::{Diagnostic, Kind, Parser, push};

#[derive(Debug, PartialEq, Eq)]
pub enum GrammarItem {
    Symbol(String),
    Literal(String),
    Capture(String),
    Open,
    Close,
    Alternative,
    Wildcard,
    Badness(i32),
}

#[derive(Debug)]
pub struct GrammarRule {
    pub production: String,
    pub tag: Option<String>,
    pub match_name: Option<String>,
    pub items: Vec<GrammarItem>,
}

impl Parser<'_> {
    pub(super) fn grammar_rule(&mut self) -> Result<GrammarRule, Diagnostic> {
        let start = self.byte();
        let production = self.name()?;
        if self.take(";") {
            return Ok(GrammarRule {
                production,
                tag: None,
                match_name: None,
                items: Vec::new(),
            });
        }
        let tag = if self.take("(") {
            let tag = self.name()?;
            self.expect(")")?;
            Some(tag)
        } else {
            None
        };
        self.expect(":")?;
        let mut items = Vec::new();
        let mut depth = 0usize;
        let mut capture_allowed = false;
        loop {
            if self.take(":") {
                if depth != 0 {
                    return Err(self.error("unclosed grammar group"));
                }
                break;
            }
            let item = if self.take("(") {
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| Diagnostic::resource(start))?;
                capture_allowed = false;
                GrammarItem::Open
            } else if self.take(")") {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| self.error("unmatched grammar group"))?;
                capture_allowed = false;
                GrammarItem::Close
            } else if self.take("|") {
                capture_allowed = false;
                GrammarItem::Alternative
            } else if self.take("->") {
                if !capture_allowed {
                    return Err(self.error("grammar capture requires a token or production"));
                }
                capture_allowed = false;
                GrammarItem::Capture(self.name()?)
            } else if self.take("[") {
                if !self.take_word("badness") {
                    return Err(self.error("expected a grammar badness annotation"));
                }
                let token = self.token();
                let Kind::Integer(10) = token.kind else {
                    return Err(self.error("badness requires a decimal integer"));
                };
                let value = self
                    .text(token)?
                    .parse::<i32>()
                    .map_err(|_| self.error("badness is out of range"))?;
                self.cursor += 1;
                self.expect("]")?;
                capture_allowed = false;
                GrammarItem::Badness(value)
            } else if self.take("*") {
                capture_allowed = true;
                GrammarItem::Wildcard
            } else if matches!(
                self.token().kind,
                Kind::Quoted {
                    emitting: false,
                    ..
                }
            ) {
                capture_allowed = true;
                GrammarItem::Literal(self.declaration_string()?)
            } else {
                capture_allowed = true;
                // `vocab.noun` names one part of speech of a labelled
                // vocabulary relation. A plain symbol is unchanged.
                let name = self.name()?;
                if self.take(".") {
                    let part = self.name()?;
                    let mut qualified = String::new();
                    qualified
                        .try_reserve(name.len() + part.len() + 1)
                        .map_err(|_| Diagnostic::resource(start))?;
                    qualified.push_str(&name);
                    qualified.push('.');
                    qualified.push_str(&part);
                    GrammarItem::Symbol(qualified)
                } else {
                    GrammarItem::Symbol(name)
                }
            };
            push(&mut items, item, start)?;
        }
        let match_name = Some(if let Some(tag) = &tag {
            format!("{production}({tag})")
        } else {
            format!("$grammar{}", self.nodes.len())
        });
        Ok(GrammarRule {
            production,
            tag,
            match_name,
            items,
        })
    }
}
