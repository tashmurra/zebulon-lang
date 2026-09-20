// indexOf asks where something is in a sequence, and that is the same
// question of a list and of a vector. It lowered to a vector operation
// unconditionally, so a list literal failed the tag guard and the session died
// -- and a list is what a comprehension answers, so the value a query produces
// was the one that could not be asked.
//
// Ordinary TADS behaviour Zebulon does not redefine, so frobtads is an oracle.
enum token tokWord;
a: object n = 1;
b: object n = 2;
c: object n = 3;
main()
{
    local nums = [10, 20, 30];
    "first: <<nums.indexOf(10)>>.\n";
    "middle: <<nums.indexOf(20)>>.\n";
    "last: <<nums.indexOf(30)>>.\n";
    "absent: <<nums.indexOf(99) == nil ? 'nil' : 'wrong'>>.\n";
    local objs = [a, b, c];
    "object: <<objs.indexOf(c)>>.\n";
    "text: <<['x', 'y'].indexOf('y')>>.\n";
    local empty = [];
    "empty: <<empty.indexOf(1) == nil ? 'nil' : 'wrong'>>.\n";
    // A vector answers the same way it always did. `new` needs the lifetimes
    // model, so the vector case is covered by the unit tests instead.
    return nil;
}
