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
    local obj = identityValue(door);
    if (obj == door && stack.ready && door.locked == nil)
        return read(obj);
    return -1;
}
