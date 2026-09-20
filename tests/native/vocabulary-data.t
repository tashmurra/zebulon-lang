// a vocabulary relation is data. `all` answers the words that name an
// entity, `contains` asks whether one does, and `set`/`unset` change what a thing
// answers to at run time — which the grammar then sees, because the relation and
// the grammar read the same dictionary.
//
// Self-authored: TADS has no relations, and its dictionary has no relation shape.
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
dictionary cmdDict;
relation names(entity: Entity, word: Text) many_to_many;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = 'thing';
room: Thing name = 'room';
coin: Thing name = 'coin' names = 'coin' location = room;
class Prod: object tag = nil item_ = nil;
grammar command(take): 'take' names->item_ : Prod tag = 'take';
startup() { "ready\n> "; return nil; }
report()
{
    local words = names.all(coin);
    "coin answers to <<words.length()>>:";
    foreach (local w in words)
        " <<w>>";
    "\n";
    return nil;
}
turn(toks)
{
    local m = command.parseTokens(toks, cmdDict);
    if (m.length() > 0 && m[1].tag == 'take')
    {
        "matched take, <<m[1].item_.length()>> candidate(s)\n";
        return nil;
    }
    report();
    "contains 'coin' = <<names.contains(coin, 'coin') ? 'yes' : 'no'>>\n";
    "contains 'shilling' = <<names.contains(coin, 'shilling') ? 'yes' : 'no'>>\n";
    names.set(coin, 'shilling');
    "after set: ";
    report();
    "contains 'shilling' = <<names.contains(coin, 'shilling') ? 'yes' : 'no'>>\n";
    names.unset(coin, 'coin');
    "after unset of coin: ";
    report();
    return nil;
}
