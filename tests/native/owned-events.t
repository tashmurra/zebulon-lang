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
    for(local i=1;i<=registry.events.ownedLength();i+=1) {
        local event=registry.events.ownedAt(i);
        event.fire();
    }
    local first=registry.events.ownedAt(1);
    registry.events.removeOwned(first);
    "Remaining: <<registry.events.ownedLength()>>/<<registry.events.ownedAt(1).number>>.\n";
    return nil;
}
