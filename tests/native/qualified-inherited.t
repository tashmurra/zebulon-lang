class Problem: object;
class Common: object value = 0;
class Left: Common
    construct(n) { left = n + value; }
    calc(n) { return value + n; }
    risk(n) { if(n == 0) throw new Problem(); return value + n; }
    constant = 17
    left = 0
;
class Right: Common
    construct([params]) { right = value; foreach(local n in params) right += n; }
    calc(n) { return value * n; }
    right = 0
;
class Derived: Left, Right
    value = 10
    construct([params]) { inherited Left(params[1]); inherited Right(params...); }
    calc(n) { return inherited Left(n) + inherited Right(n); }
    risk([params]) { return inherited Left(params...); }
    constant() { return inherited Left(); }
;
arguments: object values = [7, 8];
main() {
    local owned value = new Derived(arguments.values...);
    "Constructed: <<value.left>>/<<value.right>>/<<value.value>>.\n";
    "Results: <<value.calc(3)>>/<<value.risk(2)>>/<<value.constant()>>.\n";
    local caught = 0;
    for(local i in 1..100) {
        try { value.risk(0); }
        catch(Problem e) { caught += 1; }
    }
    "Caught: <<caught>>.\n";
    return nil;
}
