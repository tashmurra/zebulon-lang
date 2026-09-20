//! First-write snapshots of mutable World containers. Immutable values need no journal.
use super::*;

pub struct ContainerDelta {
    object: ObjectRef,
    vector: Option<Vec<Value>>,
    vector_charge: usize,
    lookup: Option<LookupTable>,
    dictionary: Option<Vec<DictionaryEntry>>,
    string_buffer: Option<String>,
    prototypes: Vec<ObjectRef>,
    transient: bool,
}

pub(super) fn copy_slice<T: Copy>(source: &[T]) -> Result<Vec<T>, Error> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(source.len())
        .map_err(|_| Error::Allocation)?;
    result.extend_from_slice(source);
    Ok(result)
}
pub(super) fn copy_string(source: &str) -> Result<String, Error> {
    let mut result = String::new();
    result
        .try_reserve_exact(source.len())
        .map_err(|_| Error::Allocation)?;
    result.push_str(source);
    Ok(result)
}
impl ContainerDelta {
    fn capture(object: ObjectRef, record: &Record) -> Result<Self, Error> {
        let lookup = record
            .lookup
            .as_ref()
            .map(|t| {
                let mut scalars = HashMap::new();
                scalars
                    .try_reserve(t.scalars.len())
                    .map_err(|_| Error::Allocation)?;
                scalars.extend(t.scalars.iter().map(|(k, v)| (*k, *v)));
                let mut strings = HashMap::new();
                strings
                    .try_reserve(t.strings.len())
                    .map_err(|_| Error::Allocation)?;
                for (k, v) in &t.strings {
                    strings.insert(copy_string(k)?, *v);
                }
                Ok(LookupTable {
                    buckets: t.buckets,
                    reserved_entries: t.reserved_entries,
                    scalars,
                    strings,
                    default: t.default,
                })
            })
            .transpose()?;
        let dictionary = record
            .dictionary
            .as_ref()
            .map(|entries| {
                let mut result = Vec::new();
                result
                    .try_reserve_exact(entries.len())
                    .map_err(|_| Error::Allocation)?;
                for e in entries {
                    result.push(DictionaryEntry {
                        word: copy_string(&e.word)?,
                        object: e.object,
                        property: e.property,
                    });
                }
                Ok::<_, Error>(result)
            })
            .transpose()?;
        let vector = record
            .vector
            .as_deref()
            .map(copy_slice)
            .transpose()?
            .map(|mut v| {
                v.try_reserve_exact(record.vector_charge.saturating_sub(v.len()))
                    .map_err(|_| Error::Allocation)?;
                Ok::<_, Error>(v)
            })
            .transpose()?;
        Ok(Self {
            object,
            vector,
            vector_charge: record.vector_charge,
            lookup,
            dictionary,
            string_buffer: record
                .string_buffer
                .as_deref()
                .map(copy_string)
                .transpose()?,
            prototypes: copy_slice(&record.prototypes)?,
            transient: record.transient,
        })
    }
}
fn charges(record: &Record) -> (usize, usize) {
    let mut properties = record.vector_charge;
    let mut bytes = record.string_buffer.as_ref().map_or(0, String::len);
    if let Some(t) = &record.lookup {
        properties += t.reserved_entries.max(t.scalars.len() + t.strings.len()) * 2;
        bytes += t.strings.keys().map(String::len).sum::<usize>();
    }
    if let Some(d) = &record.dictionary {
        properties += d.len();
        bytes += d.iter().map(|e| e.word.len()).sum::<usize>();
    }
    (properties, bytes)
}
impl Objects<'_> {
    pub(super) fn journal_container(&mut self, object: ObjectRef) -> Result<(), Error> {
        if !self.store.journalling
            || self.store.record(object)?.lifetime != Lifetime::World
            || self
                .store
                .container_journal
                .iter()
                .any(|d| d.object == object)
        {
            return Ok(());
        }
        let record = self.store.record(object)?;
        let (properties, bytes) = charges(record);
        let properties = properties.saturating_add(record.prototypes.len());
        if self.store.container_journal.len() >= self.store.limits.objects
            || properties
                > self
                    .store
                    .limits
                    .properties
                    .saturating_sub(self.store.container_journal_properties)
            || bytes
                > self
                    .store
                    .limits
                    .string_bytes
                    .saturating_sub(self.store.container_journal_bytes)
        {
            return Err(Error::ResourceLimit);
        }
        let snapshot = ContainerDelta::capture(object, record)?;
        self.store
            .container_journal
            .try_reserve(1)
            .map_err(|_| Error::Allocation)?;
        self.store.container_journal.push(snapshot);
        self.store.container_journal_properties += properties;
        self.store.container_journal_bytes += bytes;
        Ok(())
    }
    pub fn take_container_journal(&mut self) -> Vec<ContainerDelta> {
        self.store.container_journal_properties = 0;
        self.store.container_journal_bytes = 0;
        std::mem::take(&mut self.store.container_journal)
    }
    pub fn container_journal_len(&self) -> usize {
        self.store.container_journal.len()
    }
    /// Move reserved storage back; rollback needs no allocation.
    pub fn revert_containers(&mut self, deltas: Vec<ContainerDelta>) -> usize {
        let mut count = 0;
        for d in deltas.into_iter().rev() {
            if self.store.record(d.object).is_err() {
                continue;
            }
            let record = self
                .store
                .records
                .get_mut(&d.object.logical)
                .expect("checked record");
            let (old_properties, old_bytes) = charges(record);
            self.store.inheritance_edges =
                self.store.inheritance_edges - record.prototypes.len() + d.prototypes.len();
            record.vector = d.vector;
            record.vector_charge = d.vector_charge;
            record.lookup = d.lookup;
            record.dictionary = d.dictionary;
            record.string_buffer = d.string_buffer;
            record.prototypes = d.prototypes;
            record.transient = d.transient;
            let (new_properties, new_bytes) = charges(record);
            self.store.properties = self.store.properties - old_properties + new_properties;
            self.store.string_bytes = self.store.string_bytes - old_bytes + new_bytes;
            count += 1;
        }
        count
    }
}
