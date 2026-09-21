#![forbid(unsafe_code)]
use zeb_frontend::llvm::Target;

#[test]
fn supported_hosts_have_explicit_target_sets() {
    for (os, arch, expected, slices) in [
        ("macos", "x86_64", "macos-universal", 2),
        ("macos", "aarch64", "macos-universal", 2),
        ("linux", "x86_64", "linux-x86_64", 1),
        ("windows", "x86_64", "windows-x86_64", 1),
    ] {
        let target = Target::for_host(os, arch).unwrap();
        assert_eq!(target.bundle_name(), expected);
        assert_eq!(target.slices().len(), slices);
    }
    for (os, arch) in [
        ("linux", "aarch64"),
        ("windows", "aarch64"),
        ("ios", "aarch64"),
    ] {
        assert!(Target::for_host(os, arch).is_err());
    }
    assert_eq!(Target::WindowsX86_64.executable("consumer"), "consumer.exe");
    assert_eq!(Target::WindowsX86_64.object_ext(), "obj");
    assert_eq!(Target::LinuxX86_64.shared_ext(), "so");
    assert_eq!(Target::MacX86_64.ir_symbol("__Rname"), "_Rname");
    assert_eq!(Target::LinuxX86_64.ir_symbol("_Rname"), "_Rname");
}

#[test]
fn new_targets_do_not_inherit_stack_qualification() {
    for target in [Target::LinuxX86_64, Target::WindowsX86_64] {
        assert!(zebc::stack_machine::validate("", "", "_Rleaf", target).is_err());
    }
}
