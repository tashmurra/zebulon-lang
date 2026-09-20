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
