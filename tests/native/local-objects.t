class Item: object value = 0 construct(n) { value = n; };
class Problem: object;
class Broken: Item construct(n) { throw new Problem; };
returning() {
    local owned item = new Item(4);
    return item.value;
}
main() {
    local total = 0;
    for (local i = 0; i < 100; i += 1) {
        local owned item = new Item(i);
        total += item.value;
    }
    "Total: <<total>>.\n";
    {
        local owned item = new Item(7);
        "Local: <<item.value>>.\n";
    }
    try {
        local owned item = new Item(9);
        "Before throw: <<item.value>>.\n";
        throw new Problem;
    }
    catch (Problem exc) { "Caught.\n"; }
    try { local owned item = new Broken(1); }
    catch (Problem exc) { "Constructor failed.\n"; }
    "Returned value: <<returning()>>.\n";
    return nil;
}
