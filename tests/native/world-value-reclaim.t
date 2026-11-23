// Long sessions that keep replacing world text and lists. A value stored into
// world state is promoted; once nothing refers to it, it must not count
// against the session's limits for ever.
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
class Profile: object clock = 0 label = 'unset' recent = [];
p1: Profile; p2: Profile; p3: Profile; p4: Profile;
class Result: object value = 0;
startup() { "Ready.\n"; return nil; }
step(p, i) {
    local r = new Result();
    r.value = i;
    p.clock = p.clock + 1;
    p.label = 'clock <<p.clock>>';
    p.recent = [r.value, p.label];
    return nil;
}
skip(n) {
    for (local i = 1; i <= n; ++i) { step(p1, i); step(p2, i); step(p3, i); step(p4, i); }
    return nil;
}
act(verb, subjects) {
    if (verb == 1) skip(810);
    if (verb == 2) undo();
    if (verb == 3) skip(25);
    local last = p3.recent.length() == 2 ? p3.recent[2] : 'none';
    "[clock <<p1.clock>> <<p4.label>> <<p2.recent.length()>> <<last>>]\n"; return nil;
}
turn(tokens) { return nil; }
