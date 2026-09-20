// a many_to_many relation may state several rows in one declaration.
//
// A row rides as the property's value, so the one-definition-per-property rule
// refused a second. For one_to_one and one_to_many that is right, since a
// second row displaces the first. For many_to_many it was the only cardinality
// whose declaration could not say what the relation permits — and a door is one
// entity in two rooms.
//
// Self-authored: TADS has no relations.
enum token tokWord;
relation connects(door: Entity, room: Entity) many_to_many reverse doorsOf;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = nil;
Thing template 'name';

hall:  Thing 'hall';
drive: Thing 'drive';
yard:  Thing 'yard';

/* Two rooms, one door, one declaration. */
frontDoor: Thing 'front door' connects(hall) connects(drive);
/* And as many as it likes. */
junction: Thing 'junction' connects(hall) connects(drive) connects(yard);

startup()
{
    "door sides: <<connects.all(frontDoor).length()>>\n";
    "junction:   <<connects.all(junction).length()>>\n";
    // Declaration order is the order the rows answer in.
    "order:";
    foreach (local room in connects.all(junction))
        " <<room.name>>";
    "\n";
    // A reverse name reads the table the other way: which doors reach the hall.
    "hall's doors: <<doorsOf.all(hall).length()>>\n";
    "door reaches drive: <<connects.contains(frontDoor, drive) ? 'yes' : 'no'>>\n";
    "door reaches yard:  <<connects.contains(frontDoor, yard) ? 'yes' : 'no'>>\n";
    return nil;
}

turn(toks)
{
    return nil;
}
