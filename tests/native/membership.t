state: object count = 0;
class Problem: object;
mark(n) { state.count += 1; return n; }
fail() { throw new Problem; }
main() {
    local matched = mark(2) is in (mark(1), mark(2), fail());
    "Match: <<matched ? 1 : 0>>; calls: <<state.count>>.\n";
    state.count = 0;
    local absent = mark(7) not in (mark(1), mark(2), mark(3));
    "Absent: <<absent ? 1 : 0>>; calls: <<state.count>>.\n";
    local denied = mark(1) not in (1, fail());
    "Denied: <<denied ? 1 : 0>>.\n";
    "Types: <<'hello' is in ('no', 'hello') ? 1 : 0>>; <<state is in (nil, state) ? 1 : 0>>; <<&count is in (&count) ? 1 : 0>>.\n";
    local n = 2;
    "Precedence: <<n + 1 is in (3) && n not in (4) ? 1 : 0>>; <<n is in (2) == true ? 1 : 0>>.\n";
    state.count = 0;
    "Comma: <<n is in ((mark(0), 2), fail()) ? 1 : 0>>; calls: <<state.count>>.\n";
    try { local found = n is in (1, fail(), 2); }
    catch (Problem exc) { "Reached exception.\n"; }
    "Nested: <<n is in (1 is in (2, 3), 2) ? 1 : 0>>.\n";
    return nil;
}
