property checkDirect, checkIndirect;
registry: object
    owned checks = static new LookupTable()
    owned defaults = static new LookupTable()
    initialize() {
        checks[1] = &checkDirect;
        checks[2] = &checkIndirect;
        defaults.setDefaultValue(nil);
        defaults['target'] = target;
    }
;
target: object checkDirect = 10 checkIndirect = 20;
main() {
    registry.initialize();
    "Checks: <<registry.checks.getEntryCount()>>/<<target.(registry.checks[1])>>/<<target.(registry.checks[2])>>.\n";
    "Default: <<registry.defaults['missing'] == nil ? 1 : 0>>.\n";
    local borrowed = registry.defaults['target'];
    registry.checks[1] = &checkIndirect;
    "Updated: <<target.(registry.checks[1])>>.\n";
    registry.checks = nil;
    registry.defaults = nil;
    "Borrowed object: <<borrowed.checkDirect>>.\n";
    return nil;
}
