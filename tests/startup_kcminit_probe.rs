const PROBE: &str = include_str!("../assets/localdesktop-kcminit-stack-probe.c");
const WRAPPER: &str = include_str!("../assets/localdesktop-kcminit-stack-wrapper.sh");
const SCRIPT: &str = include_str!("../scripts/capture-kcminit-stack.ps1");

#[test]
fn probe_captures_waiting_registers_maps_and_best_effort_backtrace() {
    assert!(PROBE.contains("sigaction(SIGUSR2"));
    assert!(PROBE.contains("kcminit-stack-probe-start"));
    assert!(PROBE.contains("pc"));
    assert!(PROBE.contains("sp"));
    assert!(PROBE.contains("maps_begin"));
    assert!(PROBE.contains("backtrace_begin"));
    assert!(PROBE.contains("LOCALDESKTOP_KCMINIT_STACK_LOG"));
    assert!(!PROBE.contains("sigaction(SIGSEGV"));
    assert!(!PROBE.contains("sigaction(SIGABRT"));
}

#[test]
fn wrapper_targets_only_kcminit_and_script_restores_app_private_state() {
    assert!(WRAPPER.contains("exec /usr/bin/kcminit_startup"));
    assert!(WRAPPER.contains("LD_PRELOAD"));
    assert!(SCRIPT.contains("run-as app.polarbear"));
    assert!(SCRIPT.contains("kill -USR2"));
    assert!(SCRIPT.contains(".before"));
    assert!(SCRIPT.contains("SigCgt"));
    assert!(SCRIPT.contains("sigusr2_caught=1"));
    assert!(SCRIPT.contains("wrapper_restored="));
    assert!(SCRIPT.contains("data_clear=false"));
    assert!(!SCRIPT.contains("am clear"));
    assert!(!SCRIPT.contains("pm clear"));
    assert!(!SCRIPT.contains("install -r"));
}
