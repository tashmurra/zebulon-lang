class Counter: object
    count = 1
    next() { return ++count; }
;
class Fancy: Counter
    next() { return inherited() + 10; }
;
first: Fancy;
second: Fancy;
class ReversedQuery: object
    isClass() { return !inherited(); }
;
reversed: ReversedQuery;
main() {
    if (Counter.isClass() != true || Fancy.isClass != true) return 1;
    if (first.isClass() != nil || second.isClass != nil) return 2;
    if (first.next() != 12 || first.count != 2 || second.count != 1) return 3;
    if (Counter.count != 1 || Fancy.count != 1) return 4;
    if (ReversedQuery.isClass() != nil || reversed.isClass() != true) return 5;
    "PASS\n";
    return nil;
}
