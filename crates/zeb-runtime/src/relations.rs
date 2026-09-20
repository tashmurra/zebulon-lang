//! Relations between entities, stored as tables rather than as pointers inside
//! objects. A room containing an item is a row here, not ownership,
//! so a world graph may be cyclic without any ownership question arising.
use crate::objects::{Error, ObjectRef};
use std::collections::HashMap;

/// How many partners each side may have. Storage follows from this: a side
/// limited to one partner keeps a single entry, so `one_to_many` answers in
/// both directions without a scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    OneToOne,
    OneToMany,
    ManyToMany,
}
impl Cardinality {
    pub fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(Self::OneToOne),
            1 => Some(Self::OneToMany),
            2 => Some(Self::ManyToMany),
            _ => None,
        }
    }
    /// Whether the left side of a pair may name more than one partner.
    fn left_is_many(self) -> bool {
        matches!(self, Self::OneToMany | Self::ManyToMany)
    }
    fn right_is_many(self) -> bool {
        matches!(self, Self::ManyToMany)
    }
}

/// One change to one relation. Replaying these inverted is what undo needs, so
/// the journal retains the information needed to reverse a cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delta {
    pub relation: usize,
    pub left: ObjectRef,
    pub right: ObjectRef,
    pub added: bool,
    forward_position: usize,
    reverse_position: usize,
}

#[derive(Default)]
struct Table {
    cardinality: Option<Cardinality>,
    forward: HashMap<u64, Vec<ObjectRef>>,
    reverse: HashMap<u64, Vec<ObjectRef>>,
    /// The end of the chain each entity reaches by walking left, kept until any
    /// change to this relation makes it stale.
    outermost: HashMap<u64, ObjectRef>,
}

#[derive(Default)]
pub struct Relations {
    tables: Vec<Table>,
    /// Which table holds one label's rows, for a relation with a third column.
    /// Labels are sparse, so they are mapped rather than indexed.
    labelled: HashMap<(usize, u32), usize>,
    journal: Vec<Delta>,
    /// Whether changes are being recorded. Off until the first cycle begins,
    /// so world construction cannot be reverted.
    journalling: bool,
}

fn reserve<T>(values: &mut Vec<T>, count: usize) -> Result<(), Error> {
    values.try_reserve(count).map_err(|_| Error::Allocation)
}

impl Relations {
    /// Make sure the table a call site names exists. Relations are numbered by
    /// declaration order at compile time, so the table is created on first use
    /// with the cardinality the declaration gave.
    /// The table one label of a labelled relation uses, created on first use.
    /// A labelled relation is a family of ordinary tables, one per label, so
    /// every query it answers is the same query an unlabelled one answers.
    pub fn ensure_labelled(
        &mut self,
        relation: usize,
        label: u32,
        cardinality: Cardinality,
    ) -> Result<usize, Error> {
        if let Some(index) = self.labelled.get(&(relation, label)) {
            let index = *index;
            self.ensure(index, cardinality)?;
            return Ok(index);
        }
        let index = self.tables.len();
        self.ensure(index, cardinality)?;
        self.labelled
            .try_reserve(1)
            .map_err(|_| Error::Allocation)?;
        self.labelled.insert((relation, label), index);
        Ok(index)
    }

    /// Every table one labelled relation has made so far, in label order so a
    /// query across the family answers the same way twice.
    pub fn tables_of(&self, relation: usize) -> Vec<(u32, usize)> {
        let mut found: Vec<(u32, usize)> = self
            .labelled
            .iter()
            .filter(|((declared, _), _)| *declared == relation)
            .map(|((_, label), index)| (*label, *index))
            .collect();
        found.sort_unstable();
        found
    }

    pub fn ensure(&mut self, index: usize, cardinality: Cardinality) -> Result<(), Error> {
        if index >= self.tables.len() {
            let extra = index + 1 - self.tables.len();
            reserve(&mut self.tables, extra)?;
            for _ in 0..extra {
                self.tables.push(Table::default());
            }
        }
        let table = &mut self.tables[index];
        match table.cardinality {
            Some(existing) if existing != cardinality => Err(Error::WrongType),
            Some(_) => Ok(()),
            None => {
                table.cardinality = Some(cardinality);
                Ok(())
            }
        }
    }

    pub fn declare(&mut self, cardinality: Cardinality) -> Result<usize, Error> {
        reserve(&mut self.tables, 1)?;
        self.tables.push(Table {
            cardinality: Some(cardinality),
            ..Table::default()
        });
        Ok(self.tables.len() - 1)
    }

    fn table(&self, relation: usize) -> Result<&Table, Error> {
        self.tables
            .get(relation)
            .filter(|table| table.cardinality.is_some())
            .ok_or(Error::InvalidArgument)
    }

    fn cardinality(&self, relation: usize) -> Result<Cardinality, Error> {
        self.table(relation)?
            .cardinality
            .ok_or(Error::InvalidArgument)
    }

    pub fn clear_caches(&mut self, relation: usize) {
        if let Some(table) = self.tables.get_mut(relation) {
            table.outermost = HashMap::new();
        }
    }

    /// Relate `left` to `right`, dropping whatever pairing the cardinality
    /// cannot keep. Returns the pairs removed to make room, then the addition.
    pub fn set(&mut self, relation: usize, left: ObjectRef, right: ObjectRef) -> Result<(), Error> {
        let cardinality = self.cardinality(relation)?;
        if self.contains(relation, left, right)? {
            return Ok(());
        }
        // Single-partner sides cannot require an allocating temporary copy.
        let old_left = if cardinality.right_is_many() {
            None
        } else {
            self.table(relation)?
                .reverse
                .get(&right.handle())
                .and_then(|v| v.first())
                .copied()
        };
        let old_right = if cardinality.left_is_many() {
            None
        } else {
            self.table(relation)?
                .forward
                .get(&left.handle())
                .and_then(|v| v.first())
                .copied()
        };
        if self.journalling {
            if self.journal.len().saturating_add(3) > 1_000_000 {
                return Err(Error::ResourceLimit);
            }
            reserve(&mut self.journal, 3)?;
        }
        self.prepare_insert(relation, left, right)?;
        if let Some(previous) = old_left {
            self.unset(relation, previous, right)?;
        }
        if let Some(previous) = old_right {
            self.unset(relation, left, previous)?;
        }
        self.insert(relation, left, right)?;
        if self.journalling {
            self.journal.push(Delta {
                relation,
                left,
                right,
                added: true,
                forward_position: 0,
                reverse_position: 0,
            });
        }
        Ok(())
    }

    pub fn unset(
        &mut self,
        relation: usize,
        left: ObjectRef,
        right: ObjectRef,
    ) -> Result<bool, Error> {
        if !self.contains(relation, left, right)? {
            return Ok(false);
        }
        if self.journalling {
            if self.journal.len() >= 1_000_000 {
                return Err(Error::ResourceLimit);
            }
            reserve(&mut self.journal, 1)?;
        }
        let table = self
            .tables
            .get_mut(relation)
            .ok_or(Error::InvalidArgument)?;
        let forward_position = table.forward[&left.handle()]
            .iter()
            .position(|p| *p == right)
            .ok_or(Error::InvalidArgument)?;
        let reverse_position = table.reverse[&right.handle()]
            .iter()
            .position(|p| *p == left)
            .ok_or(Error::InvalidArgument)?;
        if let Some(forward) = table.forward.get_mut(&left.handle()) {
            forward.retain(|value| *value != right);
        }
        if let Some(reverse) = table.reverse.get_mut(&right.handle()) {
            reverse.retain(|value| *value != left);
        }
        table.outermost = HashMap::new();
        if self.journalling {
            self.journal.push(Delta {
                relation,
                left,
                right,
                added: false,
                forward_position,
                reverse_position,
            });
        }
        Ok(true)
    }

    pub fn contains(
        &self,
        relation: usize,
        left: ObjectRef,
        right: ObjectRef,
    ) -> Result<bool, Error> {
        Ok(self
            .table(relation)?
            .forward
            .get(&left.handle())
            .is_some_and(|partners| partners.contains(&right)))
    }

    /// Everything on the other side of `key`. `reversed` reads the relation the
    /// other way, which is what its reverse name does.
    pub fn all(
        &self,
        relation: usize,
        key: ObjectRef,
        reversed: bool,
    ) -> Result<&[ObjectRef], Error> {
        let table = self.table(relation)?;
        let side = if reversed {
            &table.reverse
        } else {
            &table.forward
        };
        Ok(side.get(&key.handle()).map_or(&[][..], Vec::as_slice))
    }

    /// Everything on the other side of `key` in one table of a labelled family.
    ///
    /// Read-only: a label whose table has never been written answers nothing,
    /// rather than creating the table the way a call site's `ensure_labelled`
    /// would. An inspection may not change the world it is looking at.
    pub fn all_labelled(
        &self,
        relation: usize,
        key: ObjectRef,
        reversed: bool,
        label: u32,
    ) -> Result<&[ObjectRef], Error> {
        let Some(index) = self.labelled.get(&(relation, label)) else {
            return Ok(&[]);
        };
        self.all(*index, key, reversed)
    }

    /// The single partner, where the cardinality allows only one. Reading the
    /// many side this way is a type error rather than an arbitrary pick.
    pub fn get(
        &self,
        relation: usize,
        key: ObjectRef,
        reversed: bool,
    ) -> Result<Option<ObjectRef>, Error> {
        let cardinality = self.cardinality(relation)?;
        let many = if reversed {
            cardinality.right_is_many()
        } else {
            cardinality.left_is_many()
        };
        if many {
            return Err(Error::WrongType);
        }
        Ok(self.all(relation, key, reversed)?.first().copied())
    }

    /// Walk left from `entity`: its container, that container's container, and
    /// so on. A relation whose left side is unique gives one chain. Under a
    /// reverse name the walk runs the other way, because that is what reading the
    /// table right to left means.
    pub fn ancestors(
        &self,
        relation: usize,
        entity: ObjectRef,
        reversed: bool,
    ) -> Result<Vec<ObjectRef>, Error> {
        let mut chain = Vec::new();
        let mut current = entity;
        while let Some(next) = self.all(relation, current, !reversed)?.first().copied() {
            if chain.contains(&next) || next == entity {
                break;
            }
            reserve(&mut chain, 1)?;
            chain.push(next);
            current = next;
        }
        Ok(chain)
    }

    /// Everything reachable by walking right, depth first. Under a reverse name
    /// the walk runs the other way.
    pub fn descendants(
        &self,
        relation: usize,
        entity: ObjectRef,
        reversed: bool,
    ) -> Result<Vec<ObjectRef>, Error> {
        let mut found = Vec::new();
        let mut pending: Vec<ObjectRef> = Vec::new();
        reserve(&mut pending, 1)?;
        pending.push(entity);
        while let Some(current) = pending.pop() {
            for partner in self.all(relation, current, reversed)? {
                if found.contains(partner) || *partner == entity {
                    continue;
                }
                reserve(&mut found, 1)?;
                found.push(*partner);
                reserve(&mut pending, 1)?;
                pending.push(*partner);
            }
        }
        Ok(found)
    }

    /// The far end of the ancestor chain, cached until the relation changes.
    /// A reverse name is refused: walking the other way reaches the far end of a
    /// chain that need not be one chain, so there is no single answer to cache.
    pub fn outermost(
        &mut self,
        relation: usize,
        entity: ObjectRef,
        reversed: bool,
    ) -> Result<ObjectRef, Error> {
        if reversed {
            return Err(Error::WrongType);
        }
        if let Some(cached) = self.table(relation)?.outermost.get(&entity.handle()) {
            return Ok(*cached);
        }
        let root = self
            .ancestors(relation, entity, false)?
            .last()
            .copied()
            .unwrap_or(entity);
        let table = self
            .tables
            .get_mut(relation)
            .ok_or(Error::InvalidArgument)?;
        table
            .outermost
            .try_reserve(1)
            .map_err(|_| Error::Allocation)?;
        table.outermost.insert(entity.handle(), root);
        Ok(root)
    }

    pub fn journal(&self) -> &[Delta] {
        &self.journal
    }

    /// Retract every row naming `entity`, in every table, journalling each one
    /// so a failed turn puts them back. A despawned entity leaves no
    /// row behind that would answer for it.
    pub fn forget(&mut self, entity: ObjectRef) -> Result<usize, Error> {
        let mut pairs: Vec<(usize, ObjectRef, ObjectRef)> = Vec::new();
        for relation in 0..self.tables.len() {
            let Some(table) = self.tables.get(relation) else {
                continue;
            };
            let key = entity.handle();
            let rights = table.forward.get(&key).cloned().unwrap_or_default();
            let lefts = table.reverse.get(&key).cloned().unwrap_or_default();
            reserve(&mut pairs, rights.len() + lefts.len())?;
            for right in rights {
                pairs.push((relation, entity, right));
            }
            for left in lefts {
                pairs.push((relation, left, entity));
            }
        }
        let mut removed = 0;
        for (relation, left, right) in pairs {
            if self.unset(relation, left, right)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Record changes, or stop recording.
    pub fn set_journalling(&mut self, on: bool) {
        self.journalling = on;
    }

    /// Take the cycle's changes, leaving the journal empty.
    pub fn take_journal(&mut self) -> Vec<Delta> {
        std::mem::take(&mut self.journal)
    }

    /// Put each change back as it was, newest first, without recording the
    /// reversal. A row a cardinality change displaced is journalled
    /// too, so replaying backwards restores it. Answers how many rows moved.
    pub fn revert(&mut self, deltas: &[Delta]) -> usize {
        let recording = self.journalling;
        self.journalling = false;
        let mut reverted = 0;
        for delta in deltas.iter().rev() {
            let restored = if delta.added {
                self.unset(delta.relation, delta.left, delta.right)
                    .map(|removed| removed as usize)
            } else {
                // Put the row back directly: going through `set` would displace
                // whatever the cardinality now holds, and the deltas that
                // displaced it are still to come.
                self.insert(delta.relation, delta.left, delta.right)
                    .map(|added| added as usize)
            };
            if !delta.added && restored == Ok(1) {
                let table = &mut self.tables[delta.relation];
                for (side, key, position) in [
                    (
                        &mut table.forward,
                        delta.left.handle(),
                        delta.forward_position,
                    ),
                    (
                        &mut table.reverse,
                        delta.right.handle(),
                        delta.reverse_position,
                    ),
                ] {
                    let partners = side.get_mut(&key).expect("restored row");
                    let at = position.min(partners.len() - 1);
                    partners[at..].rotate_right(1);
                }
            }
            reverted += restored.unwrap_or(0);
        }
        self.journalling = recording;
        reverted
    }

    /// Admit both sides before changing either; empty reserved entries are not rows.
    fn prepare_insert(
        &mut self,
        relation: usize,
        left: ObjectRef,
        right: ObjectRef,
    ) -> Result<(), Error> {
        let table = self
            .tables
            .get_mut(relation)
            .ok_or(Error::InvalidArgument)?;
        if !table.forward.contains_key(&left.handle()) {
            table
                .forward
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
        }
        if !table.reverse.contains_key(&right.handle()) {
            table
                .reverse
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
        }
        reserve(table.forward.entry(left.handle()).or_default(), 1)?;
        reserve(table.reverse.entry(right.handle()).or_default(), 1)
    }
    /// Add one row with no displacement and no journal entry.
    fn insert(
        &mut self,
        relation: usize,
        left: ObjectRef,
        right: ObjectRef,
    ) -> Result<bool, Error> {
        if self.contains(relation, left, right)? {
            return Ok(false);
        }
        self.prepare_insert(relation, left, right)?;
        let table = &mut self.tables[relation];
        table
            .forward
            .get_mut(&left.handle())
            .expect("reserved forward")
            .push(right);
        table
            .reverse
            .get_mut(&right.handle())
            .expect("reserved reverse")
            .push(left);
        table.outermost.clear();
        Ok(true)
    }

    pub fn clear_journal(&mut self) {
        self.journal = Vec::new();
    }

    pub fn reset(&mut self) {
        self.journalling = false;
        self.tables = Vec::new();
        self.labelled = HashMap::new();
        self.journal = Vec::new();
    }
}

impl Relations {
    pub(crate) fn write_world(
        &self,
        store: &crate::objects::Store,
        w: &mut crate::save::Writer,
    ) -> Result<(), Error> {
        if !self.journal.is_empty() {
            return Err(Error::InvalidArgument);
        }
        w.count(self.tables.len())?;
        for table in &self.tables {
            let code = match table.cardinality {
                None => 0,
                Some(Cardinality::OneToOne) => 1,
                Some(Cardinality::OneToMany) => 2,
                Some(Cardinality::ManyToMany) => 3,
            };
            w.word(code)?;
            for side in [&table.forward, &table.reverse] {
                let mut keys = Vec::new();
                keys.try_reserve(side.len())
                    .map_err(|_| Error::Allocation)?;
                keys.extend(side.keys().copied().filter(|h| {
                    store
                        .saved_reference(h & 0xffff_ffff)
                        .is_some_and(|r| r.handle() == *h)
                }));
                keys.sort_unstable_by_key(|h| h & 0xffff_ffff);
                w.count(keys.len())?;
                for h in keys {
                    w.word(h & 0xffff_ffff)?;
                    let partners = &side[&h];
                    let valid = |r: &&ObjectRef| {
                        store.saved_reference(r.handle() & 0xffff_ffff) == Some(**r)
                    };
                    w.count(partners.iter().filter(valid).count())?;
                    for p in partners.iter().filter(valid) {
                        w.word(p.handle() & 0xffff_ffff)?;
                    }
                }
            }
        }
        let mut labelled = Vec::new();
        labelled
            .try_reserve(self.labelled.len())
            .map_err(|_| Error::Allocation)?;
        labelled.extend(self.labelled.iter());
        labelled.sort_unstable_by_key(|(k, _)| **k);
        w.count(labelled.len())?;
        for ((base, label), table) in labelled {
            w.count(*base)?;
            w.word(u64::from(*label))?;
            w.count(*table)?;
        }
        Ok(())
    }
    pub(crate) fn read_world(
        store: &crate::objects::Store,
        r: &mut crate::save::Reader<'_>,
        limit: usize,
    ) -> Result<Self, Error> {
        let mut result = Self::default();
        let count = r.count(limit)?;
        result
            .tables
            .try_reserve(count)
            .map_err(|_| Error::Allocation)?;
        let mut rows = 0usize;
        for _ in 0..count {
            let cardinality = match r.word()? {
                0 => None,
                1 => Some(Cardinality::OneToOne),
                2 => Some(Cardinality::OneToMany),
                3 => Some(Cardinality::ManyToMany),
                _ => return Err(Error::InvalidArgument),
            };
            let mut table = Table {
                cardinality,
                ..Table::default()
            };
            for (reverse, side) in [(false, &mut table.forward), (true, &mut table.reverse)] {
                let n = r.count(limit)?;
                side.try_reserve(n).map_err(|_| Error::Allocation)?;
                for _ in 0..n {
                    let key = store
                        .saved_reference(r.word()?)
                        .ok_or(Error::InvalidArgument)?;
                    let n = r.count(limit)?;
                    rows = rows.checked_add(n).ok_or(Error::ResourceLimit)?;
                    if rows > limit.saturating_mul(2) {
                        return Err(Error::ResourceLimit);
                    }
                    let card = cardinality.ok_or(Error::InvalidArgument)?;
                    if n > 1
                        && !(if reverse {
                            card.right_is_many()
                        } else {
                            card.left_is_many()
                        })
                    {
                        return Err(Error::InvalidArgument);
                    }
                    let mut partners = Vec::new();
                    partners
                        .try_reserve_exact(n)
                        .map_err(|_| Error::Allocation)?;
                    for _ in 0..n {
                        let p = store
                            .saved_reference(r.word()?)
                            .ok_or(Error::InvalidArgument)?;
                        if partners.contains(&p) {
                            return Err(Error::InvalidArgument);
                        }
                        partners.push(p);
                    }
                    if side.insert(key.handle(), partners).is_some() {
                        return Err(Error::InvalidArgument);
                    }
                }
            }
            for (left, rights) in &table.forward {
                for right in rights {
                    if !table
                        .reverse
                        .get(&right.handle())
                        .is_some_and(|ps| ps.iter().any(|p| p.handle() == *left))
                    {
                        return Err(Error::InvalidArgument);
                    }
                }
            }
            for (right, lefts) in &table.reverse {
                for left in lefts {
                    if !table
                        .forward
                        .get(&left.handle())
                        .is_some_and(|ps| ps.iter().any(|p| p.handle() == *right))
                    {
                        return Err(Error::InvalidArgument);
                    }
                }
            }
            result.tables.push(table);
        }
        let n = r.count(limit)?;
        result
            .labelled
            .try_reserve(n)
            .map_err(|_| Error::Allocation)?;
        let mut tables = std::collections::HashSet::new();
        tables.try_reserve(n).map_err(|_| Error::Allocation)?;
        for _ in 0..n {
            let base = usize::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?;
            let label = u32::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?;
            let table = usize::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?;
            if base >= count
                || table >= count
                || base == table
                || label > 4095
                || !tables.insert(table)
                || result.tables[base].cardinality != result.tables[table].cardinality
                || result.labelled.insert((base, label), table).is_some()
            {
                return Err(Error::InvalidArgument);
            }
        }
        Ok(result)
    }
}
