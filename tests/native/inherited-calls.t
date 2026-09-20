base: object
    count = 1
    calculate(n) { return n + count; }
    desc = "Base.\n"
    empty(n) { return inherited(n); }
;
left: base
    calculate(n) { return inherited(n) + 100; }
    desc { "Left.\n"; inherited; }
;
right: base
    calculate(n) { return inherited(n) + 10; }
    baseCount() { return inherited.count; }
    desc { "Right.\n"; inherited; }
;
child: left,right count = 5;
main() {
    if (child.calculate(2) != 117) return 1;
    if (left.calculate(2) != 103) return 2;
    if (child.baseCount() != 1) return 3;
    if (child.empty(8) != nil) return 4;
    child.desc;
    "PASS\n";
    return nil;
}
