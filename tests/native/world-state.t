// self-authored world history, enum guard and logical save fixture.
property firstTokenIndex, lastTokenIndex, tokenList;
enum token tokWord;
enum Direction: north, south;
enum Colour: red;
dictionary cmdDict;
property noun;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
relation exits(from: Entity, to: Entity, via: Direction) one_to_one;
class Thing: object name = 'thing';
room: Thing name = 'room';
box: Thing name = 'box' isOpen = nil;
coin: Thing name = 'coin';
ghost: Thing name = 'ghost';
state: object count = 0 vec = nil lookup = nil buffer = nil;
startup() {
    contains.set(room, box); contains.set(box, coin); contains.set(room, ghost);
    exits.set(room, box, north); exits.set(box, room, south);
    cmdDict.addWord(coin, 'coin', &noun);
    local vector = new Vector(2); state.vec = vector; state.vec.append(1);
    local lookup = new LookupTable(); state.lookup = lookup; state.lookup[1] = 2;
    local buffer = new StringBuffer(); state.buffer = buffer; state.buffer.append('base');
    return nil;
}
show() {
    "[world count=<<state.count>> box=<<box.isOpen ? 1 : 0>> in=<<contains.contains(box, coin) ? 1 : 0>> exits=<<exits.get(room,north)==box ? 1 : 0>>/<<exits.get(box,south)==room ? 1 : 0>> ghost=<<contains.all(room).length()>> vocab=<<cmdDict.isWordDefined('token') ? 1 : 0>> vec=<<state.vec[1]>>/<<state.vec.length()>> lookup=<<state.lookup[1]>> bytes=<<state.buffer.length()>> undo=<<undoDepth()>>]\n";
    return nil;
}
change() {
    state.count += 1; state.count += 1;
    state.vec[1] = 9; state.vec.append(8);
    state.lookup[1] = 7; state.lookup[2] = 6;
    state.buffer.append('!');
    cmdDict.removeWord(coin, 'coin', &noun); cmdDict.addWord(coin, 'token', &noun);
    return nil;
}
wrong(label) { return exits.get(room,label); }
act(verb, subjects) {
    if (verb == 1) change();
    if (verb == 2) undo();
    if (verb == 3) { change(); local zero=0; state.count=1/zero; }
    if (verb == 4) { despawn(ghost); }
    if (verb == 5) { change(); savepoint(); state.count=99; }
    if (verb == 6) { wrong(red); }
    show(); return nil;
}
turn(tokens) { show(); return nil; }
recover(code) { "[recovered]\n"; show(); return nil; }
