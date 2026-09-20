state: object trace = 0;
mark(value) { state.trace = state.trace * 10 + value; return value; }
main() {
    local sum = 0;
    for (local i in 1..4) sum += i;
    "Ascending: <<sum>>.\n";
    sum = 0;
    for (local i in 5..1 step -2) sum = sum * 10 + i;
    "Descending: <<sum>>.\n";
    local upper = 4, delta = 1, runs = 0;
    for (local i in 1..upper step delta) { upper = 0; delta = 9; ++runs; }
    "Saved: <<runs>>.\n";
    sum = 0;
    for (local i in mark(1)..mark(3) step mark(2)) sum += i;
    "Evaluation: <<state.trace>>/<<sum>>.\n";
    local pick = 10, updated = 0;
    for (local i in 1..4; ; pick -= i, updated = updated * 10 + i) {
        if (i == 2) continue;
        if (i == 4) break;
    }
    "Hybrid: <<pick>>/<<updated>>.\n";
    sum = 0;
    for (local i in 1..3, local j in 9..5 step -2) sum += i * j;
    "Multiple: <<sum>>.\n";
    local i = 0;
    for (i in 2..4) { }
    "Existing: <<i>>.\n";
    local conditions = 0;
    for (local j in 3..1; ++conditions > 0; ) { }
    "Empty: <<conditions>>.\n";
    runs = 0;
    for (local j in 1..3 step 0) { if (++runs == 3) break; }
    "Zero: <<runs>>.\n";
    runs = 0;
    for (local j in 1..100) {
        local owned v = new Vector(1);
        try { v.append(j); ++runs; continue; }
        finally { sum = v.length(); }
    }
    "Cleanup: <<runs>>/<<sum>>.\n";
    return nil;
}
