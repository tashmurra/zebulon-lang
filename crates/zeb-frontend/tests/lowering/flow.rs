#![forbid(unsafe_code)]
use zeb_frontend::{
    Diagnostic, flow,
    parser::{self, Model},
    source::{Encoding, Source},
};
fn check(text: &str) -> Result<flow::Checked, Diagnostic> {
    flow::check(
        &parser::parse_with(
            &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            Model::Ownership,
        )
        .unwrap(),
    )
}
fn fail(text: &str, code: &str) {
    assert_eq!(check(text).unwrap_err().code, code, "{text}");
}

#[test]
fn initialization_must_cover_all_incoming_paths() {
    check("f(x){local y;if(x)y=1;else y=2;return y;}").unwrap();
    fail("f(x){local y;if(x)y=1;return y;}", "flow-uninitialized");
    fail("f(){local y=y;return y;}", "flow-uninitialized");
    check("f(){local y=(y=1);return y;}").unwrap();
    check("f(x){local y;if(x)return 0;else y=1;return y;}").unwrap();
}
#[test]
fn loop_backedges_do_not_initialize_the_first_iteration() {
    fail(
        "f(x){local y;while(x){y=1;}return y;}",
        "flow-uninitialized",
    );
    fail(
        "f(x){local y;while(x){x=y;y=1;}return 0;}",
        "flow-uninitialized",
    );
    check("f(x){local y=0;while(x){y+=1;x=nil;}return y;}").unwrap();
    check("f(){local y;while(true){y=1;break;}return y;}").unwrap();
    fail(
        "f(x){local y;while(true){if(x)break;y=1;}return y;}",
        "flow-uninitialized",
    );
}
#[test]
fn loop_local_reset_prevents_previous_iteration_leakage() {
    fail(
        "f(x){while(x){local y;if(x)y=1;x=y;}return 0;}",
        "flow-uninitialized",
    );
    check("f(x){while(x){local y=1;x=y==0;}return 0;}").unwrap();
}
#[test]
fn short_circuit_and_argument_order_control_initialization() {
    fail(
        "f(x){local y;x && (y=true);return y;}",
        "flow-uninitialized",
    );
    check("f(){local y;true && (y=true);return y;}").unwrap();
    check("f(){local y;nil ?? (y=1);return y;}").unwrap();
    check("g(a,b){return a;} f(){local x;return g(x,x=1);}").unwrap();
    fail(
        "g(a,b){return a;} f(){local x;return g(x=1,x);}",
        "flow-uninitialized",
    );
    fail("f(){local x;return x+=(x=1);}", "flow-uninitialized");
}
#[test]
fn only_reachable_normal_ends_are_errors() {
    fail("f(x){if(x)return 1;}", "flow-return");
    check("f(){while(true){continue;}}").unwrap();
    check("f(x){if(x)return 1;else return 2;}").unwrap();
    check("f(){return;local x;return x;}").unwrap();
    fail("f(){while(true){break;}}", "flow-return");
    fail("f(){}", "flow-return");
}
#[test]
fn definite_type_errors_and_dynamic_obligations_remain_distinct() {
    fail("f(){local x=true;return x+1;}", "flow-type");
    fail("f(){local x=1;if(x)return 0;return 1;}", "flow-type");
    fail(
        "f(x){local y=true;while(x){y=nil;x=nil;}return y+1;}",
        "flow-type",
    );
    let checked = check("f(x){local y;if(x)y=1;else y=nil;return y+1;}").unwrap();
    assert!(!checked.facts[0].runtime_checks.is_empty());
    check("f(x){return x+1;}").unwrap();
    check("f(x){return x==true;}").unwrap();
}
#[test]
fn widening_loop_conditions_reaches_the_exit() {
    fail("f(){local x=true;while(x){x=nil;}}", "flow-return");
    let checked = check("f(){local x=true;while(x){x=nil;}return 1;}").unwrap();
    assert!(checked.facts[0].reachable.iter().filter(|r| **r).count() >= 4);
}
#[test]
fn diagnostic_locations_point_to_the_source_operation() {
    let text = "f(){local x;return x;}";
    let error = check(text).unwrap_err();
    assert_eq!(error.byte, text.rfind('x').unwrap());
}

#[test]
fn switches_preserve_case_entry_initialization_and_control_targets() {
    check("main(v){local x; switch(v){case 1:x=2;break;default:x=3;} return x;}").unwrap();
    assert!(check("main(v){local x; switch(v){case 1:x=2;break;} return x;}").is_err());
    assert!(check("main(v){switch(v){case 1:local x=2;case 2:return x;}return nil;}").is_err());
    check("main(v){while(true){switch(v){case 1:continue;default:break;}break;}return nil;}")
        .unwrap();
    for text in [
        "main(v){switch(v){case 1:continue;}return nil;}",
        "main(v){switch(v){case 1:break;case 1+0:break;}return nil;}",
        "main(v){switch(v){case v:break;}return nil;}",
        "enum e; main(v){local e=1;switch(v){case e:break;}return nil;}",
    ] {
        assert!(check(text).is_err(), "{text}");
    }
}

#[test]
fn switch_object_and_property_labels_bind_as_constants() {
    check("property p; a: object; main(v){switch(v){case a:return 1;case &p:return 2;default:return 3;}}").unwrap();
    assert!(
        check("property p; main(v){switch(v){case &p:break;case &p:break;}return nil;}").is_err()
    );
}

#[test]
fn for_loops_keep_initializer_scope_and_correct_continue_flow() {
    check("main(){local x=0;for(local i=0,x+=1,local j=i+1;i<3;i+=1){if(j==1)continue;x+=j;}return x;}").unwrap();
    check("main(){for(;;){return 1;}}").unwrap();
    assert!(check("main(){for(local i=0;i<1;i+=1){}return i;}").is_err());
    assert!(check("main(){local x;for(;nil;x=1){}return x;}").is_err());
    assert!(check("main(){for(;;x+=1){local x=0;break;}return nil;}").is_err());
}
