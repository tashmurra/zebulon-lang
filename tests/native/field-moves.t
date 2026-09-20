holder: object table = nil item = nil;
class Item: object construct(n) { value = n; } value = 0;
install(n) {
    local owned table = new LookupTable();
    table['number'] = n;
    table.moveToField(holder, &table);
    local owned item = new Item(n + 1);
    item.moveToField(holder, &item);
    return nil;
}
main() {
    install(7);
    "Retained: <<holder.table['number']>>/<<holder.item.value>>.\n";
    for (local i in 1..100) install(i);
    "Replacement: <<holder.table['number']>>/<<holder.item.value>>.\n";
    holder.table = nil;
    holder.item = nil;
    "Released.\n";
    return nil;
}
