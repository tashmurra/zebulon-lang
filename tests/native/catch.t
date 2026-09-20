class Problem: object;
class Specific: Problem;
class Other: object;
failure: Specific message='caught';
values: object items=static [1,2] events=static newOwnedCollection();
class Event: object
 number=42
 construct(){self.reserveInCollection(values.events);throw failure;}
;
raise(value){throw value;}
invoke(fn,value){return fn(value);}
relay: object run(value){return invoke(raise,value);};
early(){try {return 9;}catch(Problem exc){return 0;}}
main(){
 local total=0;
 foreach(local x in values.items){
   try {
     foreach(local y in values.items){total+=x;relay.run(failure);}
   }
   catch(Other wrong){"WRONG";}
   catch(Problem exc){"Caught <<x>>/<<exc.message>>.\n";}
 }
 try {try {raise(failure);}catch(Other wrong){"WRONG";}}
 catch(Specific exc){"Outer: <<exc==failure?'same':'wrong'>>.\n";}
 for(local i=0;i<3;i+=1){try {if(i==0)continue;break;}catch(Problem exc){"WRONG";}}
 try {try{throw failure;}catch(Specific exc){throw exc;}}
 catch(Problem exc){"Rethrow: <<exc==failure?'same':'wrong'>>.\n";}
 try {new published Event();}catch(Problem exc){"Published failure: <<values.events.ownedLength()>>/<<values.events.ownedAt(1).number>>.\n";}
 "Total: <<total>>/<<early()>>.\n";
 return nil;
}
