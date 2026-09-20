#![forbid(unsafe_code)]
#[path = "semantics/callbacks.rs"]
mod callbacks;
#[path = "semantics/closure_capture.rs"]
mod closure_capture;
#[path = "semantics/delegated.rs"]
mod delegated;
#[path = "semantics/exceptions.rs"]
mod exceptions;
#[path = "semantics/expanded_constructors.rs"]
mod expanded_constructors;
#[path = "semantics/expanded_tail.rs"]
mod expanded_tail;
#[path = "semantics/field_moves.rs"]
mod field_moves;
#[path = "semantics/field_owner_transfers.rs"]
mod field_owner_transfers;
#[path = "semantics/intrinsics.rs"]
mod intrinsics;
#[path = "semantics/member_transfers.rs"]
mod member_transfers;
#[path = "semantics/modify.rs"]
mod modify;
#[path = "semantics/nested_objects.rs"]
mod nested_objects;
#[path = "semantics/object_init.rs"]
mod object_init;
#[path = "semantics/object_values.rs"]
mod object_values;
#[path = "semantics/owning_returns.rs"]
mod owning_returns;
#[path = "semantics/qualified_inherited.rs"]
mod qualified_inherited;
#[path = "semantics/regex.rs"]
mod regex;
#[path = "semantics/semantic.rs"]
mod semantic;
#[path = "semantics/type_checks.rs"]
mod type_checks;
#[path = "semantics/vm_intrinsics.rs"]
mod vm_intrinsics;
