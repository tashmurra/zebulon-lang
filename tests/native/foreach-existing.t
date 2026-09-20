defaults: object values = [1,2,3] empty = [];
class Walker: object
    current = 9
    run() {
        local sum = 0;
        foreach (current in defaults.values) {
            if (current == 2) continue;
            sum += current;
        }
        return sum;
    }
;
main() {
    local n = 8;
    foreach (n in defaults.empty) { n = 0; }
    "Empty: <<n>>.\n";
    local sum = 0;
    foreach (n in defaults.values) { sum += n; if (n == 2) break; }
    "Break: <<n>>/<<sum>>.\n";
    local owned walker = new Walker();
    "Field: <<walker.run()>>/<<walker.current>>.\n";
    return nil;
}
