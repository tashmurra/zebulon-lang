counter: object
    count = 1
    add(a, b) { self.count += a * 10 + b; return self.count; }
    ring() { "Click.\n"; }
    doubled { return self.count * 2; }
    desc = "A brass counter.\n"
;
other: object
    add(a, b) { return a - b; }
;
trace: object value = 0;
arg(n) { trace.value = trace.value * 10 + n; return n; }
receiver() { trace.value = trace.value * 10 + 3; return counter; }
main() {
    if (receiver().add(arg(1), arg(2)) != 13) return 1;
    if (trace.value != 213) return 2;
    if (counter.doubled != 26) return 3;
    if (other.add(9, 4) != 5) return 4;
    if (counter.ring() != nil) return 5;
    if (counter.count() != 13) return 6;
    counter.desc;
    "PASS\n";
    return nil;
}
