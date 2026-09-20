class Item: object value = 0 construct(n) { value = n; };
class Problem: object;
owned make(n) {
    local owned item = new Item(n);
    item.value += 1;
    return move item;
}
owned relay(n) {
    local owned item = make(n);
    return move item;
}
owned guarded(n) {
    try {
        if (n == 0) throw new Problem;
        local owned item = make(n);
        return move item;
    }
    catch (Problem exc) {
        local owned fallback = new Item(9);
        return move fallback;
    }
}
factory: object
    owned makeItem(n?) {
        local owned item = new Item(n == nil ? 12 : n);
        return move item;
    }
;
main() {
    local total = 0;
    for (local i = 0; i < 100; i += 1) {
        local owned item = relay(i);
        total += item.value;
    }
    "Factory total: <<total>>.\n";
    local owned first = guarded(0);
    local owned second = guarded(4);
    "Caught: <<first.value>>; normal: <<second.value>>.\n";
    local owned methodItem = factory.makeItem();
    local owned indirectItem = factory.(&makeItem)(14);
    "Methods: <<methodItem.value>>; <<indirectItem.value>>.\n";
    return nil;
}
