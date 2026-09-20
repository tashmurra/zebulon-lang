// a declared relation row moves with the name when an object is
// modified. `modify` renames the target to a private base class and gives its
// name to the modification; a row left on the base named something that was no
// longer the object, and was a class besides.
//
// The symptom was that the table contradicted itself: `all` found the row by
// scanning while `get` and `contains` looked under the name and found nothing.
//
// Self-authored: TADS has no relations, so there is no reference for how one
// should survive a modification.
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
+ property location;
class Thing: object name = nil;
Thing template 'name';

room1: Thing 'room';
+ alice: Thing 'alice';
+ bob:   Thing 'bob';

cellar: Thing 'cellar';
crate: Thing 'crate' location(room1);

modify alice
    greet() { return nil; }
;

/* A modification may state its own row, which is the later word. */
modify crate location(cellar);

startup()
{
    "room holds:           <<contains.all(room1).length()>>\n";
    "alice's location:     <<location.get(alice) == nil ? 'nil' : location.get(alice).name>>\n";
    "bob's location:       <<location.get(bob) == nil ? 'nil' : location.get(bob).name>>\n";
    "contains(room,alice): <<contains.contains(room1, alice) ? 'yes' : 'no'>>\n";
    "contains(room,bob):   <<contains.contains(room1, bob) ? 'yes' : 'no'>>\n";
    "alice ancestors:      <<contains.ancestors(alice).length()>>\n";
    "crate's location:     <<location.get(crate).name>>\n";
    "cellar holds:         <<contains.all(cellar).length()>>\n";
    return nil;
}

turn(toks)
{
    return nil;
}
