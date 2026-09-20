class Problem: object
    total = 0
    construct(start, [args]) {
        total = start;
        foreach (local value in args) total += value;
    }
;
base: object
    sum(first, [args]) {
        local total = first;
        foreach (local value in args) total += value;
        return total;
    }
    ignore(first, ...) { return first; }
;
child: base
    sum(first, [args]) { return inherited(first, 10, 20) + args.length(); }
;
main() {
    "Methods: <<base.sum(1)>>/<<base.sum(1,2,3)>>/<<base.ignore(4,5,6)>>.\n";
    "Inherited: <<child.sum(2,3,4)>>.\n";
    try { throw new Problem(5,6,7); }
    catch (Problem exc) { "Constructor: <<exc.total>>.\n"; }
    return nil;
}
