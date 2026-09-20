class Problem: object code = 6;
state: object total = 0 values = static [2, 3];
apply(fn, x) { return fn(x); }
factory() {
    return function(x) {
        local value = x * 2;
        if (value > 5) return value + 1;
        return value;
    };
}
main() {
    local items = nil;
    local fn = function(n) {
        local items = 0;
        for (local i = 0; i < n; i += 1) items += i;
        state.total = items;
        return items;
    };
    "Local: <<apply(fn, 5)>>/<<state.total>>.\n";
    "Returned: <<factory()(4)>>.\n";
    local walk = function(values) {
        foreach (local value in values) "Item: <<value>>.\n";
    };
    walk(state.values);
    local raising = function(x) {
        try { throw new Problem; }
        catch (Problem exc) { return x + exc.code; }
        finally { "Callback cleanup.\n"; }
    };
    "Caught result: <<raising(4)>>.\n";
    return nil;
}
