class Problem: object;
values: object items=static [1,2,3];
main(){
 local count=0;
 outer: foreach(local x in values.items){
   foreach(local y in values.items){count+=1;if(x==1)continue outer;break outer;}
 }
 "Snapshots: <<count>>.\n";
 marked: for(local i=0;i<3;i+=1){
   try {
     local owned v=new Vector(1);v.append(i);
     foreach(local x in values.items){if(i==0)continue marked;break marked;}
   } finally {"Finally: <<i>>.\n";}
 }
 protected: foreach(local x in values.items){
   try {foreach(local y in values.items){break protected;}}
   catch(Problem exc){"WRONG";}
 }
 local n=0;
 again: while(n<3){n+=1;while(true){if(n<2)continue again;break again;}}
 "While: <<n>>.\n";
 foreach(local x in values.items){"After: <<x>>.\n";}
 return nil;
}
