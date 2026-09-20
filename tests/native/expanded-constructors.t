class Pair: object construct(a, b) { first = a; second = b; } first = nil second = nil;
class Optional: object construct(a?, b?) { first = a; second = b; } first = nil second = nil;
class Many: object construct(first, [rest]) { total = first; foreach(local n in rest) total += n; } total = 0;
class Hybrid: object construct(first?, [rest]) { total = (first == nil ? 0 : first); foreach(local n in rest) total += n; } total = 0;
class Empty: object;
class Problem: object;
class Failing: object construct([rest]) { if(rest.length() > 0) throw new Problem(); };
arguments: object pair = [7, 8] optional = [9] many = [1, 2, 3, 4] empty = [];
main() {
    local pairArgs = arguments.pair;
    local owned pair = new Pair(pairArgs...);
    local optionalArgs = arguments.optional;
    local owned opt = new Optional(optionalArgs...);
    local manyArgs = arguments.many;
    local owned many = new Many(manyArgs...);
    local emptyArgs = arguments.empty;
    local owned empty = new Empty(emptyArgs...);
    local owned hybrid = new Hybrid(manyArgs...);
    local owned omitted = new Hybrid(emptyArgs...);
    "Combined: <<hybrid.total>>/<<omitted.total>>.\n";
    "Arguments: <<pair.first>>/<<pair.second>>/<<opt.first>>/<<opt.second == nil ? 1 : 0>>/<<many.total>>.\n";
    local caught = 0;
    for(local i in 1..100) {
        try { local owned failure = new Failing(pairArgs...); }
        catch(Problem e) { caught += 1; }
    }
    "Failures: <<caught>>.\n";
    return nil;
}
