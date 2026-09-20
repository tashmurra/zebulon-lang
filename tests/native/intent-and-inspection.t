// a host may submit an action instead of a line, ask the game to wait
// while it acts, and answer a question only it knows. : it may also
// read stored state between commands. Self-authored; no TADS counterpart.
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = 'thing';
room: Thing name = 'room';
coin: Thing name = 'coin';
player: Thing name = 'you';
startup()
{
    contains.set(room, coin);
    contains.set(room, player);
    "room holds <<contains.all(room).length()>>\n";
    return nil;
}
// The parser front end's shape.
turn(toks)
{
    "line intent, <<toks.length()>> token(s)\n";
    return nil;
}
// The host chose the action itself; no parsing happened.
act(verb, subjects)
{
    "action <<verb>> on <<subjects.length()>> subject(s)";
    if (subjects.length() > 0)
        " first=<<subjects[1].name>>";
    "\n";
    // The game proposes and waits; the world advances on what comes back.
    if (hostAction(verb))
        "the host accepted\n";
    else
        "the host refused\n";
    "the host says <<engineQuery(7)>>\n";
    return nil;
}
recover(code)
{
    "failed with <<code>>\n";
    return nil;
}
