registry: object events=static newOwnedCollection() later=static newOwnedCollection();
class Event: object
 number=0
 construct(n){number=n;self.reserveInCollection(registry.events);}
 after(){number+=100;"Active after removal: <<number>>.\n";}
 fire(){registry.events.removeOwned(self);self.after();}
;
class Requeued: Event
 fire(){
   registry.events.removeOwned(self);
   self.after();
   self.reserveInCollection(registry.later);
   number+=1;
 }
;
main(){
 new published Event(7);
 new published Requeued(8);
 local event=registry.events.ownedAt(1);
 event.fire();
 "First removed: <<registry.events.ownedLength()>>.\n";
 event=registry.events.ownedAt(1);
 event.fire();
 "Queues: <<registry.events.ownedLength()>>/<<registry.later.ownedLength()>>.\n";
 "Requeued: <<registry.later.ownedAt(1).number>>.\n";
 return nil;
}
