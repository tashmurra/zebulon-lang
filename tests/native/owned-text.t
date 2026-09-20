transient status: object value = 7;
owned make() {
    local owned buffer = new StringBuffer();
    try { buffer.append('hello'); }
    finally { "Before snapshot.\n"; }
    local owned text = toString(buffer);
    buffer.append('!');
    "Buffer length: <<buffer.length()>>; snapshot: <<text>>.\n";
    return move text;
}
main() {
    local owned text = make();
    "Returned: <<text>>; length: <<text.length()>>.\n";
    local total = 0;
    for (local i = 0; i < 100; i += 1) {
        local owned buffer = new StringBuffer();
        buffer.append('abc');
        local owned copy = toString(buffer);
        total += copy.length();
    }
    "Repeated: <<total>>; transient value: <<status.value>>.\n";
    return nil;
}
