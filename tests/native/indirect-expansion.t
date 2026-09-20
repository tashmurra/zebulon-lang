// Independently authored fixture: expanding arguments into an indirect property.
property greet, tally, plain, pick;
holder: object
    greet(a, b) { return a * 10 + b; }
    tally([nums])
    {
        local total = 0;
        foreach (local n in nums)
            total += n;
        return total;
    }
    plain = 7
    pick(first, [rest]) { return first * 100 + rest.length(); }
;
main()
{
    local p = &greet;
    local pair = [3, 4];
    "method: <<holder.(p)(pair...)>>.\n";
    local t = &tally;
    local three = [1, 2, 3];
    "rest: <<holder.(t)(three...)>>.\n";
    local none = [];
    "empty: <<holder.(t)(none...)>>.\n";
    local k = &pick;
    "mixed: <<holder.(k)(5, pair...)>>.\n";
    local q = &plain;
    "value: <<holder.(q)>>.\n";
    return nil;
}
