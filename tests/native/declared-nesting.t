// Containment prefixes declare relation rows for nested entities.
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = 'thing';
Thing template 'name';

// The reverse name reads right to left, so `+ desk` under `study` puts the desk
// in the study rather than the study in the desk.
+ property location;

study: Thing 'study';
+ desk: Thing 'desk';
++ drawer: Thing 'drawer';
+++ tin: Thing 'tin';
++++ coin: Thing 'coin';
+ rug: Thing 'rug';

// Four levels down, then back out to the study: a prefix that dropped a level
// has to resume against the right container, not the last one declared.
show(container)
{
    "<<container.name>> holds";
    local held = [for item in contains.all(container) : item.name];
    if (held.length() == 0)
        " nothing";
    foreach (local name in held)
        " <<name>>";
    "\n";
    return nil;
}

startup()
{
    show(study);
    show(desk);
    show(drawer);
    show(tin);
    show(coin);
    show(rug);
    // The row reads from either side, so the reverse name answers the parent.
    "coin's location is <<location.get(coin).name>>\n";
    "outermost above coin is <<contains.outermost(coin).name>>\n";
    return nil;
}

turn(toks)
{
    return nil;
}
