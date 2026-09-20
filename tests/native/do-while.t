main() {
    local once = 0;
    do { ++once; } while (nil);
    "Once: <<once>>.\n";
    local n = 0, checks = 0, cleanups = 0;
    do {
        try {
            ++n;
            if (n < 3) continue;
            break;
        } finally { ++cleanups; }
    } while (++checks < 10);
    "Control: <<n>>/<<checks>>/<<cleanups>>.\n";
    local total = 0, outer = 0;
    again: do {
        ++outer;
        do {
            ++total;
            continue again;
        } while (true);
    } while (outer < 3);
    "Nested: <<outer>>/<<total>>.\n";
    local scopes = 0;
    do {
        local owned v = new Vector(1);
        v.append(scopes);
        ++scopes;
        continue;
    } while (scopes < 100);
    "Scopes: <<scopes>>.\n";
    return nil;
}
