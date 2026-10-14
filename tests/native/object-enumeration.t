// Original enumeration fixture: Turn expiry, despawn and process restore.
enum token tokWord;
class Item: object value = 0;
one: Item value = 1;
unrelated: object;
two: Item value = 2;
three: Item value = 3;

startup() { return nil; }
act(verb, args)
{
    if (verb == 1)
    {
        local temporary = new Item();
        temporary.value = 4;
    }
    if (verb == 2) despawn(two);
    if (verb == 3)
    {
        for (local i = 0; i < 1000; i = i + 1)
        {
            local temporary = new Item();
            despawn(temporary);
        }
    }
    local count = 0;
    local total = 0;
    for (local item = firstObj(Item); item != nil; item = nextObj(item, Item))
    {
        count = count + 1;
        total = total + item.value;
    }
    "[enumeration <<count>> <<total>>]\n";
    return nil;
}
