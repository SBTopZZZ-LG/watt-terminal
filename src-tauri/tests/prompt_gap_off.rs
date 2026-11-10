//! AGENT_TERM_PROMPT_GAP=0 turns the blank row before the prompt off. The shell reads the variable when it sources
//! the integration, and every session inherits this process's environment. The variable is set here, so this file
//! is its own test binary with a single test: nothing else in the process reads it or runs beside it.

mod common;

use std::time::Duration;

use common::*;

#[test]
fn gap_off_prompt_after_a_command_has_no_blank_row() {
    let _g = serial();
    // SAFETY: this binary has one test, and no other thread in the process touches the environment.
    unsafe { std::env::set_var("AGENT_TERM_PROMPT_GAP", "0") };
    let (s, seen) = start_recording();
    wait_prompt(&s);
    let mark = seen.lock().unwrap().len();
    run(&s, "echo hi");
    let after_cmd = wait_until(Duration::from_secs(5), || prompt_bytes(&seen.lock().unwrap(), mark, s.nonce()))
        .expect("prompt bytes after the command");
    assert!(!starts_with_blank_row(&after_cmd), "blank row with AGENT_TERM_PROMPT_GAP=0: {after_cmd:?}");
    assert_eq!(s.integration_status(), agent_term_lib::core::observer::IntegrationStatus::Live);
}
