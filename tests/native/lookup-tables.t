state: object calls = 0 order = 0;
key(n) { state.calls += 1; return n; }
trace(n) { state.order = state.order * 10 + n; return n; }
owned make() {
    local owned table = [key(1) -> 10, key(2) -> 20, 1 -> 11, 'word' -> 30, * -> 99];
    return move table;
}
main() {
    local owned table = make();
    "Entries: <<table.getEntryCount()>>; calls: <<state.calls>>.\n";
    "Values: <<table[1]>>; <<table[2]>>; <<table['word']>>; <<table[7]>>.\n";
    "Present: <<table.isKeyPresent(1) ? 1 : 0>>; <<table.isKeyPresent(7) ? 1 : 0>>.\n";
    table[1] = 12;
    table['new'] = 40;
    "Updated: <<table[1]>>; <<table['new']>>.\n";
    "Setter: <<table.setDefaultValue(77) == table ? 1 : 0>>.\n";
    "Default: <<table.getDefaultValue()>>; <<table[7]>>.\n";
    local owned ordered = [trace(1) -> trace(2), trace(3) -> trace(4)];
    "Order: <<state.order>>; value: <<ordered[1]>>.\n";
    local total = 0;
    for (local i = 0; i < 100; i += 1) {
        local owned item = [i -> i + 1];
        total += item[i];
    }
    "Total: <<total>>.\n";
    local owned empty = new LookupTable();
    "Empty: <<empty.getEntryCount()>>; <<empty[0] == nil ? 1 : 0>>.\n";
    return nil;
}
