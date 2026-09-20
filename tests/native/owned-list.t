holder: object values = nil text = nil;
owned make(n) {
    local owned vector = new Vector(2);
    vector.append(n);
    vector.append(n + 1);
    local owned result = vector.toList();
    vector[1] = 999;
    return move result;
}
install(n) {
    local owned values = make(n);
    values.moveToField(holder, &values);
    local owned buffer = new StringBuffer();
    buffer.append('stored');
    local owned text = toString(buffer);
    text.moveToField(holder, &text);
    return nil;
}
main() {
    install(7);
    "Snapshot: <<holder.values[1]>>/<<holder.values[2]>>/<<holder.values.length()>>.\n";
    local total = 0;
    for (local i in 1..100) {
        install(i);
        total += holder.values[1];
    }
    "Replacement: <<holder.values[1]>>/<<holder.values[2]>>; total: <<total>>; text: <<holder.text>>.\n";
    holder.values = nil;
    holder.text = nil;
    "Released.\n";
    return nil;
}
