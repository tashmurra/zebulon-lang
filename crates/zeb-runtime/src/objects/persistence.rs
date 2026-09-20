//! The object section of logical save version 1. No native resources or active frames.
use super::*;
use crate::save::{Reader, Writer};

fn live(store: &Store, r: ObjectRef) -> bool {
    store
        .record(r)
        .is_ok_and(|r| r.lifetime == Lifetime::World && !r.transient)
}
fn reference(w: &mut Writer, store: &Store, r: ObjectRef) -> Result<(), Error> {
    w.word(if live(store, r) { r.logical } else { 0 })
}
fn value(w: &mut Writer, store: &Store, v: Value) -> Result<(), Error> {
    let (tag, bits) = match v {
        Value::Nil => (0, 0),
        Value::Bool(b) => (1, u64::from(b)),
        Value::Int(n) => (2, u64::from(n as u32)),
        Value::Reference(r) | Value::Text(r) | Value::List(r) if !live(store, r) => (0, 0),
        Value::Reference(r) => (3, r.logical),
        Value::String(r) => (4, u64::from(r.index)),
        Value::Text(r) => (5, r.logical),
        Value::List(r) => (6, r.logical),
        Value::Method(n) => (7, u64::from(n)),
        Value::Property(n) => (8, u64::from(n.0)),
        Value::Enumerator(n) => (9, u64::from(n)),
        Value::Function(n) => (10, u64::from(n)),
        Value::Initializing => return Err(Error::InvalidArgument),
    };
    w.word(tag)?;
    w.word(bits)
}
fn values(w: &mut Writer, store: &Store, values: &[Value]) -> Result<(), Error> {
    w.count(values.len())?;
    for v in values {
        value(w, store, *v)?;
    }
    Ok(())
}
fn key_value(k: LookupKey) -> Value {
    match k {
        LookupKey::Nil => Value::Nil,
        LookupKey::Bool(b) => Value::Bool(b),
        LookupKey::Int(n) => Value::Int(n),
        LookupKey::Object(r) => Value::Reference(r),
        LookupKey::Property(p) => Value::Property(p),
        LookupKey::Enumerator(n) => Value::Enumerator(n),
        LookupKey::Function(n) => Value::Function(n),
    }
}
impl Store {
    pub(crate) fn save_row_limit(&self) -> usize {
        self.limits.properties
    }
    pub(crate) fn incarnation_ceiling(&self) -> u64 {
        self.next_identity.saturating_sub(1).max(
            self.records
                .values()
                .map(|r| r.incarnation)
                .max()
                .unwrap_or(0),
        )
    }
    pub(crate) fn saved_reference(&self, id: u64) -> Option<ObjectRef> {
        self.reference_to(id).filter(|r| live(self, *r))
    }
    pub(crate) fn write_world(&self, w: &mut Writer) -> Result<(), Error> {
        if !self.lifetimes
            || self.pending_constructions != 0
            || !self.journal.is_empty()
            || !self.container_journal.is_empty()
        {
            return Err(Error::InvalidArgument);
        }
        w.word(self.incarnation_ceiling())?;
        w.word(self.class_property.map_or(0, |p| u64::from(p.0) + 1))?;
        let mut ids = Vec::new();
        ids.try_reserve(self.records.len())
            .map_err(|_| Error::Allocation)?;
        ids.extend(
            self.records
                .iter()
                .filter(|(_, r)| r.live && r.lifetime == Lifetime::World && !r.transient)
                .map(|(id, _)| *id),
        );
        ids.sort_unstable();
        w.count(ids.len())?;
        for id in &ids {
            w.word(*id)?;
        }
        for id in ids {
            let r = &self.records[&id];
            if r.list_remaining.is_some() || !r.pending_constructions.is_empty() {
                return Err(Error::InvalidArgument);
            }
            w.word(u64::from(r.is_class))?;
            w.word(r.parent.filter(|p| live(self, *p)).map_or(0, |p| p.logical))?;
            w.count(r.prototypes.len())?;
            for p in &r.prototypes {
                if !live(self, *p) {
                    return Err(Error::InvalidArgument);
                }
                reference(w, self, *p)?;
            }
            let mut properties = Vec::new();
            properties
                .try_reserve(r.properties.len())
                .map_err(|_| Error::Allocation)?;
            properties.extend(r.properties.keys().copied());
            properties.sort_unstable_by_key(|p| p.0);
            w.count(properties.len())?;
            for p in properties {
                w.word(u64::from(p.0))?;
                value(w, self, r.properties[&p])?;
            }
            let mut owners = Vec::new();
            owners
                .try_reserve(r.owned_value_fields.len())
                .map_err(|_| Error::Allocation)?;
            owners.extend(
                r.owned_value_fields
                    .iter()
                    .filter(|(_, child)| live(self, **child)),
            );
            owners.sort_unstable_by_key(|(p, _)| p.0);
            w.count(owners.len())?;
            for (p, child) in owners {
                w.word(u64::from(p.0))?;
                reference(w, self, *child)?;
            }
            w.count(
                r.children
                    .iter()
                    .filter(|child| live(self, **child))
                    .count(),
            )?;
            for child in r.children.iter().filter(|child| live(self, **child)) {
                reference(w, self, *child)?;
            }
            for text in [&r.text, &r.string_buffer] {
                w.word(u64::from(text.is_some()))?;
                if let Some(t) = text {
                    w.text(t)?;
                }
            }
            for seq in [&r.list, &r.vector] {
                w.word(u64::from(seq.is_some()))?;
                if let Some(seq) = seq {
                    values(w, self, seq)?;
                }
            }
            w.count(r.vector_charge)?;
            w.word(u64::from(r.closure.is_some()))?;
            if let Some(c) = &r.closure {
                w.word(u64::from(c.function))?;
                values(w, self, &c.values)?;
            }
            w.word(u64::from(r.lookup.is_some()))?;
            if let Some(t) = &r.lookup {
                w.word(t.buckets as u64)?;
                w.count(t.reserved_entries)?;
                value(w, self, t.default)?;
                let mut entries = Vec::new();
                entries
                    .try_reserve(t.scalars.len())
                    .map_err(|_| Error::Allocation)?;
                for (k, v) in &t.scalars {
                    if let LookupKey::Object(r) = k
                        && !live(self, *r)
                    {
                        continue;
                    }
                    let mut encoded = Writer::new();
                    value(&mut encoded, self, key_value(*k))?;
                    value(&mut encoded, self, *v)?;
                    entries.push(encoded.bytes);
                }
                entries.sort_unstable();
                w.count(entries.len())?;
                for e in entries {
                    w.raw(&e)?;
                }
                let mut strings = Vec::new();
                strings
                    .try_reserve(t.strings.len())
                    .map_err(|_| Error::Allocation)?;
                strings.extend(t.strings.iter());
                strings.sort_unstable_by_key(|(k, _)| *k);
                w.count(strings.len())?;
                for (k, v) in strings {
                    w.text(k)?;
                    value(w, self, *v)?;
                }
            }
            w.word(u64::from(r.dictionary.is_some()))?;
            if let Some(d) = &r.dictionary {
                w.count(d.iter().filter(|e| live(self, e.object)).count())?;
                for e in d.iter().filter(|e| live(self, e.object)) {
                    w.text(&e.word)?;
                    reference(w, self, e.object)?;
                    w.word(u64::from(e.property.0))?;
                }
            }
            w.word(u64::from(r.collection.is_some()))?;
            if let Some(members) = &r.collection {
                w.count(
                    members
                        .iter()
                        .filter(|m| live(self, m.slot) && live(self, m.object))
                        .count(),
                )?;
                for m in members
                    .iter()
                    .filter(|m| live(self, m.slot) && live(self, m.object))
                {
                    reference(w, self, m.slot)?;
                    reference(w, self, m.object)?;
                }
            }
        }
        let mut statics = Vec::new();
        statics
            .try_reserve(self.statics.len())
            .map_err(|_| Error::Allocation)?;
        statics.extend(self.statics.iter().filter(|(_, r)| live(self, **r)));
        statics.sort_unstable_by_key(|(n, _)| **n);
        w.count(statics.len())?;
        for (slot, r) in statics {
            w.word(u64::from(*slot))?;
            reference(w, self, *r)?;
        }
        Ok(())
    }
}
struct Decode<'a> {
    refs: &'a HashMap<u64, ObjectRef>,
    session: u64,
    limits: Limits,
}
impl Decode<'_> {
    fn reference(&self, r: &mut Reader<'_>) -> Result<ObjectRef, Error> {
        self.refs
            .get(&r.word()?)
            .copied()
            .ok_or(Error::InvalidArgument)
    }
    fn value(&self, r: &mut Reader<'_>) -> Result<Value, Error> {
        let tag = r.word()?;
        let bits = r.word()?;
        let n = || u32::try_from(bits).map_err(|_| Error::InvalidArgument);
        Ok(match tag {
            0 if bits == 0 => Value::Nil,
            1 if bits <= 1 => Value::Bool(bits != 0),
            2 => Value::Int(n()? as i32),
            3 | 5 | 6 => {
                let reference = *self.refs.get(&bits).ok_or(Error::InvalidArgument)?;
                match tag {
                    3 => Value::Reference(reference),
                    5 => Value::Text(reference),
                    _ => Value::List(reference),
                }
            }
            4 => {
                let index = n()?;
                if index as usize >= crate::tables::installed().literals.len() {
                    return Err(Error::InvalidArgument);
                }
                Value::String(LiteralRef {
                    session: self.session,
                    index,
                })
            }
            7 => Value::Method(n()?),
            8 => Value::Property(PropertyId(n()?)),
            9 => Value::Enumerator(n()?),
            10 => Value::Function(n()?),
            _ => return Err(Error::InvalidArgument),
        })
    }
    fn values(&self, r: &mut Reader<'_>) -> Result<Vec<Value>, Error> {
        let n = r.count(self.limits.properties)?;
        let mut values = Vec::new();
        values.try_reserve_exact(n).map_err(|_| Error::Allocation)?;
        for _ in 0..n {
            values.push(self.value(r)?);
        }
        Ok(values)
    }
}
impl Store {
    pub(crate) fn read_world(&self, r: &mut Reader<'_>) -> Result<Store, Error> {
        let incarnation = r
            .word()?
            .max(self.incarnation_ceiling())
            .checked_add(1)
            .filter(|n| *n <= u32::MAX as u64)
            .ok_or(Error::IdentityExhausted)?;
        let class = r.word()?;
        let class_property = if class == 0 {
            None
        } else {
            Some(PropertyId(
                u32::try_from(class - 1).map_err(|_| Error::InvalidArgument)?,
            ))
        };
        let count = r.count(self.limits.objects)?;
        let mut refs = HashMap::new();
        refs.try_reserve(count).map_err(|_| Error::Allocation)?;
        let mut ids = Vec::new();
        ids.try_reserve_exact(count)
            .map_err(|_| Error::Allocation)?;
        let mut previous = 0;
        for _ in 0..count {
            let id = r.word()?;
            if id <= previous || id > u32::MAX as u64 {
                return Err(Error::InvalidArgument);
            }
            previous = id;
            refs.insert(
                id,
                ObjectRef {
                    session: self.session,
                    logical: id,
                    incarnation,
                },
            );
            ids.push(id);
        }
        let d = Decode {
            refs: &refs,
            session: self.session,
            limits: self.limits,
        };
        let mut records = HashMap::new();
        records.try_reserve(count).map_err(|_| Error::Allocation)?;
        let (mut properties, mut bytes, mut edges) = (0usize, 0usize, 0usize);
        for id in ids {
            let is_class = r.flag()?;
            let parent = r.word()?;
            let parent = if parent == 0 {
                None
            } else {
                Some(*refs.get(&parent).ok_or(Error::InvalidArgument)?)
            };
            let n = r.count(self.limits.properties)?;
            let mut prototypes = Vec::new();
            prototypes
                .try_reserve_exact(n)
                .map_err(|_| Error::Allocation)?;
            for _ in 0..n {
                prototypes.push(d.reference(r)?);
            }
            edges = edges.checked_add(n).ok_or(Error::ResourceLimit)?;
            let n = r.count(self.limits.properties)?;
            let mut fields = HashMap::new();
            fields.try_reserve(n).map_err(|_| Error::Allocation)?;
            for _ in 0..n {
                let p = PropertyId(u32::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?);
                if fields.insert(p, d.value(r)?).is_some() {
                    return Err(Error::InvalidArgument);
                }
            }
            let n = r.count(self.limits.properties)?;
            let mut owned_value_fields = HashMap::new();
            owned_value_fields
                .try_reserve(n)
                .map_err(|_| Error::Allocation)?;
            for _ in 0..n {
                let p = PropertyId(u32::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?);
                if !fields.contains_key(&p)
                    || owned_value_fields.insert(p, d.reference(r)?).is_some()
                {
                    return Err(Error::InvalidArgument);
                }
            }
            let n = r.count(self.limits.objects)?;
            let mut children = Vec::new();
            children
                .try_reserve_exact(n)
                .map_err(|_| Error::Allocation)?;
            for _ in 0..n {
                children.push(d.reference(r)?);
            }
            let text = if r.flag()? {
                Some(r.text(self.limits.string_bytes)?)
            } else {
                None
            };
            let string_buffer = if r.flag()? {
                Some(r.text(self.limits.string_bytes)?)
            } else {
                None
            };
            let list = if r.flag()? { Some(d.values(r)?) } else { None };
            let mut vector = if r.flag()? { Some(d.values(r)?) } else { None };
            let vector_charge = usize::try_from(r.word()?).map_err(|_| Error::ResourceLimit)?;
            if vector_charge > self.limits.properties
                || vector.as_ref().is_none_or(|v| vector_charge < v.len()) && vector_charge != 0
            {
                return Err(Error::InvalidArgument);
            }
            if let Some(v) = &mut vector {
                v.try_reserve_exact(vector_charge.saturating_sub(v.len()))
                    .map_err(|_| Error::Allocation)?;
            }
            let closure = if r.flag()? {
                Some(ClosureEnvironment {
                    function: u32::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?,
                    values: d.values(r)?,
                })
            } else {
                None
            };
            let lookup = if r.flag()? {
                let buckets = i32::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?;
                if buckets <= 0 {
                    return Err(Error::InvalidArgument);
                }
                let reserved_entries =
                    usize::try_from(r.word()?).map_err(|_| Error::ResourceLimit)?;
                if reserved_entries > self.limits.properties / 2 {
                    return Err(Error::ResourceLimit);
                }
                let default = d.value(r)?;
                let n = r.count(self.limits.properties / 2)?;
                let mut scalars = HashMap::new();
                scalars.try_reserve(n).map_err(|_| Error::Allocation)?;
                for _ in 0..n {
                    let k = match d.value(r)? {
                        Value::Nil => LookupKey::Nil,
                        Value::Bool(b) => LookupKey::Bool(b),
                        Value::Int(n) => LookupKey::Int(n),
                        Value::Reference(o) => LookupKey::Object(o),
                        Value::Property(p) => LookupKey::Property(p),
                        Value::Enumerator(n) => LookupKey::Enumerator(n),
                        Value::Function(n) => LookupKey::Function(n),
                        _ => return Err(Error::InvalidArgument),
                    };
                    if scalars.insert(k, d.value(r)?).is_some() {
                        return Err(Error::InvalidArgument);
                    }
                }
                let n = r.count(self.limits.properties / 2)?;
                let mut strings = HashMap::new();
                strings.try_reserve(n).map_err(|_| Error::Allocation)?;
                for _ in 0..n {
                    if strings
                        .insert(r.text(self.limits.string_bytes)?, d.value(r)?)
                        .is_some()
                    {
                        return Err(Error::InvalidArgument);
                    }
                }
                Some(LookupTable {
                    buckets,
                    reserved_entries,
                    scalars,
                    strings,
                    default,
                })
            } else {
                None
            };
            let dictionary = if r.flag()? {
                let n = r.count(self.limits.properties)?;
                let mut entries = Vec::new();
                entries
                    .try_reserve_exact(n)
                    .map_err(|_| Error::Allocation)?;
                for _ in 0..n {
                    entries.push(DictionaryEntry {
                        word: r.text(self.limits.string_bytes)?,
                        object: d.reference(r)?,
                        property: PropertyId(
                            u32::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?,
                        ),
                    });
                }
                Some(entries)
            } else {
                None
            };
            let collection = if r.flag()? {
                let n = r.count(self.limits.objects)?;
                let mut members = Vec::new();
                members
                    .try_reserve_exact(n)
                    .map_err(|_| Error::Allocation)?;
                for _ in 0..n {
                    members.push(OwnedMember {
                        slot: d.reference(r)?,
                        object: d.reference(r)?,
                    });
                }
                Some(members)
            } else {
                None
            };
            let kinds = usize::from(collection.is_some())
                + usize::from(text.is_some())
                + usize::from(string_buffer.is_some())
                + usize::from(list.is_some())
                + usize::from(vector.is_some())
                + usize::from(closure.is_some())
                + usize::from(lookup.is_some())
                + usize::from(dictionary.is_some());
            if kinds > 1 {
                return Err(Error::InvalidArgument);
            }
            let mut pc = fields.len()
                + vector_charge
                + list.as_ref().map_or(0, Vec::len)
                + closure.as_ref().map_or(0, |c| c.values.len());
            let mut bc = text.as_ref().map_or(0, String::len)
                + string_buffer.as_ref().map_or(0, String::len);
            if let Some(t) = &lookup {
                pc += 1 + 2 * t.reserved_entries.max(t.scalars.len() + t.strings.len());
                bc += t.strings.keys().map(String::len).sum::<usize>();
            }
            if let Some(d) = &dictionary {
                pc += d.len();
                bc += d.iter().map(|e| e.word.len()).sum::<usize>();
            }
            properties = properties.checked_add(pc).ok_or(Error::ResourceLimit)?;
            bytes = bytes.checked_add(bc).ok_or(Error::ResourceLimit)?;
            if properties.saturating_add(edges) > self.limits.properties
                || bytes > self.limits.string_bytes
            {
                return Err(Error::ResourceLimit);
            }
            records.insert(
                id,
                Record {
                    incarnation,
                    lifetime: Lifetime::World,
                    live: true,
                    is_class,
                    closure,
                    transient: false,
                    text,
                    string_buffer,
                    list,
                    list_remaining: None,
                    vector,
                    lookup,
                    vector_charge,
                    dictionary,
                    collection,
                    parent,
                    prototypes,
                    children,
                    properties: fields,
                    owned_value_fields,
                    pending_constructions: HashMap::new(),
                },
            );
        }
        // Validate ownership and inheritance without recursion or repeated walks.
        for owner_graph in [true, false] {
            let mut done = HashSet::new();
            done.try_reserve(count).map_err(|_| Error::Allocation)?;
            let mut active = HashSet::new();
            active.try_reserve(count).map_err(|_| Error::Allocation)?;
            let mut stack = Vec::new();
            stack
                .try_reserve(count.saturating_add(1))
                .map_err(|_| Error::Allocation)?;
            for &id in records.keys() {
                if done.contains(&id) {
                    continue;
                }
                stack.push((id, 0usize));
                active.insert(id);
                while let Some((at, next)) = stack.pop() {
                    let record = records.get(&at).ok_or(Error::InvalidArgument)?;
                    let edge = if owner_graph {
                        if next == 0 { record.parent } else { None }
                    } else {
                        record.prototypes.get(next).copied()
                    };
                    if let Some(edge) = edge {
                        stack.push((at, next + 1));
                        if active.contains(&edge.logical) {
                            return Err(Error::InvalidArgument);
                        }
                        if !done.contains(&edge.logical) {
                            active.insert(edge.logical);
                            stack.push((edge.logical, 0));
                        }
                    } else {
                        active.remove(&at);
                        done.insert(at);
                    }
                }
            }
        }
        for record in records.values() {
            // No native data may disguise a text/list value of another kind.
            let valid = |v: &Value| match v {
                Value::Text(o) => records[&o.logical].text.is_some(),
                Value::List(o) => records[&o.logical].list.is_some(),
                _ => true,
            };
            if !record.properties.values().all(valid)
                || !record.list.iter().flatten().all(valid)
                || !record.vector.iter().flatten().all(valid)
                || record
                    .closure
                    .as_ref()
                    .is_some_and(|c| !c.values.iter().all(valid))
                || record.lookup.as_ref().is_some_and(|t| {
                    !valid(&t.default)
                        || !t.scalars.values().all(valid)
                        || !t.strings.values().all(valid)
                })
            {
                return Err(Error::InvalidArgument);
            }
        }
        let mut roots = Vec::new();
        roots.try_reserve(count).map_err(|_| Error::Allocation)?;
        let mut owned = HashSet::new();
        owned.try_reserve(count).map_err(|_| Error::Allocation)?;
        for (&id, reference) in &refs {
            let record = &records[&id];
            for child in &record.children {
                if records[&child.logical].parent != Some(*reference)
                    || !owned.insert(child.logical)
                {
                    return Err(Error::InvalidArgument);
                }
            }
            for child in record.owned_value_fields.values() {
                if records[&child.logical].parent != Some(*reference) {
                    return Err(Error::InvalidArgument);
                }
            }
            if let Some(members) = &record.collection {
                for m in members {
                    if records[&m.slot.logical].parent != Some(*reference)
                        || records[&m.object.logical].parent != Some(m.slot)
                    {
                        return Err(Error::InvalidArgument);
                    }
                }
            }
            if record.parent.is_none() {
                roots.push(*reference);
            }
        }
        if records
            .iter()
            .any(|(id, r)| r.parent.is_some() != owned.contains(id))
        {
            return Err(Error::InvalidArgument);
        }
        let n = r.count(self.limits.objects)?;
        let mut statics = HashMap::new();
        statics.try_reserve(n).map_err(|_| Error::Allocation)?;
        for _ in 0..n {
            let slot = u32::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?;
            if statics.insert(slot, d.reference(r)?).is_some() {
                return Err(Error::InvalidArgument);
            }
        }
        let mut cleanup = Vec::new();
        cleanup
            .try_reserve(count.saturating_mul(2))
            .map_err(|_| Error::Allocation)?;
        let mut free_slots = Vec::new();
        free_slots
            .try_reserve(count)
            .map_err(|_| Error::Allocation)?;
        let mut store = Store::new(self.limits)?;
        store.session = self.session;
        store.next_identity = previous
            .max(incarnation)
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        store.records = records;
        store.roots = roots;
        store.statics = statics;
        store.class_property = class_property;
        store.cleanup = cleanup;
        store.free_slots = free_slots;
        store.properties = properties;
        store.inheritance_edges = edges;
        store.string_bytes = bytes;
        store.lifetimes = true;
        Ok(store)
    }
}
