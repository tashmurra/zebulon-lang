// A single-quoted interpolated string is a value that can be returned,
// held and compared; a double-quoted expression emits output.

class Pencil: object
    isSharp = nil
    /* Bare property names, so `self` has to resolve inside the string. */
    desc = 'a pencil, <<isSharp ? 'sharp' : 'blunt'>>.'
;
blunt: Pencil;
sharp: Pencil isSharp = true;

counter: object n = 7 word = 'seven';

startup()
{
    "initializer: <<blunt.desc>>\n";
    "another:     <<sharp.desc>>\n";
    blunt.isSharp = true;
    "after:       <<blunt.desc>>\n";
    // It is text, so it compares and measures like any other.
    local made = 'count <<counter.n>>.';
    "number:      <<made>>\n";
    "equal:       <<made == 'count 7.' ? 'yes' : 'no'>>\n";
    "length:      <<made.length()>>\n";
    "text part:   <<'word <<counter.word>>!'>>\n";
    "nothing:     <<'<<counter.n>>'>>\n";
    return nil;
}

turn(toks)
{
    return nil;
}
