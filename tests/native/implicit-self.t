base: object
    count = 2
    read() { return count; }
    bump() { count++; return count; }
    adjust() { count += bump(); return count; }
    overwrite() { count = bump(); }
    announce() { desc; }
    desc = "Base description.\n"
    parameter(count) { return count; }
    localValue() { local count = 17; return count; }
;
child: base
    count = 10
    desc = "Child description.\n"
;
main() {
    if (child.read() != 10) return 1;
    if (child.adjust() != 21) return 2;
    child.overwrite();
    if (child.count != 22 || base.count != 2) return 3;
    if (child.parameter(8) != 8 || child.localValue() != 17) return 4;
    child.announce();
    "PASS\n";
    return nil;
}
