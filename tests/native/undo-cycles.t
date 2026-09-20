// undo replays completed cycles backwards, restoring relation rows
// and property values together. Self-authored: this has no TADS counterpart.
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = 'thing' score = 0;
room: Thing name = 'room';
box: Thing name = 'box';
coin: Thing name = 'coin';
startup()
{
    contains.set(room, box);
    contains.set(room, coin);
    return nil;
}
where()
{
    local at = location.get(coin);
    return at == nil ? 'nowhere' : at.name;
}
turn(toks)
{
    if (toks.length() > 0 && toks[1][1] == 'undo')
    {
        if (undo())
            "undone. coin in <<where()>>, score <<room.score>>\n";
        else
            "nothing to undo.\n";
        return nil;
    }
    contains.set(box, coin);
    room.score = room.score + 1;
    "moved. coin in <<where()>>, score <<room.score>>\n";
    return nil;
}
