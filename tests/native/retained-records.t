action: object records = nil results = nil;
class Record: object
    construct(n) { value = n; }
    value = 0
    transfer(source, destination) {
        source.moveOwnedTo(self, destination);
        value += 1;
        return value;
    }
;
install(n) {
    local owned next = new OwnedCollection();
    local owned values = new Vector(2);
    if (action.records != nil) {
        local retained = action.results[1];
        retained.transfer(action.records, next);
        values.append(retained);
    } else {
        local owned first = new Record(7);
        values.append(first);
        first.moveToCollection(next);
    }
    local owned fresh = new Record(n);
    values.append(fresh);
    fresh.moveToCollection(next);
    local owned result = values.toList();
    next.moveToField(action, &records);
    result.moveToField(action, &results);
    return nil;
}
main() {
    install(8);
    local retained = action.results[1];
    "Initial: <<retained.value>>/<<action.results[2].value>>.\n";
    for (local i in 1..100) install(i);
    "Retained: <<retained == action.results[1] ? 1 : 0>>/<<retained.value>>; fresh: <<action.results[2].value>>; owners: <<action.records.ownedLength()>>.\n";
    action.results = nil;
    action.records = nil;
    "Released.\n";
    return nil;
}
