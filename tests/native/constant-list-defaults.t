enum flag;
class Base: object
    before = []
    after = [later, [10, 'hello', true, nil], &value, flag, callback]
;
child: Base;
later: object value = 7;
callback() { return 42; }
main() {
    local defaults = child.after;
    "Defaults: <<child.before.length()>>/<<defaults.length()>>.\n";
    "Values: <<defaults[1].(defaults[3])>>/<<defaults[2][1]>>/<<defaults[2][2]>>/<<defaults[4] == flag ? 1 : 0>>.\n";
    local fn = defaults[5];
    "Callback: <<fn()>>.\n";
    Base.after = nil;
    "Retained: <<defaults.length()>>/<<defaults[2][2]>>.\n";
    local owned instance = new Base();
    "Instance: <<instance.before.length()>>.\n";
    return nil;
}
