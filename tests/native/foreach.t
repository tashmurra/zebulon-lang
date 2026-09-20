values: object items=static [1,nil,3] empty=static [];
early(){foreach(local x in values.items){foreach(local y in values.items){return x;}}return 0;}
main(){
 local count=0;
 foreach(local x in values.items){count+=1;if(x==nil)continue;"Value <<x>>.\n";}
 foreach(local x in values.empty){count+=100;}
 foreach(local x in values.items){count+=1;break;}
 foreach(local z in values.items){if(z!=nil)"Nested: <<early()+z>>.\n";}
 "Count: <<count>>/<<early()>>.\n";
 return nil;
}
