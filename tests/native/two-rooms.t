// Independent world fixture: travel, scope, visibility and disambiguation.
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
dictionary cmdDict;
enum Direction: north, south;

// Three kinds of link, three declarations. Containment and travel are tables,
// and so is vocabulary: a word names entities, and a grammar slot declared
// against it binds what the word names.
relation contains(container: Entity, item: Entity) one_to_many reverse location;
relation exits(from: Entity, to: Entity, via: Direction) one_to_one;
relation names(entity: Entity, word: Text) many_to_many;

class Room: object name = 'room';
class Thing: object name = 'thing' container = nil open = true;
Room template 'name';
Thing template 'name';

// Where everything starts is part of what it is, so containment and travel are
// declared here rather than assembled by the first turn . A relation
// row is written `relation(partner)`, which cannot be mistaken for a field:
// `location = study` would still be an ordinary property. `location` is the
// reverse name of `contains`, so it reads the way it is written — the desk's
// location is the study — and a labelled relation names its label too.
study: Room 'study' exits(cellar, north);
cellar: Room 'cellar' exits(study, south);
desk: Thing 'desk' names = 'desk' location(study);
crate: Thing 'crate' names = 'crate' container = true open = nil location(study);
coin: Thing 'coin' names = 'coin' location(crate);
rope: Thing 'rope' names = 'rope' location(cellar);
brass: Thing 'brass lamp' names = 'lamp' location(study);
oil: Thing 'oil lamp' names = 'lamp' location(study);
player: Thing 'you' location(study);

class Prod: object tag = nil item_ = nil way_ = nil;
grammar command(look): 'look' : Prod tag = 'look';
grammar command(take): 'take' names->item_ : Prod tag = 'take';
grammar command(drop): 'drop' names->item_ : Prod tag = 'drop';
grammar command(open): 'open' names->item_ : Prod tag = 'open';
grammar command(north): 'north' : Prod tag = 'go' way_ = 'north';
grammar command(south): 'south' : Prod tag = 'go' way_ = 'south';
grammar command(burn): 'burn' names->item_ : Prod tag = 'burn';
grammar command(undoCmd): 'undo' : Prod tag = 'undo';

here()
{
    return location.get(player);
}

// Visible means reachable without opening anything: every container between
// the item and the room has to be open. One walk up the containment table.
visible(item)
{
    local room = here();
    foreach (local step in contains.ancestors(item))
    {
        if (step == room || step == player)
            return true;
        if (step.container && !step.open)
            return nil;
    }
    return nil;
}

// Scope is a relation query: everything the room holds, transitively, that is
// not hidden, plus whatever the player is carrying.
inScope(item)
{
    return item != player && visible(item);
}

describeRoom()
{
    "You are in the <<here().name>>. ";
    foreach (local item in contains.all(here()))
    {
        if (item == player)
            continue;
        "There is a <<item.name>> here";
        if (item.container)
        {
            if (!item.open)
                ", closed";
            else
            {
                local inside = [for held in contains.all(item) : held.name];
                if (inside.length() > 0)
                    ", with a <<inside[1]>> in it";
            }
        }
        ". ";
    }
    local carried = [for item in contains.all(player) : item.name];
    if (carried.length() > 0)
        "You are carrying <<carried.length()>> thing(s). ";
    "\n";
    return nil;
}

// A word can name more than one thing. Narrowing it is a question asked inside
// the same cycle, which is what the suspension protocol is for .
choose(candidates)
{
    local reachable = [for c in candidates : c if inScope(c)];
    if (reachable.length() == 0)
        return nil;
    if (reachable.length() == 1)
        return reachable[1];
    "Which do you mean";
    foreach (local option in reachable)
        ", the <<option.name>>";
    "? ";
    local answer = inputLine();
    if (answer == nil)
        return nil;
    foreach (local option in reachable)
    {
        if (option.name == answer)
            return option;
    }
    "That was not one of them.\n";
    return nil;
}

takeItem(item)
{
    if (location.get(item) == player)
        "You already have the <<item.name>>.\n";
    else
    {
        contains.set(player, item);
        "Taken: <<item.name>>.\n";
    }
    return nil;
}

dropItem(item)
{
    if (location.get(item) != player)
        "You are not carrying the <<item.name>>.\n";
    else
    {
        contains.set(here(), item);
        "Dropped: <<item.name>>.\n";
    }
    return nil;
}

openItem(item)
{
    if (!item.container)
        "The <<item.name>> is not a container.\n";
    else
    {
        item.open = true;
        "Opened: <<item.name>>.\n";
    }
    return nil;
}

travel(way)
{
    local to = exits.get(here(), way == 'north' ? north : south);
    if (to == nil)
        "You cannot go that way.\n";
    else
    {
        contains.set(to, player);
        describeRoom();
    }
    return nil;
}

startup()
{
    describeRoom();
    "> ";
    return nil;
}

// One command. The host opened the cycle and tokenized the line; it closes the
// cycle when this returns, and puts every change back if this fails.
turn(toks)
{
    local m = command.parseTokens(toks, cmdDict);
    if (m.length() == 0)
        "I don't understand that.\n";
    else if (m[1].tag == 'look')
        describeRoom();
    else if (m[1].tag == 'go')
        travel(m[1].way_);
    else if (m[1].tag == 'undo')
    {
        if (undo())
            "Undone.\n";
        else
            "There is nothing to undo.\n";
    }
    else
    {
        local item = choose(m[1].item_);
        if (item == nil)
            "You see no such thing.\n";
        else if (m[1].tag == 'take')
            takeItem(item);
        else if (m[1].tag == 'drop')
            dropItem(item);
        else if (m[1].tag == 'open')
            openItem(item);
        else
        {
            // Burning a thing removes it from the world outright.
            "The <<item.name>> burns away.\n";
            despawn(item);
        }
    }
    "> ";
    return nil;
}

// Called after a failed turn, once its changes have been put back.
recover(code)
{
    "Something went wrong (<<code>>); nothing changed.\n> ";
    return nil;
}
