// A tiny instrument world driven by host action IDs.
enum token tokWord;
property firstTokenIndex, lastTokenIndex, tokenList;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Instrument: object name = 'instrument' reading = 0;
bench: object name = 'bench';
probe: Instrument name = 'probe' location(bench);
startup() { "Ready. Actions: 1 increments, 2 undoes.\n"; return nil; }
show() { "reading=<<probe.reading>> onBench=<<location.get(probe)==bench ? 1 : 0>>\n"; return nil; }
act(verb, subjects) {
    if (verb == 1) probe.reading += 1;
    if (verb == 2) undo();
    show(); return nil;
}
turn(tokens) { show(); return nil; }
