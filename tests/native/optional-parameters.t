value(x) { return x == nil ? 0 : x; }
f(a, b?, c?) { return a * 100 + value(b) * 10 + value(c); }
g(a?, [extra]) { return value(a) + extra.length() * 100; }
state: object one = static [2] empty = static [];
base: object m(a?, ...) { return value(a) + 1; };
child: base m(a?, [extra]) { return inherited(a) + extra.length() * 10; };
class Problem: object code = 0 construct(msg?, ...) { code = msg == nil ? 7 : msg; };
main() {
    "Direct: <<f(1)>>/<<f(1,2)>>/<<f(1,2,3)>>/<<g()>>/<<g(3,4,5)>>.\n";
    local fn = f;
    local rest = g;
    "Values: <<fn(2)>>/<<fn(state.one...)>>/<<rest(state.empty...)>>/<<rest(3,4,5)>>.\n";
    local callback = function(a?, b?) { return value(a) + value(b); };
    "Callback: <<callback()>>/<<callback(2,3)>>.\n";
    "Methods: <<base.m()>>/<<base.m(4,5)>>/<<child.m()>>/<<child.m(5,6,7)>>.\n";
    try { throw new Problem; }
    catch (Problem exc) { "Default: <<exc.code>>.\n"; }
    try { throw new Problem(9,10); }
    catch (Problem exc) { "Supplied: <<exc.code>>.\n"; }
    return nil;
}
