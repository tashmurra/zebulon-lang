owned make() {
    local owned buffer = new StringBuffer();
    buffer.append('hé漢');
    return move buffer;
}
main() {
    local owned buffer = make();
    "Length: <<buffer.length()>>.\n";
    "Append self: <<buffer.append(buffer) == buffer ? 1 : 0>>; length: <<buffer.length()>>.\n";
    local owned other = new StringBuffer();
    other.append(buffer);
    other.append('!');
    "Other: <<other.length()>>; original: <<buffer.length()>>.\n";
    local total = 0;
    for (local i = 0; i < 100; i += 1) {
        local owned temporary = new StringBuffer();
        temporary.append('abc');
        total += temporary.length();
    }
    "Repeated: <<total>>.\n";
    return nil;
}
