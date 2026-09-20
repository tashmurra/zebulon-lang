//! Internal Rust object storage. Scope capabilities own objects; references do not.
//! Public native handles, nested source scopes and serialization are separate work.
mod history;
mod persistence;
pub use history::ContainerDelta;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    session: u64,
    logical: u64,
    incarnation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PropertyId(pub u32);

/// How long a record lives. World records are freed only by an
/// explicit despawn or a wholesale reset; Turn records are freed together at
/// the end of a command cycle. A value reaching world state is promoted, so
/// nothing an author writes has to say which class a value belongs to.
/// One property write, with the value the property held before it.
/// Replaying a cycle's writes backwards is what makes a failed turn and an
/// undone turn the same operation.
#[derive(Clone, Copy, Debug)]
pub struct PropertyDelta {
    pub object: ObjectRef,
    pub property: PropertyId,
    /// What the property held, or `None` when it had no value at all.
    pub previous: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifetime {
    World,
    Turn,
}

/// Non-owning reference to immutable, module-owned literal metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiteralRef {
    session: u64,
    index: u32,
}
impl LiteralRef {
    pub(crate) fn index(self) -> u32 {
        self.index
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reservation {
    recipient: ObjectRef,
    property: PropertyId,
    revision: u64,
}
#[derive(Clone, Copy)]
struct PendingConstruction {
    object: ObjectRef,
    revision: u64,
}

/// Distinct from owned-result construction: completion yields only a borrow.
#[derive(Debug)]
pub struct PublishedConstruction {
    construction: Construction,
    reservation: Option<Reservation>,
}
impl PublishedConstruction {
    pub fn reference(&self) -> Option<ObjectRef> {
        self.construction.reference()
    }
}

/// An owned traversal snapshot; elements remain ordinary checked borrows.
#[derive(Debug)]
pub struct ValueIteration {
    snapshot: Option<ObjectRef>,
    next: usize,
}

/// Temporary exclusive ownership while a collection member executes.
#[derive(Debug)]
pub struct MemberCall {
    handoff: PublishedConstruction,
}
impl ValueIteration {
    pub(crate) fn cleanup_identity(&self) -> Option<u64> {
        self.snapshot.map(|value| value.handle())
    }
}

impl MemberCall {
    pub(crate) fn cleanup_identity(&self) -> Option<u64> {
        self.handoff
            .construction
            .active
            .map(|(activation, _)| activation.handle())
    }
    pub fn reference(&self) -> Option<ObjectRef> {
        self.handoff.reference()
    }
}

/// Move-only activation for owned-result construction. Aliases carry no ownership.
/// Source lowering must finalize every activation; the enclosing store still
/// reclaims its root inventory if an internal caller forgets this token.
#[derive(Debug)]
pub struct Construction {
    active: Option<(ObjectRef, ObjectRef)>,
}
impl Construction {
    pub fn reference(&self) -> Option<ObjectRef> {
        self.active.map(|(_, object)| object)
    }
}

/// Exclusive ownership carried by a pending or handled source exception.
/// Moving the token preserves the object's identity; ordinary references do not
/// acquire ownership. The source outcome/handler must explicitly release it.
#[derive(Debug)]
pub struct ExceptionOwner {
    active: Option<(ObjectRef, ObjectRef)>,
}
impl ExceptionOwner {
    pub(crate) fn token(&self) -> Option<u64> {
        self.active.map(|(activation, _)| activation.handle())
    }
    pub fn reference(&self) -> Option<ObjectRef> {
        self.active.map(|(_, object)| object)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i32),
    Reference(ObjectRef),
    String(LiteralRef),
    Text(ObjectRef),
    List(ObjectRef),
    Method(u32),
    Property(PropertyId),
    Enumerator(u32),
    Function(u32),
    Initializing,
}

/// Explicit capture modes. Neither variant acquires ownership of a referent.
#[derive(Clone, Copy, Debug)]
pub enum Capture {
    Copy(Value),
    Reference(Value),
}

struct ClosureEnvironment {
    function: u32,
    values: Vec<Value>,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub objects: usize,
    pub properties: usize,
    pub string_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    ForeignSession,
    Expired,
    NotOwned,
    OwnershipCycle,
    InheritanceCycle,
    InitializationCycle,
    WrongType,
    InvalidArgument,
    NumericOverflow,
    ResourceLimit,
    Allocation,
    IdentityExhausted,
    Terminal,
}

struct DictionaryEntry {
    word: String,
    object: ObjectRef,
    property: PropertyId,
}

#[derive(Clone, Copy)]
struct OwnedMember {
    slot: ObjectRef,
    object: ObjectRef,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum LookupKey {
    Nil,
    Bool(bool),
    Int(i32),
    Object(ObjectRef),
    Property(PropertyId),
    Enumerator(u32),
    Function(u32),
}
// Separate standard maps allow allocation-free borrowed string queries.
struct LookupTable {
    buckets: i32,
    reserved_entries: usize,
    scalars: HashMap<LookupKey, Value>,
    strings: HashMap<String, Value>,
    default: Value,
}
enum LookupQuery<'a> {
    Scalar(LookupKey),
    Text(&'a str),
}

struct Record {
    incarnation: u64,
    lifetime: Lifetime,
    live: bool,
    is_class: bool,
    closure: Option<ClosureEnvironment>,
    transient: bool,
    text: Option<String>,
    string_buffer: Option<String>,
    list: Option<Vec<Value>>,
    list_remaining: Option<usize>,
    vector: Option<Vec<Value>>,
    lookup: Option<LookupTable>,
    vector_charge: usize,
    dictionary: Option<Vec<DictionaryEntry>>,
    collection: Option<Vec<OwnedMember>>,
    parent: Option<ObjectRef>,
    prototypes: Vec<ObjectRef>,
    children: Vec<ObjectRef>,
    properties: HashMap<PropertyId, Value>,
    owned_value_fields: HashMap<PropertyId, ObjectRef>,
    pending_constructions: HashMap<PropertyId, PendingConstruction>,
}

#[derive(Clone, Copy)]
enum Cleanup {
    Enter(ObjectRef),
    Release(ObjectRef),
}

pub struct Store {
    session: u64,
    next_identity: u64,
    /// The class new records take. World during world construction, Turn once a
    /// command cycle has begun.
    default_lifetime: Lifetime,
    /// Logical slots a released Turn record left behind. A reused slot keeps a
    /// fresh incarnation, so a handle to the previous occupant stays detectably
    /// stale rather than silently naming its replacement.
    free_slots: Vec<u64>,
    /// Property writes since the cycle began, oldest first. Empty
    /// while `journalling` is off, which is how world construction avoids
    /// recording anything.
    journal: Vec<PropertyDelta>,
    container_journal: Vec<ContainerDelta>,
    container_journal_properties: usize,
    container_journal_bytes: usize,
    journalling: bool,
    next_reservation: u64,
    pending_constructions: usize,
    records: HashMap<u64, Record>,
    statics: HashMap<u32, ObjectRef>,
    class_property: Option<PropertyId>,
    roots: Vec<ObjectRef>,
    cleanup: Vec<Cleanup>,
    properties: usize,
    inheritance_edges: usize,
    string_bytes: usize,
    limits: Limits,
    /// Whether this program uses World/Turn lifetimes rather than
    /// scope ownership. It decides who owns a construction that no field
    /// reserved: under lifetimes the turn does, and under scope ownership
    /// nothing does, so it is destroyed.
    lifetimes: bool,
    terminal: bool,
    #[cfg(test)]
    fail_reservation: bool,
}

impl Store {
    /// Session uniqueness covers this loaded runtime, not unload/reload or saves.
    pub fn new(limits: Limits) -> Result<Self, Error> {
        let session = NEXT_SESSION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| Error::IdentityExhausted)?;
        Ok(Self {
            session,
            next_identity: 1,
            default_lifetime: Lifetime::World,
            free_slots: Vec::new(),
            journal: Vec::new(),
            container_journal: Vec::new(),
            container_journal_properties: 0,
            container_journal_bytes: 0,
            journalling: false,
            next_reservation: 1,
            pending_constructions: 0,
            records: HashMap::new(),
            statics: HashMap::new(),
            class_property: None,
            roots: Vec::new(),
            cleanup: Vec::new(),
            properties: 0,
            inheritance_edges: 0,
            string_bytes: 0,
            limits,
            lifetimes: false,
            terminal: false,
            #[cfg(test)]
            fail_reservation: false,
        })
    }

    pub fn scope(&mut self) -> Result<Scope<'_>, Error> {
        if self.terminal {
            return Err(Error::Terminal);
        }
        // A forgotten scope cannot orphan its ownership inventory.
        self.clear_roots();
        Ok(Scope {
            objects: Objects { store: self },
        })
    }

    pub(crate) fn terminate(&mut self, error: Error) -> Result<(), Error> {
        self.objects().resource_failure(error)
    }

    pub(crate) fn ensure_active(&self) -> Result<(), Error> {
        if self.terminal {
            Err(Error::Terminal)
        } else {
            Ok(())
        }
    }

    pub(crate) fn session_id(&self) -> u64 {
        self.session
    }

    pub(crate) fn literal(&self, index: u32) -> Result<LiteralRef, Error> {
        if self.terminal {
            return Err(Error::Terminal);
        }
        crate::tables::installed()
            .literals
            .get(index as usize)
            .ok_or(Error::Expired)?;
        Ok(LiteralRef {
            session: self.session,
            index,
        })
    }

    /// A reference to a live record named by its slot, for a walk inside the
    /// store that has no handle to start from.
    pub(crate) fn reference_to(&self, logical: u64) -> Option<ObjectRef> {
        let record = self.records.get(&logical).filter(|record| record.live)?;
        Some(ObjectRef {
            session: self.session,
            logical,
            incarnation: record.incarnation,
        })
    }

    /// Rebuild a handle that crossed the boundary. Both halves come back, so a
    /// reference to a freed slot stays detectably stale.
    pub(crate) fn native_reference(&self, handle: u64) -> ObjectRef {
        ObjectRef {
            session: self.session,
            logical: handle & 0xffff_ffff,
            incarnation: handle >> 32,
        }
    }

    pub(crate) fn bind_static(&mut self, slot: u32, object: ObjectRef) -> Result<(), Error> {
        if self.terminal {
            return Err(Error::Terminal);
        }
        self.record(object)?;
        if self.statics.contains_key(&slot) {
            return Err(Error::NotOwned);
        }
        if self.statics.len() >= self.limits.objects {
            return self.objects().resource_failure(Error::ResourceLimit);
        }
        if self.statics.try_reserve(1).is_err() {
            return self.objects().resource_failure(Error::Allocation);
        }
        self.statics.insert(slot, object);
        Ok(())
    }

    pub(crate) fn static_object(&self, slot: u32) -> Result<ObjectRef, Error> {
        if self.terminal {
            return Err(Error::Terminal);
        }
        let object = *self.statics.get(&slot).ok_or(Error::Expired)?;
        self.record(object)?;
        Ok(object)
    }

    pub(crate) fn register_class_property(&mut self, property: PropertyId) -> Result<(), Error> {
        if self.terminal {
            return Err(Error::Terminal);
        }
        self.class_property = Some(property);
        Ok(())
    }
    pub(crate) fn mark_class(&mut self, object: ObjectRef) -> Result<(), Error> {
        if self.terminal {
            return Err(Error::Terminal);
        }
        self.record(object)?;
        self.records
            .get_mut(&object.logical)
            .expect("validated object")
            .is_class = true;
        Ok(())
    }
    fn class_fallback(
        &self,
        object: ObjectRef,
        property: PropertyId,
        value: Option<Value>,
    ) -> Result<Option<Value>, Error> {
        if value == Some(Value::Initializing) {
            return Err(Error::InitializationCycle);
        }
        if value.is_none() && self.class_property == Some(property) {
            Ok(Some(Value::Bool(self.record(object)?.is_class)))
        } else {
            Ok(value)
        }
    }

    pub(crate) fn objects(&mut self) -> Objects<'_> {
        Objects { store: self }
    }

    fn record(&self, object: ObjectRef) -> Result<&Record, Error> {
        if object.session != self.session {
            return Err(Error::ForeignSession);
        }
        self.records
            .get(&object.logical)
            .filter(|r| r.live && r.incarnation == object.incarnation)
            .ok_or(Error::Expired)
    }

    fn descends(
        &self,
        from: ObjectRef,
        ancestor: ObjectRef,
        pending: &mut Vec<ObjectRef>,
        seen: &mut HashSet<ObjectRef>,
    ) -> Result<bool, Error> {
        pending.clear();
        seen.clear();
        pending.push(from);
        seen.insert(from);
        while let Some(object) = pending.pop() {
            for parent in &self.record(object)?.prototypes {
                self.record(*parent)?;
                if *parent == ancestor {
                    return Ok(true);
                }
                if seen.insert(*parent) {
                    pending.push(*parent);
                }
            }
        }
        Ok(false)
    }

    // Reverse postorder with rightmost branches first retains the last occurrence
    // of each class in the expanded preorder, without expanding duplicate subgraphs.
    fn lookup(
        &self,
        mut object: ObjectRef,
        property: PropertyId,
        mut after: Option<ObjectRef>,
    ) -> Result<Option<Value>, Error> {
        if let Some(definer) = after {
            self.record(definer)?;
        }
        while after.is_none() {
            let record = self.record(object)?;
            if let Some(value) = record.properties.get(&property) {
                return Ok(Some(*value));
            }
            match record.prototypes.as_slice() {
                [] => return Ok(None),
                [parent] => object = *parent,
                _ => break,
            }
        }
        let count = self.records.len();
        let mut frames = Vec::new();
        let mut order = Vec::new();
        let mut seen = HashSet::new();
        frames.try_reserve(count).map_err(|_| Error::Allocation)?;
        order.try_reserve(count).map_err(|_| Error::Allocation)?;
        seen.try_reserve(count).map_err(|_| Error::Allocation)?;
        frames.push((object, 0usize));
        seen.insert(object);
        while let Some((current, next)) = frames.pop() {
            let record = self.record(current)?;
            if next < record.prototypes.len() {
                let parent = record.prototypes[record.prototypes.len() - 1 - next];
                self.record(parent)?;
                frames.push((current, next + 1));
                if seen.insert(parent) {
                    frames.push((parent, 0));
                }
            } else {
                order.push(current);
            }
        }
        for current in order.into_iter().rev() {
            if let Some(definer) = after {
                if current == definer {
                    after = None;
                }
                continue;
            }
            if let Some(value) = self.record(current)?.properties.get(&property) {
                return Ok(Some(*value));
            }
        }
        Ok(None)
    }

    /// The object that defines `property` for `object`, in the same order the
    /// lookup uses, or None when nothing defines it.
    fn definer(&self, object: ObjectRef, property: PropertyId) -> Result<Option<ObjectRef>, Error> {
        let mut current = object;
        loop {
            let record = self.record(current)?;
            if record.properties.contains_key(&property) {
                return Ok(Some(current));
            }
            match record.prototypes.as_slice() {
                [] => return Ok(None),
                [parent] => current = *parent,
                _ => break,
            }
        }
        let count = self.records.len();
        let mut frames = Vec::new();
        let mut order = Vec::new();
        let mut seen = HashSet::new();
        frames.try_reserve(count).map_err(|_| Error::Allocation)?;
        order.try_reserve(count).map_err(|_| Error::Allocation)?;
        seen.try_reserve(count).map_err(|_| Error::Allocation)?;
        frames.push((current, 0usize));
        seen.insert(current);
        while let Some((at, next)) = frames.pop() {
            let record = self.record(at)?;
            if next < record.prototypes.len() {
                let parent = record.prototypes[record.prototypes.len() - 1 - next];
                self.record(parent)?;
                frames.push((at, next + 1));
                if seen.insert(parent) {
                    frames.push((parent, 0));
                }
            } else {
                order.push(at);
            }
        }
        for at in order.into_iter().rev() {
            if self.record(at)?.properties.contains_key(&property) {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }

    pub fn is_valid(&self, object: ObjectRef) -> bool {
        self.record(object).is_ok()
    }
    pub fn live_objects(&self) -> usize {
        self.records.len()
    }

    fn clear_roots(&mut self) {
        while let Some(root) = self.roots.pop() {
            self.destroy(root);
        }
    }

    fn reservation(&mut self) -> Result<(), Error> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_reservation) {
            return Err(Error::Allocation);
        }
        Ok(())
    }

    // Capacity for one pending step per live object is admitted at creation.
    // Parent identity expires before children; its payload is released after them.
    fn destroy(&mut self, root: ObjectRef) {
        self.cleanup.push(Cleanup::Enter(root));
        while let Some(step) = self.cleanup.pop() {
            match step {
                Cleanup::Enter(object) => {
                    let record = self.records.get_mut(&object.logical).expect("owned object");
                    record.live = false;
                    self.cleanup.push(Cleanup::Release(object));
                    for child in &record.children {
                        self.cleanup.push(Cleanup::Enter(*child));
                    }
                }
                Cleanup::Release(object) => {
                    let record = self
                        .records
                        .remove(&object.logical)
                        .expect("revoked object");
                    self.string_bytes -= record.text.as_ref().map_or(0, String::len)
                        + record.string_buffer.as_ref().map_or(0, String::len);
                    self.properties -= record.properties.len() + record.vector_charge;
                    self.pending_constructions -= record.pending_constructions.len();
                    self.properties -= record.closure.as_ref().map_or(0, |env| env.values.len());
                    self.properties -= record.list.as_ref().map_or(0, Vec::len)
                        + record.list_remaining.unwrap_or(0);
                    if let Some(entries) = record.dictionary {
                        self.properties -= entries.len();
                        self.string_bytes -=
                            entries.iter().map(|entry| entry.word.len()).sum::<usize>();
                    }
                    if let Some(table) = record.lookup {
                        self.properties -= 1 + 2 * table
                            .reserved_entries
                            .max(table.scalars.len() + table.strings.len());
                        self.string_bytes -= table.strings.keys().map(String::len).sum::<usize>();
                    }
                    self.inheritance_edges -= record.prototypes.len();
                    // the slot goes back on the free list. Reservation
                    // is fallible everywhere else, so a slot that cannot be
                    // remembered is simply not reused rather than failing here.
                    if self.free_slots.try_reserve(1).is_ok() {
                        self.free_slots.push(object.logical);
                    }
                }
            }
        }
    }
}

/// Temporary exclusive access to a store-owned forest.
/// A reference selects an object; this capability controls ownership transfers.
pub struct Objects<'a> {
    store: &'a mut Store,
}

/// Which records an enumeration walk takes. `firstObj`/`nextObj`
/// name this with a flag: 1 instances, 2 classes, 3 both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Derived {
    Instances,
    Classes,
    Both,
}

impl Derived {
    /// Whether a record of this kind belongs in the walk.
    pub fn admits(self, is_class: bool) -> bool {
        match self {
            Self::Instances => !is_class,
            Self::Classes => is_class,
            Self::Both => true,
        }
    }

    /// The flag spelling `firstObj`/`nextObj` take, or `None` when it names no
    /// kind at all — a walk over nothing is a mistake rather than an empty
    /// answer.
    pub fn from_flags(flags: u64) -> Option<Self> {
        match flags {
            1 => Some(Self::Instances),
            2 => Some(Self::Classes),
            3 => Some(Self::Both),
            _ => None,
        }
    }
}

impl Objects<'_> {
    fn ready(&self) -> Result<(), Error> {
        if self.store.terminal {
            Err(Error::Terminal)
        } else {
            Ok(())
        }
    }

    fn resource_failure<T>(&mut self, error: Error) -> Result<T, Error> {
        while let Some(root) = self.store.roots.pop() {
            self.store.destroy(root);
        }
        self.store.terminal = true;
        self.store.records = HashMap::new();
        self.store.statics = HashMap::new();
        self.store.cleanup = Vec::new();
        self.store.roots = Vec::new();
        Err(error)
    }

    fn prospective_cycle(&mut self, parent: ObjectRef, child: ObjectRef) -> Result<bool, Error> {
        self.store.record(parent)?;
        self.store.record(child)?;
        let mut ancestor = Some(parent);
        while let Some(object) = ancestor {
            if object == child {
                return Ok(true);
            }
            ancestor = self.store.record(object)?.parent;
        }
        if self.store.pending_constructions == 0 {
            return Ok(false);
        }
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        let reserve = self.store.reservation().and_then(|()| {
            pending
                .try_reserve(self.store.records.len())
                .map_err(|_| Error::Allocation)?;
            seen.try_reserve(self.store.records.len())
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        seen.insert(child);
        pending.push(child);
        while let Some(object) = pending.pop() {
            if object == parent {
                return Ok(true);
            }
            let record = self.store.record(object)?;
            for next in record.children.iter().copied().chain(
                record
                    .pending_constructions
                    .values()
                    .map(|entry| entry.object),
            ) {
                if seen.insert(next) {
                    pending.push(next);
                }
            }
        }
        Ok(false)
    }

    /// Ordered exclusive recipients for published construction. Each private slot
    /// supplies the existing revision-checked handoff; membership itself is a borrow.
    pub fn new_owned_collection(&mut self) -> Result<ObjectRef, Error> {
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new object")
            .collection = Some(Vec::new());
        Ok(object)
    }

    fn owned_members(&self, collection: ObjectRef) -> Result<&[OwnedMember], Error> {
        self.ready()?;
        self.store
            .record(collection)?
            .collection
            .as_deref()
            .ok_or(Error::WrongType)
    }

    pub fn owned_collection_len(&self, collection: ObjectRef) -> Result<usize, Error> {
        Ok(self.owned_members(collection)?.len())
    }

    pub fn owned_collection_get(
        &self,
        collection: ObjectRef,
        index: i32,
    ) -> Result<ObjectRef, Error> {
        let index = usize::try_from(index)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .ok_or(Error::InvalidArgument)?;
        let member = self
            .owned_members(collection)?
            .get(index)
            .ok_or(Error::InvalidArgument)?;
        self.store.record(member.object)?;
        Ok(member.object)
    }

    /// Move a completed collection member into an invocation owner. Ordinary
    /// objects and nested calls already protected by an activation need no transfer.
    pub fn begin_member_call(&mut self, object: ObjectRef) -> Result<Option<MemberCall>, Error> {
        self.ready()?;
        let Some(slot) = self.store.record(object)?.parent else {
            return Ok(None);
        };
        let Some(collection) = self.store.record(slot)?.parent else {
            return Ok(None);
        };
        let record = self.store.record(collection)?;
        if !record
            .collection
            .as_ref()
            .is_some_and(|members| members.iter().any(|m| m.slot == slot && m.object == object))
        {
            return Ok(None);
        }
        if self
            .store
            .record(slot)?
            .owned_value_fields
            .get(&PropertyId(0))
            != Some(&object)
        {
            return Err(Error::NotOwned);
        }
        let revision = self.store.next_reservation;
        let Some(next) = revision.checked_add(1) else {
            return self.resource_failure(Error::IdentityExhausted);
        };
        let activation = self.create()?;
        let reserve = self.store.reservation().and_then(|()| {
            self.store
                .records
                .get_mut(&activation.logical)
                .expect("new activation")
                .children
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
            self.store
                .records
                .get_mut(&slot.logical)
                .expect("member slot")
                .pending_constructions
                .try_reserve(1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        // The previous child/field capacity remains available for the return edge.
        let record = self
            .store
            .records
            .get_mut(&slot.logical)
            .expect("member slot");
        record.owned_value_fields.remove(&PropertyId(0));
        record.properties.insert(PropertyId(0), Value::Nil);
        record.children.retain(|child| *child != object);
        record
            .pending_constructions
            .insert(PropertyId(0), PendingConstruction { object, revision });
        self.store
            .records
            .get_mut(&activation.logical)
            .expect("activation")
            .children
            .push(object);
        self.store
            .records
            .get_mut(&object.logical)
            .expect("member")
            .parent = Some(activation);
        self.store.next_reservation = next;
        self.store.pending_constructions += 1;
        Ok(Some(MemberCall {
            handoff: PublishedConstruction {
                construction: Construction {
                    active: Some((activation, object)),
                },
                reservation: Some(Reservation {
                    recipient: slot,
                    property: PropertyId(0),
                    revision,
                }),
            },
        }))
    }

    pub fn finish_member_call(&mut self, call: &mut MemberCall) -> Result<ObjectRef, Error> {
        self.finish_published_construction(&mut call.handoff)
    }

    /// A withdrawn executing member may explicitly register a new recipient.
    pub fn register_executing_member(
        &mut self,
        collection: ObjectRef,
        call: &mut MemberCall,
    ) -> Result<(), Error> {
        self.register_owned_construction(collection, &mut call.handoff)
    }

    /// Publish membership only after the slot, handoff, and vector capacity exist.
    /// No author code runs between reservation and membership publication.
    pub fn register_owned_construction(
        &mut self,
        collection: ObjectRef,
        construction: &mut PublishedConstruction,
    ) -> Result<(), Error> {
        self.owned_members(collection)?;
        let object = construction.reference().ok_or(Error::InvalidArgument)?;
        self.store.record(object)?;
        if construction
            .reservation
            .is_some_and(|r| self.reservation_live(r, object))
        {
            return Err(Error::NotOwned);
        }
        if self.prospective_cycle(collection, object)? {
            return Err(Error::OwnershipCycle);
        }
        let reserve = self.store.reservation().and_then(|()| {
            self.store
                .records
                .get_mut(&collection.logical)
                .expect("validated collection")
                .collection
                .as_mut()
                .expect("collection")
                .try_reserve(1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        let slot = self.create()?;
        let result = (|| {
            self.adopt(collection, slot)?;
            self.reserve_construction(construction, slot, PropertyId(0))?;
            Ok(())
        })();
        if let Err(error) = result {
            if !self.store.terminal {
                if self.store.record(slot)?.parent.is_some() {
                    let record = self
                        .store
                        .records
                        .get_mut(&collection.logical)
                        .expect("collection");
                    record.children.retain(|child| *child != slot);
                    self.store.destroy(slot);
                } else {
                    self.destroy(slot)?;
                }
            }
            return Err(error);
        }
        self.store
            .records
            .get_mut(&collection.logical)
            .expect("validated collection")
            .collection
            .as_mut()
            .expect("collection")
            .push(OwnedMember { slot, object });
        Ok(())
    }

    /// Transfer a completed local root into an ordered collection recipient.
    pub(crate) fn move_root_into_collection(
        &mut self,
        collection: ObjectRef,
        root: ObjectRef,
    ) -> Result<(), Error> {
        self.owned_members(collection)?;
        let record = self.store.record(root)?;
        // A member is held through a private slot field, and that field takes
        // text and lists as readily as an object reference, so a collection
        // can own any of the three.
        if record.parent.is_some() {
            return Err(Error::NotOwned);
        }
        if self.prospective_cycle(collection, root)? {
            return Err(Error::OwnershipCycle);
        }
        let reserve = self.store.reservation().and_then(|()| {
            self.store
                .records
                .get_mut(&collection.logical)
                .expect("collection")
                .collection
                .as_mut()
                .expect("collection")
                .try_reserve(1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        let slot = self.create()?;
        let result = self
            .adopt(collection, slot)
            .and_then(|()| self.move_root_into_field(slot, PropertyId(0), root));
        if let Err(error) = result {
            if !self.store.terminal {
                if self.store.record(slot)?.parent.is_some() {
                    self.store
                        .records
                        .get_mut(&collection.logical)
                        .expect("collection")
                        .children
                        .retain(|child| *child != slot);
                    self.store.destroy(slot);
                } else {
                    self.destroy(slot)?;
                }
            }
            return Err(error);
        }
        self.store
            .records
            .get_mut(&collection.logical)
            .expect("collection")
            .collection
            .as_mut()
            .expect("collection")
            .push(OwnedMember { slot, object: root });
        Ok(())
    }

    /// Reparent a member's private recipient slot without changing its identity
    /// or pending return edge. An absent member is a checked no-op.
    pub(crate) fn move_owned_member(
        &mut self,
        source: ObjectRef,
        object: ObjectRef,
        destination: ObjectRef,
    ) -> Result<bool, Error> {
        self.store.record(object)?;
        self.owned_members(destination)?;
        let Some(index) = self
            .owned_members(source)?
            .iter()
            .position(|m| m.object == object)
        else {
            return Ok(false);
        };
        if source == destination {
            return Ok(true);
        }
        let member = self.owned_members(source)?[index];
        // During a call/construction the object is under an activation, but its
        // reserved return edge still points to this slot. Check both edges.
        if self.prospective_cycle(destination, member.slot)?
            || self.prospective_cycle(destination, object)?
        {
            return Err(Error::OwnershipCycle);
        }
        let reserve = self.store.reservation().and_then(|()| {
            let record = self
                .store
                .records
                .get_mut(&destination.logical)
                .expect("destination");
            record
                .children
                .try_reserve(record.pending_constructions.len() + 1)
                .map_err(|_| Error::Allocation)?;
            record
                .collection
                .as_mut()
                .expect("collection")
                .try_reserve(1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        // No allocation or author code after detaching the slot.
        let record = self.store.records.get_mut(&source.logical).expect("source");
        record
            .collection
            .as_mut()
            .expect("collection")
            .remove(index);
        record.children.retain(|child| *child != member.slot);
        self.store
            .records
            .get_mut(&member.slot.logical)
            .expect("slot")
            .parent = Some(destination);
        let record = self
            .store
            .records
            .get_mut(&destination.logical)
            .expect("destination");
        record.children.push(member.slot);
        record.collection.as_mut().expect("collection").push(member);
        Ok(true)
    }

    /// Withdrawal cancels pending ownership, or destroys the completed owned member.
    /// Active construction remains owned by its activation until finalization.
    pub fn remove_owned_member(
        &mut self,
        collection: ObjectRef,
        object: ObjectRef,
    ) -> Result<bool, Error> {
        self.store.record(object)?;
        let Some(index) = self
            .owned_members(collection)?
            .iter()
            .position(|member| member.object == object)
        else {
            return Ok(false);
        };
        let record = self
            .store
            .records
            .get_mut(&collection.logical)
            .expect("validated collection");
        let member = record
            .collection
            .as_mut()
            .expect("collection")
            .remove(index);
        record.children.retain(|child| *child != member.slot);
        self.store.destroy(member.slot);
        Ok(true)
    }

    pub fn begin_published_construction(
        &mut self,
        prototype: ObjectRef,
    ) -> Result<PublishedConstruction, Error> {
        Ok(PublishedConstruction {
            construction: self.begin_construction(prototype)?,
            reservation: None,
        })
    }

    fn reservation_live(&self, reservation: Reservation, object: ObjectRef) -> bool {
        self.store
            .record(reservation.recipient)
            .ok()
            .and_then(|record| record.pending_constructions.get(&reservation.property))
            .is_some_and(|pending| {
                pending.object == object && pending.revision == reservation.revision
            })
    }

    /// Reserve exactly one empty field. Ordinary set/clear cancels its pending claim.
    pub fn reserve_construction(
        &mut self,
        construction: &mut PublishedConstruction,
        recipient: ObjectRef,
        property: PropertyId,
    ) -> Result<(), Error> {
        self.ready()?;
        let object = construction.reference().ok_or(Error::InvalidArgument)?;
        self.store.record(object)?;
        if construction
            .reservation
            .is_some_and(|reservation| self.reservation_live(reservation, object))
        {
            return Err(Error::NotOwned);
        }
        let record = self.store.record(recipient)?;
        if record.text.is_some()
            || record.list.is_some()
            || record.dictionary.is_some()
            || record.collection.is_some()
        {
            return Err(Error::WrongType);
        }
        if record.pending_constructions.contains_key(&property)
            || record.owned_value_fields.contains_key(&property)
            || !matches!(self.get(recipient, property)?, Value::Nil)
        {
            return Err(Error::NotOwned);
        }
        if self.prospective_cycle(recipient, object)? {
            return Err(Error::OwnershipCycle);
        }
        let revision = self.store.next_reservation;
        let Some(next) = revision.checked_add(1) else {
            return self.resource_failure(Error::IdentityExhausted);
        };
        let reserve = self.store.reservation().and_then(|()| {
            let record = self
                .store
                .records
                .get_mut(&recipient.logical)
                .expect("validated recipient");
            let extra = record
                .pending_constructions
                .len()
                .checked_add(1)
                .ok_or(Error::ResourceLimit)?;
            record
                .pending_constructions
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
            record
                .children
                .try_reserve(extra)
                .map_err(|_| Error::Allocation)?;
            record
                .owned_value_fields
                .try_reserve(extra)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        // Admit the property before publishing the reservation. No author callback.
        self.set(recipient, property, Value::Nil)?;
        self.store.next_reservation = next;
        self.store.pending_constructions += 1;
        self.store
            .records
            .get_mut(&recipient.logical)
            .expect("validated recipient")
            .pending_constructions
            .insert(property, PendingConstruction { object, revision });
        construction.reservation = Some(Reservation {
            recipient,
            property,
            revision,
        });
        Ok(())
    }

    /// Same finalization on normal return and source throw, after author finally.
    /// All handoff capacity was reserved earlier; no callback or allocation here.
    pub fn finish_published_construction(
        &mut self,
        construction: &mut PublishedConstruction,
    ) -> Result<ObjectRef, Error> {
        let (activation, object) = construction
            .construction
            .active
            .ok_or(Error::InvalidArgument)?;
        if activation.session != self.store.session {
            return Err(Error::ForeignSession);
        }
        self.ready()?;
        self.store.record(object)?;
        if let Some(reservation) = construction
            .reservation
            .filter(|reservation| self.reservation_live(*reservation, object))
        {
            let recipient = reservation.recipient;
            let record = self
                .store
                .records
                .get_mut(&activation.logical)
                .expect("active construction");
            let index = record
                .children
                .iter()
                .position(|child| *child == object)
                .expect("activation owns instance");
            record.children.remove(index);
            self.store
                .records
                .get_mut(&object.logical)
                .expect("live instance")
                .parent = Some(recipient);
            let record = self
                .store
                .records
                .get_mut(&recipient.logical)
                .expect("live reserved recipient");
            record.pending_constructions.remove(&reservation.property);
            self.store.pending_constructions -= 1;
            record
                .properties
                .insert(reservation.property, Value::Reference(object));
            record
                .owned_value_fields
                .insert(reservation.property, object);
            record.children.push(object);
        } else if self.store.lifetimes {
            // Nothing reserved a field for it, so the **turn** owns it.
            // The activation is about to be destroyed
            // and destruction cascades to its children, so the instance has to
            // come out of the activation first and go back to being a root.
            // `release_turn_records` frees it at the end of the cycle unless
            // world state has kept a reference, in which case storing it
            // promoted it already.
            //
            // Without this, `new Thing` handed back a reference to an object
            // it had just destroyed: the allocation appeared to work and the
            // first property read faulted.
            if self.store.roots.try_reserve(1).is_err() {
                self.resource_failure::<()>(Error::Allocation)?;
            }
            let record = self
                .store
                .records
                .get_mut(&activation.logical)
                .expect("active construction");
            if let Some(index) = record.children.iter().position(|child| *child == object) {
                record.children.remove(index);
            }
            self.store
                .records
                .get_mut(&object.logical)
                .expect("live instance")
                .parent = None;
            self.store.roots.push(object);
        }
        construction.construction.active = None;
        construction.reservation = None;
        self.destroy(activation)?;
        Ok(object)
    }

    /// Establish identity and exclusive activation ownership before author code.
    pub fn begin_construction(&mut self, prototype: ObjectRef) -> Result<Construction, Error> {
        self.ready()?;
        let record = self.store.record(prototype)?;
        if record.closure.is_some()
            || record.text.is_some()
            || record.list.is_some()
            || record.dictionary.is_some()
            || record.collection.is_some()
        {
            return Err(Error::WrongType);
        }
        let activation = self.create()?;
        let result = (|| {
            let object = self.create()?;
            self.adopt(activation, object)?;
            self.add_prototype(object, prototype)?;
            Ok(object)
        })();
        match result {
            Ok(object) => Ok(Construction {
                active: Some((activation, object)),
            }),
            Err(error) => {
                // Allocation failures already terminalize and clean the store.
                if !self.store.terminal {
                    self.destroy(activation)?;
                }
                Err(error)
            }
        }
    }

    /// Finish successful owned construction into an explicit owning field.
    /// No constructor callbacks run here; failed admission leaves prior ordinary
    /// effects intact, and the unfinished activation is released.
    pub fn finish_construction(
        &mut self,
        construction: &mut Construction,
        destination: ObjectRef,
        property: PropertyId,
    ) -> Result<ObjectRef, Error> {
        let (activation, object) = construction.active.ok_or(Error::InvalidArgument)?;
        // A foreign caller cannot consume the original store's activation.
        if activation.session != self.store.session {
            return Err(Error::ForeignSession);
        }
        let result = self.assign_owned_value(
            destination,
            property,
            object,
            activation,
            Value::Reference(object),
        );
        construction.active = None;
        if !self.store.terminal {
            self.destroy(activation)?;
        }
        result.map(|()| object)
    }

    /// Transfer a successful constructor into a local root without allocating.
    /// Replace the private activation's root slot; its object keeps identity and
    /// descendants, and no ordinary alias gains ownership.
    pub fn finish_local_construction(
        &mut self,
        construction: &mut Construction,
    ) -> Result<ObjectRef, Error> {
        self.ready()?;
        let (activation, object) = construction.active.ok_or(Error::InvalidArgument)?;
        let parent = self.store.record(activation)?;
        let child_index = parent
            .children
            .iter()
            .position(|child| *child == object)
            .ok_or(Error::NotOwned)?;
        if self.store.record(object)?.parent != Some(activation) {
            return Err(Error::NotOwned);
        }
        let root_index = self
            .store
            .roots
            .iter()
            .position(|root| *root == activation)
            .ok_or(Error::NotOwned)?;
        self.store
            .records
            .get_mut(&activation.logical)
            .expect("validated activation")
            .children
            .remove(child_index);
        self.store
            .records
            .get_mut(&object.logical)
            .expect("validated instance")
            .parent = None;
        self.store.roots[root_index] = object;
        construction.active = None;
        self.store.destroy(activation);
        Ok(object)
    }

    /// Transfer a completed constructor activation to a source exception outcome.
    /// This does not allocate, copy ownership, run author code or publish a field.
    pub fn finish_exception(
        &mut self,
        construction: &mut Construction,
    ) -> Result<ExceptionOwner, Error> {
        self.ready()?;
        let (activation, object) = construction.active.ok_or(Error::InvalidArgument)?;
        self.store.record(activation)?;
        let record = self.store.record(object)?;
        if record.parent != Some(activation) {
            return Err(Error::NotOwned);
        }
        construction.active = None;
        Ok(ExceptionOwner {
            active: Some((activation, object)),
        })
    }

    /// Release a handled, superseded or unhandled owned exception exactly once.
    /// A foreign store cannot consume the owning store's token.
    pub fn release_exception(&mut self, owner: &mut ExceptionOwner) -> Result<(), Error> {
        let (activation, _) = owner.active.ok_or(Error::InvalidArgument)?;
        if activation.session != self.store.session {
            return Err(Error::ForeignSession);
        }
        self.ready()?;
        self.store.record(activation)?;
        owner.active = None;
        self.destroy(activation)
    }

    /// Unpublished/owned-result failure returns no owner and does not write a destination.
    pub fn abort_construction(&mut self, construction: &mut Construction) -> Result<(), Error> {
        let (activation, _) = construction.active.ok_or(Error::InvalidArgument)?;
        if activation.session != self.store.session {
            return Err(Error::ForeignSession);
        }
        construction.active = None;
        if self.store.terminal {
            return Err(Error::Terminal);
        }
        self.destroy(activation)
    }

    pub fn create(&mut self) -> Result<ObjectRef, Error> {
        self.ready()?;
        if self.store.records.len() >= self.store.limits.objects {
            return self.resource_failure(Error::ResourceLimit);
        }
        let Some(next) = self
            .store
            .next_identity
            .checked_add(1)
            .filter(|n| *n <= u32::MAX as u64)
        else {
            return self.resource_failure(Error::IdentityExhausted);
        };
        let Some(count) = self.store.records.len().checked_add(1) else {
            return self.resource_failure(Error::ResourceLimit);
        };
        let reserve = self.store.reservation().and_then(|()| {
            self.store
                .roots
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
            self.store
                .records
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
            self.store
                .cleanup
                .try_reserve(count)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        // a freed slot is handed out again, with a fresh incarnation.
        // The boundary carries both halves of a handle, so a reference to the
        // previous occupant reads as expired rather than naming its successor.
        let logical = match self.store.free_slots.pop() {
            Some(slot) => slot,
            None => self.store.next_identity,
        };
        let object = ObjectRef {
            session: self.store.session,
            logical,
            incarnation: self.store.next_identity,
        };
        self.store.records.insert(
            object.logical,
            Record {
                incarnation: object.incarnation,
                lifetime: self.store.default_lifetime,
                live: true,
                is_class: false,
                closure: None,
                transient: false,
                text: None,
                string_buffer: None,
                list: None,
                list_remaining: None,
                vector: None,
                lookup: None,
                vector_charge: 0,
                dictionary: None,
                collection: None,
                parent: None,
                prototypes: Vec::new(),
                children: Vec::new(),
                properties: HashMap::new(),
                owned_value_fields: HashMap::new(),
                pending_constructions: HashMap::new(),
            },
        );
        self.store.roots.push(object);
        self.store.next_identity = next;
        Ok(object)
    }

    /// The class new records take from here on.
    /// Select the World/Turn lifetime model for this program.
    pub fn set_lifetime_model(&mut self, on: bool) {
        self.store.lifetimes = on;
    }

    pub fn set_default_lifetime(&mut self, lifetime: Lifetime) {
        self.store.default_lifetime = lifetime;
    }

    /// Record property writes, or stop recording. World construction runs with
    /// this off, so nothing before the first cycle can be undone.
    pub fn set_journalling(&mut self, on: bool) {
        self.store.journalling = on;
    }

    /// Take the cycle's property writes, leaving the journal empty.
    pub fn take_property_journal(&mut self) -> Vec<PropertyDelta> {
        std::mem::take(&mut self.store.journal)
    }

    /// How many property writes the current cycle has recorded.
    pub fn property_journal_len(&self) -> usize {
        self.store.journal.len()
    }

    /// Put each write back as it was, newest first. Reverting is not itself
    /// recorded. A delta naming a record that no longer exists is skipped
    /// rather than refused: a turn record freed with its cycle cannot be
    /// restored and nothing can still be looking at it. Answers how many
    /// writes were put back.
    pub fn revert_properties(&mut self, deltas: &[PropertyDelta]) -> usize {
        let recording = self.store.journalling;
        self.store.journalling = false;
        let mut reverted = 0;
        for delta in deltas.iter().rev() {
            if self.store.record(delta.object).is_err() {
                continue;
            }
            let restored = match delta.previous {
                Some(value) => self.set(delta.object, delta.property, value),
                None => self.clear_property(delta.object, delta.property),
            };
            if restored.is_ok() {
                reverted += 1;
            }
        }
        self.store.journalling = recording;
        reverted
    }

    /// Remove a property a write had created, so reverting that write leaves no
    /// trace of it. Mirrors `set`'s owned-field bookkeeping.
    pub fn clear_property(&mut self, object: ObjectRef, property: PropertyId) -> Result<(), Error> {
        self.ready()?;
        if !self
            .store
            .record(object)?
            .properties
            .contains_key(&property)
        {
            return Ok(());
        }
        if self.store.journalling
            && self.store.record(object)?.lifetime == Lifetime::World
            && !self
                .store
                .journal
                .iter()
                .any(|d| d.object == object && d.property == property)
        {
            let previous = self
                .store
                .record(object)?
                .properties
                .get(&property)
                .copied();
            if self.store.journal.len() >= self.store.limits.properties {
                return Err(Error::ResourceLimit);
            }
            self.store
                .journal
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
            self.store.journal.push(PropertyDelta {
                object,
                property,
                previous,
            });
        }
        if let Some(previous) = self
            .store
            .record(object)?
            .owned_value_fields
            .get(&property)
            .copied()
        {
            let record = self
                .store
                .records
                .get_mut(&object.logical)
                .expect("validated object");
            record.owned_value_fields.remove(&property);
            if let Some(index) = record.children.iter().position(|child| *child == previous) {
                record.children.remove(index);
            }
            self.store.destroy(previous);
        }
        self.store
            .records
            .get_mut(&object.logical)
            .expect("validated object")
            .properties
            .remove(&property);
        self.store.properties = self.store.properties.saturating_sub(1);
        Ok(())
    }

    /// Put one record in a class directly. Module data such as the text a
    /// literal stands for is world-lived however it was made.
    pub fn set_lifetime(&mut self, object: ObjectRef, lifetime: Lifetime) -> Result<(), Error> {
        self.store.record(object)?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked record")
            .lifetime = lifetime;
        Ok(())
    }

    pub fn lifetime(&self, object: ObjectRef) -> Result<Lifetime, Error> {
        Ok(self.store.record(object)?.lifetime)
    }

    /// Move a value and everything it reaches into world state. A value stored
    /// into a World record is promoted at the store, so an author never writes
    /// this; the destination decides. Returns how many records changed class.
    pub fn promote(&mut self, object: ObjectRef) -> Result<usize, Error> {
        self.ready()?;
        // Storing a reference that has already expired stays allowed; the read
        // reports it, so promotion passes over what is no longer there.
        let mut pending = Vec::new();
        pending.try_reserve(1).map_err(|_| Error::Allocation)?;
        pending.push(object);
        let mut promoted = 0;
        while let Some(current) = pending.pop() {
            // A stale reference inside world state is reported by its own read,
            // not by promotion, so skip what is no longer there.
            let Ok(record) = self.store.record(current) else {
                continue;
            };
            if record.lifetime == Lifetime::World {
                continue;
            }
            let mut reached = Vec::new();
            let visit = |value: &Value| match value {
                Value::Reference(r) | Value::Text(r) | Value::List(r) => Some(*r),
                _ => None,
            };
            for value in record.properties.values() {
                if let Some(r) = visit(value) {
                    reached.try_reserve(1).map_err(|_| Error::Allocation)?;
                    reached.push(r);
                }
            }
            for list in [record.list.as_ref(), record.vector.as_ref()] {
                for value in list.into_iter().flatten() {
                    if let Some(r) = visit(value) {
                        reached.try_reserve(1).map_err(|_| Error::Allocation)?;
                        reached.push(r);
                    }
                }
            }
            for child in &record.children {
                reached.try_reserve(1).map_err(|_| Error::Allocation)?;
                reached.push(*child);
            }
            self.store
                .records
                .get_mut(&current.logical)
                .expect("checked record")
                .lifetime = Lifetime::World;
            promoted += 1;
            pending
                .try_reserve(reached.len())
                .map_err(|_| Error::Allocation)?;
            pending.extend(reached);
        }
        Ok(promoted)
    }

    /// Free every Turn record together and return their slots to the free list.
    /// A Turn record still held by world state is promoted instead,
    /// which should not happen once promotion runs at every store.
    pub fn release_turn_records(&mut self) -> Result<usize, Error> {
        self.ready()?;
        let mut held = Vec::new();
        let mut roots = Vec::new();
        for (logical, record) in &self.store.records {
            if record.lifetime != Lifetime::Turn {
                continue;
            }
            let reference = ObjectRef {
                session: self.store.session,
                logical: *logical,
                incarnation: record.incarnation,
            };
            let target = match record.parent {
                // Held by world state: promote rather than free. Promotion at
                // every store should already have done this.
                Some(parent)
                    if self
                        .store
                        .records
                        .get(&parent.logical)
                        .is_some_and(|p| p.lifetime == Lifetime::World) =>
                {
                    &mut held
                }
                // Held by another turn record: it goes when that one does.
                Some(_) => continue,
                None => &mut roots,
            };
            target.try_reserve(1).map_err(|_| Error::Allocation)?;
            target.push(reference);
        }
        for object in held {
            self.promote(object)?;
        }
        let mut released = 0;
        for object in roots {
            // A nested Turn record goes with the one that owns it.
            if !self.store.records.contains_key(&object.logical) {
                continue;
            }
            if let Some(index) = self.store.roots.iter().position(|r| *r == object) {
                self.store.roots.remove(index);
            }
            let before = self.store.records.len();
            self.store.destroy(object);
            released += before - self.store.records.len();
        }
        // nothing is swept here. Destroying a record is the only way a
        // record leaves the store, and it puts the slot back on the free list as
        // it goes, so rediscovering freed slots by walking the whole identity
        // space cost the cycle a scan of every identity ever issued — with a
        // linear membership test inside it — for a list that was already right.
        Ok(released)
    }

    /// A dictionary owns its spelling buffers; vocabulary objects are checked borrows.
    pub fn new_dictionary(&mut self) -> Result<ObjectRef, Error> {
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new object")
            .dictionary = Some(Vec::new());
        Ok(object)
    }

    fn dictionary(&self, dictionary: ObjectRef) -> Result<&[DictionaryEntry], Error> {
        self.ready()?;
        self.store
            .record(dictionary)?
            .dictionary
            .as_deref()
            .ok_or(Error::WrongType)
    }

    /// Exact, case-sensitive default comparison; identical associations are idempotent.
    pub fn dictionary_add(
        &mut self,
        dictionary: ObjectRef,
        object: ObjectRef,
        word: &str,
        property: PropertyId,
    ) -> Result<(), Error> {
        if self.store.lifetimes && self.store.record(dictionary)?.lifetime == Lifetime::World {
            self.promote(object)?;
        }
        self.journal_container(dictionary)?;
        self.dictionary(dictionary)?;
        self.store.record(object)?;
        if self
            .dictionary(dictionary)?
            .iter()
            .any(|e| e.word == word && e.object == object && e.property == property)
        {
            return Ok(());
        }
        let Some(properties) = self.store.properties.checked_add(1).filter(|n| {
            n.saturating_add(self.store.inheritance_edges) <= self.store.limits.properties
        }) else {
            return self.resource_failure(Error::ResourceLimit);
        };
        let Some(bytes) = self
            .store
            .string_bytes
            .checked_add(word.len())
            .filter(|n| *n <= self.store.limits.string_bytes)
        else {
            return self.resource_failure(Error::ResourceLimit);
        };
        let mut spelling = String::new();
        let reserve = self.store.reservation().and_then(|()| {
            spelling
                .try_reserve_exact(word.len())
                .map_err(|_| Error::Allocation)?;
            self.store
                .records
                .get_mut(&dictionary.logical)
                .expect("validated dictionary")
                .dictionary
                .as_mut()
                .expect("validated kind")
                .try_reserve(1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        spelling.push_str(word);
        self.store
            .records
            .get_mut(&dictionary.logical)
            .expect("validated dictionary")
            .dictionary
            .as_mut()
            .expect("validated kind")
            .push(DictionaryEntry {
                word: spelling,
                object,
                property,
            });
        self.store.properties = properties;
        self.store.string_bytes = bytes;
        Ok(())
    }

    /// Removal also permits expired local targets, so a stale association is removable.
    pub fn dictionary_remove(
        &mut self,
        dictionary: ObjectRef,
        object: ObjectRef,
        word: &str,
        property: PropertyId,
    ) -> Result<(), Error> {
        self.journal_container(dictionary)?;
        self.dictionary(dictionary)?;
        if object.session != self.store.session {
            return Err(Error::ForeignSession);
        }
        let entries = self
            .store
            .records
            .get_mut(&dictionary.logical)
            .expect("validated dictionary")
            .dictionary
            .as_mut()
            .expect("validated kind");
        if let Some(index) = entries
            .iter()
            .position(|e| e.word == word && e.object == object && e.property == property)
        {
            let removed = entries.remove(index);
            self.store.properties -= 1;
            self.store.string_bytes -= removed.word.len();
        }
        Ok(())
    }

    pub fn dictionary_defined(&self, dictionary: ObjectRef, word: &str) -> Result<bool, Error> {
        Ok(self
            .dictionary(dictionary)?
            .iter()
            .any(|entry| entry.word == word))
    }

    /// Allocation-free part-of-speech query used by grammar matching.
    pub fn dictionary_has(
        &self,
        dictionary: ObjectRef,
        word: &str,
        property: PropertyId,
    ) -> Result<bool, Error> {
        Ok(self
            .dictionary(dictionary)?
            .iter()
            .any(|entry| entry.word == word && entry.property == property))
    }

    /// Each result carries the default comparator's match code, including repeated
    /// objects with distinct vocabulary properties. Results never own their targets.
    pub fn dictionary_find(
        &mut self,
        dictionary: ObjectRef,
        word: &str,
        property: Option<PropertyId>,
    ) -> Result<Vec<(ObjectRef, i32)>, Error> {
        let matches = |entry: &&DictionaryEntry| {
            entry.word == word && property.is_none_or(|p| p == entry.property)
        };
        let count = self.dictionary(dictionary)?.iter().filter(matches).count();
        for entry in self.dictionary(dictionary)?.iter().filter(matches) {
            self.store.record(entry.object)?;
        }
        let mut result = Vec::new();
        if count != 0
            && let Err(error) = self.store.reservation().and_then(|()| {
                result
                    .try_reserve_exact(count)
                    .map_err(|_| Error::Allocation)
            })
        {
            return self.resource_failure(error);
        }
        for entry in self.dictionary(dictionary)?.iter().filter(matches) {
            result.push((entry.object, 1));
        }
        Ok(result)
    }

    fn list_buffer(&mut self, length: usize) -> Result<Vec<Value>, Error> {
        self.ready()?;
        let available = self.store.limits.properties.saturating_sub(
            self.store
                .properties
                .saturating_add(self.store.inheritance_edges),
        );
        if length > available {
            return self.resource_failure(Error::ResourceLimit);
        }
        let mut values = Vec::new();
        if length != 0
            && let Err(error) = self.store.reservation().and_then(|()| {
                values
                    .try_reserve_exact(length)
                    .map_err(|_| Error::Allocation)
            })
        {
            return self.resource_failure(error);
        }
        Ok(values)
    }

    fn publish_list(&mut self, values: Vec<Value>) -> Result<ObjectRef, Error> {
        let count = values.len();
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new list")
            .list = Some(values);
        self.store.properties += count;
        Ok(object)
    }

    pub(crate) fn begin_list(&mut self, length: usize) -> Result<ObjectRef, Error> {
        let buffer = self.list_buffer(length)?;
        let object = self.publish_list(buffer)?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new builder")
            .list_remaining = Some(length);
        self.store.properties += length;
        Ok(object)
    }

    pub(crate) fn push_list(
        &mut self,
        builder: ObjectRef,
        value: Value,
        region: ObjectRef,
    ) -> Result<(), Error> {
        self.ready()?;
        self.list_element(value)?;
        if !self
            .store
            .record(builder)?
            .list_remaining
            .is_some_and(|n| n > 0)
        {
            return Err(Error::InvalidArgument);
        }
        if self.store.record(region)?.parent.is_some() || builder == region {
            return Err(Error::NotOwned);
        }
        let child = match value {
            Value::Text(v) => {
                self.text(v)?;
                Some(v)
            }
            Value::List(v) => {
                self.list(v)?;
                Some(v)
            }
            _ => None,
        };
        if let Some(child) = child
            && self.store.record(child)?.parent == Some(region)
        {
            let mut ancestor = Some(builder);
            while let Some(current) = ancestor {
                if current == child {
                    return Err(Error::OwnershipCycle);
                }
                ancestor = self.store.record(current)?.parent;
            }
            let index = self
                .store
                .record(region)?
                .children
                .iter()
                .position(|v| *v == child)
                .ok_or(Error::NotOwned)?;
            let reserve = self.store.reservation().and_then(|()| {
                self.store
                    .records
                    .get_mut(&builder.logical)
                    .expect("builder")
                    .children
                    .try_reserve(1)
                    .map_err(|_| Error::Allocation)
            });
            if let Err(error) = reserve {
                return self.resource_failure(error);
            }
            self.store
                .records
                .get_mut(&region.logical)
                .expect("region")
                .children
                .remove(index);
            self.store
                .records
                .get_mut(&child.logical)
                .expect("child")
                .parent = Some(builder);
            self.store
                .records
                .get_mut(&builder.logical)
                .expect("builder")
                .children
                .push(child);
        }
        let record = self
            .store
            .records
            .get_mut(&builder.logical)
            .expect("builder");
        record.list.as_mut().expect("builder buffer").push(value);
        *record.list_remaining.as_mut().expect("building") -= 1;
        Ok(())
    }

    /// Fill an already admitted call-argument buffer. Unlike static literal
    /// construction, argument capture borrows heap values and never adopts them.
    pub(crate) fn push_constant_list(
        &mut self,
        builder: ObjectRef,
        value: Value,
        region: ObjectRef,
    ) -> Result<(), Error> {
        if self.store.record(builder)?.parent != Some(region) {
            return Err(Error::NotOwned);
        }
        self.push_borrowed_list(builder, value)
    }

    pub(crate) fn push_borrowed_list(
        &mut self,
        builder: ObjectRef,
        value: Value,
    ) -> Result<(), Error> {
        self.ready()?;
        self.list_element(value)?;
        if !self
            .store
            .record(builder)?
            .list_remaining
            .is_some_and(|count| count > 0)
        {
            return Err(Error::InvalidArgument);
        }
        let record = self
            .store
            .records
            .get_mut(&builder.logical)
            .expect("checked builder");
        record.list.as_mut().expect("admitted builder").push(value);
        *record.list_remaining.as_mut().expect("building") -= 1;
        Ok(())
    }

    pub(crate) fn finish_list(&mut self, builder: ObjectRef) -> Result<(), Error> {
        self.ready()?;
        if self.store.record(builder)?.list_remaining != Some(0) {
            return Err(Error::InvalidArgument);
        }
        self.store
            .records
            .get_mut(&builder.logical)
            .expect("builder")
            .list_remaining = None;
        Ok(())
    }

    fn list_element(&self, value: Value) -> Result<(), Error> {
        match value {
            Value::Reference(reference) | Value::Text(reference) | Value::List(reference)
                if reference.session != self.store.session =>
            {
                Err(Error::ForeignSession)
            }
            Value::String(literal) if literal.session != self.store.session => {
                Err(Error::ForeignSession)
            }
            Value::String(literal) => self.store.literal(literal.index).map(|_| ()),
            Value::Method(_) | Value::Initializing => Err(Error::WrongType),
            _ => Ok(()),
        }
    }

    /// The buffer has one owner. Its ordinary elements hold values or checked
    /// borrows; copying a list never duplicates ownership of its target objects.
    pub fn new_list(&mut self, values: &[Value]) -> Result<ObjectRef, Error> {
        self.ready()?;
        for value in values {
            self.list_element(*value)?;
        }
        let mut buffer = self.list_buffer(values.len())?;
        buffer.extend_from_slice(values);
        self.publish_list(buffer)
    }

    /// Allocate one owned callback environment. Code remains a native function ID;
    /// captured values use existing session checks, quotas and structural cleanup.
    pub fn new_closure(&mut self, function: u32, captures: &[Capture]) -> Result<ObjectRef, Error> {
        self.ready()?;
        for capture in captures {
            let valid = matches!(
                capture,
                Capture::Copy(
                    Value::Nil
                        | Value::Bool(_)
                        | Value::Int(_)
                        | Value::Property(_)
                        | Value::Enumerator(_)
                        | Value::Function(_),
                ) | Capture::Reference(
                    Value::Nil
                        | Value::Reference(_)
                        | Value::Text(_)
                        | Value::List(_)
                        | Value::String(_),
                )
            );
            if !valid {
                return Err(Error::WrongType);
            }
            let (Capture::Copy(value) | Capture::Reference(value)) = *capture;
            self.list_element(value)?;
        }
        let mut values = self.list_buffer(captures.len())?;
        for capture in captures {
            let (Capture::Copy(value) | Capture::Reference(value)) = *capture;
            values.push(value);
        }
        let object = self.create()?;
        self.store.properties += values.len();
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new object")
            .closure = Some(ClosureEnvironment { function, values });
        Ok(object)
    }

    pub fn closure_function(&self, closure: ObjectRef) -> Result<u32, Error> {
        self.ready()?;
        self.store
            .record(closure)?
            .closure
            .as_ref()
            .map(|env| env.function)
            .ok_or(Error::WrongType)
    }

    pub fn closure_capture(&self, closure: ObjectRef, index: usize) -> Result<Value, Error> {
        self.ready()?;
        self.store
            .record(closure)?
            .closure
            .as_ref()
            .ok_or(Error::WrongType)?
            .values
            .get(index)
            .copied()
            .ok_or(Error::InvalidArgument)
    }

    /// Immutable snapshot with its own root; elements retain ordinary borrow semantics.
    pub(crate) fn vector_snapshot(&mut self, object: ObjectRef) -> Result<ObjectRef, Error> {
        let mut values = self.list_buffer(self.vector(object)?.len())?;
        values.extend_from_slice(self.vector(object)?);
        self.publish_list(values)
    }

    pub fn list(&self, object: ObjectRef) -> Result<&[Value], Error> {
        self.ready()?;
        let record = self.store.record(object)?;
        if record.list_remaining.is_some() {
            return Err(Error::InitializationCycle);
        }
        record.list.as_deref().ok_or(Error::WrongType)
    }

    pub(crate) fn argument_tail(
        &mut self,
        object: ObjectRef,
        skip: usize,
    ) -> Result<ObjectRef, Error> {
        let length = self
            .list(object)?
            .len()
            .checked_sub(skip)
            .ok_or(Error::InvalidArgument)?;
        let mut values = self.list_buffer(length)?;
        values.extend_from_slice(&self.list(object)?[skip..]);
        self.publish_list(values)
    }

    pub fn list_get(&self, object: ObjectRef, index: i32) -> Result<Value, Error> {
        let values = self.list(object)?;
        let index = usize::try_from(index)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .ok_or(Error::InvalidArgument)?;
        values.get(index).copied().ok_or(Error::InvalidArgument)
    }

    /// An exclusive local/root owner; keys and values never adopt game objects.
    pub fn new_lookup(&mut self) -> Result<ObjectRef, Error> {
        self.ready()?;
        self.lookup_admit(1, 0)?;
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new lookup")
            .lookup = Some(LookupTable {
            buckets: 32,
            reserved_entries: 0,
            scalars: HashMap::new(),
            strings: HashMap::new(),
            default: Value::Nil,
        });
        self.store.properties += 1;
        Ok(object)
    }

    pub fn new_lookup_sized(&mut self, buckets: i32, capacity: i32) -> Result<ObjectRef, Error> {
        self.ready()?;
        if buckets <= 0 || capacity <= 0 {
            return Err(Error::InvalidArgument);
        }
        let capacity = capacity as usize;
        let charge = capacity
            .checked_mul(2)
            .and_then(|n| n.checked_add(1))
            .ok_or(Error::ResourceLimit)?;
        self.lookup_admit(charge, 0)?;
        let mut scalars = HashMap::new();
        let mut strings = HashMap::new();
        let reserve = self.store.reservation().and_then(|()| {
            scalars
                .try_reserve(capacity)
                .map_err(|_| Error::Allocation)?;
            strings.try_reserve(capacity).map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new lookup")
            .lookup = Some(LookupTable {
            buckets,
            reserved_entries: capacity,
            scalars,
            strings,
            default: Value::Nil,
        });
        self.store.properties += charge;
        Ok(object)
    }

    pub fn lookup_bucket_count(&self, object: ObjectRef) -> Result<i32, Error> {
        Ok(self.lookup(object)?.buckets)
    }

    fn lookup(&self, object: ObjectRef) -> Result<&LookupTable, Error> {
        self.ready()?;
        self.store
            .record(object)?
            .lookup
            .as_ref()
            .ok_or(Error::WrongType)
    }

    fn lookup_query(&self, key: Value) -> Result<LookupQuery<'_>, Error> {
        self.list_element(key)?;
        Ok(match key {
            Value::Nil | Value::Bool(false) => LookupQuery::Scalar(LookupKey::Nil),
            Value::Bool(value) => LookupQuery::Scalar(LookupKey::Bool(value)),
            Value::Int(value) => LookupQuery::Scalar(LookupKey::Int(value)),
            Value::Property(value) => LookupQuery::Scalar(LookupKey::Property(value)),
            Value::Enumerator(value) => LookupQuery::Scalar(LookupKey::Enumerator(value)),
            Value::Function(value) => LookupQuery::Scalar(LookupKey::Function(value)),
            Value::String(value) => {
                LookupQuery::Text(crate::tables::installed().literals[value.index as usize])
            }
            Value::Text(value) => LookupQuery::Text(self.text(value)?),
            Value::Reference(value) => {
                let record = self.store.record(value)?;
                // Content-hashed mutable collection keys need their semantic adapter.
                if record.vector.is_some()
                    || record.list.is_some()
                    || record.text.is_some()
                    || record.string_buffer.is_some()
                {
                    return Err(Error::WrongType);
                }
                LookupQuery::Scalar(LookupKey::Object(value))
            }
            _ => return Err(Error::WrongType),
        })
    }

    pub fn indexed_set(
        &mut self,
        object: ObjectRef,
        key: Value,
        value: Value,
    ) -> Result<(), Error> {
        self.ready()?;
        if self.store.record(object)?.lookup.is_some() {
            return self.lookup_set(object, key, value);
        }
        let Value::Int(index) = key else {
            return Err(Error::WrongType);
        };
        self.vector_set(object, index, value)
    }

    pub fn indexed_get(&self, object: ObjectRef, key: Value) -> Result<Value, Error> {
        self.ready()?;
        let record = self.store.record(object)?;
        if record.lookup.is_some() {
            return self.lookup_get(object, key);
        }
        let Value::Int(index) = key else {
            return Err(Error::WrongType);
        };
        if record.vector.is_some() {
            self.vector_get(object, index)
        } else {
            self.list_get(object, index)
        }
    }

    pub fn lookup_get(&self, object: ObjectRef, key: Value) -> Result<Value, Error> {
        let table = self.lookup(object)?;
        let found = match self.lookup_query(key)? {
            LookupQuery::Scalar(key) => table.scalars.get(&key),
            LookupQuery::Text(key) => table.strings.get(key),
        };
        Ok(found.copied().unwrap_or(table.default))
    }

    pub fn lookup_contains(&self, object: ObjectRef, key: Value) -> Result<bool, Error> {
        let table = self.lookup(object)?;
        Ok(match self.lookup_query(key)? {
            LookupQuery::Scalar(key) => table.scalars.contains_key(&key),
            LookupQuery::Text(key) => table.strings.contains_key(key),
        })
    }

    pub fn lookup_len(&self, object: ObjectRef) -> Result<usize, Error> {
        let table = self.lookup(object)?;
        Ok(table.scalars.len() + table.strings.len())
    }

    fn lookup_admit(&mut self, properties: usize, bytes: usize) -> Result<(), Error> {
        if properties
            > self.store.limits.properties.saturating_sub(
                self.store
                    .properties
                    .saturating_add(self.store.inheritance_edges),
            )
            || bytes
                > self
                    .store
                    .limits
                    .string_bytes
                    .saturating_sub(self.store.string_bytes)
        {
            return self.resource_failure(Error::ResourceLimit);
        }
        Ok(())
    }

    pub fn lookup_set(&mut self, object: ObjectRef, key: Value, value: Value) -> Result<(), Error> {
        if self.store.lifetimes
            && self.store.record(object)?.lifetime == Lifetime::World
            && let Value::Reference(reference) | Value::Text(reference) | Value::List(reference) =
                value
        {
            self.promote(reference)?;
        }
        self.journal_container(object)?;
        if self.store.lifetimes
            && self.store.record(object)?.lifetime == Lifetime::World
            && let Value::Reference(reference) = key
        {
            self.promote(reference)?;
        }
        let exists = self.lookup_contains(object, key)?;
        self.list_element(value)?;
        let bytes = match self.lookup_query(key)? {
            LookupQuery::Text(text) => text.len(),
            _ => 0,
        };
        let charge = if self.lookup_len(object)? < self.lookup(object)?.reserved_entries {
            0
        } else {
            2
        };
        if !exists {
            self.lookup_admit(charge, bytes)?;
            if let Err(error) = self.store.reservation() {
                return self.resource_failure(error);
            }
        }
        match self.lookup_query(key)? {
            LookupQuery::Scalar(key) => {
                let table = self
                    .store
                    .records
                    .get_mut(&object.logical)
                    .expect("checked lookup")
                    .lookup
                    .as_mut()
                    .expect("lookup");
                if !exists && table.scalars.try_reserve(1).is_err() {
                    return self.resource_failure(Error::Allocation);
                }
                table.scalars.insert(key, value);
            }
            LookupQuery::Text(text) => {
                if exists {
                    match key {
                        Value::String(literal) => {
                            let text = crate::tables::installed().literals[literal.index as usize];
                            let table = self
                                .store
                                .records
                                .get_mut(&object.logical)
                                .expect("checked lookup")
                                .lookup
                                .as_mut()
                                .expect("lookup");
                            *table.strings.get_mut(text).expect("existing key") = value;
                        }
                        Value::Text(key_object) => {
                            // Validation above proves these are different record kinds.
                            if key_object == object {
                                return Err(Error::WrongType);
                            }
                            let [Some(table_record), Some(key_record)] = self
                                .store
                                .records
                                .get_disjoint_mut([&object.logical, &key_object.logical])
                            else {
                                unreachable!("validated lookup and key")
                            };
                            let text = key_record.text.as_deref().expect("text key");
                            *table_record
                                .lookup
                                .as_mut()
                                .expect("lookup")
                                .strings
                                .get_mut(text)
                                .expect("existing key") = value;
                        }
                        _ => unreachable!("text query"),
                    }
                } else {
                    let mut owned = String::new();
                    if owned.try_reserve_exact(text.len()).is_err() {
                        return self.resource_failure(Error::Allocation);
                    }
                    owned.push_str(text);
                    let table = self
                        .store
                        .records
                        .get_mut(&object.logical)
                        .expect("checked lookup")
                        .lookup
                        .as_mut()
                        .expect("lookup");
                    if table.strings.try_reserve(1).is_err() {
                        return self.resource_failure(Error::Allocation);
                    }
                    table.strings.insert(owned, value);
                }
            }
        }
        if !exists {
            self.store.properties += charge;
            self.store.string_bytes += bytes;
        }
        Ok(())
    }

    pub fn lookup_default(&self, object: ObjectRef) -> Result<Value, Error> {
        Ok(self.lookup(object)?.default)
    }
    pub fn lookup_set_default(&mut self, object: ObjectRef, value: Value) -> Result<(), Error> {
        if self.store.lifetimes
            && self.store.record(object)?.lifetime == Lifetime::World
            && let Value::Reference(reference) | Value::Text(reference) | Value::List(reference) =
                value
        {
            self.promote(reference)?;
        }
        self.journal_container(object)?;
        self.lookup(object)?;
        self.list_element(value)?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked lookup")
            .lookup
            .as_mut()
            .expect("lookup")
            .default = value;
        Ok(())
    }

    /// A mutable value buffer with one structural owner. Elements are ordinary
    /// values or checked borrows; insertion never adopts referenced objects.
    pub fn new_vector(&mut self, capacity: i32) -> Result<ObjectRef, Error> {
        self.ready()?;
        let capacity = usize::try_from(capacity).map_err(|_| Error::InvalidArgument)?;
        let buffer = self.list_buffer(capacity)?;
        let object = self.create()?;
        let record = self
            .store
            .records
            .get_mut(&object.logical)
            .expect("new vector");
        record.vector = Some(buffer);
        record.vector_charge = capacity;
        self.store.properties += capacity;
        Ok(object)
    }

    pub fn vector(&self, object: ObjectRef) -> Result<&[Value], Error> {
        self.ready()?;
        self.store
            .record(object)?
            .vector
            .as_deref()
            .ok_or(Error::WrongType)
    }

    /// The elements of a vector or of a list.
    ///
    /// the two hold values the same way and differ in whether they can
    /// be changed, so a question that only reads them is the same question of
    /// either.
    pub fn sequence(&self, object: ObjectRef) -> Result<&[Value], Error> {
        self.ready()?;
        let record = self.store.record(object)?;
        if let Some(vector) = record.vector.as_deref() {
            return Ok(vector);
        }
        if record.list_remaining.is_some() {
            return Err(Error::InitializationCycle);
        }
        record.list.as_deref().ok_or(Error::WrongType)
    }

    /// First one-based match using the currently supported source equality forms.
    pub(crate) fn vector_index_of(
        &self,
        object: ObjectRef,
        value: Value,
    ) -> Result<Option<i32>, Error> {
        for (index, candidate) in self.sequence(object)?.iter().copied().enumerate() {
            let text = |v| -> Result<Option<&str>, Error> {
                match v {
                    Value::String(v) => {
                        if v.session != self.store.session {
                            return Err(Error::ForeignSession);
                        }
                        self.store.literal(v.index)?;
                        Ok(Some(crate::tables::installed().literals[v.index as usize]))
                    }
                    Value::Text(v) => self.text(v).map(Some),
                    _ => Ok(None),
                }
            };
            let equal = match (candidate, value) {
                (Value::List(_), Value::List(_)) => return Err(Error::WrongType),
                (Value::String(_) | Value::Text(_), Value::String(_) | Value::Text(_)) => {
                    text(candidate)? == text(value)?
                }
                _ => candidate == value,
            };
            if equal {
                return i32::try_from(index + 1)
                    .map(Some)
                    .map_err(|_| Error::NumericOverflow);
            }
        }
        Ok(None)
    }

    fn vector_index(&self, object: ObjectRef, index: i32, negative: bool) -> Result<usize, Error> {
        let length = self.vector(object)?.len();
        let offset = if index < 0 && negative {
            length.checked_sub(index.unsigned_abs() as usize)
        } else {
            usize::try_from(index).ok().and_then(|n| n.checked_sub(1))
        }
        .ok_or(Error::InvalidArgument)?;
        if offset >= length {
            return Err(Error::InvalidArgument);
        }
        Ok(offset)
    }

    pub fn vector_get(&self, object: ObjectRef, index: i32) -> Result<Value, Error> {
        let index = self.vector_index(object, index, false)?;
        Ok(self.vector(object)?[index])
    }

    pub fn vector_set(&mut self, object: ObjectRef, index: i32, value: Value) -> Result<(), Error> {
        if self.store.lifetimes
            && self.store.record(object)?.lifetime == Lifetime::World
            && let Value::Reference(reference) | Value::Text(reference) | Value::List(reference) =
                value
        {
            self.promote(reference)?;
        }
        self.journal_container(object)?;
        let index = self.vector_index(object, index, false)?;
        self.list_element(value)?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked vector")
            .vector
            .as_mut()
            .expect("vector")[index] = value;
        Ok(())
    }

    fn vector_reserve(&mut self, object: ObjectRef, length: usize) -> Result<(), Error> {
        self.journal_container(object)?;
        self.vector(object)?;
        let charge = self.store.record(object)?.vector_charge;
        if length <= charge {
            return Ok(());
        }
        let additional = length - charge;
        let available = self.store.limits.properties.saturating_sub(
            self.store
                .properties
                .saturating_add(self.store.inheritance_edges),
        );
        if additional > available {
            return self.resource_failure(Error::ResourceLimit);
        }
        if let Err(error) = self.store.reservation() {
            return self.resource_failure(error);
        }
        let buffer = self
            .store
            .records
            .get_mut(&object.logical)
            .expect("checked vector")
            .vector
            .as_mut()
            .expect("vector");
        if buffer.try_reserve_exact(length - buffer.len()).is_err() {
            return self.resource_failure(Error::Allocation);
        }
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked vector")
            .vector_charge = length;
        self.store.properties += additional;
        Ok(())
    }

    pub fn append_value(&mut self, object: ObjectRef, value: Value) -> Result<ObjectRef, Error> {
        self.ready()?;
        if self.store.record(object)?.string_buffer.is_some() {
            self.string_buffer_append_value(object, value)
        } else {
            self.vector_append(object, value)
        }
    }
    pub fn object_length(&self, object: ObjectRef) -> Result<usize, Error> {
        self.ready()?;
        if self.store.record(object)?.string_buffer.is_some() {
            Ok(self.string_buffer(object)?.chars().count())
        } else {
            Ok(self.vector(object)?.len())
        }
    }

    pub fn vector_append(&mut self, object: ObjectRef, value: Value) -> Result<ObjectRef, Error> {
        if self.store.lifetimes
            && self.store.record(object)?.lifetime == Lifetime::World
            && let Value::Reference(reference) | Value::Text(reference) | Value::List(reference) =
                value
        {
            self.promote(reference)?;
        }
        self.vector(object)?;
        self.list_element(value)?;
        let Some(length) = self.vector(object)?.len().checked_add(1) else {
            return self.resource_failure(Error::ResourceLimit);
        };
        self.vector_reserve(object, length)?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked vector")
            .vector
            .as_mut()
            .expect("vector")
            .push(value);
        Ok(object)
    }

    pub fn vector_set_length(
        &mut self,
        object: ObjectRef,
        length: i32,
    ) -> Result<ObjectRef, Error> {
        self.vector(object)?;
        let length = usize::try_from(length).map_err(|_| Error::InvalidArgument)?;
        self.vector_reserve(object, length)?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked vector")
            .vector
            .as_mut()
            .expect("vector")
            .resize(length, Value::Nil);
        Ok(object)
    }

    /// Insert a complete borrowed-value batch before the requested position.
    pub(crate) fn vector_insert_values(
        &mut self,
        object: ObjectRef,
        index: i32,
        values: ObjectRef,
    ) -> Result<ObjectRef, Error> {
        let length = self.vector(object)?.len();
        let position = if index == 0 {
            length as i64
        } else if index > 0 {
            i64::from(index) - 1
        } else {
            length as i64 + i64::from(index)
        };
        let position = usize::try_from(position).map_err(|_| Error::InvalidArgument)?;
        if position > length {
            return Err(Error::InvalidArgument);
        }
        let inserted = self.list(values)?.len();
        for index in 0..inserted {
            let value = self.list(values)?[index];
            self.list_element(value)?;
            if self.store.lifetimes
                && self.store.record(object)?.lifetime == Lifetime::World
                && let Value::Reference(reference) | Value::Text(reference) | Value::List(reference) =
                    value
            {
                self.promote(reference)?;
            }
        }
        let total = length.checked_add(inserted).ok_or(Error::NumericOverflow)?;
        self.vector_reserve(object, total)?;
        let [Some(vector_record), Some(list_record)] = self
            .store
            .records
            .get_disjoint_mut([&object.logical, &values.logical])
        else {
            unreachable!("validated vector and list")
        };
        let vector = vector_record.vector.as_mut().expect("vector");
        vector.extend_from_slice(list_record.list.as_deref().expect("list"));
        vector[position..].rotate_right(inserted);
        Ok(object)
    }

    pub fn vector_remove_range(
        &mut self,
        object: ObjectRef,
        start: i32,
        end: i32,
    ) -> Result<ObjectRef, Error> {
        self.journal_container(object)?;
        let start = self.vector_index(object, start, true)?;
        let end = self.vector_index(object, end, true)?;
        if start > end {
            return Err(Error::InvalidArgument);
        }
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked vector")
            .vector
            .as_mut()
            .expect("vector")
            .drain(start..=end);
        Ok(object)
    }

    /// Validate a checked borrow without evaluating any author property.
    /// Persistence metadata only: journal/save support must consume this flag.
    pub fn set_transient(&mut self, object: ObjectRef, transient: bool) -> Result<(), Error> {
        self.journal_container(object)?;
        self.ready()?;
        self.store.record(object)?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked object")
            .transient = transient;
        Ok(())
    }
    pub fn is_transient(&self, object: ObjectRef) -> Result<bool, Error> {
        self.ready()?;
        Ok(self.store.record(object)?.transient)
    }

    pub fn validate_object(&self, object: ObjectRef) -> Result<(), Error> {
        self.ready()?;
        self.store.record(object).map(|_| ())
    }

    /// Snapshot values once so source membership mutation cannot change traversal.
    /// The snapshot owns its buffer, never the objects referred to by its elements.
    pub fn begin_iteration(&mut self, source: Value) -> Result<ValueIteration, Error> {
        let length = match source {
            Value::List(list) => self.list(list)?.len(),
            Value::Reference(collection) => {
                if self.store.record(collection)?.vector.is_some() {
                    self.vector(collection)?.len()
                } else {
                    self.owned_members(collection)?.len()
                }
            }
            _ => return Err(Error::WrongType),
        };
        let mut buffer = self.list_buffer(length)?;
        match source {
            Value::List(list) => buffer.extend_from_slice(self.list(list)?),
            Value::Reference(collection) => {
                if self.store.record(collection)?.vector.is_some() {
                    buffer.extend_from_slice(self.vector(collection)?);
                } else {
                    for member in self.owned_members(collection)? {
                        buffer.push(Value::Reference(member.object));
                    }
                }
            }
            _ => unreachable!(),
        }
        let snapshot = self.publish_list(buffer)?;
        Ok(ValueIteration {
            snapshot: Some(snapshot),
            next: 0,
        })
    }

    pub fn iteration_next(&self, iteration: &mut ValueIteration) -> Result<Option<Value>, Error> {
        let snapshot = iteration.snapshot.ok_or(Error::InvalidArgument)?;
        let values = self.list(snapshot)?;
        let Some(value) = values.get(iteration.next).copied() else {
            return Ok(None);
        };
        // A successful slice access proves the next index is representable.
        iteration.next += 1;
        Ok(Some(value))
    }

    pub fn end_iteration(&mut self, iteration: &mut ValueIteration) -> Result<(), Error> {
        let snapshot = iteration.snapshot.ok_or(Error::InvalidArgument)?;
        self.destroy(snapshot)?;
        iteration.snapshot = None;
        Ok(())
    }

    pub fn list_append(&mut self, object: ObjectRef, value: Value) -> Result<ObjectRef, Error> {
        self.ready()?;
        self.list_element(value)?;
        let Some(length) = self.list(object)?.len().checked_add(1) else {
            return self.resource_failure(Error::ResourceLimit);
        };
        let mut buffer = self.list_buffer(length)?;
        buffer.extend_from_slice(self.list(object)?);
        buffer.push(value);
        self.publish_list(buffer)
    }

    /// Functional replacement leaves the input and all of its borrowers unchanged.
    pub fn list_replace(
        &mut self,
        object: ObjectRef,
        index: i32,
        value: Value,
    ) -> Result<ObjectRef, Error> {
        self.list_get(object, index)?;
        self.list_element(value)?;
        let mut buffer = self.list_buffer(self.list(object)?.len())?;
        buffer.extend_from_slice(self.list(object)?);
        buffer[index as usize - 1] = value;
        self.publish_list(buffer)
    }

    /// Returns the actual alternating object/match-code list needed by findWord.
    pub fn dictionary_find_list(
        &mut self,
        dictionary: ObjectRef,
        word: &str,
        property: Option<PropertyId>,
    ) -> Result<ObjectRef, Error> {
        let matches = |entry: &&DictionaryEntry| {
            entry.word == word && property.is_none_or(|p| p == entry.property)
        };
        let count = self.dictionary(dictionary)?.iter().filter(matches).count();
        for entry in self.dictionary(dictionary)?.iter().filter(matches) {
            self.store.record(entry.object)?;
        }
        let Some(length) = count.checked_mul(2) else {
            return self.resource_failure(Error::ResourceLimit);
        };
        let mut values = self.list_buffer(length)?;
        for entry in self.dictionary(dictionary)?.iter().filter(matches) {
            values.push(Value::Reference(entry.object));
            values.push(Value::Int(1));
        }
        self.publish_list(values)
    }

    /// The entities a word names, as a plain list. `dictionary_find_list`
    /// answers in the reference's `[object, flags,...]` pair shape, which is
    /// the wrong shape for a grammar slot: a capture binds candidates, and a
    /// flags column the author never reads only makes indexing wrong
    ///.
    pub fn dictionary_candidates(
        &mut self,
        dictionary: ObjectRef,
        word: &str,
        property: PropertyId,
    ) -> Result<ObjectRef, Error> {
        let matches = |entry: &&DictionaryEntry| entry.word == word && entry.property == property;
        let count = self.dictionary(dictionary)?.iter().filter(matches).count();
        for entry in self.dictionary(dictionary)?.iter().filter(matches) {
            self.store.record(entry.object)?;
        }
        let mut values = self.list_buffer(count)?;
        for entry in self.dictionary(dictionary)?.iter().filter(matches) {
            values.push(Value::Reference(entry.object));
        }
        self.publish_list(values)
    }

    /// The words that name an entity under one vocabulary property, as a list of
    /// text, in the order they were filed. This is a vocabulary relation read
    /// forward: `names.all(coin)` answers what the coin is called.
    pub fn dictionary_words(
        &mut self,
        dictionary: ObjectRef,
        object: ObjectRef,
        property: PropertyId,
    ) -> Result<ObjectRef, Error> {
        self.store.record(object)?;
        let matches =
            |entry: &&DictionaryEntry| entry.object == object && entry.property == property;
        let count = self.dictionary(dictionary)?.iter().filter(matches).count();
        // Call-scoped Rust workspace: the spellings are copied out before any
        // text object is created, because creating one borrows the store.
        let mut spellings: Vec<String> = Vec::new();
        spellings
            .try_reserve_exact(count)
            .map_err(|_| Error::Allocation)?;
        for entry in self.dictionary(dictionary)?.iter().filter(matches) {
            let mut word = String::new();
            word.try_reserve_exact(entry.word.len())
                .map_err(|_| Error::Allocation)?;
            word.push_str(&entry.word);
            spellings.push(word);
        }
        let mut values = self.list_buffer(count)?;
        for word in &spellings {
            let text = self.new_text(word)?;
            values.push(Value::Text(text));
        }
        self.publish_list(values)
    }

    /// Whether one entity answers to one word under a vocabulary property. The
    /// dictionary's own `isWordDefined` asks only whether any entity does, which
    /// is not what a relation's `contains` means.
    pub fn dictionary_names(
        &self,
        dictionary: ObjectRef,
        object: ObjectRef,
        word: &str,
        property: PropertyId,
    ) -> Result<bool, Error> {
        Ok(self.dictionary(dictionary)?.iter().any(|entry| {
            entry.object == object && entry.word == word && entry.property == property
        }))
    }

    /// Mutable text is a distinct object; snapshots never alias its storage.
    pub fn new_string_buffer(&mut self) -> Result<ObjectRef, Error> {
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new buffer")
            .string_buffer = Some(String::new());
        Ok(object)
    }

    pub fn string_buffer(&self, object: ObjectRef) -> Result<&str, Error> {
        self.ready()?;
        self.store
            .record(object)?
            .string_buffer
            .as_deref()
            .ok_or(Error::WrongType)
    }

    fn string_buffer_reserve(&mut self, object: ObjectRef, additional: usize) -> Result<(), Error> {
        self.journal_container(object)?;
        self.string_buffer(object)?;
        if additional
            > self
                .store
                .limits
                .string_bytes
                .saturating_sub(self.store.string_bytes)
        {
            return self.resource_failure(Error::ResourceLimit);
        }
        if additional == 0 {
            return Ok(());
        }
        if let Err(error) = self.store.reservation() {
            return self.resource_failure(error);
        }
        if self
            .store
            .records
            .get_mut(&object.logical)
            .expect("checked buffer")
            .string_buffer
            .as_mut()
            .expect("buffer")
            .try_reserve_exact(additional)
            .is_err()
        {
            return self.resource_failure(Error::Allocation);
        }
        Ok(())
    }

    pub fn string_buffer_append(
        &mut self,
        object: ObjectRef,
        text: &str,
    ) -> Result<ObjectRef, Error> {
        self.string_buffer_reserve(object, text.len())?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("checked buffer")
            .string_buffer
            .as_mut()
            .expect("buffer")
            .push_str(text);
        self.store.string_bytes += text.len();
        Ok(object)
    }

    /// Copy characters, never ownership, from immutable text or another buffer.
    pub fn string_buffer_append_value(
        &mut self,
        object: ObjectRef,
        value: Value,
    ) -> Result<ObjectRef, Error> {
        self.string_buffer(object)?;
        self.list_element(value)?;
        match value {
            Value::String(literal) => self.string_buffer_append(
                object,
                crate::tables::installed().literals[literal.index as usize],
            ),
            Value::Text(source) | Value::Reference(source) => {
                let mutable = matches!(value, Value::Reference(_));
                let length = if mutable {
                    self.string_buffer(source)?.len()
                } else {
                    self.text(source)?.len()
                };
                self.string_buffer_reserve(object, length)?;
                if object == source {
                    // A StringBuffer may append itself without a temporary copy.
                    self.store
                        .records
                        .get_mut(&object.logical)
                        .expect("checked buffer")
                        .string_buffer
                        .as_mut()
                        .expect("buffer")
                        .extend_from_within(..);
                } else {
                    let [Some(target), Some(source)] = self
                        .store
                        .records
                        .get_disjoint_mut([&object.logical, &source.logical])
                    else {
                        unreachable!("validated buffer and text")
                    };
                    let text = if mutable {
                        source.string_buffer.as_deref()
                    } else {
                        source.text.as_deref()
                    }
                    .expect("checked text");
                    target
                        .string_buffer
                        .as_mut()
                        .expect("buffer")
                        .push_str(text);
                }
                self.store.string_bytes += length;
                Ok(object)
            }
            _ => Err(Error::WrongType),
        }
    }

    pub fn string_buffer_snapshot(&mut self, object: ObjectRef) -> Result<ObjectRef, Error> {
        let length = self.string_buffer(object)?.len();
        if length
            > self
                .store
                .limits
                .string_bytes
                .saturating_sub(self.store.string_bytes)
        {
            return self.resource_failure(Error::ResourceLimit);
        }
        if let Err(error) = self.store.reservation() {
            return self.resource_failure(error);
        }
        let mut text = String::new();
        if text.try_reserve_exact(length).is_err() {
            return self.resource_failure(Error::Allocation);
        }
        text.push_str(self.string_buffer(object)?);
        let snapshot = self.create()?;
        self.store
            .records
            .get_mut(&snapshot.logical)
            .expect("snapshot")
            .text = Some(text);
        self.store.string_bytes += length;
        Ok(snapshot)
    }

    /// Creates one uniquely owned UTF-8 buffer in this scope's existing forest.
    pub fn new_text(&mut self, text: &str) -> Result<ObjectRef, Error> {
        self.ready()?;
        let total = self
            .store
            .string_bytes
            .checked_add(text.len())
            .ok_or(Error::ResourceLimit);
        let total = match total {
            Ok(total) if total <= self.store.limits.string_bytes => total,
            _ => return self.resource_failure(Error::ResourceLimit),
        };
        let mut buffer = String::new();
        if let Err(error) = self.store.reservation().and_then(|()| {
            buffer
                .try_reserve_exact(text.len())
                .map_err(|_| Error::Allocation)
        }) {
            return self.resource_failure(error);
        }
        buffer.push_str(text);
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new object")
            .text = Some(buffer);
        self.store.string_bytes = total;
        Ok(object)
    }

    pub fn text(&self, object: ObjectRef) -> Result<&str, Error> {
        self.ready()?;
        self.store
            .record(object)?
            .text
            .as_deref()
            .ok_or(Error::WrongType)
    }

    pub(crate) fn copy_text_buffer(&mut self, object: ObjectRef) -> Result<String, Error> {
        let length = self.text(object)?.len();
        let mut buffer = String::new();
        if let Err(error) = self.store.reservation().and_then(|()| {
            buffer
                .try_reserve_exact(length)
                .map_err(|_| Error::Allocation)
        }) {
            return self.resource_failure(error);
        }
        buffer.push_str(self.text(object)?);
        Ok(buffer)
    }

    /// Content comparison never treats distinct string identities as unequal text.
    pub fn text_equal(&self, left: ObjectRef, right: ObjectRef) -> Result<bool, Error> {
        Ok(self.text(left)? == self.text(right)?)
    }

    fn value_text(&self, value: Value) -> Result<Option<&str>, Error> {
        Ok(match value {
            Value::String(literal) => Some(
                crate::tables::installed()
                    .literals
                    .get(literal.index as usize)
                    .copied()
                    .ok_or(Error::InvalidArgument)?,
            ),
            Value::Text(text) => Some(self.text(text)?),
            _ => None,
        })
    }

    /// Source list equality, as observed externally: equal length and elements in
    /// order; nested lists compare recursively, literal and dynamic text by
    /// content, every other value by identity. Traversal is iterative.
    pub fn list_equal(&self, left: ObjectRef, right: ObjectRef) -> Result<bool, Error> {
        let mut pending = Vec::new();
        pending.try_reserve(1).map_err(|_| Error::Allocation)?;
        pending.push((left, right));
        while let Some((left, right)) = pending.pop() {
            if left == right {
                self.list(left)?;
                continue;
            }
            let (a, b) = (self.list(left)?, self.list(right)?);
            if a.len() != b.len() {
                return Ok(false);
            }
            for (x, y) in a.iter().zip(b) {
                if let (Value::List(l), Value::List(r)) = (*x, *y) {
                    pending.try_reserve(1).map_err(|_| Error::Allocation)?;
                    pending.push((l, r));
                    continue;
                }
                let equal = match (self.value_text(*x)?, self.value_text(*y)?) {
                    (Some(l), Some(r)) => l == r,
                    (None, None) => x == y,
                    _ => false,
                };
                if !equal {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    pub fn concat_text(&mut self, left: ObjectRef, right: ObjectRef) -> Result<ObjectRef, Error> {
        self.ready()?;
        let length = self.text(left)?.len().checked_add(self.text(right)?.len());
        let Some(length) = length else {
            return self.resource_failure(Error::ResourceLimit);
        };
        let total = self.store.string_bytes.checked_add(length);
        let Some(total) = total.filter(|total| *total <= self.store.limits.string_bytes) else {
            return self.resource_failure(Error::ResourceLimit);
        };
        let mut buffer = String::new();
        if let Err(error) = self.store.reservation().and_then(|()| {
            buffer
                .try_reserve_exact(length)
                .map_err(|_| Error::Allocation)
        }) {
            return self.resource_failure(error);
        }
        buffer.push_str(self.text(left)?);
        buffer.push_str(self.text(right)?);
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new object")
            .text = Some(buffer);
        self.store.string_bytes = total;
        Ok(object)
    }

    pub fn substring_text(
        &mut self,
        source: ObjectRef,
        start: i32,
        length: Option<i32>,
    ) -> Result<ObjectRef, Error> {
        let text = self.text(source)?;
        let count = text.chars().count() as i128;
        let start = if start < 0 {
            count + i128::from(start)
        } else {
            i128::from(start) - 1
        }
        .clamp(0, count);
        let end = match length {
            None => count,
            Some(length) if length < 0 => (count + i128::from(length)).clamp(start, count),
            Some(length) => (start + i128::from(length)).min(count),
        };
        let first = text
            .char_indices()
            .nth(start as usize)
            .map_or(text.len(), |(index, _)| index);
        let last = text
            .char_indices()
            .nth(end as usize)
            .map_or(text.len(), |(index, _)| index);
        let bytes = last - first;
        let Some(total) = self
            .store
            .string_bytes
            .checked_add(bytes)
            .filter(|total| *total <= self.store.limits.string_bytes)
        else {
            return self.resource_failure(Error::ResourceLimit);
        };
        let mut buffer = String::new();
        if let Err(error) = self.store.reservation().and_then(|()| {
            buffer
                .try_reserve_exact(bytes)
                .map_err(|_| Error::Allocation)
        }) {
            return self.resource_failure(error);
        }
        buffer.push_str(
            self.text(source)?
                .get(first..last)
                .ok_or(Error::InvalidArgument)?,
        );
        let object = self.create()?;
        self.store
            .records
            .get_mut(&object.logical)
            .expect("new object")
            .text = Some(buffer);
        self.store.string_bytes = total;
        Ok(object)
    }

    pub fn integer_text(
        &mut self,
        value: i32,
        radix: u32,
        signed: bool,
    ) -> Result<ObjectRef, Error> {
        self.ready()?;
        if !(2..=36).contains(&radix) {
            return Err(Error::InvalidArgument);
        }
        let negative = signed && value < 0;
        let mut value = if signed {
            value.unsigned_abs()
        } else {
            value as u32
        };
        let mut digits = [0u8; 33];
        let mut start = digits.len();
        loop {
            start -= 1;
            digits[start] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"[(value % radix) as usize];
            value /= radix;
            if value == 0 {
                break;
            }
        }
        if negative {
            start -= 1;
            digits[start] = b'-';
        }
        let text = std::str::from_utf8(&digits[start..]).map_err(|_| Error::InvalidArgument)?;
        self.new_text(text)
    }

    pub fn text_integer(&self, source: ObjectRef, radix: u32) -> Result<i32, Error> {
        if !(2..=36).contains(&radix) {
            return Err(Error::InvalidArgument);
        }
        let text = self.text(source)?.trim();
        if text == "true" {
            return Ok(1);
        }
        if text == "nil" {
            return Ok(0);
        }
        let (negative, digits) = if let Some(tail) = text.strip_prefix('-') {
            (true, tail)
        } else {
            (false, text.strip_prefix('+').unwrap_or(text))
        };
        let mut value = 0u32;
        for ch in digits.chars() {
            let Some(digit) = ch.to_digit(radix) else {
                break;
            };
            value = value
                .checked_mul(radix)
                .and_then(|value| value.checked_add(digit))
                .ok_or(Error::NumericOverflow)?;
        }
        if negative {
            if value > 0x8000_0000 {
                return Err(Error::NumericOverflow);
            }
            Ok(value.wrapping_neg() as i32)
        } else if radix == 10 && value > i32::MAX as u32 {
            Err(Error::NumericOverflow)
        } else {
            Ok(value as i32)
        }
    }

    /// Enumerate live classes in increasing identity order, excluding the filter itself.
    /// The returned reference is a borrow and does not extend object lifetime.
    /// The classes derived from `ancestor`, in slot order.
    pub fn next_class(
        &mut self,
        after: Option<ObjectRef>,
        ancestor: ObjectRef,
    ) -> Result<Option<ObjectRef>, Error> {
        self.next_derived(after, ancestor, Derived::Classes)
    }

    /// The instances of `ancestor`, in slot order.
    pub fn next_instance(
        &mut self,
        after: Option<ObjectRef>,
        ancestor: ObjectRef,
    ) -> Result<Option<ObjectRef>, Error> {
        self.next_derived(after, ancestor, Derived::Instances)
    }

    /// walk what derives from `ancestor`, taking classes, instances or
    /// both. The three answer the same question of different records, so they
    /// are one walk with a filter rather than three; only instance enumeration
    /// was missing, and it is the one a library asks for.
    pub fn next_derived(
        &mut self,
        after: Option<ObjectRef>,
        ancestor: ObjectRef,
        want: Derived,
    ) -> Result<Option<ObjectRef>, Error> {
        self.ready()?;
        self.store.record(ancestor)?;
        let mut cursor = if let Some(after) = after {
            self.store.record(after)?;
            after.logical
        } else {
            0
        };
        loop {
            let candidate = self
                .store
                .records
                .iter()
                .filter(|(id, record)| **id > cursor && record.live && want.admits(record.is_class))
                .map(|(id, _)| *id)
                .min();
            let Some(id) = candidate else {
                return Ok(None);
            };
            // An internal walk names a slot, not a handle from outside, so the
            // reference is built from the record's own incarnation.
            let Some(candidate) = self.store.reference_to(id) else {
                cursor = id;
                continue;
            };
            cursor = id;
            if candidate != ancestor && self.of_kind(candidate, ancestor)? {
                return Ok(Some(candidate));
            }
        }
    }

    pub fn property_defined(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
    ) -> Result<bool, Error> {
        self.ready()?;
        match self.store.lookup(object, property, None) {
            Err(error @ (Error::Allocation | Error::ResourceLimit)) => self.resource_failure(error),
            result => result
                .and_then(|value| self.store.class_fallback(object, property, value))
                .map(|value| value.is_some()),
        }
    }

    pub fn get(&mut self, object: ObjectRef, property: PropertyId) -> Result<Value, Error> {
        self.ready()?;
        match self.store.lookup(object, property, None) {
            Err(error @ (Error::Allocation | Error::ResourceLimit)) => self.resource_failure(error),
            result => result
                .and_then(|value| self.store.class_fallback(object, property, value))
                .map(|value| value.unwrap_or(Value::Nil)),
        }
    }

    pub fn get_inherited(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        definer: ObjectRef,
    ) -> Result<Option<Value>, Error> {
        self.ready()?;
        match self.store.lookup(object, property, Some(definer)) {
            Err(error @ (Error::Allocation | Error::ResourceLimit)) => self.resource_failure(error),
            result => result.and_then(|value| self.store.class_fallback(object, property, value)),
        }
    }

    /// Which object defines a property: the object itself, an ancestor, or none.
    /// `PropDefDirectly` asks only about the object, `PropDefInherits` only about
    /// its ancestors, and `PropDefGetClass` names the definer.
    pub fn property_definer(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
    ) -> Result<Option<ObjectRef>, Error> {
        self.ready()?;
        self.store.definer(object, property)
    }

    /// Nominal kind membership includes identity and transitive superclasses.
    pub fn of_kind(&mut self, object: ObjectRef, ancestor: ObjectRef) -> Result<bool, Error> {
        self.ready()?;
        self.store.record(object)?;
        self.store.record(ancestor)?;
        if object == ancestor {
            return Ok(true);
        }
        if self.store.record(object)?.prototypes.is_empty() {
            return Ok(false);
        }
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        let reserve = self.store.reservation().and_then(|()| {
            pending
                .try_reserve(self.store.records.len())
                .map_err(|_| Error::Allocation)?;
            seen.try_reserve(self.store.records.len())
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        self.store
            .descends(object, ancestor, &mut pending, &mut seen)
    }

    /// Append one non-owning superclass in source order, without transferring ownership.
    pub fn add_prototype(&mut self, object: ObjectRef, prototype: ObjectRef) -> Result<(), Error> {
        self.journal_container(object)?;
        self.ready()?;
        self.store.record(object)?;
        self.store.record(prototype)?;
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        if pending.try_reserve(self.store.records.len()).is_err()
            || seen.try_reserve(self.store.records.len()).is_err()
        {
            return self.resource_failure(Error::Allocation);
        }
        if object == prototype
            || self
                .store
                .descends(prototype, object, &mut pending, &mut seen)?
        {
            return Err(Error::InheritanceCycle);
        }
        if self
            .store
            .properties
            .saturating_add(self.store.inheritance_edges)
            >= self.store.limits.properties
        {
            return self.resource_failure(Error::ResourceLimit);
        }
        let reserve = self.store.reservation().and_then(|()| {
            self.store
                .records
                .get_mut(&object.logical)
                .expect("validated object")
                .prototypes
                .try_reserve(1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        self.store
            .records
            .get_mut(&object.logical)
            .expect("validated object")
            .prototypes
            .push(prototype);
        self.store.inheritance_edges += 1;
        Ok(())
    }

    pub fn set(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        value: Value,
    ) -> Result<(), Error> {
        self.ready()?;
        let exists = self
            .store
            .record(object)?
            .properties
            .contains_key(&property);
        // Only world state is worth recording. A turn record's
        // fields cannot be restored once the cycle frees it, and journalling
        // them would make every cycle look as though it changed something,
        // which would make undo put back a turn that did nothing.
        if self.store.journalling
            && self.store.record(object)?.lifetime == Lifetime::World
            && !self
                .store
                .journal
                .iter()
                .any(|d| d.object == object && d.property == property)
        {
            let previous = self
                .store
                .record(object)?
                .properties
                .get(&property)
                .copied();
            if self.store.journal.len() >= self.store.limits.properties {
                return Err(Error::ResourceLimit);
            }
            self.store
                .journal
                .try_reserve(1)
                .map_err(|_| Error::Allocation)?;
            self.store.journal.push(PropertyDelta {
                object,
                property,
                previous,
            });
        }
        if let Value::Reference(reference) | Value::Text(reference) | Value::List(reference) = value
            && reference.session != self.store.session
        {
            return Err(Error::ForeignSession);
        }
        if let Value::String(literal) = value {
            if literal.session != self.store.session {
                return Err(Error::ForeignSession);
            }
            self.store.literal(literal.index)?;
        }
        // the destination decides the class. A turn value stored into
        // world state is promoted with everything it reaches, so an author
        // never writes a lifetime down.
        if let Value::Reference(reference) | Value::Text(reference) | Value::List(reference) = value
            && self.store.record(object)?.lifetime == Lifetime::World
        {
            self.promote(reference)?;
        }
        if !exists {
            if self
                .store
                .properties
                .saturating_add(self.store.inheritance_edges)
                >= self.store.limits.properties
            {
                return self.resource_failure(Error::ResourceLimit);
            }
            let reserve = self.store.reservation().and_then(|()| {
                self.store
                    .records
                    .get_mut(&object.logical)
                    .expect("validated object")
                    .properties
                    .try_reserve(1)
                    .map_err(|_| Error::Allocation)
            });
            if let Err(error) = reserve {
                return self.resource_failure(error);
            }
            self.store.properties += 1;
        }
        if self
            .store
            .records
            .get_mut(&object.logical)
            .expect("validated object")
            .pending_constructions
            .remove(&property)
            .is_some()
        {
            self.store.pending_constructions -= 1;
        }
        if let Some(previous) = self
            .store
            .record(object)?
            .owned_value_fields
            .get(&property)
            .copied()
        {
            if matches!(value, Value::Reference(reference) | Value::Text(reference) | Value::List(reference) if reference == previous)
            {
                return Ok(());
            }
            let record = self
                .store
                .records
                .get_mut(&object.logical)
                .expect("validated object");
            record.owned_value_fields.remove(&property);
            let index = record
                .children
                .iter()
                .position(|child| *child == previous)
                .expect("owned field child");
            record.children.remove(index);
            self.store.destroy(previous);
        }
        self.store
            .records
            .get_mut(&object.logical)
            .expect("validated object")
            .properties
            .insert(property, value);
        Ok(())
    }

    /// Consume a temporary buffer into a field; ordinary input references stay borrowed.
    pub(crate) fn assign_text(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        text: ObjectRef,
        region: ObjectRef,
    ) -> Result<(), Error> {
        self.text(text)?;
        self.assign_owned_value(object, property, text, region, Value::Text(text))
    }

    pub fn assign_list(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        list: ObjectRef,
        region: ObjectRef,
    ) -> Result<(), Error> {
        self.list(list)?;
        self.assign_owned_value(object, property, list, region, Value::List(list))
    }

    pub(crate) fn assign_lookup(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        table: ObjectRef,
        region: ObjectRef,
    ) -> Result<(), Error> {
        self.lookup_len(table)?;
        if self.store.record(table)?.parent != Some(region) {
            return Err(Error::NotOwned);
        }
        self.assign_owned_value(object, property, table, region, Value::Reference(table))
    }

    pub(crate) fn assign_vector(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        vector: ObjectRef,
        region: ObjectRef,
    ) -> Result<(), Error> {
        self.vector(vector)?;
        if self.store.record(vector)?.parent != Some(region) {
            return Err(Error::NotOwned);
        }
        self.assign_owned_value(object, property, vector, region, Value::Reference(vector))
    }

    pub(crate) fn assign_owned_collection(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        collection: ObjectRef,
        region: ObjectRef,
    ) -> Result<(), Error> {
        self.owned_members(collection)?;
        self.assign_owned_value(
            object,
            property,
            collection,
            region,
            Value::Reference(collection),
        )
    }

    fn assign_owned_value(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        text: ObjectRef,
        region: ObjectRef,
        value: Value,
    ) -> Result<(), Error> {
        self.ready()?;

        if self.store.record(region)?.parent.is_some() {
            return Err(Error::NotOwned);
        }
        if self.store.record(object)?.text.is_some()
            || self.store.record(object)?.list.is_some()
            || object == region
        {
            return Err(Error::WrongType);
        }
        if self.store.record(text)?.parent != Some(region) {
            return self.set(object, property, value);
        }
        if self.prospective_cycle(object, text)? {
            return Err(Error::OwnershipCycle);
        }
        let index = self
            .store
            .record(region)?
            .children
            .iter()
            .position(|child| *child == text)
            .ok_or(Error::NotOwned)?;
        let reserve = self.store.reservation().and_then(|()| {
            let record = self
                .store
                .records
                .get_mut(&object.logical)
                .expect("validated destination");
            record
                .owned_value_fields
                .try_reserve(record.pending_constructions.len() + 1)
                .map_err(|_| Error::Allocation)?;
            record
                .children
                .try_reserve(record.pending_constructions.len() + 1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        // set admits the property and releases the old owned field before publication.
        self.set(object, property, value)?;
        self.store
            .records
            .get_mut(&region.logical)
            .expect("validated region")
            .children
            .remove(index);
        self.store
            .records
            .get_mut(&text.logical)
            .expect("validated text")
            .parent = Some(object);
        let destination = self
            .store
            .records
            .get_mut(&object.logical)
            .expect("validated destination");
        destination.children.push(text);
        destination.owned_value_fields.insert(property, text);
        Ok(())
    }

    /// Move a field's exclusive owner, leaving its ordinary source value as a borrow.
    /// Borrowed/inherited values have no owner here and return false unchanged.
    pub(crate) fn move_field_owner(
        &mut self,
        source: ObjectRef,
        source_property: PropertyId,
        destination: ObjectRef,
        destination_property: PropertyId,
    ) -> Result<bool, Error> {
        self.ready()?;
        let target = self.store.record(destination)?;
        if target.text.is_some() || target.list.is_some() {
            return Err(Error::WrongType);
        }
        let record = self.store.record(source)?;
        let Some(root) = record.owned_value_fields.get(&source_property).copied() else {
            return Ok(false);
        };
        let value = *record
            .properties
            .get(&source_property)
            .expect("owned field value");
        if source == destination && source_property == destination_property {
            return Ok(true);
        }
        if self.prospective_cycle(destination, root)? {
            return Err(Error::OwnershipCycle);
        }
        let reserve = self.store.reservation().and_then(|()| {
            let target = self
                .store
                .records
                .get_mut(&destination.logical)
                .expect("destination");
            target
                .children
                .try_reserve(target.pending_constructions.len() + 1)
                .map_err(|_| Error::Allocation)?;
            target
                .owned_value_fields
                .try_reserve(target.pending_constructions.len() + 1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        // Detach before replacing the destination: its old subtree can contain
        // the source object. Any subsequent admission failure is terminal and
        // reclaims the entire store, including this temporarily detached child.
        let record = self.store.records.get_mut(&source.logical).expect("source");
        record.owned_value_fields.remove(&source_property);
        record.children.retain(|child| *child != root);
        self.set(destination, destination_property, value)?;
        self.store
            .records
            .get_mut(&root.logical)
            .expect("retained child")
            .parent = Some(destination);
        let target = self
            .store
            .records
            .get_mut(&destination.logical)
            .expect("destination");
        target.children.push(root);
        target.owned_value_fields.insert(destination_property, root);
        Ok(true)
    }

    /// Consume a root owner into a field, admitting replacement before publication.
    /// The native caller removes its local-owner slot only after this succeeds.
    pub(crate) fn move_root_into_field(
        &mut self,
        object: ObjectRef,
        property: PropertyId,
        root: ObjectRef,
    ) -> Result<(), Error> {
        self.ready()?;
        let source = self.store.record(root)?;
        let value = if source.text.is_some() {
            self.text(root)?;
            Value::Text(root)
        } else if source.list.is_some() {
            self.list(root)?;
            Value::List(root)
        } else {
            Value::Reference(root)
        };

        if self.store.record(object)?.text.is_some() || self.store.record(object)?.list.is_some() {
            return Err(Error::WrongType);
        }
        let root_index = self
            .store
            .roots
            .iter()
            .position(|value| *value == root)
            .ok_or(Error::NotOwned)?;
        if self.prospective_cycle(object, root)? {
            return Err(Error::OwnershipCycle);
        }
        let reserve = self.store.reservation().and_then(|()| {
            let destination = self
                .store
                .records
                .get_mut(&object.logical)
                .expect("validated destination");
            destination
                .owned_value_fields
                .try_reserve(destination.pending_constructions.len() + 1)
                .map_err(|_| Error::Allocation)?;
            destination
                .children
                .try_reserve(destination.pending_constructions.len() + 1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        self.set(object, property, value)?;
        self.store.roots.remove(root_index);
        self.store
            .records
            .get_mut(&root.logical)
            .expect("validated root")
            .parent = Some(object);
        let destination = self
            .store
            .records
            .get_mut(&object.logical)
            .expect("validated destination");
        destination.children.push(root);
        destination.owned_value_fields.insert(property, root);
        Ok(())
    }

    /// Transfer a region root into a parent's exclusive inventory, atomically.
    pub fn adopt(&mut self, parent: ObjectRef, child: ObjectRef) -> Result<(), Error> {
        self.ready()?;
        self.store.record(child)?;
        if self.store.record(parent)?.text.is_some() {
            return Err(Error::WrongType);
        }
        let index = self
            .store
            .roots
            .iter()
            .position(|r| *r == child)
            .ok_or(Error::NotOwned)?;
        if self.prospective_cycle(parent, child)? {
            return Err(Error::OwnershipCycle);
        }
        let reserve = self.store.reservation().and_then(|()| {
            let record = self
                .store
                .records
                .get_mut(&parent.logical)
                .expect("validated parent");
            record
                .children
                .try_reserve(record.pending_constructions.len() + 1)
                .map_err(|_| Error::Allocation)
        });
        if let Err(error) = reserve {
            return self.resource_failure(error);
        }
        self.store.roots.remove(index);
        self.store
            .records
            .get_mut(&child.logical)
            .expect("validated child")
            .parent = Some(parent);
        self.store
            .records
            .get_mut(&parent.logical)
            .expect("validated parent")
            .children
            .push(child);
        Ok(())
    }

    /// The property ids an entity defines itself, in a stable order, for the
    /// inspection boundary. Inherited properties are not included:
    /// what an entity is given is a different question from what it is.
    pub fn property_ids(&self, object: ObjectRef) -> Result<Vec<u64>, Error> {
        self.ready()?;
        let record = self.store.record(object)?;
        let mut ids: Vec<u64> = record
            .properties
            .keys()
            .map(|key| u64::from(key.0))
            .collect();
        ids.sort_unstable();
        Ok(ids)
    }

    /// The handle of the entity a game declared at this index.
    ///
    /// Every declared object is bound to its declaration index while the world
    /// is built, because that is how a compiled reference to a named object
    /// finds it. The manifest publishes the same index beside the name its
    /// author wrote, so `lampRoom` in an engine project and a handle in a
    /// running session are joined by a number both sides already have.
    ///
    /// An index nothing was bound to answers nothing, rather than guessing.
    pub fn declared(&self, index: u32) -> Option<u64> {
        self.store.static_object(index).ok().map(|o| o.handle())
    }

    /// The declared presentation state of an entity, for the inspection
    /// boundary.
    ///
    /// An entity's `presentation` property is a list of property addresses —
    /// `[&name, &isOpen, &isLit]` — and this answers the **stored** value of
    /// each, as a triple of property id, value tag and payload. Inherited
    /// values count: what a `Thing` is drawn as is mostly what its class says.
    ///
    /// **Nothing here evaluates game code**, which is the rule the whole
    /// inspection boundary keeps. A property whose value is a method
    /// is answered as its tag with no payload rather than run, so a description
    /// that is computed reads as "computed" instead of secretly calling into
    /// the game between commands.
    ///
    /// **A game says what is presentation.** Reading arbitrary properties would
    /// make every internal field part of the published contract; a class listing
    /// its own is the same shape `vocab` uses to declare words.
    pub fn presentation(&self, object: ObjectRef, property: u32) -> Result<Vec<u64>, Error> {
        self.presentation_impl(object, property, false)
    }

    /// Complete presentation or an explicit limit error, for framed snapshots.
    pub fn presentation_checked(
        &self,
        object: ObjectRef,
        property: u32,
    ) -> Result<Vec<u64>, Error> {
        self.presentation_impl(object, property, true)
    }

    fn presentation_impl(
        &self,
        object: ObjectRef,
        property: u32,
        strict: bool,
    ) -> Result<Vec<u64>, Error> {
        self.ready()?;
        /// A page, not a world: the same bound the other inspections keep.
        const LIMIT: usize = 256;
        let listed = self
            .store
            .lookup(object, PropertyId(property), None)?
            .unwrap_or(Value::Nil);
        let Value::List(list) = listed else {
            return Ok(Vec::new());
        };
        if strict
            && self
                .list(list)?
                .iter()
                .filter(|v| matches!(v, Value::Property(_)))
                .count()
                > LIMIT
        {
            return Err(Error::ResourceLimit);
        }
        let wanted: Vec<PropertyId> = self
            .list(list)?
            .iter()
            .filter_map(|value| match value {
                Value::Property(id) => Some(*id),
                _ => None,
            })
            .take(LIMIT)
            .collect();
        let mut answers = Vec::new();
        answers
            .try_reserve(wanted.len() * 3)
            .map_err(|_| Error::Allocation)?;
        for id in wanted {
            let value = self.store.lookup(object, id, None)?.unwrap_or(Value::Nil);
            let (tag, payload) = match value {
                Value::Nil => (0u64, 0u64),
                Value::Bool(set) => (1, u64::from(set)),
                Value::Int(number) => (2, u64::from(number as u32)),
                Value::Reference(entity) => (3, entity.handle()),
                // A literal is read with the bundle's `text_byte` export, which
                // a host already has for an outcome that is text.
                Value::String(literal) => (4, u64::from(literal.index())),
                Value::Enumerator(value) => (5, u64::from(value)),
                // Text built at run time, and anything the game would have to be
                // run to answer. Named, not evaluated.
                Value::Text(_) => (7, 0),
                Value::List(_) => (9, 0),
                _ => (15, 0),
            };
            answers.push(u64::from(id.0));
            answers.push(tag);
            answers.push(payload);
        }
        Ok(answers)
    }

    /// The entity's prototypes, nearest first.
    pub fn prototype_list(&self, object: ObjectRef) -> Result<Vec<ObjectRef>, Error> {
        self.ready()?;
        Ok(self.store.record(object)?.prototypes.clone())
    }

    /// Every live world-lifetime record, oldest slot first, for an inspector
    /// with no handle to start from. Bounded by `limit`.
    pub fn world_entities(&self, limit: usize) -> Vec<u64> {
        let mut slots: Vec<u64> = self
            .store
            .records
            .iter()
            .filter(|(_, record)| record.live && record.lifetime == Lifetime::World)
            .map(|(slot, _)| *slot)
            .collect();
        slots.sort_unstable();
        slots
            .into_iter()
            .take(limit)
            .filter_map(|slot| self.store.reference_to(slot))
            .map(|object| object.handle())
            .collect()
    }

    /// A reference to a live record named by a boundary handle, or nothing.
    pub fn reference(&self, handle: u64) -> Option<ObjectRef> {
        let object = self.store.native_reference(handle);
        self.store.record(object).ok().map(|_| object)
    }

    /// Confirm a record is still live, without reading it.
    pub fn record_exists(&self, object: ObjectRef) -> Result<(), Error> {
        self.ready()?;
        self.store.record(object).map(|_| ())
    }

    /// Remove an entity from the world. Unlike `destroy` this does
    /// not require the record to be a root: an entity owned by another record
    /// is detached from it first. Handles to it, and to anything it owned, read
    /// as expired afterwards rather than naming something else.
    pub fn despawn(&mut self, object: ObjectRef) -> Result<(), Error> {
        self.ready()?;
        self.store.record(object)?;
        if let Some(index) = self.store.roots.iter().position(|root| *root == object) {
            self.store.roots.remove(index);
        } else if let Some(parent) = self.store.record(object)?.parent
            && let Some(record) = self.store.records.get_mut(&parent.logical)
            && let Some(index) = record.children.iter().position(|child| *child == object)
        {
            record.children.remove(index);
        }
        self.store.destroy(object);
        Ok(())
    }

    pub fn destroy(&mut self, object: ObjectRef) -> Result<(), Error> {
        self.ready()?;
        self.store.record(object)?;
        let index = self
            .store
            .roots
            .iter()
            .position(|r| *r == object)
            .ok_or(Error::NotOwned)?;
        self.store.roots.remove(index);
        self.store.destroy(object);
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.clear_roots();
    }
}

pub struct Scope<'a> {
    objects: Objects<'a>,
}

impl<'a> std::ops::Deref for Scope<'a> {
    type Target = Objects<'a>;
    fn deref(&self) -> &Self::Target {
        &self.objects
    }
}
impl std::ops::DerefMut for Scope<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.objects
    }
}

impl ObjectRef {
    /// The value a handle has outside the store : the slot in the low
    /// half, the incarnation in the high half. Carrying the incarnation is what
    /// lets a slot be reused without a stale handle silently naming whatever
    /// took its place.
    pub fn handle(self) -> u64 {
        (self.logical & 0xffff_ffff) | (self.incarnation << 32)
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        while let Some(root) = self.store.roots.pop() {
            self.store.destroy(root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store(objects: usize, properties: usize) -> Store {
        Store::new(Limits {
            objects,
            properties,
            string_bytes: 8 * 1024 * 1024,
        })
        .unwrap()
    }

    #[test]
    fn closure_environment_keeps_copies_and_checked_borrows_without_ownership() {
        let mut state = store(5, 10);
        let mut objects = state.scope().unwrap();
        let target = objects.create().unwrap();
        objects.set(target, PropertyId(1), Value::Int(7)).unwrap();
        let closure = objects
            .new_closure(
                42,
                &[
                    Capture::Copy(Value::Int(3)),
                    Capture::Reference(Value::Reference(target)),
                ],
            )
            .unwrap();
        assert_eq!(objects.closure_function(closure), Ok(42));
        assert_eq!(objects.closure_capture(closure, 0), Ok(Value::Int(3)));
        objects.set(target, PropertyId(1), Value::Int(8)).unwrap();
        assert_eq!(
            objects.closure_capture(closure, 1),
            Ok(Value::Reference(target))
        );
        assert_eq!(objects.get(target, PropertyId(1)), Ok(Value::Int(8)));
        assert_eq!(
            objects.closure_capture(closure, 2),
            Err(Error::InvalidArgument)
        );
        assert_eq!(objects.list(closure), Err(Error::WrongType));
        objects.destroy(target).unwrap();
        assert_eq!(
            objects.closure_capture(closure, 1),
            Ok(Value::Reference(target))
        );
        assert_eq!(objects.get(target, PropertyId(1)), Err(Error::Expired));
        objects.destroy(closure).unwrap();
        assert_eq!(objects.closure_function(closure), Err(Error::Expired));
        assert_eq!(objects.store.properties, 0);
    }

    #[test]
    fn closure_capture_modes_and_quota_cleanup_are_checked() {
        let mut state = store(4, 2);
        let mut objects = state.scope().unwrap();
        let target = objects.create().unwrap();
        assert_eq!(
            objects.new_closure(0, &[Capture::Copy(Value::Reference(target))]),
            Err(Error::WrongType)
        );
        assert_eq!(
            objects.new_closure(0, &[Capture::Reference(Value::Int(1))]),
            Err(Error::WrongType)
        );
        for _ in 0..100 {
            let closure = objects
                .new_closure(
                    0,
                    &[Capture::Copy(Value::Int(1)), Capture::Reference(Value::Nil)],
                )
                .unwrap();
            assert_eq!(objects.store.properties, 2);
            objects.destroy(closure).unwrap();
            assert_eq!(objects.store.properties, 0);
        }
        assert_eq!(
            objects.new_closure(0, &[Capture::Copy(Value::Nil); 3]),
            Err(Error::ResourceLimit)
        );
        assert_eq!(objects.get(target, PropertyId(1)), Err(Error::Terminal));
    }

    #[test]
    fn class_enumeration_excludes_base_instances_and_expired_records() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let base = objects.create().unwrap();
        let child = objects.create().unwrap();
        let instance = objects.create().unwrap();
        objects.store.mark_class(base).unwrap();
        objects.store.mark_class(child).unwrap();
        objects.add_prototype(child, base).unwrap();
        objects.add_prototype(instance, base).unwrap();
        assert_eq!(objects.next_class(None, base), Ok(Some(child)));
        assert_eq!(objects.next_class(Some(child), base), Ok(None));
        objects.destroy(child).unwrap();
        assert_eq!(objects.next_class(None, base), Ok(None));
        assert_eq!(objects.next_class(Some(child), base), Err(Error::Expired));
    }

    /// indexOf reads a sequence, and a list is one.
    #[test]
    fn index_of_reads_a_list_as_well_as_a_vector() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let mut values = objects.list_buffer(3).unwrap();
        values.push(Value::Int(10));
        values.push(Value::Int(20));
        values.push(Value::Int(30));
        let list = objects.publish_list(values).unwrap();
        assert_eq!(objects.vector_index_of(list, Value::Int(10)), Ok(Some(1)));
        assert_eq!(objects.vector_index_of(list, Value::Int(30)), Ok(Some(3)));
        assert_eq!(objects.vector_index_of(list, Value::Int(99)), Ok(None));
    }

    /// the walk the class one was missing. An instance walk answers
    /// records, skips classes, and excludes the ancestor itself exactly as the
    /// class walk does.
    #[test]
    fn instance_enumeration_answers_records_and_skips_classes() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let base = objects.create().unwrap();
        let child = objects.create().unwrap();
        let first = objects.create().unwrap();
        let second = objects.create().unwrap();
        objects.store.mark_class(base).unwrap();
        objects.store.mark_class(child).unwrap();
        objects.add_prototype(child, base).unwrap();
        objects.add_prototype(first, base).unwrap();
        objects.add_prototype(second, child).unwrap();
        // An instance of a subclass is still an instance of the ancestor.
        assert_eq!(objects.next_instance(None, base), Ok(Some(first)));
        assert_eq!(objects.next_instance(Some(first), base), Ok(Some(second)));
        assert_eq!(objects.next_instance(Some(second), base), Ok(None));
        // The class walk is unchanged by any of this.
        assert_eq!(objects.next_class(None, base), Ok(Some(child)));
        assert_eq!(objects.next_class(Some(child), base), Ok(None));
    }

    /// Both takes classes and instances in slot order, interleaved rather than
    /// one kind after the other.
    #[test]
    fn enumerating_both_interleaves_classes_and_instances_in_slot_order() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let base = objects.create().unwrap();
        let child = objects.create().unwrap();
        let instance = objects.create().unwrap();
        objects.store.mark_class(base).unwrap();
        objects.store.mark_class(child).unwrap();
        objects.add_prototype(child, base).unwrap();
        objects.add_prototype(instance, base).unwrap();
        assert_eq!(
            objects.next_derived(None, base, Derived::Both),
            Ok(Some(child))
        );
        assert_eq!(
            objects.next_derived(Some(child), base, Derived::Both),
            Ok(Some(instance))
        );
        assert_eq!(
            objects.next_derived(Some(instance), base, Derived::Both),
            Ok(None)
        );
    }

    /// A destroyed instance leaves the walk, and a handle to it is expired
    /// rather than a resume point — the same rule the class walk follows.
    #[test]
    fn instance_enumeration_drops_destroyed_records() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let base = objects.create().unwrap();
        let instance = objects.create().unwrap();
        objects.store.mark_class(base).unwrap();
        objects.add_prototype(instance, base).unwrap();
        assert_eq!(objects.next_instance(None, base), Ok(Some(instance)));
        objects.destroy(instance).unwrap();
        assert_eq!(objects.next_instance(None, base), Ok(None));
        assert_eq!(
            objects.next_instance(Some(instance), base),
            Err(Error::Expired)
        );
    }

    /// A flag naming no kind is a mistake, not an empty walk.
    #[test]
    fn a_flag_naming_no_kind_is_refused() {
        assert_eq!(Derived::from_flags(1), Some(Derived::Instances));
        assert_eq!(Derived::from_flags(2), Some(Derived::Classes));
        assert_eq!(Derived::from_flags(3), Some(Derived::Both));
        assert_eq!(Derived::from_flags(0), None);
        assert_eq!(Derived::from_flags(4), None);
    }

    #[test]
    fn property_definition_distinguishes_nil_and_missing_through_inheritance() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let base = objects.create().unwrap();
        let child = objects.create().unwrap();
        objects.add_prototype(child, base).unwrap();
        objects.set(base, PropertyId(1), Value::Nil).unwrap();
        objects.set(base, PropertyId(2), Value::Method(42)).unwrap();
        assert_eq!(objects.property_defined(child, PropertyId(1)), Ok(true));
        assert_eq!(objects.property_defined(child, PropertyId(2)), Ok(true));
        assert_eq!(objects.property_defined(child, PropertyId(3)), Ok(false));
        objects.destroy(child).unwrap();
        assert_eq!(
            objects.property_defined(child, PropertyId(1)),
            Err(Error::Expired)
        );
    }

    #[test]
    fn collection_move_slot_admission_failure_cleans_both_roots() {
        let mut state = store(2, 30);
        let mut scope = state.scope().unwrap();
        let group = scope.new_owned_collection().unwrap();
        let member = scope.create().unwrap();
        assert_eq!(
            scope.move_root_into_collection(group, member),
            Err(Error::ResourceLimit)
        );
        assert!(scope.store.records.is_empty());
    }

    #[test]
    fn completed_roots_join_collections_without_duplicate_ownership() {
        let mut state = store(20, 30);
        let mut scope = state.scope().unwrap();
        let group = scope.new_owned_collection().unwrap();
        let first = scope.create().unwrap();
        let second = scope.create().unwrap();
        scope.move_root_into_collection(group, first).unwrap();
        scope.move_root_into_collection(group, second).unwrap();
        assert_eq!(scope.owned_collection_len(group), Ok(2));
        assert_eq!(scope.owned_collection_get(group, 1), Ok(first));
        assert_eq!(
            scope.move_root_into_collection(group, first),
            Err(Error::NotOwned)
        );
        assert_eq!(
            scope.move_root_into_collection(group, group),
            Err(Error::OwnershipCycle)
        );
        let mut call = scope.begin_member_call(first).unwrap().unwrap();
        assert_eq!(scope.remove_owned_member(group, first), Ok(true));
        scope.validate_object(first).unwrap();
        scope.finish_member_call(&mut call).unwrap();
        assert_eq!(scope.validate_object(first), Err(Error::Expired));
        scope.destroy(group).unwrap();
        assert_eq!(scope.validate_object(second), Err(Error::Expired));
        assert_eq!(scope.store.properties, 0);
    }

    #[test]
    fn sized_lookup_reserves_quota_grows_and_releases_capacity() {
        let mut state = store(10, 30);
        let mut scope = state.scope().unwrap();
        let table = scope.new_lookup_sized(3, 2).unwrap();
        assert_eq!(scope.lookup_len(table), Ok(0));
        assert_eq!(scope.lookup_bucket_count(table), Ok(3));
        assert_eq!(scope.store.properties, 5);
        for n in 1..=2 {
            scope
                .lookup_set(table, Value::Int(n), Value::Int(n))
                .unwrap();
        }
        assert_eq!(scope.store.properties, 5);
        scope
            .lookup_set(table, Value::Int(3), Value::Int(3))
            .unwrap();
        assert_eq!(scope.store.properties, 7);
        scope
            .lookup_set(table, Value::Int(3), Value::Int(4))
            .unwrap();
        assert_eq!(scope.store.properties, 7);
        scope.destroy(table).unwrap();
        assert_eq!(scope.store.properties, 0);
        assert_eq!(scope.new_lookup_sized(0, 2), Err(Error::InvalidArgument));
        assert_eq!(scope.new_lookup_sized(8, 0), Err(Error::InvalidArgument));
        assert_eq!(scope.new_lookup_sized(-1, 2), Err(Error::InvalidArgument));
        assert_eq!(scope.new_lookup_sized(8, 20), Err(Error::ResourceLimit));
        assert!(scope.store.records.is_empty());
    }

    #[test]
    fn vector_insert_validates_before_mutation() {
        let mut state = store(20, 40);
        let mut objects = state.scope().unwrap();
        let vector = objects.new_vector(2).unwrap();
        objects.vector_append(vector, Value::Int(3)).unwrap();
        let values = objects.new_list(&[Value::Int(1), Value::Int(2)]).unwrap();
        assert_eq!(
            objects.vector_insert_values(vector, 3, values),
            Err(Error::InvalidArgument)
        );
        assert_eq!(objects.vector(vector).unwrap(), &[Value::Int(3)]);
        assert_eq!(objects.vector_insert_values(vector, 1, values), Ok(vector));
        assert_eq!(
            objects.vector(vector).unwrap(),
            &[Value::Int(1), Value::Int(2), Value::Int(3)]
        );
        let target = objects.create().unwrap();
        let stale = objects
            .new_list(&[Value::Int(9), Value::Reference(target)])
            .unwrap();
        objects.destroy(target).unwrap();
        assert_eq!(objects.vector_insert_values(vector, 0, stale), Ok(vector));
        assert_eq!(objects.vector_get(vector, 5), Ok(Value::Reference(target)));
        assert_eq!(objects.get(target, PropertyId(1)), Err(Error::Expired));
    }

    #[test]
    fn vector_search_returns_first_typed_match_and_compares_text_content() {
        let mut state = store(20, 40);
        let mut objects = state.scope().unwrap();
        let vector = objects.new_vector(5).unwrap();
        let report = objects.create().unwrap();
        let text = objects.new_text("same").unwrap();
        let equal = objects.new_text("same").unwrap();
        for value in [
            Value::Int(1),
            Value::Reference(report),
            Value::Nil,
            Value::Reference(report),
            Value::Text(text),
        ] {
            objects.vector_append(vector, value).unwrap();
        }
        assert_eq!(
            objects.vector_index_of(vector, Value::Reference(report)),
            Ok(Some(2))
        );
        assert_eq!(objects.vector_index_of(vector, Value::Nil), Ok(Some(3)));
        assert_eq!(objects.vector_index_of(vector, Value::Bool(true)), Ok(None));
        assert_eq!(
            objects.vector_index_of(vector, Value::Text(equal)),
            Ok(Some(5))
        );
        objects.destroy(text).unwrap();
        assert_eq!(
            objects.vector_index_of(vector, Value::Text(equal)),
            Err(Error::Expired)
        );
        let lists = objects.new_vector(1).unwrap();
        let list = objects.new_list(&[]).unwrap();
        objects.vector_append(lists, Value::List(list)).unwrap();
        assert_eq!(
            objects.vector_index_of(lists, Value::List(list)),
            Err(Error::WrongType)
        );
    }

    #[test]
    fn field_owner_transfer_keeps_borrows_and_can_replace_source_ancestor() {
        let mut state = store(20, 40);
        let mut objects = state.scope().unwrap();
        let outer = objects.create().unwrap();
        let source = objects.create().unwrap();
        objects
            .move_root_into_field(outer, PropertyId(1), source)
            .unwrap();
        let list = objects.new_list(&[Value::Int(7)]).unwrap();
        objects
            .move_root_into_field(source, PropertyId(2), list)
            .unwrap();
        let keeper = objects.create().unwrap();
        assert_eq!(
            objects.move_field_owner(source, PropertyId(2), keeper, PropertyId(3)),
            Ok(true)
        );
        assert_eq!(objects.get(source, PropertyId(2)), Ok(Value::List(list)));
        assert_eq!(
            objects.move_field_owner(source, PropertyId(2), keeper, PropertyId(4)),
            Ok(false)
        );
        assert_eq!(
            objects.move_field_owner(keeper, PropertyId(3), keeper, PropertyId(3)),
            Ok(true)
        );
        objects.set(source, PropertyId(2), Value::Nil).unwrap();
        assert_eq!(objects.list_get(list, 1), Ok(Value::Int(7)));
        objects.destroy(keeper).unwrap();
        assert_eq!(objects.list(list), Err(Error::Expired));
        let text = objects.new_text("retained").unwrap();
        objects
            .move_root_into_field(source, PropertyId(2), text)
            .unwrap();
        assert_eq!(
            objects.move_field_owner(source, PropertyId(2), outer, PropertyId(1)),
            Ok(true)
        );
        assert_eq!(objects.of_kind(source, source), Err(Error::Expired));
        assert_eq!(objects.text(text), Ok("retained"));
        objects.destroy(outer).unwrap();
        assert!(objects.store.records.is_empty());
    }

    #[test]
    fn field_owner_transfer_checks_cycles_and_terminal_property_admission() {
        let mut state = store(20, 3);
        let mut objects = state.scope().unwrap();
        let source = objects.create().unwrap();
        let child = objects.create().unwrap();
        let target = objects.create().unwrap();
        objects
            .move_root_into_field(source, PropertyId(1), child)
            .unwrap();
        assert_eq!(
            objects.move_field_owner(source, PropertyId(1), child, PropertyId(2)),
            Err(Error::OwnershipCycle)
        );
        assert_eq!(
            objects.get(source, PropertyId(1)),
            Ok(Value::Reference(child))
        );
        objects.set(source, PropertyId(2), Value::Nil).unwrap();
        objects.set(source, PropertyId(3), Value::Nil).unwrap();
        assert_eq!(
            objects.move_field_owner(source, PropertyId(1), target, PropertyId(1)),
            Err(Error::ResourceLimit)
        );
        assert!(objects.store.records.is_empty());
    }

    #[test]
    fn vector_snapshots_and_text_move_into_fields_without_owning_entries() {
        let mut state = store(12, 50);
        let mut scope = state.scope().unwrap();
        let recipient = scope.create().unwrap();
        let item = scope.create().unwrap();
        let vector = scope.new_vector(2).unwrap();
        scope.vector_append(vector, Value::Reference(item)).unwrap();
        scope.vector_append(vector, Value::Int(7)).unwrap();
        let list = scope.vector_snapshot(vector).unwrap();
        scope.vector_set(vector, 2, Value::Int(8)).unwrap();
        scope.destroy(vector).unwrap();
        assert_eq!(scope.list_get(list, 2), Ok(Value::Int(7)));
        scope
            .move_root_into_field(recipient, PropertyId(1), list)
            .unwrap();
        let text = scope.new_text("hello").unwrap();
        scope
            .move_root_into_field(recipient, PropertyId(2), text)
            .unwrap();
        assert_eq!(scope.get(recipient, PropertyId(1)), Ok(Value::List(list)));
        assert_eq!(scope.get(recipient, PropertyId(2)), Ok(Value::Text(text)));
        scope.destroy(recipient).unwrap();
        assert_eq!(scope.list(list), Err(Error::Expired));
        assert_eq!(scope.text(text), Err(Error::Expired));
        assert_eq!(scope.of_kind(item, item), Ok(true));
    }

    #[test]
    fn root_field_transfer_replaces_storage_and_rejects_cycles_before_mutation() {
        let mut state = store(10, 30);
        let mut scope = state.scope().unwrap();
        let destination = scope.create().unwrap();
        let old = scope.new_lookup().unwrap();
        scope
            .move_root_into_field(destination, PropertyId(1), old)
            .unwrap();
        let root = scope.new_lookup().unwrap();
        let child = scope.create().unwrap();
        scope.adopt(root, child).unwrap();
        assert_eq!(
            scope.move_root_into_field(child, PropertyId(1), root),
            Err(Error::OwnershipCycle)
        );
        assert_eq!(
            scope.get(destination, PropertyId(1)),
            Ok(Value::Reference(old))
        );
        scope
            .move_root_into_field(destination, PropertyId(1), root)
            .unwrap();
        assert_eq!(scope.lookup_len(old), Err(Error::Expired));
        assert_eq!(
            scope.move_root_into_field(destination, PropertyId(2), root),
            Err(Error::NotOwned)
        );
        scope.set(destination, PropertyId(1), Value::Nil).unwrap();
        assert_eq!(scope.lookup_len(root), Err(Error::Expired));
        assert_eq!(scope.validate_object(child), Err(Error::Expired));
    }

    #[test]
    fn root_field_transfer_admission_failure_terminates_and_cleans_both_owners() {
        let mut state = store(10, 30);
        let mut scope = state.scope().unwrap();
        let destination = scope.create().unwrap();
        let root = scope.new_lookup().unwrap();
        scope.store.fail_reservation = true;
        assert_eq!(
            scope.move_root_into_field(destination, PropertyId(1), root),
            Err(Error::Allocation)
        );
        assert!(scope.store.records.is_empty());
    }

    #[test]
    fn lookup_field_owns_storage_but_borrows_entries() {
        let mut state = store(10, 20);
        let mut scope = state.scope().unwrap();
        let owner = scope.create().unwrap();
        let region = scope.create().unwrap();
        let borrowed = scope.create().unwrap();
        let table = scope.new_lookup().unwrap();
        assert_eq!(
            scope.assign_lookup(owner, PropertyId(1), table, region),
            Err(Error::NotOwned)
        );
        scope
            .lookup_set(table, Value::Int(1), Value::Reference(borrowed))
            .unwrap();
        scope.adopt(region, table).unwrap();
        scope
            .assign_lookup(owner, PropertyId(1), table, region)
            .unwrap();
        scope.destroy(region).unwrap();
        assert_eq!(
            scope.lookup_get(table, Value::Int(1)),
            Ok(Value::Reference(borrowed))
        );
        scope.set(owner, PropertyId(1), Value::Nil).unwrap();
        assert_eq!(scope.lookup_len(table), Err(Error::Expired));
        scope.validate_object(borrowed).unwrap();
        assert_eq!(scope.store.properties, 1);
    }

    #[test]
    fn vector_field_survives_initializer_and_releases_only_its_owned_storage() {
        let mut state = store(10, 20);
        let mut scope = state.scope().unwrap();
        let owner = scope.create().unwrap();
        let region = scope.create().unwrap();
        let borrowed = scope.create().unwrap();
        let vector = scope.new_vector(2).unwrap();
        assert_eq!(
            scope.assign_vector(owner, PropertyId(1), vector, region),
            Err(Error::NotOwned)
        );
        scope
            .vector_append(vector, Value::Reference(borrowed))
            .unwrap();
        scope.adopt(region, vector).unwrap();
        scope
            .assign_vector(owner, PropertyId(1), vector, region)
            .unwrap();
        scope.destroy(region).unwrap();
        assert_eq!(scope.vector_get(vector, 1), Ok(Value::Reference(borrowed)));
        scope.set(owner, PropertyId(1), Value::Nil).unwrap();
        assert_eq!(scope.vector(vector), Err(Error::Expired));
        scope.validate_object(borrowed).unwrap();
        assert_eq!(scope.store.properties, 1);
    }

    #[test]
    fn string_buffer_append_and_snapshot_keep_utf8_and_independent_lifetimes() {
        let mut state = store(10, 20);
        let mut scope = state.scope().unwrap();
        let buffer = scope.new_string_buffer().unwrap();
        scope.string_buffer_append(buffer, "aé🙂").unwrap();
        let first = scope.string_buffer_snapshot(buffer).unwrap();
        scope
            .string_buffer_append_value(buffer, Value::Reference(buffer))
            .unwrap();
        assert_eq!(scope.string_buffer(buffer), Ok("aé🙂aé🙂"));
        assert_eq!(scope.text(first), Ok("aé🙂"));
        let other = scope.new_string_buffer().unwrap();
        scope
            .string_buffer_append_value(other, Value::Text(first))
            .unwrap();
        scope
            .string_buffer_append_value(other, Value::Reference(buffer))
            .unwrap();
        assert_eq!(scope.string_buffer(other), Ok("aé🙂aé🙂aé🙂"));
        assert_eq!(
            scope.lookup_query(Value::Reference(other)).err(),
            Some(Error::WrongType)
        );
        scope.destroy(buffer).unwrap();
        assert_eq!(scope.text(first), Ok("aé🙂"));
        scope.destroy(first).unwrap();
        assert_eq!(
            scope.string_buffer_append_value(other, Value::Text(first)),
            Err(Error::Expired)
        );
        assert_eq!(scope.string_buffer(other), Ok("aé🙂aé🙂aé🙂"));
        scope.destroy(other).unwrap();
        assert_eq!(scope.store.string_bytes, 0);
    }

    #[test]
    fn string_buffer_growth_and_snapshot_failures_clean_the_store() {
        for snapshot in [false, true] {
            let mut state = store(10, 20);
            state.limits.string_bytes = 5;
            let mut scope = state.scope().unwrap();
            let buffer = scope.new_string_buffer().unwrap();
            scope.string_buffer_append(buffer, "abc").unwrap();
            let result = if snapshot {
                scope.string_buffer_snapshot(buffer)
            } else {
                scope.string_buffer_append(buffer, "def")
            };
            assert_eq!(result, Err(Error::ResourceLimit));
            assert!(scope.store.records.is_empty());
            assert_eq!(scope.store.string_bytes, 0);
        }
        let mut state = store(10, 20);
        let mut scope = state.scope().unwrap();
        let buffer = scope.new_string_buffer().unwrap();
        scope.store.fail_reservation = true;
        assert_eq!(
            scope.string_buffer_append(buffer, "x"),
            Err(Error::Allocation)
        );
        assert!(scope.store.records.is_empty());
        assert_eq!(scope.store.string_bytes, 0);
    }

    #[test]
    fn indexed_access_distinguishes_tables_vectors_and_immutable_lists() {
        let mut state = store(10, 30);
        let mut scope = state.scope().unwrap();
        let list = scope.new_list(&[Value::Int(5)]).unwrap();
        assert_eq!(scope.indexed_get(list, Value::Int(1)), Ok(Value::Int(5)));
        assert_eq!(scope.indexed_get(list, Value::Nil), Err(Error::WrongType));
        assert_eq!(
            scope.indexed_set(list, Value::Int(1), Value::Int(6)),
            Err(Error::WrongType)
        );
        let vector = scope.new_vector(1).unwrap();
        scope.vector_append(vector, Value::Int(1)).unwrap();
        scope
            .indexed_set(vector, Value::Int(1), Value::Int(7))
            .unwrap();
        assert_eq!(scope.indexed_get(vector, Value::Int(1)), Ok(Value::Int(7)));
        assert_eq!(
            scope.indexed_get(vector, Value::Int(0)),
            Err(Error::InvalidArgument)
        );
        let table = scope.new_lookup().unwrap();
        scope.indexed_set(table, Value::Nil, Value::Int(9)).unwrap();
        assert_eq!(scope.indexed_get(table, Value::Nil), Ok(Value::Int(9)));
    }

    #[test]
    fn lookup_keys_use_content_and_typed_identity_without_adopting_values() {
        let mut state = store(20, 40);
        let mut scope = state.scope().unwrap();
        let table = scope.new_lookup().unwrap();
        let text = scope.new_text("literal").unwrap();
        let same = scope.new_text("literal").unwrap();
        let literal = scope.store.literal(0).unwrap();
        let child = scope.create().unwrap();
        scope
            .lookup_set(table, Value::Int(-1), Value::Int(11))
            .unwrap();
        scope
            .lookup_set(table, Value::Text(text), Value::Reference(child))
            .unwrap();
        assert_eq!(
            scope.lookup_get(table, Value::Text(same)),
            Ok(Value::Reference(child))
        );
        assert_eq!(
            scope.lookup_get(table, Value::String(literal)),
            Ok(Value::Reference(child))
        );
        scope.store.fail_reservation = true;
        scope
            .lookup_set(table, Value::String(literal), Value::Int(22))
            .unwrap();
        scope
            .lookup_set(table, Value::Text(same), Value::Int(23))
            .unwrap();
        assert!(scope.store.fail_reservation);
        scope.store.fail_reservation = false;
        assert_eq!(scope.lookup_len(table), Ok(2));
        scope
            .lookup_set(table, Value::Reference(child), Value::Int(33))
            .unwrap();
        scope
            .lookup_set(table, Value::Property(PropertyId(1)), Value::Int(44))
            .unwrap();
        scope
            .lookup_set(table, Value::Enumerator(1), Value::Int(55))
            .unwrap();
        scope
            .lookup_set(table, Value::Function(1), Value::Int(66))
            .unwrap();
        assert_eq!(
            scope.lookup_get(table, Value::Reference(child)),
            Ok(Value::Int(33))
        );
        assert_eq!(
            scope.lookup_get(table, Value::Property(PropertyId(1))),
            Ok(Value::Int(44))
        );
        assert_eq!(
            scope.lookup_get(table, Value::Enumerator(1)),
            Ok(Value::Int(55))
        );
        assert_eq!(
            scope.lookup_get(table, Value::Function(1)),
            Ok(Value::Int(66))
        );
        scope.lookup_set_default(table, Value::Int(99)).unwrap();
        assert_eq!(scope.lookup_get(table, Value::Nil), Ok(Value::Int(99)));
        assert_eq!(scope.lookup_contains(table, Value::Nil), Ok(false));
        scope.lookup_set(table, Value::Nil, Value::Nil).unwrap();
        assert_eq!(scope.lookup_get(table, Value::Nil), Ok(Value::Nil));
        assert_eq!(scope.lookup_contains(table, Value::Nil), Ok(true));
        scope.destroy(table).unwrap();
        assert_eq!(scope.lookup_len(table), Err(Error::Expired));
        scope.validate_object(child).unwrap();
        assert_eq!(scope.store.properties, 0);
        assert_eq!(scope.store.string_bytes, 14);
    }

    #[test]
    fn lookup_admission_failure_is_terminal_and_reclaims_keys() {
        let mut state = store(10, 3);
        let mut scope = state.scope().unwrap();
        let table = scope.new_lookup().unwrap();
        let literal = scope.store.literal(0).unwrap();
        scope
            .lookup_set(table, Value::String(literal), Value::Int(1))
            .unwrap();
        assert_eq!(
            scope.lookup_set(table, Value::Int(2), Value::Int(2)),
            Err(Error::ResourceLimit)
        );
        assert!(scope.store.terminal);
        assert!(scope.store.records.is_empty());
        assert_eq!(scope.store.properties, 0);
        assert_eq!(scope.store.string_bytes, 0);
        let mut state = store(10, 20);
        let mut scope = state.scope().unwrap();
        let table = scope.new_lookup().unwrap();
        scope.store.fail_reservation = true;
        assert_eq!(
            scope.lookup_set(table, Value::Int(1), Value::Nil),
            Err(Error::Allocation)
        );
        assert!(scope.store.records.is_empty());
        assert_eq!(scope.store.properties, 0);
    }

    #[test]
    fn lookup_rejects_foreign_keys_and_unsupported_content_keys_without_mutation() {
        let mut foreign = store(10, 20);
        let mut other = foreign.scope().unwrap();
        let key = other.create().unwrap();
        let mut state = store(10, 20);
        let mut scope = state.scope().unwrap();
        let table = scope.new_lookup().unwrap();
        assert_eq!(
            scope.lookup_set(table, Value::Reference(key), Value::Nil),
            Err(Error::ForeignSession)
        );
        let vector = scope.new_vector(1).unwrap();
        assert_eq!(
            scope.lookup_set(table, Value::Reference(vector), Value::Nil),
            Err(Error::WrongType)
        );
        assert_eq!(
            scope.lookup_set_default(table, Value::Initializing),
            Err(Error::WrongType)
        );
        assert_eq!(scope.lookup_len(table), Ok(0));
        assert_eq!(scope.lookup_default(table), Ok(Value::Nil));
        for _ in 0..100 {
            let next = scope.new_lookup().unwrap();
            scope.lookup_set(next, Value::Int(1), Value::Nil).unwrap();
            scope.destroy(next).unwrap();
        }
        assert_eq!(scope.store.properties, 2);
    }

    #[test]
    fn nominal_kind_checks_identity_diamonds_and_expired_prototypes() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let base = objects.create().unwrap();
        let left = objects.create().unwrap();
        let right = objects.create().unwrap();
        let instance = objects.create().unwrap();
        let other = objects.create().unwrap();
        objects.add_prototype(left, base).unwrap();
        objects.add_prototype(right, base).unwrap();
        objects.add_prototype(instance, left).unwrap();
        objects.add_prototype(instance, right).unwrap();
        assert_eq!(objects.of_kind(instance, base), Ok(true));
        assert_eq!(objects.of_kind(instance, right), Ok(true));
        assert_eq!(objects.of_kind(instance, instance), Ok(true));
        assert_eq!(objects.of_kind(instance, other), Ok(false));
        assert_eq!(objects.of_kind(base, instance), Ok(false));
        objects.destroy(base).unwrap();
        assert_eq!(objects.of_kind(base, base), Err(Error::Expired));
        assert_eq!(objects.of_kind(instance, other), Err(Error::Expired));
        objects.store.fail_reservation = true;
        assert_eq!(objects.of_kind(instance, right), Err(Error::Allocation));
        assert!(objects.store.records.is_empty());
    }

    #[test]
    fn iteration_snapshot_preserves_order_without_retaining_removed_members() {
        let mut state = store(30, 40);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let collection = objects.new_owned_collection().unwrap();
        let mut members = Vec::new();
        for _ in 0..3 {
            let mut construction = objects.begin_published_construction(prototype).unwrap();
            members.push(construction.reference().unwrap());
            objects
                .register_owned_construction(collection, &mut construction)
                .unwrap();
            objects
                .finish_published_construction(&mut construction)
                .unwrap();
        }
        let mut cursor = objects
            .begin_iteration(Value::Reference(collection))
            .unwrap();
        assert_eq!(
            objects.iteration_next(&mut cursor),
            Ok(Some(Value::Reference(members[0])))
        );
        objects.remove_owned_member(collection, members[0]).unwrap();
        objects.remove_owned_member(collection, members[1]).unwrap();
        let mut added = objects.begin_published_construction(prototype).unwrap();
        objects
            .register_owned_construction(collection, &mut added)
            .unwrap();
        objects.finish_published_construction(&mut added).unwrap();
        assert_eq!(
            objects.iteration_next(&mut cursor),
            Ok(Some(Value::Reference(members[1])))
        );
        assert_eq!(objects.get(members[1], PropertyId(0)), Err(Error::Expired));
        assert_eq!(
            objects.iteration_next(&mut cursor),
            Ok(Some(Value::Reference(members[2])))
        );
        assert_eq!(objects.iteration_next(&mut cursor), Ok(None));
        objects.end_iteration(&mut cursor).unwrap();
        assert_eq!(
            objects.iteration_next(&mut cursor),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn iteration_owns_its_buffer_and_early_exit_reclaims_it() {
        let mut state = store(10, 10);
        let mut objects = state.scope().unwrap();
        let list = objects
            .new_list(&[Value::Int(1), Value::Nil, Value::Int(3)])
            .unwrap();
        let mut first = objects.begin_iteration(Value::List(list)).unwrap();
        objects.destroy(list).unwrap();
        assert_eq!(objects.iteration_next(&mut first), Ok(Some(Value::Int(1))));
        assert_eq!(objects.iteration_next(&mut first), Ok(Some(Value::Nil)));
        objects.end_iteration(&mut first).unwrap();
        assert!(objects.store.records.is_empty());
        assert_eq!(objects.store.string_bytes, 0);
        let empty = objects.new_list(&[]).unwrap();
        for _ in 0..30 {
            let mut cursor = objects.begin_iteration(Value::List(empty)).unwrap();
            assert_eq!(objects.iteration_next(&mut cursor), Ok(None));
            objects.end_iteration(&mut cursor).unwrap();
            assert_eq!(objects.store.records.len(), 1);
        }
        objects.store.fail_reservation = true;
        assert!(matches!(
            objects.begin_iteration(Value::List(empty)),
            Err(Error::Allocation)
        ));
        assert!(objects.store.records.is_empty());
    }

    #[test]
    fn a_collection_owns_text_and_lists_as_well_as_objects() {
        // A member is held through a private slot field, which takes all three
        // kinds, so an owner collection can hold text and lists too.
        let mut state = store(40, 60);
        let mut objects = state.scope().unwrap();
        let group = objects.new_owned_collection().unwrap();
        let object = objects.create().unwrap();
        let text = objects.new_text("token").unwrap();
        let list = objects
            .new_list(&[Value::Int(1), Value::Text(text)])
            .unwrap();
        objects.move_root_into_collection(group, object).unwrap();
        objects.move_root_into_collection(group, text).unwrap();
        objects.move_root_into_collection(group, list).unwrap();
        assert_eq!(objects.owned_collection_len(group), Ok(3));
        assert_eq!(objects.text(text), Ok("token"));
        assert_eq!(objects.list(list).map(<[Value]>::len), Ok(2));
        // Each is owned now, so a second move reports it is no longer a root.
        assert_eq!(
            objects.move_root_into_collection(group, text),
            Err(Error::NotOwned)
        );
        objects.destroy(group).unwrap();
        assert_eq!(objects.text(text), Err(Error::Expired));
        assert_eq!(objects.list(list).map(|_| ()), Err(Error::Expired));
    }

    #[test]
    fn member_transfer_preserves_order_identity_and_active_return_edge() {
        let mut state = store(20, 30);
        let mut objects = state.scope().unwrap();
        let a = objects.new_owned_collection().unwrap();
        let b = objects.new_owned_collection().unwrap();
        let first = objects.create().unwrap();
        let second = objects.create().unwrap();
        objects.move_root_into_collection(a, first).unwrap();
        objects.move_root_into_collection(a, second).unwrap();
        let mut call = objects.begin_member_call(first).unwrap().unwrap();
        assert_eq!(objects.move_owned_member(a, first, b), Ok(true));
        assert_eq!(objects.move_owned_member(a, first, b), Ok(false));
        assert_eq!(objects.move_owned_member(b, first, b), Ok(true));
        assert_eq!(objects.owned_collection_get(a, 1), Ok(second));
        assert_eq!(objects.owned_collection_get(b, 1), Ok(first));
        objects.destroy(a).unwrap();
        assert_eq!(objects.of_kind(second, second), Err(Error::Expired));
        objects.store.fail_reservation = true;
        assert_eq!(objects.finish_member_call(&mut call), Ok(first));
        objects.store.fail_reservation = false;
        objects.destroy(b).unwrap();
        assert_eq!(objects.of_kind(first, first), Err(Error::Expired));
        assert!(objects.store.records.is_empty());
    }

    #[test]
    fn pending_member_transfer_keeps_constructor_recipient_live() {
        let mut state = store(20, 30);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let a = objects.new_owned_collection().unwrap();
        let b = objects.new_owned_collection().unwrap();
        let mut construction = objects.begin_published_construction(prototype).unwrap();
        let member = construction.reference().unwrap();
        objects
            .register_owned_construction(a, &mut construction)
            .unwrap();
        assert_eq!(objects.move_owned_member(a, member, b), Ok(true));
        objects.destroy(a).unwrap();
        objects.store.fail_reservation = true;
        assert_eq!(
            objects.finish_published_construction(&mut construction),
            Ok(member)
        );
        objects.store.fail_reservation = false;
        assert_eq!(objects.owned_collection_get(b, 1), Ok(member));
        objects.destroy(b).unwrap();
        assert_eq!(objects.of_kind(member, member), Err(Error::Expired));
    }

    #[test]
    fn member_transfer_rejects_cycles_and_reclaims_on_admission_failure() {
        let mut state = store(20, 30);
        let mut objects = state.scope().unwrap();
        let a = objects.new_owned_collection().unwrap();
        let b = objects.new_owned_collection().unwrap();
        let member = objects.create().unwrap();
        objects.adopt(member, b).unwrap();
        objects.move_root_into_collection(a, member).unwrap();
        assert_eq!(
            objects.move_owned_member(a, member, b),
            Err(Error::OwnershipCycle)
        );
        let mut call = objects.begin_member_call(member).unwrap().unwrap();
        assert_eq!(
            objects.move_owned_member(a, member, b),
            Err(Error::OwnershipCycle)
        );
        assert_eq!(objects.owned_collection_get(a, 1), Ok(member));
        objects.finish_member_call(&mut call).unwrap();
        let c = objects.new_owned_collection().unwrap();
        objects.store.fail_reservation = true;
        assert_eq!(
            objects.move_owned_member(a, member, c),
            Err(Error::Allocation)
        );
        assert!(objects.store.records.is_empty());
    }

    #[test]
    fn executing_member_survives_withdrawal_and_can_select_a_new_recipient() {
        let mut state = store(30, 30);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let a = objects.new_owned_collection().unwrap();
        let b = objects.new_owned_collection().unwrap();
        let mut construction = objects.begin_published_construction(prototype).unwrap();
        let object = construction.reference().unwrap();
        objects
            .register_owned_construction(a, &mut construction)
            .unwrap();
        objects
            .finish_published_construction(&mut construction)
            .unwrap();
        let mut call = objects.begin_member_call(object).unwrap().unwrap();
        assert!(objects.begin_member_call(object).unwrap().is_none());
        assert_eq!(objects.owned_collection_get(a, 1), Ok(object));
        objects.remove_owned_member(a, object).unwrap();
        objects.set(object, PropertyId(1), Value::Int(42)).unwrap();
        objects.register_executing_member(b, &mut call).unwrap();
        objects.store.fail_reservation = true;
        objects.finish_member_call(&mut call).unwrap();
        assert!(objects.store.fail_reservation);
        objects.store.fail_reservation = false;
        assert_eq!(objects.owned_collection_get(b, 1), Ok(object));
        assert_eq!(objects.get(object, PropertyId(1)), Ok(Value::Int(42)));
        let mut call = objects.begin_member_call(object).unwrap().unwrap();
        objects.destroy(b).unwrap();
        assert_eq!(objects.get(object, PropertyId(1)), Ok(Value::Int(42)));
        objects.finish_member_call(&mut call).unwrap();
        assert_eq!(objects.get(object, PropertyId(1)), Err(Error::Expired));
        assert_eq!(objects.store.pending_constructions, 0);
        assert_eq!(objects.store.records.len(), 2);
    }

    #[test]
    fn member_call_return_restores_ownership_without_retaining_old_activations() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let collection = objects.new_owned_collection().unwrap();
        let mut construction = objects.begin_published_construction(prototype).unwrap();
        let object = construction.reference().unwrap();
        objects
            .register_owned_construction(collection, &mut construction)
            .unwrap();
        objects
            .finish_published_construction(&mut construction)
            .unwrap();
        let baseline = objects.store.records.len();
        for _ in 0..30 {
            let mut call = objects.begin_member_call(object).unwrap().unwrap();
            assert_eq!(
                objects.adopt(object, collection),
                Err(Error::OwnershipCycle)
            );
            objects.finish_member_call(&mut call).unwrap();
            assert_eq!(objects.store.records.len(), baseline);
            assert_eq!(objects.store.pending_constructions, 0);
        }
        objects.store.fail_reservation = true;
        assert!(matches!(
            objects.begin_member_call(object),
            Err(Error::Allocation)
        ));
        assert!(objects.store.records.is_empty());
    }

    #[test]
    fn collection_registration_cancellation_and_republication_are_atomic() {
        let mut state = store(30, 30);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let a = objects.new_owned_collection().unwrap();
        let b = objects.new_owned_collection().unwrap();
        let mut first = objects.begin_published_construction(prototype).unwrap();
        let member = first.reference().unwrap();
        objects.register_owned_construction(a, &mut first).unwrap();
        assert_eq!(objects.owned_collection_get(a, 1), Ok(member));
        assert_eq!(
            objects.register_owned_construction(b, &mut first),
            Err(Error::NotOwned)
        );
        assert_eq!(objects.owned_collection_len(b), Ok(0));
        assert_eq!(objects.remove_owned_member(a, member), Ok(true));
        assert_eq!(objects.get(member, PropertyId(1)), Ok(Value::Nil));
        objects.register_owned_construction(b, &mut first).unwrap();
        objects.store.fail_reservation = true;
        assert_eq!(
            objects.finish_published_construction(&mut first),
            Ok(member)
        );
        assert!(objects.store.fail_reservation);
        assert_eq!(objects.remove_owned_member(b, member), Ok(true));
        assert_eq!(objects.get(member, PropertyId(1)), Err(Error::Expired));
        assert!(objects.store.fail_reservation);
        objects.store.fail_reservation = false;
        assert_eq!(objects.store.pending_constructions, 0);
        assert_eq!(objects.store.records.len(), 3);
        for _ in 0..20 {
            let mut pending = objects.begin_published_construction(prototype).unwrap();
            let value = pending.reference().unwrap();
            objects
                .register_owned_construction(a, &mut pending)
                .unwrap();
            objects.remove_owned_member(a, value).unwrap();
            objects.finish_published_construction(&mut pending).unwrap();
            assert_eq!(objects.store.records.len(), 3);
        }
    }

    #[test]
    fn collection_checks_cycles_recipient_loss_and_admission_failure() {
        let mut state = store(20, 20);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let collection = objects.new_owned_collection().unwrap();
        let mut pending = objects.begin_published_construction(prototype).unwrap();
        let value = pending.reference().unwrap();
        objects.adopt(value, collection).unwrap();
        assert_eq!(
            objects.register_owned_construction(collection, &mut pending),
            Err(Error::OwnershipCycle)
        );
        assert_eq!(objects.owned_collection_len(collection), Ok(0));
        let recipient = objects.new_owned_collection().unwrap();
        objects
            .register_owned_construction(recipient, &mut pending)
            .unwrap();
        objects.destroy(recipient).unwrap();
        assert_eq!(objects.get(value, PropertyId(1)), Ok(Value::Nil));
        objects.finish_published_construction(&mut pending).unwrap();
        assert_eq!(objects.get(value, PropertyId(1)), Err(Error::Expired));
        assert_eq!(objects.store.pending_constructions, 0);
        let recipient = objects.new_owned_collection().unwrap();
        let mut pending = objects.begin_published_construction(prototype).unwrap();
        objects.store.fail_reservation = true;
        assert_eq!(
            objects.register_owned_construction(recipient, &mut pending),
            Err(Error::Allocation)
        );
        assert!(objects.store.terminal);
        assert!(objects.store.records.is_empty());
    }

    #[test]
    fn borrowed_container_backlinks_are_not_prospective_ownership_edges() {
        let mut state = store(10, 20);
        let mut objects = state.scope().unwrap();
        let region = objects.create().unwrap();
        let list = objects.new_list(&[]).unwrap();
        let child = objects.create().unwrap();
        objects.adopt(list, child).unwrap();
        objects
            .assign_list(child, PropertyId(1), list, region)
            .unwrap();
        assert_eq!(objects.get(child, PropertyId(1)), Ok(Value::List(list)));
        objects.destroy(list).unwrap();
        assert_eq!(objects.get(child, PropertyId(1)), Err(Error::Expired));
    }

    #[test]
    fn published_construction_reserves_one_recipient_and_finishes_without_allocation() {
        let mut state = store(40, 80);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let recipient = objects.create().unwrap();
        let registry = objects.create().unwrap();
        let mut construction = objects.begin_published_construction(prototype).unwrap();
        let object = construction.reference().unwrap();
        objects
            .reserve_construction(&mut construction, recipient, PropertyId(1))
            .unwrap();
        assert_eq!(
            objects.reserve_construction(&mut construction, registry, PropertyId(2)),
            Err(Error::NotOwned)
        );
        objects
            .set(registry, PropertyId(1), Value::Reference(object))
            .unwrap();
        assert_eq!(objects.get(recipient, PropertyId(1)), Ok(Value::Nil));
        // Other acquisitions must preserve capacity reserved for this handoff.
        for property in 2..12 {
            let mut other = objects.begin_construction(prototype).unwrap();
            objects
                .finish_construction(&mut other, recipient, PropertyId(property))
                .unwrap();
        }
        objects.store.fail_reservation = true;
        assert_eq!(
            objects.finish_published_construction(&mut construction),
            Ok(object)
        );
        assert!(objects.store.fail_reservation);
        assert_eq!(objects.store.pending_constructions, 0);
        assert_eq!(
            objects.get(recipient, PropertyId(1)),
            Ok(Value::Reference(object))
        );
        objects.store.fail_reservation = false;
        objects.set(recipient, PropertyId(1), Value::Nil).unwrap();
        assert_eq!(
            objects.get(registry, PropertyId(1)),
            Ok(Value::Reference(object))
        );
        assert_eq!(objects.get(object, PropertyId(0)), Err(Error::Expired));
    }

    #[test]
    fn published_construction_withdrawal_and_replacement_never_retarget_stale_slots() {
        let mut state = store(20, 30);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let recipient = objects.create().unwrap();
        let mut first = objects.begin_published_construction(prototype).unwrap();
        let a = first.reference().unwrap();
        objects
            .reserve_construction(&mut first, recipient, PropertyId(1))
            .unwrap();
        objects.set(recipient, PropertyId(1), Value::Nil).unwrap();
        objects.set(a, PropertyId(2), Value::Int(12)).unwrap();
        objects
            .reserve_construction(&mut first, recipient, PropertyId(1))
            .unwrap();
        objects.set(recipient, PropertyId(1), Value::Nil).unwrap();
        let mut second = objects.begin_published_construction(prototype).unwrap();
        let b = second.reference().unwrap();
        objects
            .reserve_construction(&mut second, recipient, PropertyId(1))
            .unwrap();
        assert_eq!(objects.finish_published_construction(&mut first), Ok(a));
        assert_eq!(objects.get(a, PropertyId(2)), Err(Error::Expired));
        assert_eq!(objects.get(recipient, PropertyId(1)), Ok(Value::Nil));
        objects.finish_published_construction(&mut second).unwrap();
        assert_eq!(
            objects.get(recipient, PropertyId(1)),
            Ok(Value::Reference(b))
        );
    }

    #[test]
    fn pending_handoffs_reject_prospective_cycles_and_survive_recipient_destruction() {
        let mut state = store(20, 30);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let recipient = objects.create().unwrap();
        let mut construction = objects.begin_published_construction(prototype).unwrap();
        let object = construction.reference().unwrap();
        assert_eq!(
            objects.reserve_construction(&mut construction, object, PropertyId(1)),
            Err(Error::OwnershipCycle)
        );
        objects
            .reserve_construction(&mut construction, recipient, PropertyId(1))
            .unwrap();
        assert_eq!(objects.adopt(object, recipient), Err(Error::OwnershipCycle));
        objects.destroy(recipient).unwrap();
        assert_eq!(objects.store.pending_constructions, 0);
        objects.set(object, PropertyId(2), Value::Int(3)).unwrap();
        objects
            .finish_published_construction(&mut construction)
            .unwrap();
        assert_eq!(objects.get(object, PropertyId(2)), Err(Error::Expired));
        assert_eq!(objects.store.live_objects(), 1);
    }

    #[test]
    fn completed_local_construction_transfers_identity_and_owned_children() {
        let mut state = store(12, 12);
        let mut foreign = store(12, 12);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let borrowed = objects.create().unwrap();
        let mut construction = objects.begin_construction(prototype).unwrap();
        let object = construction.reference().unwrap();
        let child = objects.create().unwrap();
        objects.adopt(object, child).unwrap();
        objects
            .set(object, PropertyId(1), Value::Reference(borrowed))
            .unwrap();
        assert_eq!(
            foreign
                .objects()
                .finish_local_construction(&mut construction),
            Err(Error::ForeignSession)
        );
        assert_eq!(construction.reference(), Some(object));
        objects.store.fail_reservation = true;
        assert_eq!(
            objects.finish_local_construction(&mut construction),
            Ok(object)
        );
        assert_eq!(construction.reference(), None);
        assert_eq!(objects.store.live_objects(), 4);
        assert_eq!(
            objects.finish_local_construction(&mut construction),
            Err(Error::InvalidArgument)
        );
        objects.destroy(object).unwrap();
        assert_eq!(objects.get(child, PropertyId(1)), Err(Error::Expired));
        assert_eq!(objects.get(borrowed, PropertyId(1)), Ok(Value::Nil));
        assert_eq!(objects.store.live_objects(), 2);
    }

    #[test]
    fn exception_owner_moves_without_allocation_and_reclaims_its_subtree() {
        let mut state = store(12, 12);
        let mut foreign = store(12, 12);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let borrowed = objects.create().unwrap();
        let mut construction = objects.begin_construction(prototype).unwrap();
        let object = construction.reference().unwrap();
        let child = objects.create().unwrap();
        objects.adopt(object, child).unwrap();
        objects
            .set(object, PropertyId(1), Value::Reference(borrowed))
            .unwrap();
        // Neither ownership handoff nor release needs a fresh reservation.
        objects.store.fail_reservation = true;
        let owner = objects.finish_exception(&mut construction).unwrap();
        assert!(construction.reference().is_none());
        let mut received = owner;
        assert_eq!(received.reference(), Some(object));
        assert_eq!(
            foreign.objects().release_exception(&mut received),
            Err(Error::ForeignSession)
        );
        assert_eq!(received.reference(), Some(object));
        assert_eq!(
            objects.get(object, PropertyId(1)),
            Ok(Value::Reference(borrowed))
        );
        objects.release_exception(&mut received).unwrap();
        assert_eq!(objects.get(object, PropertyId(1)), Err(Error::Expired));
        assert_eq!(objects.get(child, PropertyId(1)), Err(Error::Expired));
        assert_eq!(objects.get(borrowed, PropertyId(1)), Ok(Value::Nil));
        assert_eq!(
            objects.release_exception(&mut received),
            Err(Error::InvalidArgument)
        );
        assert_eq!(objects.store.live_objects(), 2);
    }

    #[test]
    fn owned_construction_keeps_active_self_and_transfers_only_on_completion() {
        let mut state = store(20, 30);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        objects
            .set(prototype, PropertyId(1), Value::Int(10))
            .unwrap();
        let recipient = objects.create().unwrap();
        let registry = objects.create().unwrap();
        let mut construction = objects.begin_construction(prototype).unwrap();
        let object = construction.reference().unwrap();
        assert_eq!(objects.get(object, PropertyId(1)), Ok(Value::Int(10)));
        assert_eq!(objects.destroy(object), Err(Error::NotOwned));
        objects
            .set(registry, PropertyId(2), Value::Reference(object))
            .unwrap();
        objects.set(registry, PropertyId(2), Value::Nil).unwrap();
        objects.set(object, PropertyId(1), Value::Int(12)).unwrap();
        objects
            .set(registry, PropertyId(2), Value::Reference(object))
            .unwrap();
        assert_eq!(
            objects.finish_construction(&mut construction, recipient, PropertyId(3)),
            Ok(object)
        );
        assert_eq!(construction.reference(), None);
        assert_eq!(objects.get(object, PropertyId(1)), Ok(Value::Int(12)));
        assert_eq!(
            objects.get(recipient, PropertyId(3)),
            Ok(Value::Reference(object))
        );
        objects.set(recipient, PropertyId(3), Value::Nil).unwrap();
        assert_eq!(
            objects.get(registry, PropertyId(2)),
            Ok(Value::Reference(object))
        );
        assert_eq!(objects.get(object, PropertyId(1)), Err(Error::Expired));
        assert_eq!(
            objects.abort_construction(&mut construction),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn failed_owned_construction_preserves_destination_and_reclaims_each_activation() {
        let mut state = store(8, 20);
        let mut objects = state.scope().unwrap();
        let prototype = objects.create().unwrap();
        let holder = objects.create().unwrap();
        objects.set(holder, PropertyId(1), Value::Int(99)).unwrap();
        for _ in 0..100 {
            let mut construction = objects.begin_construction(prototype).unwrap();
            let object = construction.reference().unwrap();
            objects
                .set(holder, PropertyId(2), Value::Reference(object))
                .unwrap();
            let child = objects.create().unwrap();
            objects.adopt(object, child).unwrap();
            objects.abort_construction(&mut construction).unwrap();
            assert_eq!(objects.store.live_objects(), 2);
            assert_eq!(objects.get(child, PropertyId(0)), Err(Error::Expired));
            assert_eq!(objects.get(holder, PropertyId(1)), Ok(Value::Int(99)));
        }
        let mut construction = objects.begin_construction(prototype).unwrap();
        let object = construction.reference().unwrap();
        assert_eq!(
            objects.finish_construction(&mut construction, object, PropertyId(1)),
            Err(Error::OwnershipCycle)
        );
        assert_eq!(objects.get(object, PropertyId(0)), Err(Error::Expired));
        assert_eq!(objects.store.live_objects(), 2);
    }

    #[test]
    fn construction_terminal_admission_and_foreign_finalization_are_checked() {
        let mut first = store(8, 8);
        let mut second = store(8, 8);
        let mut objects = first.scope().unwrap();
        let prototype = objects.create().unwrap();
        let mut construction = objects.begin_construction(prototype).unwrap();
        assert_eq!(
            second.objects().abort_construction(&mut construction),
            Err(Error::ForeignSession)
        );
        assert!(construction.reference().is_some());
        objects.abort_construction(&mut construction).unwrap();
        let mut construction = objects.begin_construction(prototype).unwrap();
        let holder = objects.create().unwrap();
        objects.store.fail_reservation = true;
        assert_eq!(
            objects.finish_construction(&mut construction, holder, PropertyId(1)),
            Err(Error::Allocation)
        );
        assert_eq!(objects.store.live_objects(), 0);
        assert!(construction.reference().is_none());
    }

    #[test]
    fn lists_preserve_immutable_aliases_and_one_based_indexing() {
        let mut store = store(16, 32);
        let mut scope = store.scope().unwrap();
        let original = scope.new_list(&[Value::Int(10), Value::Int(20)]).unwrap();
        let alias = original;
        let appended = scope.list_append(original, Value::Int(30)).unwrap();
        let replaced = scope.list_replace(original, 1, Value::Int(7)).unwrap();
        assert_eq!(
            scope.list(replaced).unwrap(),
            &[Value::Int(7), Value::Int(20)]
        );
        assert_eq!(
            scope.list(alias).unwrap(),
            &[Value::Int(10), Value::Int(20)]
        );
        assert_eq!(
            scope.list(appended).unwrap(),
            &[Value::Int(10), Value::Int(20), Value::Int(30)]
        );
        assert_eq!(scope.list_get(appended, 3), Ok(Value::Int(30)));
        for index in [i32::MIN, -1, 0, 4, i32::MAX] {
            assert_eq!(scope.list_get(appended, index), Err(Error::InvalidArgument));
        }
        assert_eq!(
            scope.list_replace(original, 0, Value::Nil),
            Err(Error::InvalidArgument)
        );
        assert_eq!(
            scope.list(original).unwrap(),
            &[Value::Int(10), Value::Int(20)]
        );
        scope.destroy(original).unwrap();
        assert_eq!(scope.list_get(alias, 1), Err(Error::Expired));
        assert_eq!(scope.list_get(replaced, 1), Ok(Value::Int(7)));
        scope.destroy(appended).unwrap();
        scope.destroy(replaced).unwrap();
        assert_eq!(scope.store.properties, 0);
    }

    #[test]
    fn list_elements_borrow_and_explicit_adoption_owns() {
        let mut store = store(16, 32);
        let mut scope = store.scope().unwrap();
        let target = scope.create().unwrap();
        let text = scope.new_text("hello").unwrap();
        let inner = scope.new_list(&[Value::Reference(target)]).unwrap();
        let outer = scope
            .new_list(&[
                Value::List(inner),
                Value::Text(text),
                Value::Property(PropertyId(7)),
            ])
            .unwrap();
        scope.adopt(outer, inner).unwrap();
        scope.adopt(outer, text).unwrap();
        assert_eq!(scope.adopt(inner, outer), Err(Error::OwnershipCycle));
        scope.destroy(target).unwrap();
        assert_eq!(scope.list_get(inner, 1), Ok(Value::Reference(target)));
        assert_eq!(scope.get(target, PropertyId(1)), Err(Error::Expired));
        assert_eq!(scope.list(inner).unwrap().len(), 1);
        scope.destroy(outer).unwrap();
        assert_eq!(scope.list(inner), Err(Error::Expired));
        assert_eq!(scope.text(text), Err(Error::Expired));
        assert_eq!(scope.store.properties, 0);
        assert_eq!(scope.store.string_bytes, 0);
    }

    /// a vocabulary relation read forward answers the words that name
    /// an entity, and its `contains` asks about one entity rather than any.
    /// Self-authored: the reference dictionary has no relation shape.
    #[test]
    fn a_dictionary_answers_the_words_that_name_one_entity() {
        let mut store = store(16, 64);
        let mut scope = store.scope().unwrap();
        let dictionary = scope.new_dictionary().unwrap();
        let (coin, lamp) = (scope.create().unwrap(), scope.create().unwrap());
        let noun = PropertyId(1);
        let adjective = PropertyId(2);
        scope
            .dictionary_add(dictionary, coin, "coin", noun)
            .unwrap();
        scope
            .dictionary_add(dictionary, coin, "shilling", noun)
            .unwrap();
        scope
            .dictionary_add(dictionary, coin, "brass", adjective)
            .unwrap();
        scope
            .dictionary_add(dictionary, lamp, "lamp", noun)
            .unwrap();

        // In the order they were filed, and only this entity's words under this
        // part of speech.
        let words = scope.dictionary_words(dictionary, coin, noun).unwrap();
        let spellings: Vec<String> = scope
            .list(words)
            .unwrap()
            .iter()
            .map(|value| match value {
                Value::Text(text) => scope.text(*text).unwrap().to_owned(),
                other => panic!("not text: {other:?}"),
            })
            .collect();
        assert_eq!(spellings, vec!["coin".to_owned(), "shilling".to_owned()]);

        // A different part of speech is a different relation.
        let adjectives = scope.dictionary_words(dictionary, coin, adjective).unwrap();
        assert_eq!(scope.list(adjectives).unwrap().len(), 1);
        // An entity nothing names answers nothing rather than failing.
        let none = scope.dictionary_words(dictionary, lamp, adjective).unwrap();
        assert!(scope.list(none).unwrap().is_empty());

        // `contains` is about this entity, not about any entity: `lamp` is a word
        // in the dictionary, but it is not one of the coin's.
        assert_eq!(
            scope.dictionary_names(dictionary, coin, "coin", noun),
            Ok(true)
        );
        assert_eq!(
            scope.dictionary_names(dictionary, coin, "lamp", noun),
            Ok(false)
        );
        assert_eq!(scope.dictionary_defined(dictionary, "lamp"), Ok(true));
        // And the part of speech is part of the row.
        assert_eq!(
            scope.dictionary_names(dictionary, coin, "brass", noun),
            Ok(false)
        );
        assert_eq!(
            scope.dictionary_names(dictionary, coin, "brass", adjective),
            Ok(true)
        );
    }

    #[test]
    fn dictionary_results_are_owned_lists_with_borrowed_targets() {
        let mut store = store(16, 32);
        let mut scope = store.scope().unwrap();
        let dictionary = scope.new_dictionary().unwrap();
        let a = scope.create().unwrap();
        scope
            .dictionary_add(dictionary, a, "lamp", PropertyId(1))
            .unwrap();
        scope
            .dictionary_add(dictionary, a, "lamp", PropertyId(2))
            .unwrap();
        let all = scope
            .dictionary_find_list(dictionary, "lamp", None)
            .unwrap();
        let filtered = scope
            .dictionary_find_list(dictionary, "lamp", Some(PropertyId(1)))
            .unwrap();
        let empty = scope
            .dictionary_find_list(dictionary, "Lamp", None)
            .unwrap();
        assert_eq!(
            scope.list(all).unwrap(),
            &[
                Value::Reference(a),
                Value::Int(1),
                Value::Reference(a),
                Value::Int(1)
            ]
        );
        assert_eq!(
            scope.list(filtered).unwrap(),
            &[Value::Reference(a), Value::Int(1)]
        );
        assert!(scope.list(empty).unwrap().is_empty());
        scope.destroy(dictionary).unwrap();
        assert_eq!(scope.list_get(filtered, 1), Ok(Value::Reference(a)));
        scope.destroy(filtered).unwrap();
        scope.set(a, PropertyId(5), Value::Int(42)).unwrap();
        assert_eq!(scope.get(a, PropertyId(5)), Ok(Value::Int(42)));
    }

    #[test]
    fn vectors_mutate_identity_and_snapshot_values_without_adopting_targets() {
        let mut state = store(20, 40);
        let mut scope = state.scope().unwrap();
        let vector = scope.new_vector(3).unwrap();
        assert!(scope.vector(vector).unwrap().is_empty());
        for n in [10, 20, 30] {
            assert_eq!(scope.vector_append(vector, Value::Int(n)), Ok(vector));
        }
        assert_eq!(scope.vector_get(vector, 1), Ok(Value::Int(10)));
        assert_eq!(scope.vector_get(vector, 3), Ok(Value::Int(30)));
        assert_eq!(scope.vector_get(vector, -1), Err(Error::InvalidArgument));
        assert_eq!(scope.vector_remove_range(vector, -2, -1), Ok(vector));
        scope.vector_set_length(vector, 3).unwrap();
        assert_eq!(
            scope.vector(vector).unwrap(),
            &[Value::Int(10), Value::Nil, Value::Nil]
        );
        scope.vector_set(vector, 2, Value::Int(40)).unwrap();
        let mut snapshot = scope.begin_iteration(Value::Reference(vector)).unwrap();
        scope.vector_set_length(vector, 0).unwrap();
        for expected in [Value::Int(10), Value::Int(40), Value::Nil] {
            assert_eq!(scope.iteration_next(&mut snapshot), Ok(Some(expected)));
        }
        assert_eq!(scope.iteration_next(&mut snapshot), Ok(None));
        scope.end_iteration(&mut snapshot).unwrap();
        let target = scope.create().unwrap();
        scope
            .vector_append(vector, Value::Reference(target))
            .unwrap();
        scope.destroy(target).unwrap();
        assert_eq!(scope.vector_get(vector, 1), Ok(Value::Reference(target)));
        assert_eq!(scope.validate_object(target), Err(Error::Expired));
        let retained = scope.create().unwrap();
        scope
            .vector_append(vector, Value::Reference(retained))
            .unwrap();
        scope.destroy(vector).unwrap();
        scope.validate_object(retained).unwrap();
        assert_eq!(scope.vector(vector), Err(Error::Expired));
    }

    #[test]
    fn vector_validation_and_storage_admission_preserve_invariants() {
        let mut state = store(10, 4);
        let mut other = store(2, 2);
        let foreign = other.scope().unwrap().create().unwrap();
        let mut scope = state.scope().unwrap();
        assert_eq!(scope.new_vector(-1), Err(Error::InvalidArgument));
        let vector = scope.new_vector(2).unwrap();
        assert_eq!(
            scope.vector_append(vector, Value::Reference(foreign)),
            Err(Error::ForeignSession)
        );
        assert_eq!(
            scope.vector_append(vector, Value::Method(0)),
            Err(Error::WrongType)
        );
        scope.vector_append(vector, Value::Int(1)).unwrap();
        scope.vector_append(vector, Value::Int(2)).unwrap();
        assert_eq!(
            scope.vector_remove_range(vector, 2, 1),
            Err(Error::InvalidArgument)
        );
        assert_eq!(
            scope.vector_set(vector, 0, Value::Int(3)),
            Err(Error::InvalidArgument)
        );
        assert_eq!(
            scope.vector(vector).unwrap(),
            &[Value::Int(1), Value::Int(2)]
        );
        scope.vector_append(vector, Value::Int(3)).unwrap();
        scope.vector_set_length(vector, 0).unwrap();
        assert_eq!(scope.store.properties, 3); // Shrinking retains reserved storage.
        scope.destroy(vector).unwrap();
        assert_eq!(scope.store.properties, 0);
        let vector = scope.new_vector(4).unwrap();
        assert_eq!(
            scope.vector_set_length(vector, 5),
            Err(Error::ResourceLimit)
        );
        assert_eq!(scope.vector(vector), Err(Error::Terminal));
        assert_eq!(scope.store.properties, 0);
        let mut state = store(4, 4);
        let mut scope = state.scope().unwrap();
        let vector = scope.new_vector(0).unwrap();
        scope.store.fail_reservation = true;
        assert_eq!(
            scope.vector_append(vector, Value::Nil),
            Err(Error::Allocation)
        );
        assert_eq!(scope.store.live_objects(), 0);
    }

    #[test]
    fn lists_share_bounded_storage_and_reject_invalid_elements() {
        let mut first = store(8, 2);
        let mut second = store(2, 2);
        let foreign = second.scope().unwrap().create().unwrap();
        let mut scope = first.scope().unwrap();
        assert_eq!(
            scope.new_list(&[Value::Reference(foreign)]),
            Err(Error::ForeignSession)
        );
        assert_eq!(scope.new_list(&[Value::Method(0)]), Err(Error::WrongType));
        let list = scope.new_list(&[Value::Int(1), Value::Int(2)]).unwrap();
        assert_eq!(
            scope.list_append(list, Value::Nil),
            Err(Error::ResourceLimit)
        );
        assert!(scope.store.records.is_empty());
        assert_eq!(scope.store.properties, 0);
        assert_eq!(scope.list_get(list, 1), Err(Error::Terminal));
    }

    #[test]
    fn native_list_construction_moves_nested_temporaries_before_publication() {
        let mut store = store(20, 32);
        let mut scope = store.scope().unwrap();
        let holder = scope.create().unwrap();
        let region = scope.create().unwrap();
        let text = scope.new_text("temporary").unwrap();
        scope.adopt(region, text).unwrap();
        let inner = scope.begin_list(2).unwrap();
        scope.adopt(region, inner).unwrap();
        assert_eq!(scope.list(inner), Err(Error::InitializationCycle));
        assert_eq!(scope.finish_list(inner), Err(Error::InvalidArgument));
        scope.push_list(inner, Value::Text(text), region).unwrap();
        scope.push_list(inner, Value::Bool(true), region).unwrap();
        scope.finish_list(inner).unwrap();
        assert_eq!(
            scope.push_list(inner, Value::Nil, region),
            Err(Error::InvalidArgument)
        );
        let outer = scope.begin_list(2).unwrap();
        scope.adopt(region, outer).unwrap();
        scope.push_list(outer, Value::List(inner), region).unwrap();
        scope.push_list(outer, Value::List(inner), region).unwrap();
        scope.finish_list(outer).unwrap();
        scope
            .assign_list(holder, PropertyId(0), outer, region)
            .unwrap();
        scope.destroy(region).unwrap();
        assert_eq!(scope.list_get(outer, 2), Ok(Value::List(inner)));
        assert_eq!(scope.list_get(inner, 1), Ok(Value::Text(text)));
        assert_eq!(scope.text(text), Ok("temporary"));
        scope.destroy(holder).unwrap();
        assert_eq!(scope.text(text), Err(Error::Expired));
        assert_eq!(scope.store.properties, 0);
        assert_eq!(scope.store.string_bytes, 0);
    }

    #[test]
    fn partial_list_cleanup_releases_reserved_slots_and_owned_children() {
        let mut store = store(20, 32);
        let mut scope = store.scope().unwrap();
        let region = scope.create().unwrap();
        let text = scope.new_text("temporary").unwrap();
        scope.adopt(region, text).unwrap();
        let list = scope.begin_list(20).unwrap();
        scope.adopt(region, list).unwrap();
        scope.push_list(list, Value::Text(text), region).unwrap();
        assert_eq!(scope.store.properties, 20);
        scope.destroy(region).unwrap();
        assert_eq!(scope.store.properties, 0);
        assert_eq!(scope.store.string_bytes, 0);
    }

    #[test]
    fn temporary_lists_transfer_to_fields_and_replacement_expires_borrows() {
        let mut store = store(16, 32);
        let mut scope = store.scope().unwrap();
        let holder = scope.create().unwrap();
        let region = scope.create().unwrap();
        let first = scope.new_list(&[Value::Int(10)]).unwrap();
        scope.adopt(region, first).unwrap();
        scope
            .assign_list(holder, PropertyId(1), first, region)
            .unwrap();
        scope
            .set(holder, PropertyId(2), Value::List(first))
            .unwrap();
        scope.destroy(region).unwrap();
        assert_eq!(scope.list_get(first, 1), Ok(Value::Int(10)));
        let next_region = scope.create().unwrap();
        let next = scope.new_list(&[Value::Int(20)]).unwrap();
        scope.adopt(next_region, next).unwrap();
        scope
            .assign_list(holder, PropertyId(1), next, next_region)
            .unwrap();
        assert_eq!(scope.get(holder, PropertyId(2)), Ok(Value::List(first)));
        assert_eq!(scope.list_get(first, 1), Err(Error::Expired));
        scope.destroy(next_region).unwrap();
        assert_eq!(scope.list_get(next, 1), Ok(Value::Int(20)));
        scope.destroy(holder).unwrap();
        assert_eq!(scope.list_get(next, 1), Err(Error::Expired));
        assert_eq!(scope.store.properties, 0);
    }

    #[test]
    fn list_field_transfer_rejects_cycles_before_changing_owners() {
        let mut store = store(16, 32);
        let mut scope = store.scope().unwrap();
        let region = scope.create().unwrap();
        let list = scope.new_list(&[]).unwrap();
        let child = scope.create().unwrap();
        scope.adopt(region, list).unwrap();
        scope.adopt(list, child).unwrap();
        assert_eq!(
            scope.assign_list(child, PropertyId(1), list, region),
            Err(Error::OwnershipCycle)
        );
        assert_eq!(scope.get(child, PropertyId(1)), Ok(Value::Nil));
        scope.destroy(region).unwrap();
        assert_eq!(scope.list(list), Err(Error::Expired));
        assert_eq!(scope.get(child, PropertyId(1)), Err(Error::Expired));
    }

    #[test]
    fn copying_a_word_buffer_fails_through_terminal_cleanup() {
        let mut store = store(8, 8);
        let mut scope = store.scope().unwrap();
        let word = scope.new_text("word").unwrap();
        scope.store.fail_reservation = true;
        assert_eq!(scope.copy_text_buffer(word), Err(Error::Allocation));
        assert_eq!(scope.store.string_bytes, 0);
        assert!(scope.store.records.is_empty());
        assert_eq!(scope.text(word), Err(Error::Terminal));
    }

    #[test]
    fn list_allocation_failure_reclaims_dictionary_and_prior_lists() {
        let mut store = store(16, 32);
        let mut scope = store.scope().unwrap();
        let dictionary = scope.new_dictionary().unwrap();
        let a = scope.create().unwrap();
        scope
            .dictionary_add(dictionary, a, "lamp", PropertyId(1))
            .unwrap();
        scope.new_list(&[Value::Nil]).unwrap();
        scope.store.fail_reservation = true;
        assert_eq!(
            scope.dictionary_find_list(dictionary, "lamp", None),
            Err(Error::Allocation)
        );
        assert_eq!(scope.store.properties, 0);
        assert_eq!(scope.store.string_bytes, 0);
        assert!(scope.store.records.is_empty());
    }

    #[test]
    fn dictionary_exact_associations_match_reference_and_do_not_own_targets() {
        let mut store = store(8, 8);
        let mut scope = store.scope().unwrap();
        let dictionary = scope.new_dictionary().unwrap();
        let a = scope.create().unwrap();
        let b = scope.create().unwrap();
        let noun = PropertyId(1);
        let adjective = PropertyId(2);
        scope.dictionary_add(dictionary, a, "lamp", noun).unwrap();
        scope.dictionary_add(dictionary, a, "lamp", noun).unwrap();
        scope
            .dictionary_add(dictionary, a, "lamp", adjective)
            .unwrap();
        assert_eq!(
            scope
                .dictionary_find(dictionary, "lamp", Some(noun))
                .unwrap(),
            vec![(a, 1)]
        );
        assert!(
            scope
                .dictionary_find(dictionary, "Lamp", Some(noun))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            scope.dictionary_find(dictionary, "lamp", None).unwrap(),
            vec![(a, 1), (a, 1)]
        );
        scope
            .dictionary_remove(dictionary, a, "lamp", noun)
            .unwrap();
        assert!(
            scope
                .dictionary_find(dictionary, "lamp", Some(noun))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            scope
                .dictionary_find(dictionary, "lamp", Some(adjective))
                .unwrap(),
            vec![(a, 1)]
        );
        scope.dictionary_add(dictionary, b, "lamp", noun).unwrap();
        scope
            .dictionary_remove(dictionary, a, "lamp", noun)
            .unwrap();
        assert_eq!(
            scope
                .dictionary_find(dictionary, "lamp", Some(noun))
                .unwrap(),
            vec![(b, 1)]
        );
        scope.destroy(dictionary).unwrap();
        scope.set(a, noun, Value::Int(42)).unwrap();
        assert_eq!(scope.get(a, noun), Ok(Value::Int(42)));
        assert_eq!(scope.store.string_bytes, 0);
        assert_eq!(scope.store.properties, 1);
    }

    #[test]
    fn dictionary_borrows_expire_and_foreign_targets_are_rejected() {
        let mut first = store(6, 8);
        let mut second = store(2, 2);
        let foreign = second.scope().unwrap().create().unwrap();
        let mut scope = first.scope().unwrap();
        let dictionary = scope.new_dictionary().unwrap();
        let a = scope.create().unwrap();
        let p = PropertyId(1);
        assert_eq!(
            scope.dictionary_add(dictionary, foreign, "lamp", p),
            Err(Error::ForeignSession)
        );
        assert_eq!(
            scope.dictionary_find(a, "lamp", None),
            Err(Error::WrongType)
        );
        scope.dictionary_add(dictionary, a, "lamp", p).unwrap();
        scope.destroy(a).unwrap();
        assert_eq!(
            scope.dictionary_find(dictionary, "lamp", None),
            Err(Error::Expired)
        );
        scope.dictionary_remove(dictionary, a, "lamp", p).unwrap();
        assert!(
            scope
                .dictionary_find(dictionary, "lamp", None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(scope.store.string_bytes, 0);
        assert_eq!(scope.store.properties, 0);
    }

    #[test]
    fn dictionary_resources_are_bounded_reclaimed_and_fail_atomically() {
        let mut store = store(8, 1);
        store.limits.string_bytes = 4;
        let mut scope = store.scope().unwrap();
        let dictionary = scope.new_dictionary().unwrap();
        let a = scope.create().unwrap();
        let p = PropertyId(1);
        scope.dictionary_add(dictionary, a, "lamp", p).unwrap();
        scope.dictionary_add(dictionary, a, "lamp", p).unwrap();
        scope.dictionary_remove(dictionary, a, "lamp", p).unwrap();
        scope.dictionary_add(dictionary, a, "word", p).unwrap();
        assert_eq!(
            scope.dictionary_add(dictionary, a, "other", p),
            Err(Error::ResourceLimit)
        );
        assert!(scope.store.records.is_empty());
        assert_eq!(scope.store.string_bytes, 0);
        assert_eq!(scope.store.properties, 0);
        assert_eq!(
            scope.dictionary_find(dictionary, "word", None),
            Err(Error::Terminal)
        );
    }

    #[test]
    fn dictionary_entries_share_the_property_and_inheritance_budget() {
        let mut store = store(8, 1);
        let mut scope = store.scope().unwrap();
        let dictionary = scope.new_dictionary().unwrap();
        let a = scope.create().unwrap();
        let b = scope.create().unwrap();
        scope.add_prototype(a, b).unwrap();
        assert_eq!(
            scope.dictionary_add(dictionary, a, "lamp", PropertyId(1)),
            Err(Error::ResourceLimit)
        );
        assert_eq!(scope.store.inheritance_edges, 0);
    }

    #[test]
    fn dictionary_allocation_failures_release_all_owned_storage() {
        for fail_query in [false, true] {
            let mut store = store(8, 8);
            let mut scope = store.scope().unwrap();
            let dictionary = scope.new_dictionary().unwrap();
            let a = scope.create().unwrap();
            let p = PropertyId(1);
            scope.dictionary_add(dictionary, a, "lamp", p).unwrap();
            scope.store.fail_reservation = true;
            if fail_query {
                assert_eq!(
                    scope.dictionary_find(dictionary, "lamp", None),
                    Err(Error::Allocation)
                );
            } else {
                assert_eq!(
                    scope.dictionary_add(dictionary, a, "word", p),
                    Err(Error::Allocation)
                );
            }
            assert!(scope.store.records.is_empty());
            assert_eq!(scope.store.string_bytes, 0);
            assert_eq!(scope.store.properties, 0);
        }
    }

    #[test]
    fn owned_text_operations_copy_content_and_release_with_owners() {
        let mut store = store(20, 10);
        let mut scope = store.scope().unwrap();
        let owner = scope.create().unwrap();
        let left = scope.new_text("aé😀").unwrap();
        let right = scope.new_text("z\0").unwrap();
        let joined = scope.concat_text(left, right).unwrap();
        let same = scope.new_text("aé😀z\0").unwrap();
        assert!(scope.text_equal(joined, same).unwrap());
        let middle = scope.substring_text(joined, 3, Some(1)).unwrap();
        assert_eq!(scope.text(middle), Ok("😀"));
        let clipped = scope.substring_text(joined, 0, Some(2)).unwrap();
        assert_eq!(scope.text(clipped), Ok("aé"));
        let tail = scope.substring_text(joined, -3, Some(-1)).unwrap();
        assert_eq!(scope.text(tail), Ok("😀z"));
        scope.destroy(left).unwrap();
        scope.destroy(right).unwrap();
        assert_eq!(scope.text(joined), Ok("aé😀z\0"));
        scope.adopt(owner, joined).unwrap();
        scope.destroy(owner).unwrap();
        assert_eq!(scope.text(joined), Err(Error::Expired));
    }

    #[test]
    fn info_keys_string_operations_use_owned_temporary_results() {
        let mut store = store(20, 10);
        let mut scope = store.scope().unwrap();
        for (hex, expected) in [
            ("C0A848D6", "192.168.72.214"),
            ("C0A80B26", "192.168.11.38"),
        ] {
            let source = scope.new_text(hex).unwrap();
            let dot = scope.new_text(".").unwrap();
            let mut output = scope.new_text("").unwrap();
            for part in 0..4 {
                let slice = scope.substring_text(source, 1 + part * 2, Some(2)).unwrap();
                let number = scope.text_integer(slice, 16).unwrap();
                let decimal = scope.integer_text(number, 10, true).unwrap();
                if part != 0 {
                    let next = scope.concat_text(output, dot).unwrap();
                    scope.destroy(output).unwrap();
                    output = next;
                }
                let next = scope.concat_text(output, decimal).unwrap();
                scope.destroy(output).unwrap();
                output = next;
                scope.destroy(slice).unwrap();
                scope.destroy(decimal).unwrap();
            }
            assert_eq!(scope.text(output).unwrap(), expected);
            scope.destroy(source).unwrap();
            scope.destroy(dot).unwrap();
            scope.destroy(output).unwrap();
            assert_eq!(scope.store.string_bytes, 0);
        }
    }

    #[test]
    fn text_budget_and_failed_allocation_clean_the_store() {
        let mut store = store(10, 10);
        store.limits.string_bytes = 8;
        let mut scope = store.scope().unwrap();
        for _ in 0..32 {
            let value = scope.new_text("12345678").unwrap();
            scope.destroy(value).unwrap();
            assert_eq!(scope.store.string_bytes, 0);
        }
        let value = scope.new_text("12345").unwrap();
        assert_eq!(scope.concat_text(value, value), Err(Error::ResourceLimit));
        assert_eq!(scope.store.string_bytes, 0);
        assert_eq!(scope.store.live_objects(), 0);
        drop(scope);
        let mut fresh = Store::new(Limits {
            objects: 2,
            properties: 2,
            string_bytes: 8,
        })
        .unwrap();
        let mut scope = fresh.scope().unwrap();
        scope.new_text("a").unwrap();
        scope.store.fail_reservation = true;
        assert_eq!(scope.new_text("b"), Err(Error::Allocation));
        assert_eq!(scope.store.string_bytes, 0);
    }

    #[test]
    fn integer_text_preserves_radix_sign_and_overflow_behavior() {
        let mut store = store(20, 10);
        let mut scope = store.scope().unwrap();
        for (value, radix, signed, expected) in [
            (255, 16, false, "FF"),
            (-1, 16, false, "FFFFFFFF"),
            (-1, 16, true, "-1"),
            (i32::MIN, 10, true, "-2147483648"),
        ] {
            let text = scope.integer_text(value, radix, signed).unwrap();
            assert_eq!(scope.text(text).unwrap(), expected);
        }
        for (text, radix, expected) in [
            ("123abc", 10, 123),
            ("abc", 10, 0),
            ("  -12x", 10, -12),
            ("FFFFFFFF", 16, -1),
            (" true ", 10, 1),
            ("nil", 10, 0),
        ] {
            let text = scope.new_text(text).unwrap();
            assert_eq!(scope.text_integer(text, radix).unwrap(), expected);
        }
        let overflow = scope.new_text("2147483648").unwrap();
        assert_eq!(
            scope.text_integer(overflow, 10),
            Err(Error::NumericOverflow)
        );
        assert_eq!(scope.integer_text(1, 1, true), Err(Error::InvalidArgument));
    }

    #[test]
    fn superclass_edges_are_fallible_and_share_the_property_budget() {
        for allocation_failure in [false, true] {
            let mut store = store(3, 1);
            let mut scope = store.scope().unwrap();
            let a = scope.create().unwrap();
            let b = scope.create().unwrap();
            let c = scope.create().unwrap();
            if allocation_failure {
                scope.store.fail_reservation = true;
                assert_eq!(scope.add_prototype(a, b), Err(Error::Allocation));
            } else {
                scope.add_prototype(a, b).unwrap();
                assert_eq!(scope.add_prototype(a, c), Err(Error::ResourceLimit));
            }
            assert_eq!(scope.store.live_objects(), 0);
            assert_eq!(scope.get(a, PropertyId(0)), Err(Error::Terminal));
        }
    }

    #[test]
    fn inheritance_is_non_owning_and_rejected_changes_are_atomic() {
        let mut store = store(4, 4);
        let mut scope = store.scope().unwrap();
        let base = scope.create().unwrap();
        let child = scope.create().unwrap();
        let leaf = scope.create().unwrap();
        scope.set(base, PropertyId(0), Value::Int(7)).unwrap();
        scope.add_prototype(child, base).unwrap();
        scope.add_prototype(leaf, child).unwrap();
        assert_eq!(scope.get(leaf, PropertyId(0)), Ok(Value::Int(7)));
        assert_eq!(
            scope.add_prototype(base, leaf),
            Err(Error::InheritanceCycle)
        );
        assert_eq!(scope.get(leaf, PropertyId(0)), Ok(Value::Int(7)));
        scope.set(child, PropertyId(0), Value::Nil).unwrap();
        assert_eq!(scope.get(leaf, PropertyId(0)), Ok(Value::Nil));
        scope.destroy(base).unwrap(); // Inheritance does not retain/own base.
        assert_eq!(scope.get(leaf, PropertyId(0)), Ok(Value::Nil));
        assert_eq!(scope.get(leaf, PropertyId(1)), Err(Error::Expired));
        scope.destroy(child).unwrap();
        assert_eq!(scope.get(leaf, PropertyId(0)), Err(Error::Expired));
    }

    #[test]
    fn properties_cycles_and_expired_identity() {
        let mut store = store(8, 8);
        let stale;
        {
            let mut scope = store.scope().unwrap();
            let a = scope.create().unwrap();
            let b = scope.create().unwrap();
            assert_eq!(scope.get(a, PropertyId(0)), Ok(Value::Nil));
            scope.set(a, PropertyId(0), Value::Reference(b)).unwrap();
            scope.set(b, PropertyId(0), Value::Reference(a)).unwrap();
            scope.adopt(a, b).unwrap();
            assert_eq!(scope.adopt(b, a), Err(Error::OwnershipCycle));
            assert_eq!(scope.destroy(b), Err(Error::NotOwned));
            scope.destroy(a).unwrap();
            assert_eq!(scope.get(b, PropertyId(0)), Err(Error::Expired));
            let c = scope.create().unwrap();
            assert_ne!(a, c);
            scope.set(c, PropertyId(0), Value::Reference(a)).unwrap();
            assert_eq!(scope.get(c, PropertyId(0)), Ok(Value::Reference(a)));
            stale = c;
        }
        assert!(!store.is_valid(stale));
        assert_eq!(store.live_objects(), 0);
        assert_eq!(store.properties, 0);
    }

    #[test]
    fn forgotten_scope_inventory_remains_owned_by_store() {
        let mut store = store(2, 0);
        let mut scope = store.scope().unwrap();
        let old = scope.create().unwrap();
        std::mem::forget(scope);
        let scope = store.scope().unwrap();
        assert!(!scope.store.is_valid(old));
        assert_eq!(scope.store.live_objects(), 0);
    }

    #[test]
    fn sessions_do_not_alias() {
        let mut first = store(1, 1);
        let mut second = store(1, 1);
        let mut a = first.scope().unwrap();
        let mut b = second.scope().unwrap();
        let x = a.create().unwrap();
        let y = b.create().unwrap();
        assert_eq!(b.get(x, PropertyId(0)), Err(Error::ForeignSession));
        assert_eq!(
            b.set(y, PropertyId(0), Value::Reference(x)),
            Err(Error::ForeignSession)
        );
        let text = a.store.literal(0).unwrap();
        assert_eq!(
            b.set(y, PropertyId(0), Value::String(text)),
            Err(Error::ForeignSession)
        );
    }

    #[test]
    fn an_unreserved_construction_belongs_to_the_turn() {
        // `new Thing` with no owning field handed back a reference
        // to an object it had just destroyed, because the instance was still a
        // child of the activation when the activation was released. The turn
        // owns it instead: it survives the construction, it is a root, and it
        // is freed with the rest of the turn's records.
        let mut store = store(20, 20);
        let mut scope = store.scope().unwrap();
        let prototype = scope.create().unwrap();
        scope.set(prototype, PropertyId(0), Value::Int(5)).unwrap();
        scope.set_lifetime_model(true);
        scope.set_default_lifetime(Lifetime::Turn);
        let mut construction = scope.begin_published_construction(prototype).unwrap();
        let object = scope
            .finish_published_construction(&mut construction)
            .unwrap();
        // It is alive, it inherits, and it can be written to.
        assert_eq!(scope.get(object, PropertyId(0)), Ok(Value::Int(5)));
        scope.set(object, PropertyId(0), Value::Int(9)).unwrap();
        assert_eq!(scope.get(object, PropertyId(0)), Ok(Value::Int(9)));
        assert_eq!(scope.lifetime(object), Ok(Lifetime::Turn));
        // The activation it was built in is gone, and it is not a child of it.
        assert!(scope.release_turn_records().unwrap() >= 1);
        assert_eq!(scope.get(object, PropertyId(0)), Err(Error::Expired));
    }

    #[test]
    fn a_construction_world_state_keeps_is_promoted() {
        // The other half of : storing the new object into a World
        // record promotes it, so it outlives the turn it was made in.
        let mut store = store(20, 20);
        let mut scope = store.scope().unwrap();
        let prototype = scope.create().unwrap();
        let keeper = scope.create().unwrap();
        scope.set_lifetime_model(true);
        scope.set_default_lifetime(Lifetime::Turn);
        let mut construction = scope.begin_published_construction(prototype).unwrap();
        let object = scope
            .finish_published_construction(&mut construction)
            .unwrap();
        scope.set(object, PropertyId(0), Value::Int(3)).unwrap();
        scope
            .set(keeper, PropertyId(1), Value::Reference(object))
            .unwrap();
        assert_eq!(scope.lifetime(object), Ok(Lifetime::World));
        scope.release_turn_records().unwrap();
        assert_eq!(scope.get(object, PropertyId(0)), Ok(Value::Int(3)));
    }

    #[test]
    fn resource_failure_cleans_and_terminates() {
        let mut store = store(2, 1);
        let mut scope = store.scope().unwrap();
        let a = scope.create().unwrap();
        scope.set(a, PropertyId(0), Value::Int(1)).unwrap();
        scope.set(a, PropertyId(0), Value::Int(2)).unwrap();
        assert_eq!(
            scope.set(a, PropertyId(1), Value::Nil),
            Err(Error::ResourceLimit)
        );
        assert_eq!(scope.store.live_objects(), 0);
        assert_eq!(scope.create(), Err(Error::Terminal));
        drop(scope);
        assert!(matches!(store.scope(), Err(Error::Terminal)));
    }

    #[test]
    fn failed_reservation_and_identity_exhaustion_publish_nothing() {
        for identity in [false, true] {
            let mut store = store(3, 3);
            let mut scope = store.scope().unwrap();
            scope.create().unwrap();
            if identity {
                scope.store.next_identity = u64::MAX;
            } else {
                scope.store.fail_reservation = true;
            }
            assert_eq!(
                scope.create(),
                Err(if identity {
                    Error::IdentityExhausted
                } else {
                    Error::Allocation
                })
            );
            assert_eq!(scope.store.live_objects(), 0);
        }
    }

    #[test]
    fn deep_ownership_cleanup_is_iterative() {
        let mut store = store(20_000, 0);
        {
            let mut scope = store.scope().unwrap();
            let mut parent = scope.create().unwrap();
            // Build upwards to keep transfer cycle checking constant work.
            for _ in 1..20_000 {
                let next = scope.create().unwrap();
                scope.adopt(next, parent).unwrap();
                parent = next;
            }
        }
        assert_eq!(store.live_objects(), 0);
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;

    fn store(objects: usize, properties: usize) -> Store {
        Store::new(Limits {
            objects,
            properties,
            string_bytes: 8 * 1024,
        })
        .unwrap()
    }

    /// the declared presentation state of an entity, answered as
    /// triples of property id, value tag and payload — and inherited, because
    /// what a Thing is drawn as is mostly what its class says.
    #[test]
    fn presentation_answers_declared_properties_and_inherits_them() {
        const DECLARES: u32 = 1;
        const NAME: u32 = 2;
        const IS_OPEN: u32 = 3;
        let mut state = store(8, 16);
        let mut objects = state.scope().unwrap();

        let listed = objects
            .new_list(&[
                Value::Property(PropertyId(NAME)),
                Value::Property(PropertyId(IS_OPEN)),
            ])
            .unwrap();
        let class = objects.create().unwrap();
        objects
            .set(class, PropertyId(DECLARES), Value::List(listed))
            .unwrap();
        // The class says things are shut unless they say otherwise.
        objects
            .set(class, PropertyId(IS_OPEN), Value::Bool(false))
            .unwrap();

        let chest = objects.create().unwrap();
        objects.add_prototype(chest, class).unwrap();
        objects
            .set(chest, PropertyId(NAME), Value::Int(11))
            .unwrap();
        objects
            .set(chest, PropertyId(IS_OPEN), Value::Bool(true))
            .unwrap();

        // Its own name, and its own open state overriding the class's.
        assert_eq!(
            objects.presentation(chest, DECLARES).unwrap(),
            vec![u64::from(NAME), 2, 11, u64::from(IS_OPEN), 1, 1]
        );

        // A sibling that says nothing inherits the class's answer instead.
        let crate_ = objects.create().unwrap();
        objects.add_prototype(crate_, class).unwrap();
        let answered = objects.presentation(crate_, DECLARES).unwrap();
        assert_eq!(&answered[3..], &[u64::from(IS_OPEN), 1, 0]);
        // Nothing named it, so its name is nil rather than absent: a host
        // reading triples needs one per declared property, always.
        assert_eq!(&answered[..3], &[u64::from(NAME), 0, 0]);
    }

    /// An entity that declares nothing is not an error. A game may keep a class
    /// to itself, and a host asking gets an empty answer rather than a failure.
    #[test]
    fn an_entity_that_declares_no_presentation_answers_nothing() {
        let mut state = store(4, 4);
        let mut objects = state.scope().unwrap();
        let plain = objects.create().unwrap();
        assert!(objects.presentation(plain, 1).unwrap().is_empty());
    }

    /// an index nothing was bound to answers nothing, rather than
    /// guessing at whatever is nearby.
    ///
    /// The **bound** case needs a world built the way the emitted initializer
    /// builds one, which is more world than a unit test should assemble by
    /// hand; it is covered natively instead, by `exercises/no-parser-host.py`
    /// resolving `ingot` and `chest` from the manifest and finding both in the
    /// vault's contents.
    #[test]
    fn an_unbound_declared_index_answers_nothing() {
        let mut state = store(4, 4);
        let objects = state.scope().unwrap();
        assert_eq!(objects.declared(7), None);
        assert_eq!(objects.declared(0), None);
    }
}
