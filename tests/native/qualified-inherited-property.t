// Independently authored fixture: `inherited Class.property(...)`.
class Base: object
    greet(n) { return n + 1; }
    tag() { return 10; }
;
class Other: object
    greet(n) { return n + 100; }
;
thing: Base, Other
    greet(n) { return inherited Other.greet(n) + inherited Base.greet(n); }
    tag() { return inherited Base.tag() + 1; }
;
plain: Base
    greet(n) { return inherited(n) * 2; }
;
main()
{
    "both: <<thing.greet(1)>>.\n";
    "one: <<thing.tag()>>.\n";
    "plain: <<plain.greet(3)>>.\n";
    return nil;
}
