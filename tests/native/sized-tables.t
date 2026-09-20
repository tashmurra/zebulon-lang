state: object order = 0 table = nil;
mark(n) { state.order = state.order * 10 + n; return n; }
ensureTable() {
    if (state.table == nil) {
        local owned table = new LookupTable(8, 8);
        table.moveToField(state, &table);
    }
    return nil;
}
main() {
    local owned defaults = new LookupTable();
    local owned sized = new LookupTable(mark(3), mark(2));
    "Buckets: <<defaults.getBucketCount()>>/<<sized.getBucketCount()>>; order: <<state.order>>.\n";
    for (local i in 1..10) sized[i] = i * 2;
    sized['word'] = 31;
    "Growth: <<sized.getEntryCount()>>/<<sized.getBucketCount()>>/<<sized[10]>>/<<sized['word']>>.\n";
    ensureTable();
    state.table['name'] = 7;
    ensureTable();
    "Lazy: <<state.table.getBucketCount()>>/<<state.table['name']>>.\n";
    state.table = nil;
    for (local i in 1..100) {
        local owned table = new LookupTable(8, 8);
        table[1] = i;
    }
    "Released.\n";
    return nil;
}
