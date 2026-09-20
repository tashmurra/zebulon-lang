class Problem: object;
failure: Problem;
state: object values=static [3,4,5] empty=static [] order=0;
sum(base,[args]){foreach(local x in args){base+=x;}return base;}
fixed(a,b,c){return a*100+b*10+c;}
zero(){return 9;}
forward(fn,[args]){return fn(args...);}
raising([args]){throw failure;}
target(){state.order=state.order*10+2;return fixed;}
arguments(){state.order=state.order*10+1;return state.values;}
main(){
 local fn=sum;local fixedFn=fixed;local zeroFn=zero;
 "Function: <<fn(4,5,6)>>/<<forward(sum,10,20,30)>>.\n";
 "Expand: <<fixedFn(state.values...)>>/<<sum(state.values...)>>/<<zeroFn(state.empty...)>>.\n";
 "Order: <<target()(arguments()...)>>/<<state.order>>.\n";
 try{forward(raising,1,2);}catch(Problem exc){"Caught.\n";}
 "After: <<fn(7)>>/<<forward(fixed,1,2,3)>>.\n";
 return nil;
}
