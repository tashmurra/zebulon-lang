//! Bounded Earley recognition and ordered match-tree enumeration.
//! Trees are call-scoped Rust values; the native adapter builds explicitly owned
//! game match objects from them. Ranking, badness and dynamic rules are separate.
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    Production(usize),
    Terminal(u32),
    /// Consumes every remaining token, including none, as a source `*` does.
    Star,
}
#[derive(Debug)]
pub struct Rule {
    pub production: usize,
    pub symbols: Vec<Symbol>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidGrammar,
    ResourceLimit,
}
/// `states` and `work` bound the chart. `trees` bounds match nodes and `depth`
/// bounds nested derivations, so ambiguity explosions report ResourceLimit.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub states: usize,
    pub work: usize,
    pub trees: usize,
    pub depth: usize,
}

/// Compiler-generated immutable rule tables. Slots and property IDs use the
/// same private numbering as generated object initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticTerm {
    Production(u32),
    Literal(&'static str),
    Token(u32),
    Speech(u32),
    Star,
    /// A word a vocabulary relation names entities by. It matches like
    /// `Speech`, but its capture binds the entities rather than the word.
    Vocabulary(u32),
}
#[derive(Debug)]
pub struct StaticElement {
    pub term: StaticTerm,
    pub capture: Option<u32>,
}
#[derive(Debug)]
pub struct StaticRule {
    pub production: u32,
    /// None marks an anonymous group; its captures belong to the enclosing match.
    pub match_slot: Option<u32>,
    pub badness: i32,
    pub elements: &'static [StaticElement],
}
#[derive(Debug)]
pub struct StaticGrammar {
    /// Static object slot for each named production; None for anonymous groups.
    pub productions: &'static [Option<u32>],
    pub rules: &'static [StaticRule],
    /// the property a match reports its rule's badness on, when the
    /// program declares one. Without it a match says nothing about how good a
    /// reading it is, which is what it did before.
    pub badness: Option<u32>,
    pub first_token_index: Option<u32>,
    pub last_token_index: Option<u32>,
    pub token_list: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    Tree(usize),
    Token(usize),
    Star { start: usize, end: usize },
}
#[derive(Debug)]
pub struct Node {
    pub rule: usize,
    pub start: usize,
    pub end: usize,
    pub children: Vec<Child>,
}
/// Roots follow rule source order; within a rule, earlier children with shorter
/// spans come first. Shared subtrees appear once in `nodes`.
#[derive(Debug)]
pub struct Forest {
    pub nodes: Vec<Node>,
    pub roots: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct State {
    rule: usize,
    dot: usize,
    origin: usize,
}
struct Budget {
    states: usize,
    work: usize,
}
impl Budget {
    fn step(&mut self) -> Result<(), Error> {
        self.work = self.work.checked_sub(1).ok_or(Error::ResourceLimit)?;
        Ok(())
    }
    fn insert(&mut self, chart: &mut Vec<State>, state: State) -> Result<(), Error> {
        for old in chart.iter() {
            self.step()?;
            if *old == state {
                return Ok(());
            }
        }
        self.states = self.states.checked_sub(1).ok_or(Error::ResourceLimit)?;
        push(chart, state)
    }
}
fn push<T>(items: &mut Vec<T>, item: T) -> Result<(), Error> {
    items.try_reserve(1).map_err(|_| Error::ResourceLimit)?;
    items.push(item);
    Ok(())
}
fn copy<T: Copy>(items: &[T]) -> Result<Vec<T>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(items.len())
        .map_err(|_| Error::ResourceLimit)?;
    out.extend_from_slice(items);
    Ok(out)
}

/// Undefined productions are legal empty languages, as for forward declarations.
fn build_charts<E: From<Error>, F: FnMut(u32, usize) -> Result<bool, E>>(
    rules: &[Rule],
    productions: usize,
    start: usize,
    len: usize,
    budget: &mut Budget,
    matches: &mut F,
) -> Result<Vec<Vec<State>>, E> {
    if start >= productions
        || rules.iter().any(|r| {
            r.production >= productions
                || r.symbols
                    .iter()
                    .any(|s| matches!(s, Symbol::Production(p) if *p >= productions))
        })
    {
        return Err(Error::InvalidGrammar.into());
    }
    let count = len.checked_add(1).ok_or(Error::ResourceLimit)?;
    if count > budget.states {
        return Err(Error::ResourceLimit.into());
    }
    let mut charts = Vec::new();
    charts
        .try_reserve_exact(count)
        .map_err(|_| Error::ResourceLimit)?;
    charts.resize_with(count, Vec::new);
    for (rule, definition) in rules.iter().enumerate() {
        budget.step()?;
        if definition.production == start {
            budget.insert(
                &mut charts[0],
                State {
                    rule,
                    dot: 0,
                    origin: 0,
                },
            )?;
        }
    }
    for position in 0..count {
        let mut cursor = 0;
        while cursor < charts[position].len() {
            budget.step()?;
            let state = charts[position][cursor];
            cursor += 1;
            let rule = &rules[state.rule];
            let advanced = State {
                dot: state.dot + 1,
                ..state
            };
            match rule.symbols.get(state.dot) {
                Some(Symbol::Terminal(terminal)) => {
                    if position < len && matches(*terminal, position)? {
                        budget.insert(&mut charts[position + 1], advanced)?;
                    }
                }
                Some(Symbol::Star) => budget.insert(&mut charts[len], advanced)?,
                Some(Symbol::Production(production)) => {
                    for (index, candidate) in rules.iter().enumerate() {
                        budget.step()?;
                        if candidate.production == *production {
                            budget.insert(
                                &mut charts[position],
                                State {
                                    rule: index,
                                    dot: 0,
                                    origin: position,
                                },
                            )?;
                        }
                    }
                    // Nullable completions can have been processed before this waiter.
                    let completed_count = charts[position].len();
                    for index in 0..completed_count {
                        budget.step()?;
                        let completed = charts[position][index];
                        let definition = &rules[completed.rule];
                        if completed.origin == position
                            && definition.production == *production
                            && completed.dot == definition.symbols.len()
                        {
                            budget.insert(&mut charts[position], advanced)?;
                        }
                    }
                }
                None => {
                    let waiting_count = charts[state.origin].len();
                    for index in 0..waiting_count {
                        budget.step()?;
                        let waiting = charts[state.origin][index];
                        if rules[waiting.rule].symbols.get(waiting.dot)
                            == Some(&Symbol::Production(rule.production))
                        {
                            budget.insert(
                                &mut charts[position],
                                State {
                                    dot: waiting.dot + 1,
                                    ..waiting
                                },
                            )?;
                        }
                    }
                }
            }
        }
    }
    Ok(charts)
}

/// Token positions and states belong exclusively to this call; no game object is retained.
pub fn recognize(
    rules: &[Rule],
    productions: usize,
    start: usize,
    tokens: &[u32],
    limits: Limits,
) -> Result<bool, Error> {
    let mut budget = Budget {
        states: limits.states,
        work: limits.work,
    };
    let charts = build_charts(
        rules,
        productions,
        start,
        tokens.len(),
        &mut budget,
        &mut |terminal, position| Ok::<_, Error>(tokens.get(position) == Some(&terminal)),
    )?;
    for state in &charts[tokens.len()] {
        budget.step()?;
        let rule = &rules[state.rule];
        if state.origin == 0 && rule.production == start && state.dot == rule.symbols.len() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Enumerate every complete match tree. `matches` decides terminal IDs at token
/// positions and may report its own error type. Unit/nullable cycles are cut at
/// their first repetition, so the result is finite.
pub fn parse<E: From<Error>, F: FnMut(u32, usize) -> Result<bool, E>>(
    rules: &[Rule],
    productions: usize,
    start: usize,
    len: usize,
    limits: Limits,
    matches: &mut F,
) -> Result<Forest, E> {
    let mut budget = Budget {
        states: limits.states,
        work: limits.work,
    };
    let charts = build_charts(rules, productions, start, len, &mut budget, matches)?;
    let mut complete = HashSet::new();
    let mut ends: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    // Chart positions ascend, so every end list is ordered shortest first.
    for (end, chart) in charts.iter().enumerate() {
        for state in chart {
            budget.step()?;
            let rule = &rules[state.rule];
            if state.dot != rule.symbols.len() {
                continue;
            }
            complete.try_reserve(1).map_err(|_| Error::ResourceLimit)?;
            complete.insert((state.rule, state.origin, end));
            ends.try_reserve(1).map_err(|_| Error::ResourceLimit)?;
            let list = ends.entry((rule.production, state.origin)).or_default();
            if list.last() != Some(&end) {
                push(list, end)?;
            }
        }
    }
    let mut deriver = Deriver {
        rules,
        len,
        limits,
        budget,
        complete,
        ends,
        memo: HashMap::new(),
        active: HashSet::new(),
        nodes: Vec::new(),
        depth: 0,
        matches,
        error: PhantomData,
    };
    let roots = deriver.derive(start, 0, len)?;
    Ok(Forest {
        nodes: deriver.nodes,
        roots,
    })
}

type Span = (usize, usize, usize);
struct Deriver<'a, F, E> {
    rules: &'a [Rule],
    len: usize,
    limits: Limits,
    budget: Budget,
    complete: HashSet<Span>,
    ends: HashMap<(usize, usize), Vec<usize>>,
    memo: HashMap<Span, Vec<usize>>,
    active: HashSet<Span>,
    nodes: Vec<Node>,
    depth: usize,
    matches: &'a mut F,
    error: PhantomData<E>,
}
fn with(children: &[Child], child: Child) -> Result<Vec<Child>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(children.len() + 1)
        .map_err(|_| Error::ResourceLimit)?;
    out.extend_from_slice(children);
    out.push(child);
    Ok(out)
}
impl<E: From<Error>, F: FnMut(u32, usize) -> Result<bool, E>> Deriver<'_, F, E> {
    /// Nesting is bounded by `limits.depth`; each frame is small and fixed-size.
    fn derive(&mut self, production: usize, start: usize, end: usize) -> Result<Vec<usize>, E> {
        let key = (production, start, end);
        if let Some(found) = self.memo.get(&key) {
            return Ok(copy(found)?);
        }
        if self.active.contains(&key) {
            return Ok(Vec::new());
        }
        self.depth = self
            .depth
            .checked_add(1)
            .filter(|depth| *depth <= self.limits.depth)
            .ok_or(Error::ResourceLimit)?;
        self.active
            .try_reserve(1)
            .map_err(|_| Error::ResourceLimit)?;
        self.active.insert(key);
        let result = self.alternatives(production, start, end);
        self.active.remove(&key);
        self.depth -= 1;
        let found = result?;
        self.memo.try_reserve(1).map_err(|_| Error::ResourceLimit)?;
        self.memo.insert(key, copy(&found)?);
        Ok(found)
    }

    fn alternatives(
        &mut self,
        production: usize,
        start: usize,
        end: usize,
    ) -> Result<Vec<usize>, E> {
        let rules = self.rules;
        let mut found = Vec::new();
        for (index, rule) in rules.iter().enumerate() {
            self.budget.step()?;
            if rule.production != production || !self.complete.contains(&(index, start, end)) {
                continue;
            }
            let mut stack: Vec<(usize, usize, Vec<Child>)> = Vec::new();
            push(&mut stack, (0, start, Vec::new()))?;
            while let Some((symbol, position, children)) = stack.pop() {
                self.budget.step()?;
                let Some(next) = rule.symbols.get(symbol) else {
                    if position == end {
                        if self.nodes.len() >= self.limits.trees {
                            return Err(Error::ResourceLimit.into());
                        }
                        push(
                            &mut self.nodes,
                            Node {
                                rule: index,
                                start,
                                end,
                                children,
                            },
                        )?;
                        push(&mut found, self.nodes.len() - 1)?;
                    }
                    continue;
                };
                match *next {
                    Symbol::Terminal(terminal) => {
                        if position < end && (self.matches)(terminal, position)? {
                            let children = with(&children, Child::Token(position))?;
                            push(&mut stack, (symbol + 1, position + 1, children))?;
                        }
                    }
                    Symbol::Star => {
                        if end == self.len {
                            let children = with(
                                &children,
                                Child::Star {
                                    start: position,
                                    end,
                                },
                            )?;
                            push(&mut stack, (symbol + 1, end, children))?;
                        }
                    }
                    Symbol::Production(child) => {
                        let candidates = match self.ends.get(&(child, position)) {
                            Some(list) => copy(list)?,
                            None => Vec::new(),
                        };
                        let mut branches = Vec::new();
                        for child_end in candidates.into_iter().filter(|e| *e <= end) {
                            for tree in self.derive(child, position, child_end)? {
                                let children = with(&children, Child::Tree(tree))?;
                                push(&mut branches, (symbol + 1, child_end, children))?;
                            }
                        }
                        // Reverse onto the stack so the shortest first span is completed first.
                        while let Some(branch) = branches.pop() {
                            push(&mut stack, branch)?;
                        }
                    }
                }
            }
        }
        Ok(found)
    }
}
