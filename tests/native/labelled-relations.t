// a relation with a third column is a family of tables, one per
// label. Travel is the case it exists for. Self-authored.
enum token tokWord;
enum Direction: north, south, east, west;
relation exits(from: Entity, to: Entity, via: Direction) one_to_one;
class Room: object name = 'room';
study: Room name = 'study';
hall: Room name = 'hall';
cellar: Room name = 'cellar';
startup()
{
    exits.set(study, hall, north);
    exits.set(hall, study, south);
    exits.set(study, cellar, east);
    return nil;
}
go(from, way, label)
{
    local to = exits.get(from, way);
    "<<from.name>> <<label>> -> <<to == nil ? 'nowhere' : to.name>>\n";
    return nil;
}
turn(toks)
{
    go(study, north, 'north');
    go(study, east, 'east');
    go(study, south, 'south');
    go(hall, south, 'south');
    return nil;
}
