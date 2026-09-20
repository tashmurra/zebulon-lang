inputs: object values=[2,3] empty=[];
trace: object order=0;
class Base: object total(a,[rest]){return 0;} flag=5;
class Child: Base total(a,[rest]){local n=a+10;foreach(local x in rest)n+=x;return n;} fail(a,[rest]){throw failure;};
class Failure: object;
failure: Failure;
receiverObject: Child;
receiver(){trace.order=trace.order*10+4;return receiverObject;}
mark(){trace.order=trace.order*10+1;return 1;}
tail(){trace.order=trace.order*10+2;return inputs.values;}
main(){
 local caught=0;
 for(local i=0;i<100;++i){
  trace.order=0;
  local total=receiver().total(mark(),tail()...);
  local flag=receiverObject.flag(inputs.empty...);
  try{receiverObject.fail(1,inputs.values...);}catch(Failure e){caught+=1;}
  if(i==99) "Methods: <<total>>/<<trace.order>>/<<flag>>.\n";
 }
 "Caught: <<caught>>.\n";
 return nil;
}
