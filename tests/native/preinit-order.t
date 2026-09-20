// preinit runs before the first command, in an order settled while
// compiling rather than derived at every startup. The spelling is adv3Lite's so
// that a tutorial's PreinitObject pastes and works; the mechanism is not.
//
// Self-authored: the reference sorts at run time, so there is nothing to
// compare a build-time ordering against.
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;

class PreinitObject: object execBeforeMe = [];
// adv3Lite subclasses PreinitObject freely, so a subclass instance is one too.
class LibraryPreinit: PreinitObject;

class Thing: object name = 'thing' tally = 0;
Thing template 'name';
+ property location;

hall: Thing 'hall';
+ lamp: Thing 'lamp';
+ rope: Thing 'rope';

// Declared last, runs first: it is what the other two depend on.
countPreinit: LibraryPreinit
    execute()
    {
        // Preinit is where derived state is built from declarations. The rows
        // exist already , so this only has to read them.
        foreach (local item in contains.all(hall))
            hall.tally = hall.tally + 1;
        "counted <<hall.tally>>\n";
        return nil;
    }
;

reportPreinit: PreinitObject
    execute() { "report sees <<hall.tally>>\n"; return nil; }
    execBeforeMe = [countPreinit]
;

lastPreinit: PreinitObject
    execute() { "last\n"; return nil; }
    execBeforeMe = [reportPreinit]
;

startup()
{
    "startup sees <<hall.tally>>\n";
    return nil;
}

turn(toks)
{
    return nil;
}
