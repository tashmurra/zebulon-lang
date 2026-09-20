transient registry: object
    owned handlers = static new Vector()
    owned numbers = static new Vector(2)
    run() {
        foreach (local callback in handlers) callback();
    }
;
first() { "First.\n"; return nil; }
second() { "Second.\n"; return nil; }
main() {
    "Initial: <<registry.handlers.length()>>/<<registry.numbers.length()>>.\n";
    registry.handlers.append(first);
    registry.handlers.append(second);
    registry.run();
    registry.numbers.append(10);
    registry.numbers.append(20);
    registry.numbers[1] = 30;
    "Stored: <<registry.handlers.length()>>; <<registry.numbers[1] + registry.numbers[2]>>.\n";
    registry.handlers = nil;
    registry.numbers = nil;
    "Released.\n";
    return nil;
}
