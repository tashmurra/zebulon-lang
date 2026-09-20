action: object records = nil;
existing: object value = 5;
class Record: object construct(n) { value = n; } value = 0;
resolve(n, recipients) {
    if (n == 0) return existing;
    local owned fresh = new Record(n);
    local result = fresh;
    fresh.moveToCollection(recipients);
    return result;
}
install(n) {
    local owned recipients = new OwnedCollection();
    local old = resolve(0, recipients);
    local first = resolve(n, recipients);
    local second = resolve(n + 1, recipients);
    recipients.moveToField(action, &records);
    return old.value + first.value + second.value;
}
main() {
    "Mixed: <<install(7)>>.\n";
    "Retained: <<action.records.ownedLength()>>/<<action.records.ownedAt(1).value>>/<<action.records.ownedAt(2).value>>.\n";
    local total = 0;
    for (local i in 1..100) total += install(i);
    "Repeated: <<total>>/<<action.records.ownedAt(2).value>>.\n";
    action.records = nil;
    "Borrowed: <<existing.value>>.\n";
    return nil;
}
