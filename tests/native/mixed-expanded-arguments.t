arguments: object values=[3,4] empty=[];
trace: object order=0;
mark(n){trace.order=trace.order*10+n;return n;}
tail(){mark(3);return arguments.values;}
combine(a,b,c,d){return a*1000+b*100+c*10+d;}
optional(a,b?){return a+(b==nil ? 0 : b);}
class Count: object construct(first,[rest]){total=first;foreach(local n in rest) total+=n;} total=0;
class Failure: object;
failure: Failure;
raise(a,b,c){throw failure;}
main(){
 local caught=0;
 local combiner=combine;local raiser=raise;
 for(local i=0;i<100;++i){
  trace.order=0;
  local value=combiner(mark(1),mark(2),tail()...);
  local order=trace.order;
  local owned count=new Count(7,arguments.values...);
  local empty=optional(5,arguments.empty...);
  try{raiser(1,arguments.values...);}catch(Failure e){caught+=1;}
  if(i==99) "Mixed: <<value>>/<<order>>/<<count.total>>/<<empty>>.\n";
 }
 "Caught: <<caught>>.\n";
 return nil;
}
