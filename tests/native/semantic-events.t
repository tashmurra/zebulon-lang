// a turn hands over semantic events beside its text. An event is a
// message id and the entities it concerns, and it crosses whether or not
// anything says a sentence. Self-authored; no TADS counterpart — a TADS game is
// the process and its output is characters, so it has nowhere to send an event.
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = 'thing';
vault: Thing name = 'vault';
ingot: Thing name = 'ingot';
chest: Thing name = 'chest';
startup()
{
    contains.set(vault, ingot);
    contains.set(vault, chest);
    // An id alone: the room was entered, and a terminal has nothing to print.
    event('room.enter');
    eventSubject(vault);
    return nil;
}
turn(toks)
{
    // An id with one entity, and one with two: taking a thing, then putting it
    // somewhere. The text is what a terminal shows; the events are what an
    // engine binds a sound and an animation to.
    event('take.ok');
    eventSubject(ingot);
    "Taken.\n";
    event('putin.ok');
    eventSubject(ingot);
    eventSubject(chest);
    "You put the ingot in the chest.\n";
    // And one with nothing at all, which is the shape a message with no objects
    // takes: the record still has to be well formed.
    event('turn.failed');
    return nil;
}
recover(code)
{
    "failed with <<code>>\n";
    return nil;
}
