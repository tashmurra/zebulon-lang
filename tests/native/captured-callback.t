inputs: object modes=[0,1];
class Failure: object;
failure: Failure;
target: object number=7;
callbackBody(env,n) {
    if(n == 99) throw failure;
    return closureCapture(env,0)+closureCapture(env,1).number+n;
}
owned makeCallback() {
    local owned values=new Vector(2);
    values.append(3); values.append(target);
    local owned snapshot=values.toList();
    local owned cb=captureClosure(callbackBody,inputs.modes,snapshot);
    return move cb;
}
main() {
    local total=0;
    for(local i=0;i<100;++i) {
        local owned cb=makeCallback();
        target.number=8;
        total+=cb(4);
        try { cb(99); } catch(Failure e) { total+=1; }
        finally { target.number=9; }
        total+=cb(1);
    }
    "Captured: <<total>>.\n";
    return nil;
}
