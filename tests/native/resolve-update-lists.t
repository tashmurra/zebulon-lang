action: object direct = nil indirect = nil;
owned singleton(n) {
    local owned builder = new Vector(1);
    builder.append(n);
    local owned result = builder.toList();
    return move result;
}
owned copyList(values) {
    local owned builder = new Vector(1);
    foreach (local value in values) builder.append(value);
    local owned result = builder.toList();
    return move result;
}
update(direct, indirect) {
    local owned keeper = new OwnedCollection();
    action.moveFieldOwnerTo(&direct, keeper, &direct);
    action.moveFieldOwnerTo(&indirect, keeper, &indirect);
    local owned nextDirect = copyList(direct);
    nextDirect.moveToField(action, &direct);
    local owned nextIndirect = copyList(indirect);
    nextIndirect.moveToField(action, &indirect);
    return nil;
}
main() {
    local owned direct = singleton(7);
    local owned indirect = singleton(8);
    direct.moveToField(action, &direct);
    indirect.moveToField(action, &indirect);
    update(action.indirect, action.direct);
    "Swapped: <<action.direct[1]>>/<<action.indirect[1]>>.\n";
    for (local i in 1..100) update(action.indirect, action.direct);
    "Repeated: <<action.direct[1]>>/<<action.indirect[1]>>.\n";
    action.direct = nil;
    action.indirect = nil;
    "Released.\n";
    return nil;
}
