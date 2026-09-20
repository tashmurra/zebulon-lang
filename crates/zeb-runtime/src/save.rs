//! Versioned logical world image; deliberately independent of Rust and target layout.
use crate::objects::Error;
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
pub(crate) struct Writer {
    pub bytes: Vec<u8>,
}
impl Writer {
    pub fn new() -> Self {
        Self { bytes: Vec::new() }
    }
    pub fn raw(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > MAX_BYTES.saturating_sub(self.bytes.len()) {
            return Err(Error::ResourceLimit);
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| Error::Allocation)?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    pub fn word(&mut self, n: u64) -> Result<(), Error> {
        self.raw(&n.to_le_bytes())
    }
    pub fn count(&mut self, n: usize) -> Result<(), Error> {
        self.word(u64::try_from(n).map_err(|_| Error::ResourceLimit)?)
    }
    pub fn text(&mut self, text: &str) -> Result<(), Error> {
        self.count(text.len())?;
        self.raw(text.as_bytes())
    }
}
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::ResourceLimit);
        }
        Ok(Self { bytes, at: 0 })
    }
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::InvalidArgument)?;
        let result = self.bytes.get(self.at..end).ok_or(Error::InvalidArgument)?;
        self.at = end;
        Ok(result)
    }
    pub fn word(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.raw(8)?
                .try_into()
                .map_err(|_| Error::InvalidArgument)?,
        ))
    }
    pub fn count(&mut self, limit: usize) -> Result<usize, Error> {
        let n = usize::try_from(self.word()?).map_err(|_| Error::ResourceLimit)?;
        if n > limit || n > self.bytes.len().saturating_sub(self.at) {
            return Err(Error::ResourceLimit);
        }
        Ok(n)
    }
    pub fn flag(&mut self) -> Result<bool, Error> {
        match self.word()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::InvalidArgument),
        }
    }
    pub fn text(&mut self, limit: usize) -> Result<String, Error> {
        let len = self.count(limit)?;
        let text = std::str::from_utf8(self.raw(len)?).map_err(|_| Error::InvalidArgument)?;
        let mut result = String::new();
        result
            .try_reserve_exact(len)
            .map_err(|_| Error::Allocation)?;
        result.push_str(text);
        Ok(result)
    }
    pub fn finished(&self) -> bool {
        self.at == self.bytes.len()
    }
}

pub(crate) fn encode(
    store: &crate::objects::Store,
    relations: &crate::relations::Relations,
    schema: &[u8; 32],
    literals: &[(u32, crate::objects::ObjectRef)],
) -> Result<Vec<u8>, Error> {
    let mut w = Writer::new();
    w.raw(b"ZEBWORLD")?;
    w.word(1)?;
    w.raw(schema)?;
    store.write_world(&mut w)?;
    relations.write_world(store, &mut w)?;
    let live = |entry: &&(u32, crate::objects::ObjectRef)| {
        store.saved_reference(entry.1.handle() & 0xffff_ffff) == Some(entry.1)
    };
    w.count(literals.iter().filter(live).count())?;
    for (index, object) in literals.iter().filter(live) {
        w.word(u64::from(*index))?;
        w.word(object.handle() & 0xffff_ffff)?;
    }
    Ok(w.bytes)
}
type LiteralTexts = Vec<(u32, crate::objects::ObjectRef)>;
pub(crate) fn decode(
    bytes: &[u8],
    current: &crate::objects::Store,
    schema: &[u8; 32],
    row_limit: usize,
) -> Result<
    (
        crate::objects::Store,
        crate::relations::Relations,
        LiteralTexts,
    ),
    Error,
> {
    let mut r = Reader::new(bytes)?;
    if r.raw(8)? != b"ZEBWORLD" || r.word()? != 1 || r.raw(32)? != schema {
        return Err(Error::InvalidArgument);
    }
    let mut store = current.read_world(&mut r)?;
    let relations = crate::relations::Relations::read_world(&store, &mut r, row_limit)?;
    let table = crate::tables::installed();
    let count = r.count(table.literals.len())?;
    let mut literals = Vec::new();
    literals
        .try_reserve_exact(count)
        .map_err(|_| Error::Allocation)?;
    let mut seen = std::collections::HashSet::new();
    seen.try_reserve(count).map_err(|_| Error::Allocation)?;
    for _ in 0..count {
        let index = u32::try_from(r.word()?).map_err(|_| Error::InvalidArgument)?;
        let object = store
            .saved_reference(r.word()?)
            .ok_or(Error::InvalidArgument)?;
        if !seen.insert(index)
            || table.literals.get(index as usize).copied() != Some(store.objects().text(object)?)
        {
            return Err(Error::InvalidArgument);
        }
        literals.push((index, object));
    }
    if !r.finished() {
        return Err(Error::InvalidArgument);
    }
    Ok((store, relations, literals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Lifetime, Limits, PropertyId, Store, Value};
    use crate::relations::{Cardinality, Relations};
    fn encode(s: &Store, r: &Relations, schema: &[u8; 32]) -> Result<Vec<u8>, Error> {
        super::encode(s, r, schema, &[])
    }
    fn decode(
        bytes: &[u8],
        s: &Store,
        schema: &[u8; 32],
        limit: usize,
    ) -> Result<(Store, Relations), Error> {
        super::decode(bytes, s, schema, limit).map(|(s, r, _)| (s, r))
    }
    fn store() -> Store {
        let mut s = Store::new(Limits {
            objects: 100,
            properties: 1000,
            string_bytes: 4096,
        })
        .unwrap();
        s.objects().set_lifetime_model(true);
        s
    }
    fn world() -> (Store, Relations) {
        let mut s = store();
        let mut o = s.objects();
        let room = o.create().unwrap();
        let box_ = o.create().unwrap();
        let coin = o.create().unwrap();
        o.set(box_, PropertyId(1), Value::Bool(false)).unwrap();
        let text = o.new_text("coin λ").unwrap();
        o.set(coin, PropertyId(2), Value::Text(text)).unwrap();
        let vector = o.new_vector(4).unwrap();
        o.vector_append(vector, Value::Reference(coin)).unwrap();
        let table = o.new_lookup_sized(8, 4).unwrap();
        o.lookup_set(table, Value::Int(1), Value::Reference(box_))
            .unwrap();
        o.lookup_set_default(table, Value::Int(9)).unwrap();
        let dict = o.new_dictionary().unwrap();
        o.dictionary_add(dict, coin, "coin", PropertyId(3)).unwrap();
        let buffer = o.new_string_buffer().unwrap();
        o.string_buffer_append(buffer, "hi").unwrap();
        let gone = o.create().unwrap();
        o.set(room, PropertyId(4), Value::Reference(gone)).unwrap();
        o.despawn(gone).unwrap();
        let ephemeral = o.create().unwrap();
        o.set_transient(ephemeral, true).unwrap();
        o.set(room, PropertyId(5), Value::Reference(ephemeral))
            .unwrap();
        let mut r = Relations::default();
        r.ensure(0, Cardinality::OneToMany).unwrap();
        r.ensure(1, Cardinality::OneToOne).unwrap();
        r.set(0, room, box_).unwrap();
        r.set(0, box_, coin).unwrap();
        for label in [3, 6] {
            let i = r.ensure_labelled(1, label, Cardinality::OneToOne).unwrap();
            r.set(i, room, box_).unwrap();
        }
        (s, r)
    }
    #[test]
    fn logical_roundtrip_preserves_world_and_replaces_handles() {
        let (s, relations) = world();
        let image = encode(&s, &relations, &[42; 32]).unwrap();
        let (mut restored, mut r) = decode(&image, &s, &[42; 32], 1000).unwrap();
        let room = restored.saved_reference(1).unwrap();
        let box_ = restored.saved_reference(2).unwrap();
        let coin = restored.saved_reference(3).unwrap();
        assert_ne!(room.handle(), s.saved_reference(1).unwrap().handle());
        assert_eq!(r.all(0, room, false).unwrap(), &[box_]);
        assert_eq!(r.outermost(0, coin, false).unwrap(), room);
        for label in [3, 6] {
            assert_eq!(r.all_labelled(1, room, false, label).unwrap(), &[box_]);
        }
        let vector = restored.saved_reference(5).unwrap();
        let table = restored.saved_reference(6).unwrap();
        let dict = restored.saved_reference(7).unwrap();
        let buffer = restored.saved_reference(8).unwrap();
        let mut o = restored.objects();
        assert_eq!(o.get(box_, PropertyId(1)).unwrap(), Value::Bool(false));
        assert_eq!(o.vector(vector).unwrap(), &[Value::Reference(coin)]);
        assert_eq!(
            o.lookup_get(table, Value::Int(1)).unwrap(),
            Value::Reference(box_)
        );
        assert_eq!(o.lookup_default(table).unwrap(), Value::Int(9));
        assert!(o.dictionary_has(dict, "coin", PropertyId(3)).unwrap());
        assert_eq!(o.string_buffer(buffer).unwrap(), "hi");
        assert_eq!(o.get(room, PropertyId(4)).unwrap(), Value::Nil);
        assert_eq!(o.get(room, PropertyId(5)).unwrap(), Value::Nil);
        assert_eq!(o.lifetime(coin).unwrap(), Lifetime::World);
        let again = encode(&restored, &r, &[42; 32]).unwrap();
        let (twice, _) = decode(&again, &restored, &[42; 32], 1000).unwrap();
        assert_ne!(twice.saved_reference(1).unwrap().handle(), room.handle());
    }
    #[test]
    fn checked_parser_rejects_truncation_versions_schemas_lengths_and_trailing_bytes() {
        let (s, r) = world();
        let image = encode(&s, &r, &[1; 32]).unwrap();
        for n in 0..image.len() {
            assert!(
                decode(&image[..n], &s, &[1; 32], 1000).is_err(),
                "truncation {n}"
            );
        }
        assert!(decode(&image, &s, &[2; 32], 1000).is_err());
        for offset in [0, 8, 48, 64, 72] {
            let mut broken = image.clone();
            broken[offset..offset + 8].fill(255);
            assert!(
                decode(&broken, &s, &[1; 32], 1000).is_err(),
                "offset {offset}"
            );
        }
        let mut trailing = image.clone();
        trailing.push(0);
        assert!(decode(&trailing, &s, &[1; 32], 1000).is_err());
        let small = Store::new(Limits {
            objects: 1,
            properties: 1,
            string_bytes: 1,
        })
        .unwrap();
        assert!(decode(&image, &small, &[1; 32], 1000).is_err());
        // Invalid decoding does not consume or mutate the live store.
        assert_eq!(encode(&s, &r, &[1; 32]).unwrap(), image);
    }
    #[test]
    fn canonical_nonempty_world_matches_independent_golden() {
        let mut s = store();
        let first = s.objects().create().unwrap();
        s.objects()
            .set(first, PropertyId(7), Value::Int(42))
            .unwrap();
        let second = s.objects().new_text("λ").unwrap();
        let mut relations = Relations::default();
        let relation = relations.declare(Cardinality::OneToMany).unwrap();
        relations.set(relation, first, second).unwrap();
        let bytes = encode(&s, &relations, &[0x55; 32]).unwrap();
        assert_eq!(bytes, include_bytes!("../tests/fixtures/save-v1-world.bin"));
        let (mut restored, r) = decode(&bytes, &s, &[0x55; 32], 100).unwrap();
        let first = restored.saved_reference(1).unwrap();
        let second = restored.saved_reference(2).unwrap();
        assert_eq!(
            restored.objects().get(first, PropertyId(7)).unwrap(),
            Value::Int(42)
        );
        assert_eq!(restored.objects().text(second).unwrap(), "λ");
        assert_eq!(r.all(0, first, false).unwrap(), &[second]);
    }
    #[test]
    fn malformed_graphs_and_references_are_refused_without_recursive_walks() {
        let bytes = include_bytes!("../tests/fixtures/save-v1-world.bin");
        let s = store();
        let mut owner = bytes.to_vec();
        owner[96..104].copy_from_slice(&1u64.to_le_bytes());
        assert!(decode(&owner, &s, &[0x55; 32], 100).is_err());
        let mut inheritance = bytes.to_vec();
        inheritance[104..112].copy_from_slice(&1u64.to_le_bytes());
        inheritance.splice(112..112, 1u64.to_le_bytes());
        assert!(decode(&inheritance, &s, &[0x55; 32], 100).is_err());
        let mut foreign = bytes.to_vec();
        foreign[128..136].copy_from_slice(&5u64.to_le_bytes());
        assert!(decode(&foreign, &s, &[0x55; 32], 100).is_err());
        // A deterministic mutation corpus checks safe rejection/drop even when
        // a changed integer remains a valid logical value.
        for offset in 0..bytes.len() {
            let mut mutated = bytes.to_vec();
            mutated[offset] ^= 255;
            let _ = decode(&mutated, &s, &[0x55; 32], 100);
        }
    }
    #[test]
    fn restored_owner_fields_and_collection_slots_keep_cleanup_semantics() {
        let mut s = store();
        let owner = s.objects().create().unwrap();
        let region = s.objects().create().unwrap();
        let text = s.objects().new_text("owned").unwrap();
        s.objects().adopt(region, text).unwrap();
        s.objects()
            .assign_text(owner, PropertyId(7), text, region)
            .unwrap();
        let collection = s.objects().new_owned_collection().unwrap();
        let child = s.objects().create().unwrap();
        s.objects()
            .move_root_into_collection(collection, child)
            .unwrap();
        let bytes = encode(&s, &Relations::default(), &[7; 32]).unwrap();
        let (mut restored, _) = decode(&bytes, &s, &[7; 32], 100).unwrap();
        let owner = restored
            .saved_reference(owner.handle() & 0xffff_ffff)
            .unwrap();
        let text = restored
            .saved_reference(text.handle() & 0xffff_ffff)
            .unwrap();
        let collection = restored
            .saved_reference(collection.handle() & 0xffff_ffff)
            .unwrap();
        let child = restored
            .saved_reference(child.handle() & 0xffff_ffff)
            .unwrap();
        assert_eq!(
            restored.objects().owned_collection_len(collection).unwrap(),
            1
        );
        restored
            .objects()
            .set(owner, PropertyId(7), Value::Nil)
            .unwrap();
        assert!(!restored.is_valid(text));
        restored
            .objects()
            .remove_owned_member(collection, child)
            .unwrap();
        assert!(!restored.is_valid(child));
    }
    #[test]
    fn canonical_empty_world_matches_target_independent_golden() {
        let s = store();
        let bytes = encode(&s, &Relations::default(), &[0x55; 32]).unwrap();
        assert_eq!(bytes, include_bytes!("../tests/fixtures/save-v1-empty.bin"));
        let (restored, r) = decode(&bytes, &s, &[0x55; 32], 100).unwrap();
        assert_eq!(restored.live_objects(), 0);
        assert!(r.tables_of(0).is_empty());
    }
}
