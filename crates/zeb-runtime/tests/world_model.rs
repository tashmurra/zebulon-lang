#![forbid(unsafe_code)]
//! /: the two lifetimes and relation tables. Expectations here
//! are self-authored: this model deliberately diverges from TADS, so there is
//! no external oracle for it.
use zeb_runtime::objects::{Error, Lifetime, Limits, PropertyId, Store, Value};
use zeb_runtime::relations::{Cardinality, Relations};

fn store() -> Store {
    Store::new(Limits {
        objects: 64,
        properties: 256,
        string_bytes: 4096,
    })
    .unwrap()
}

#[test]
fn a_turn_value_stored_into_world_state_is_promoted_with_what_it_reaches() {
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let world = scope.create().unwrap();
    scope.set_default_lifetime(Lifetime::Turn);
    let record = scope.create().unwrap();
    let nested = scope.create().unwrap();
    let text = scope.new_text("kept").unwrap();
    scope
        .set(
            record,
            zeb_runtime::objects::PropertyId(1),
            Value::Reference(nested),
        )
        .unwrap();
    scope
        .set(
            nested,
            zeb_runtime::objects::PropertyId(2),
            Value::Text(text),
        )
        .unwrap();
    assert_eq!(scope.lifetime(record), Ok(Lifetime::Turn));
    // Storing it into a world record promotes the whole reachable set.
    scope
        .set(
            world,
            zeb_runtime::objects::PropertyId(0),
            Value::Reference(record),
        )
        .unwrap();
    for promoted in [record, nested, text] {
        assert_eq!(scope.lifetime(promoted), Ok(Lifetime::World));
    }
    // Anything the turn kept to itself goes when the turn ends.
    let scratch = scope.create().unwrap();
    assert_eq!(scope.lifetime(scratch), Ok(Lifetime::Turn));
    assert!(scope.release_turn_records().unwrap() >= 1);
    assert_eq!(scope.text(text), Ok("kept"));
    assert_eq!(
        scope.get(world, zeb_runtime::objects::PropertyId(0)),
        Ok(Value::Reference(record))
    );
    assert_eq!(scope.of_kind(scratch, scratch), Err(Error::Expired));
}

#[test]
fn a_reused_slot_does_not_answer_for_its_previous_occupant() {
    // the handle carries its incarnation across the boundary, which
    // is what makes reuse safe. Self-authored.
    let mut store = store();
    let mut scope = store.scope().unwrap();
    scope.set_default_lifetime(Lifetime::Turn);
    let first = scope.create().unwrap();
    let stale = first.handle();
    scope.release_turn_records().unwrap();
    let second = scope.create().unwrap();
    assert_eq!(
        second.handle() & 0xffff_ffff,
        stale & 0xffff_ffff,
        "the freed slot is handed out again"
    );
    assert_ne!(
        second.handle(),
        stale,
        "but its handle differs, because the incarnation moved on"
    );
    assert_ne!(first, second);
    assert_eq!(scope.of_kind(first, first), Err(Error::Expired));
    assert_eq!(scope.of_kind(second, second), Ok(true));
}

#[test]
fn containment_is_a_table_read_both_ways_with_a_journal_behind_it() {
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let (room, box_, coin, desk) = (
        scope.create().unwrap(),
        scope.create().unwrap(),
        scope.create().unwrap(),
        scope.create().unwrap(),
    );
    let mut relations = Relations::default();
    relations.set_journalling(true);
    let contains = relations.declare(Cardinality::OneToMany).unwrap();
    for (container, item) in [(room, box_), (room, desk), (box_, coin)] {
        relations.set(contains, container, item).unwrap();
    }
    // Forward reads the container's contents; the reverse name reads location.
    assert_eq!(relations.all(contains, room, false).unwrap().len(), 2);
    assert_eq!(relations.get(contains, coin, true), Ok(Some(box_)));
    assert_eq!(relations.get(contains, room, false), Err(Error::WrongType));
    assert_eq!(
        relations.ancestors(contains, coin, false).unwrap(),
        vec![box_, room]
    );
    assert_eq!(
        relations.descendants(contains, room, false).unwrap().len(),
        3
    );
    assert_eq!(relations.outermost(contains, coin, false), Ok(room));
    // One container at a time: moving the coin retracts the old row.
    relations.set(contains, desk, coin).unwrap();
    assert_eq!(relations.get(contains, coin, true), Ok(Some(desk)));
    assert!(!relations.contains(contains, box_, coin).unwrap());
    assert_eq!(relations.outermost(contains, coin, false), Ok(room));
    // Every change is journalled, which is what undo would replay inverted.
    let journal = relations.journal();
    assert_eq!(journal.len(), 5);
    assert!(!journal[3].added);
    assert_eq!((journal[3].left, journal[3].right), (box_, coin));
    assert!(journal[4].added);
    relations.clear_journal();
    assert!(relations.journal().is_empty());
}

#[test]
fn cardinality_decides_what_a_row_displaces() {
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let (a, b, c) = (
        scope.create().unwrap(),
        scope.create().unwrap(),
        scope.create().unwrap(),
    );
    let mut relations = Relations::default();
    relations.set_journalling(true);
    let pair = relations.declare(Cardinality::OneToOne).unwrap();
    relations.set(pair, a, b).unwrap();
    relations.set(pair, a, c).unwrap();
    assert_eq!(relations.all(pair, a, false).unwrap(), &[c]);
    assert!(relations.all(pair, b, true).unwrap().is_empty());
    let many = relations.declare(Cardinality::ManyToMany).unwrap();
    relations.set(many, a, b).unwrap();
    relations.set(many, c, b).unwrap();
    assert_eq!(relations.all(many, b, true).unwrap().len(), 2);
}

#[test]
fn a_row_may_name_the_same_entity_twice() {
    // an actor who knows about herself, a door whose two sides are
    // the same room. The walks stop on a repeat, so nothing here loops, and
    // refusing the row cost a meaning the table has no business judging.
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let a = scope.create().unwrap();
    let mut relations = Relations::default();
    let knows = relations.declare(Cardinality::ManyToMany).unwrap();
    relations.set(knows, a, a).unwrap();
    assert!(relations.contains(knows, a, a).unwrap());
    assert_eq!(relations.all(knows, a, false).unwrap(), &[a]);
    // The walk that the refusal claimed to protect terminates.
    assert!(relations.ancestors(knows, a, false).unwrap().is_empty());
    assert!(relations.descendants(knows, a, false).unwrap().is_empty());
    assert!(relations.unset(knows, a, a).unwrap());
    assert!(!relations.contains(knows, a, a).unwrap());
}

#[test]
fn reverting_a_cycle_puts_displaced_rows_back() {
    // replaying a cycle backwards restores both the row it wrote and
    // the row its cardinality displaced. Self-authored.
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let room = scope.create().unwrap();
    let box_ = scope.create().unwrap();
    let coin = scope.create().unwrap();
    let mut relations = Relations::default();
    relations.ensure(0, Cardinality::OneToMany).unwrap();
    relations.set(0, room, box_).unwrap();
    relations.set(0, room, coin).unwrap();
    // World construction records nothing, so it can never be reverted.
    assert!(relations.journal().is_empty());
    relations.set_journalling(true);
    relations.set(0, box_, coin).unwrap();
    assert_eq!(relations.get(0, coin, true).unwrap(), Some(box_));
    let cycle = relations.take_journal();
    assert_eq!(
        cycle.len(),
        2,
        "the write and the row it displaced are both recorded"
    );
    relations.revert(&cycle);
    assert_eq!(
        relations.get(0, coin, true).unwrap(),
        Some(room),
        "the coin is back where it was"
    );
    assert!(relations.contains(0, room, box_).unwrap());
}

#[test]
fn reverting_a_cycle_puts_property_writes_back() {
    // a write made during a cycle is recorded with the value it
    // replaced, and a write that created a property leaves none behind when it
    // is put back. Self-authored.
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let thing = scope.create().unwrap();
    let (score, tag) = (PropertyId(1), PropertyId(2));
    scope.set(thing, score, Value::Int(1)).unwrap();
    // World construction records nothing.
    assert_eq!(scope.property_journal_len(), 0);

    scope.set_journalling(true);
    scope.set(thing, score, Value::Int(2)).unwrap();
    scope.set(thing, tag, Value::Bool(true)).unwrap();
    assert_eq!(scope.property_journal_len(), 2);
    let cycle = scope.take_property_journal();

    assert_eq!(scope.revert_properties(&cycle), 2);
    assert_eq!(scope.get(thing, score).unwrap(), Value::Int(1));
    assert_eq!(
        scope.get(thing, tag).unwrap(),
        Value::Nil,
        "a property the cycle created reads as nil again"
    );
    // Reverting is not itself recorded, so the cycle cannot be undone twice.
    assert_eq!(scope.property_journal_len(), 0);
}

#[test]
fn promotion_follows_every_path_into_world_state() {
    // Promotion must visit a turn record reachable through several paths,
    // including a cycle in the references between them.
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let world = scope.create().unwrap();
    scope.set_default_lifetime(Lifetime::Turn);
    let (first, second, shared) = (
        scope.create().unwrap(),
        scope.create().unwrap(),
        scope.create().unwrap(),
    );
    let (p1, p2) = (PropertyId(1), PropertyId(2));

    // Two turn records reach the same third one.
    scope.set(first, p1, Value::Reference(shared)).unwrap();
    scope.set(second, p1, Value::Reference(shared)).unwrap();
    // And the references run in a cycle, which promotion must not loop on.
    scope.set(shared, p2, Value::Reference(first)).unwrap();
    for record in [first, second, shared] {
        assert_eq!(scope.lifetime(record), Ok(Lifetime::Turn));
    }

    // Storing one of them into world state promotes everything it reaches.
    scope.set(world, p1, Value::Reference(first)).unwrap();
    assert_eq!(scope.lifetime(first), Ok(Lifetime::World));
    assert_eq!(
        scope.lifetime(shared),
        Ok(Lifetime::World),
        "the shared record is reachable from the promoted one"
    );
    assert_eq!(
        scope.lifetime(second),
        Ok(Lifetime::Turn),
        "the other holder is not reachable from world state and stays a turn record"
    );

    // Ending the cycle frees the record that stayed behind and nothing else.
    scope.release_turn_records().unwrap();
    assert_eq!(scope.lifetime(second), Err(Error::Expired));
    assert_eq!(scope.lifetime(first), Ok(Lifetime::World));
    assert_eq!(
        scope.lifetime(shared),
        Ok(Lifetime::World),
        "a record the freed holder also pointed at survives, because the world reaches it too"
    );
    assert_eq!(scope.get(world, p1).unwrap(), Value::Reference(first));
    assert_eq!(scope.get(first, p1).unwrap(), Value::Reference(shared));
}

#[test]
fn a_record_two_world_subgraphs_reach_survives_either_of_them_dropping_it() {
    // Promotion is reached from two separate
    // world subgraphs rather than from one turn record holding another. Nothing
    // counts references, so dropping one path must not take the record with it,
    // and promoting through the second path must not undo the first.
    // Self-authored.
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let (left, right) = (scope.create().unwrap(), scope.create().unwrap());
    scope.set_default_lifetime(Lifetime::Turn);
    let shared = scope.create().unwrap();
    let (p1, p2) = (PropertyId(1), PropertyId(2));

    // The first world subgraph stores it, which promotes it.
    scope.set(left, p1, Value::Reference(shared)).unwrap();
    assert_eq!(scope.lifetime(shared), Ok(Lifetime::World));
    // The second stores the same record. Promotion is idempotent.
    scope.set(right, p1, Value::Reference(shared)).unwrap();
    assert_eq!(scope.lifetime(shared), Ok(Lifetime::World));

    // One subgraph drops it. Nothing is reference counted, so the record stays.
    scope.set(left, p1, Value::Nil).unwrap();
    assert_eq!(
        scope.lifetime(shared),
        Ok(Lifetime::World),
        "the other subgraph still reaches it"
    );
    assert_eq!(scope.get(right, p1).unwrap(), Value::Reference(shared));

    // And the cycle boundary does not reclaim it, because it is no longer a
    // turn record however it was reached.
    scope.release_turn_records().unwrap();
    assert_eq!(scope.lifetime(shared), Ok(Lifetime::World));
    assert_eq!(scope.get(right, p1).unwrap(), Value::Reference(shared));

    // A turn record reached only from a promoted record's own field is promoted
    // too, which is the transitive half of the same rule read from the far side.
    scope.set_default_lifetime(Lifetime::Turn);
    let deeper = scope.create().unwrap();
    scope.set(shared, p2, Value::Reference(deeper)).unwrap();
    assert_eq!(scope.lifetime(deeper), Ok(Lifetime::World));
    scope.release_turn_records().unwrap();
    assert_eq!(scope.lifetime(deeper), Ok(Lifetime::World));
}

#[test]
fn a_labelled_relation_is_a_family_of_tables_listed_in_label_order() {
    // /: one table per label, and the family can be read as a
    // whole so a room can list the ways out of it. Self-authored.
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let (study, hall, cellar) = (
        scope.create().unwrap(),
        scope.create().unwrap(),
        scope.create().unwrap(),
    );
    let mut relations = Relations::default();
    let (north, south, down) = (0u32, 1u32, 2u32);

    // Labels are created on first use, and out of order on purpose.
    let s = relations
        .ensure_labelled(0, south, Cardinality::OneToOne)
        .unwrap();
    let n = relations
        .ensure_labelled(0, north, Cardinality::OneToOne)
        .unwrap();
    let d = relations
        .ensure_labelled(0, down, Cardinality::OneToOne)
        .unwrap();
    assert_ne!(n, s, "two labels are two tables");

    relations.set(n, study, hall).unwrap();
    relations.set(s, study, cellar).unwrap();
    // The same partner reached by a second label.
    relations.set(d, study, cellar).unwrap();

    // Each table answers only its own rows.
    assert_eq!(relations.all(n, study, false).unwrap(), &[hall]);
    assert_eq!(relations.all(s, study, false).unwrap(), &[cellar]);

    // And the family is listed in label order, whatever order it was built in.
    assert_eq!(
        relations.tables_of(0),
        vec![(north, n), (south, s), (down, d)]
    );

    // A second relation's tables are not in the first one's family.
    let other = relations
        .ensure_labelled(1, north, Cardinality::OneToOne)
        .unwrap();
    assert_eq!(relations.tables_of(1), vec![(north, other)]);
    assert_eq!(
        relations.tables_of(0),
        vec![(north, n), (south, s), (down, d)],
        "declaring another relation's table does not join this family"
    );
}

#[test]
fn a_reverse_name_walks_and_writes_the_table_the_way_it_reads_it() {
    // `location` reads contains right to left, so every operation
    // through it runs that way, not just `get` and `all`. Self-authored.
    let mut store = store();
    let mut scope = store.scope().unwrap();
    let (room, box_, coin) = (
        scope.create().unwrap(),
        scope.create().unwrap(),
        scope.create().unwrap(),
    );
    let mut relations = Relations::default();
    let contains = relations.declare(Cardinality::OneToMany).unwrap();

    relations.set(contains, room, box_).unwrap();
    relations.set(contains, box_, coin).unwrap();

    // Forward: the room's chain runs down, the coin's runs up.
    assert_eq!(
        relations.ancestors(contains, coin, false).unwrap(),
        vec![box_, room]
    );
    assert_eq!(
        relations.descendants(contains, room, false).unwrap().len(),
        2
    );

    // Reversed, the same two walks trade places: reading the table right to left
    // makes the coin's descendants its containers.
    assert_eq!(
        relations.descendants(contains, coin, true).unwrap(),
        vec![box_, room]
    );
    assert_eq!(
        relations.ancestors(contains, room, true).unwrap(),
        vec![box_, coin],
        "walking right from the room reaches what it holds, transitively"
    );

    // A chain walked the other way need not be one chain, so there is no
    // outermost to answer or to cache.
    assert_eq!(relations.outermost(contains, coin, false), Ok(room));
    assert_eq!(
        relations.outermost(contains, coin, true),
        Err(Error::WrongType)
    );
}

#[test]
fn first_writes_restore_mutable_containers_vocabulary_bytes_and_metadata() {
    use zeb_runtime::objects::PropertyId;
    let mut store = store();
    let mut o = store.scope().unwrap();
    o.set_lifetime_model(true);
    let entity = o.create().unwrap();
    let prototype = o.create().unwrap();
    let vector = o.new_vector(2).unwrap();
    o.vector_append(vector, Value::Int(1)).unwrap();
    let lookup = o.new_lookup_sized(4, 2).unwrap();
    o.lookup_set(lookup, Value::Int(1), Value::Int(2)).unwrap();
    let dict = o.new_dictionary().unwrap();
    o.dictionary_add(dict, entity, "one", PropertyId(1))
        .unwrap();
    let buffer = o.new_string_buffer().unwrap();
    o.string_buffer_append(buffer, "before").unwrap();
    o.set(entity, PropertyId(7), Value::Int(1)).unwrap();
    o.set_journalling(true);
    o.set(entity, PropertyId(7), Value::Int(2)).unwrap();
    o.set(entity, PropertyId(7), Value::Int(3)).unwrap();
    assert_eq!(o.property_journal_len(), 1);
    o.vector_set(vector, 1, Value::Int(8)).unwrap();
    o.vector_append(vector, Value::Int(9)).unwrap();
    o.vector_remove_range(vector, 1, 1).unwrap();
    o.lookup_set(lookup, Value::Int(1), Value::Int(8)).unwrap();
    o.lookup_set(lookup, Value::Int(2), Value::Int(9)).unwrap();
    o.lookup_set_default(lookup, Value::Int(10)).unwrap();
    o.dictionary_remove(dict, entity, "one", PropertyId(1))
        .unwrap();
    o.dictionary_add(dict, entity, "two", PropertyId(1))
        .unwrap();
    o.string_buffer_append(buffer, " after").unwrap();
    o.string_buffer_append_value(buffer, Value::Reference(buffer))
        .unwrap();
    o.add_prototype(entity, prototype).unwrap();
    o.set_transient(entity, true).unwrap();
    assert_eq!(o.container_journal_len(), 5);
    let containers = o.take_container_journal();
    assert_eq!(o.revert_containers(containers), 5);
    let properties = o.take_property_journal();
    assert_eq!(o.revert_properties(&properties), 1);
    assert_eq!(o.vector(vector).unwrap(), &[Value::Int(1)]);
    assert_eq!(o.lookup_len(lookup).unwrap(), 1);
    assert_eq!(o.lookup_get(lookup, Value::Int(1)).unwrap(), Value::Int(2));
    assert_eq!(o.lookup_default(lookup).unwrap(), Value::Nil);
    assert!(o.dictionary_has(dict, "one", PropertyId(1)).unwrap());
    assert!(!o.dictionary_has(dict, "two", PropertyId(1)).unwrap());
    assert_eq!(o.string_buffer(buffer).unwrap(), "before");
    assert!(!o.is_transient(entity).unwrap());
    assert!(o.prototype_list(entity).unwrap().is_empty());
    assert_eq!(o.get(entity, PropertyId(7)).unwrap(), Value::Int(1));
}

#[test]
fn relation_undo_restores_both_insertion_orders() {
    let mut store = store();
    let mut o = store.scope().unwrap();
    let a = o.create().unwrap();
    let b = o.create().unwrap();
    let c = o.create().unwrap();
    let mut r = Relations::default();
    let i = r.declare(Cardinality::ManyToMany).unwrap();
    r.set(i, a, b).unwrap();
    r.set(i, a, c).unwrap();
    r.set(i, c, b).unwrap();
    r.set_journalling(true);
    r.unset(i, a, b).unwrap();
    let journal = r.take_journal();
    r.revert(&journal);
    assert_eq!(r.all(i, a, false).unwrap(), &[b, c]);
    assert_eq!(r.all(i, b, true).unwrap(), &[a, c]);
}
