// a turn that dies part-way puts its changes back and the session
// carries on. Self-authored: this has no TADS counterpart.
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = 'thing';
room: Thing name = 'room';
box: Thing name = 'box';
coin: Thing name = 'coin';
startup()
{
    contains.set(room, box);
    contains.set(room, coin);
    return nil;
}
turn(toks)
{
    local here = location.get(coin);
    "start: coin in <<here == nil ? 'nowhere' : here.name>>\n";
    contains.set(box, coin);
    local moved = location.get(coin);
    "moved: coin in <<moved == nil ? 'nowhere' : moved.name>>\n";
    if (toks.length() > 0 && toks[1][1] == 'fail')
        return toks[99];
    return nil;
}
