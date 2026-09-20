registry: object events = static newOwnedCollection();
class Event: object
    number=0
    construct(n) {
        number=n;
        self.reserveInCollection(registry.events);
    }
    fire(){ "Event <<number>>.\n"; }
;
class Withdrawn: Event
    construct(n) {
        inherited(n);
        registry.events.removeOwned(self);
        number+=100;
        "Cancelled while constructing: <<number>>.\n";
    }
;
main(){
    registry.events=registry.events;
    new published Event(7);
    new published Event(8);
    new published Withdrawn(9);
    "Retained: <<registry.events.ownedLength()>>.\n";
    foreach(local event in registry.events) {
        event.fire();
        registry.events.removeOwned(event);
    }
    "After iteration: <<registry.events.ownedLength()>>.\n";
    return nil;
}
