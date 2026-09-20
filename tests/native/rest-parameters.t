class Problem: object;
failure: Problem;
state: object order=0;
value(n){state.order=state.order*10+n;return n;}
sum(base,[args]){foreach(local x in args){base+=x;}return base;}
count([args]){return args.length();}
ignore([args]){return 7;}
raising([args]){throw failure;}
main(){
 "Sum: <<sum(10)>>/<<sum(10,2,3)>>/<<count()>>/<<count(nil,true,5)>>.\n";
 "Order: <<sum(value(1),value(2),value(3))>>/<<state.order>>.\n";
 "Nested: <<sum(1,sum(2,3,4),5)>>/<<ignore(1,2)>>.\n";
 try {raising(1,2,3);}catch(Problem exc){"Caught.\n";}
 "After: <<count(1,2)>>.\n";
 return nil;
}
