class Problem: object
    code = 0
    construct(n) { code = n; }
;
class Broken: Problem
    construct(n) { throw new Problem(n + 1); }
;
raise(n) { throw new Problem(n); }
relay(exc) { throw exc; }
suppress() {
    try { raise(7); }
    finally { return 8; }
}
main() {
    try { raise(1); }
    catch (Problem exc) { "Caught: <<exc.code>>.\n"; }
    try {
        try { raise(2); }
        catch (Problem exc) { relay(exc); }
    }
    catch (Problem exc) { "Rethrown: <<exc.code>>.\n"; }
    try {
        try { raise(3); }
        finally {
            try { raise(4); }
            catch (Problem inner) { "Inside: <<inner.code>>.\n"; }
        }
    }
    catch (Problem exc) { "Pending: <<exc.code>>.\n"; }
    try { try { raise(5); } finally { raise(6); } }
    catch (Problem exc) { "Replacement: <<exc.code>>.\n"; }
    "Suppressed: <<suppress()>>.\n";
    try { throw new Broken(10); }
    catch (Problem exc) { "Constructor: <<exc.code>>.\n"; }
    local total = 0;
    for (local i = 0; i < 100; i += 1) {
        try { try { raise(-1); } finally { raise(i); } }
        catch (Problem exc) { total += exc.code; }
        suppress();
    }
    "Repeated: <<total>>.\n";
    return nil;
}
