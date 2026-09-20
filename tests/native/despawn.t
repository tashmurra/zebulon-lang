// despawn removes an entity and every relation row naming it.
// Self-authored: this has no TADS counterpart.
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = 'thing';
room: Thing name = 'room';
coin: Thing name = 'coin';
lamp: Thing name = 'lamp';
startup()
{
    contains.set(room, coin);
    contains.set(room, lamp);
    return nil;
}
turn(toks)
{
    "before: <<contains.all(room).length()>> here, coin in <<location.get(coin).name>>\n";
    despawn(coin);
    local rest = contains.all(room);
    "after: <<rest.length()>> here, first is <<rest[1].name>>\n";
    return nil;
}
