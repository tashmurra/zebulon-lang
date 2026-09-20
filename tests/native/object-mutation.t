// Independently authored startup graph: forward references are non-owning.
stack: object
    count = 6 * 7
    ready = true
    peer = door
;
door: object
    locked = nil
    peer = stack
;

identityValue(value) {
    local alias = value;
    return alias;
}
read(obj) {
    return obj.peer.count;
}
main() {
    stack.trace = 0;
    selectReceiver().count = rhsValue();
    if (stack.trace != 21 || stack.count != 10) return -1;

    stack.trace = 0;
    selectReceiver().count += rhsValue();
    if (stack.trace != 12 || stack.count != 20) return -2;

    stack.peer = door;
    door.count = 5;
    stack.peer.count += redirectReceiver();
    if (door.count != 7 || stack.count != 20 || stack.peer != stack) return -3;

    stack.trace = 0;
    local before = selectReceiver().count++;
    local after = ++selectReceiver().count;
    if (before != 20 || after != 22 || stack.trace != 11) return -4;
    local lowered = --stack.count;
    if (lowered != 21 || stack.count-- != 21 || stack.count != 20) return -5;

    stack.ready = nil;
    if (stack.ready != nil) return -6;
    stack.ready = true;
    door.locked = nil;
    stack.peer = door;
    stack.count = 42;
    return read(identityValue(door));
}
selectReceiver() {
    stack.trace = stack.trace * 10 + 1;
    return stack;
}
rhsValue() {
    stack.trace = stack.trace * 10 + 2;
    return 10;
}
redirectReceiver() {
    stack.peer = stack;
    door.count = 100;
    return 2;
}
