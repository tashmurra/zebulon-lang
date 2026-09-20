class Base: object;
class Left: Base;
class Right: Base;
class Combined: Left, Right;
class Other: object;
instance: Combined;
holder: object owned value=static new Combined();
flag(v){return v?'yes':'no';}
main(){
 "Kinds: <<flag(instance.ofKind(Base))>>/<<flag(instance.ofKind(Left))>>/<<flag(instance.ofKind(Right))>>/<<flag(instance.ofKind(Other))>>.\n";
 "Identity: <<flag(instance.ofKind(instance))>>/<<flag(Base.ofKind(Base))>>/<<flag(Base.ofKind(instance))>>.\n";
 "Constructed: <<flag(holder.value.ofKind(Combined))>>/<<flag(holder.value.ofKind(Base))>>.\n";
 return nil;
}
