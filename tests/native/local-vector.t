class Problem: object;
failure: Problem;
state: object saved=nil;
early(){local owned vec=new Vector(2);vec.append(7);return vec[1];}
main(){
 local owned vec=new Vector(1);
 vec.append(10);vec.append(20);
 "Vector: <<vec.length()>>/<<vec[1]>>/<<vec[2]>>.\n";
 foreach(local x in vec){vec.append(30);"Item: <<x>>.\n";}
 "After: <<vec.length()>>/<<early()>>.\n";
 for(local i=0;i<3;i+=1){local owned inner=new Vector(0);inner.append(i);if(i==0)continue;break;}
 try {local owned inner=new Vector(1);inner.append(42);throw failure;}
 catch(Problem exc){"Caught: <<vec.length()>>.\n";}
 return nil;
}
