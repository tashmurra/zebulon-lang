class Problem: object;
failure: Problem label='original';
second: Problem label='replacement';
state: object number=0 items=static [1,2];
raise(){throw failure;}
early(){try {return state.number;}finally {state.number+=10;}}
overrideReturn(){try{return 1;}finally{return 7;}}
nested(){try {try{return 3;}finally{"Inner.\n";}}finally{"Outer.\n";}}
beforeScope(){raise();try{raise();}finally{"WRONG scope.\n";}return nil;}
iterationReturn(){try{foreach(local v in state.items){return v;}}finally{"Iteration return.\n";}return 0;}
main(){
 try {state.number=2;}finally {"Normal: <<state.number>>.\n";}
 "Return: <<early()>>/<<state.number>>/<<overrideReturn()>>/<<nested()>>.\n";
 try {try {raise();}finally {try{throw second;}catch(Problem other){"Inside: <<other.label>>.\n";}}}
 catch(Problem exc){"Pending: <<exc.label>>.\n";}
 try {try {throw failure;}finally {throw second;}}
 catch(Problem exc){"Override: <<exc.label>>.\n";}
 try {throw failure;}catch(Problem exc){"Catch.\n";}finally{"After catch.\n";}
 for(local i=0;i<3;i+=1){try{if(i==0)continue;break;}finally{"Loop: <<i>>.\n";}}
 try {foreach(local x in state.items){if(x==1)continue;"Item: <<x>>.\n";}}
 finally {"After iteration.\n";}
 try{beforeScope();}catch(Problem exc){"Before scope: <<exc.label>>.\n";}
 foreach(local outer in state.items){"Saved: <<iterationReturn()>>/<<outer>>.\n";}
 return nil;
}
