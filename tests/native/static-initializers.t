counts: object calls = 0;
first: object value = static (later.value + 2);
later: object value = static calculate();
unread: object value = static touch();
refs: object target = static later text = static 'ready';
calculate() { counts.calls++; return 40; }
touch() { counts.calls++; return first.value; }
main() {
    if (counts.calls != 2) return 1;
    if (first.value != 42 || later.value != 40 || unread.value != 42) return 2;
    later.value = 9;
    if (first.value != 42 || counts.calls != 2) return 3;
    if (refs.target != later || refs.text != 'ready') return 4;
    "PASS\n";
    return nil;
}
