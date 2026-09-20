#define NUMBER 42
state: object count = 0;
versionInfo: object version = '3.0' serialNum = '20260915';
item: object
    name = 'lamp'
    desc = "The <<name>> has been examined <<++state.count>> times.\n"
;
sideEffect() { "[side]"; return 7; }
main() {
    "Release <<versionInfo.version>> (<<versionInfo.serialNum>>)\n";
    item.desc;
    item.desc;
    "Order: <<++state.count>>, <<sideEffect()>>, <<state.count>>.\n";
    "Nested: <<true ? "yes <<state.count>>" : "no">>.\n";
    "Scalar: <<nil>>|<<-2147483648>>|<<(8 >> 1)>>.\n";
    "Literal: <<'quoted'>>.\n";
    "Macro: NUMBER <<NUMBER>>.\n";
    return nil;
}
