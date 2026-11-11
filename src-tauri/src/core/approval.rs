//! The approval policy for `run_command`: which calls may run without a card and which must wait for the user.
//!
//! Pure: no I/O, no shell, no Tauri. A policy is built for each user message from the settings in use (lib.rs,
//! `agent_message`), so the per-turn count of automatic approvals starts at zero for every message.
//!
//! Two modes are saved: manual (every call asks) and regex (the rules below). Approve-all is not part of this module:
//! it is the session-only flag in core/agent.rs, and it skips the policy. In a nested shell (ssh, a nested shell, one
//! the app integrated: not the local machine) an allow rule never runs a command by itself: it asks with `nested_level`.
//!
//! This is a floor, not a sandbox. The hard gate refuses shell syntax, and any command whose first word is not a plain
//! command name (a path, an assignment, `=ls`, a leading `-`), and the programs and shell builtins on a deny list,
//! judged by the first word in lowercase. It cannot see aliases or functions in the user's shell, which can shadow a
//! bare name. Build tools and other programs the user allows in their rules run without a card. The user's rules decide
//! everything the floor does not refuse.

use std::sync::atomic::{AtomicUsize, Ordering};

use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::{Value, json};

use crate::core::keys::TypedLine;
use crate::core::settings::{ApprovalMode, ApprovalSettings};

/// Automatic approvals a regex policy gives in one turn. The next one asks, with reason `auto_cap`.
pub const MAX_AUTO_PER_TURN: usize = 8;
/// Longest accepted rule list, in rules.
pub const MAX_RULES: usize = 50;
/// Longest accepted rule, in characters.
pub const MAX_RULE_CHARS: usize = 200;
/// The compiled size limit of one rule, in bytes.
const REGEX_SIZE_LIMIT: usize = 256 * 1024;
/// Punctuation a command may use besides ASCII letters and digits. A space is the only separator.
const SAFE_PUNCTUATION: &str = " _-./:=@+,";

/// First words that are never auto-approved, whatever the rules say: privilege, shell and exec primitives, and
/// destructive or network tools, and the zsh reserved words, precommand modifiers and builtins that run another word or
/// change the shell's state. Matched exactly against the first word, ASCII-lowercased (macOS paths are
/// case-insensitive, so `RM` runs `rm`). Kept sorted (byte order), all lowercase and unique: a unit test checks it, and
/// `binary_search` relies on it.
pub const BUILTIN_DENY: &[&str] = &[
    ".", "alias", "autoload", "awk", "bash", "bindkey", "builtin", "bun", "bye", "caffeinate", "case", "chgrp", "chmod",
    "chown", "command", "compdef", "coproc", "cp", "csh", "curl", "dash", "dd", "declare", "defaults", "deno",
    "disable", "diskutil", "do", "doas", "done", "ed", "elif", "else", "emacs", "emulate", "enable", "end", "env",
    "esac", "eval", "ex", "exec", "exit", "expect", "export", "fc", "fi", "find", "fish", "float", "for", "foreach",
    "ftp", "function", "functions", "gawk", "gopass", "hash", "if", "install", "integer", "kill", "killall", "ksh",
    "launchctl", "less", "ln", "local", "logout", "lua", "man", "mkfs", "more", "mount", "mv", "nano", "nc", "ncat",
    "nice", "nmap", "nocorrect", "node", "noglob", "nohup", "nvim", "op", "open", "osascript", "parallel", "pass",
    "perl", "php", "pkill", "printenv", "python", "python3", "r", "read", "readonly", "rehash", "repeat", "rm", "rmdir",
    "rsync", "ruby", "sched", "scp", "screen", "security", "sed", "select", "set", "setopt", "sftp", "sh", "shred",
    "source", "sqlite3", "ssh", "stdbuf", "su", "sudo", "tar", "tclsh", "tcsh", "tee", "telnet", "then", "time",
    "timeout", "tmux", "trap", "truncate", "typeset", "umount", "unalias", "unlink", "unset", "unsetopt", "until",
    "unzip", "vi", "view", "vim", "watch", "wget", "while", "xargs", "zftp", "zip", "zle", "zmodload", "zpty", "zsh",
    "ztcp",
];

/// Why a call asks or runs without asking. The code is the wire string in the events and in `test_approval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The session's approve-all flag skipped the card.
    ApproveAll,
    /// A saved allow rule matched.
    Rule,
    /// Manual mode: every call asks.
    ManualMode,
    /// `send_keys`: the first write to a command asks, and the approval covers that command's later writes (`Latched`).
    SendKeys,
    /// `send_keys` with only Ctrl-C, Ctrl-D or Esc: it can only interrupt or leave the command the agent started.
    Interrupt,
    /// `send_keys` into a command whose first write the user approved. The rules do not run again for it.
    Latched,
    /// `send_keys` that presses Enter in a shell the app did not integrate (ssh, a shell, su, a container shell): the rules
    /// cannot see what the shell will run, so each such Enter asks.
    UnintegratedShell,
    /// The hard gate: empty, or a character outside the safe set.
    GateChars,
    /// The hard gate: the first word does not start with a plain command name: it starts with something other than an
    /// ASCII letter or digit, or contains `=` or `/`. Covers paths, `VAR=value` prefixes and zsh `=ls` expansion.
    GatePath,
    /// The hard gate: the first word is on the built-in deny list, compared ASCII-lowercased.
    GateDenylist,
    /// A saved deny rule matched.
    DenyRule,
    /// No rule matched.
    NoRule,
    /// A rule would allow the call, but this turn already had its automatic approvals.
    AutoCap,
    /// A rule would allow the call, but the shell on top is a nested one (not the local machine): every command there asks.
    NestedLevel,
    /// Approve-all was switched on in another shell level, so it does not apply to this one.
    ApproveAllSuspended,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Reason::ApproveAll => "approve_all",
            Reason::Rule => "rule",
            Reason::ManualMode => "manual_mode",
            Reason::SendKeys => "send_keys",
            Reason::Interrupt => "interrupt",
            Reason::Latched => "latched",
            Reason::UnintegratedShell => "unintegrated_shell",
            Reason::GateChars => "gate_chars",
            Reason::GatePath => "gate_path",
            Reason::GateDenylist => "gate_denylist",
            Reason::DenyRule => "deny_rule",
            Reason::NoRule => "no_rule",
            Reason::AutoCap => "auto_cap",
            Reason::NestedLevel => "nested_level",
            Reason::ApproveAllSuspended => "approve_all_suspended",
        }
    }
}

/// The policy's answer for one call: run it without a card, or ask the user, with the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Auto(Reason),
    Ask(Reason),
}

/// Decides how a gated call is approved. Called once per call, in the order the calls run. A policy may keep a count
/// for its turn, which is why `decide` takes `&self`. `nested` is true when the shell on top is a nested level (see
/// `TopLevel` in core/tools): allow rules never run a command there without a card.
pub trait ApprovalPolicy: Send + Sync {
    fn decide(&self, tool: &str, args: &Value, nested: bool) -> Verdict;

    /// The floor for one line the agent types with `send_keys` (see `send_keys_verdict`): the hard gate, and the user's deny
    /// rules. None when the line may go on. Allow rules never apply to typed lines.
    fn check_line(&self, line: &str) -> Option<Reason> {
        hard_gate(line)
    }
}

/// Every call asks. The default, and the fallback when the settings cannot be used.
pub struct ManualPolicy;

impl ApprovalPolicy for ManualPolicy {
    fn decide(&self, tool: &str, _args: &Value, _nested: bool) -> Verdict {
        if tool == "run_command" { Verdict::Ask(Reason::ManualMode) } else { Verdict::Ask(Reason::SendKeys) }
    }
}

/// The hard gate and the saved rules. Holds the compiled rules and this turn's count of automatic approvals.
pub struct RegexPolicy {
    allow: Vec<Regex>,
    deny: Vec<Regex>,
    /// Automatic approvals given so far by this policy. A policy is built per user message, so this is per turn.
    auto_given: AtomicUsize,
}

impl RegexPolicy {
    /// Compiles the saved rules. Fails only when a rule does not compile, which settings validation already refuses.
    pub fn new(approval: &ApprovalSettings) -> Result<Self, String> {
        Ok(RegexPolicy {
            allow: compile_all(&approval.allow)?,
            deny: compile_all(&approval.deny)?,
            auto_given: AtomicUsize::new(0),
        })
    }

    /// Takes one automatic approval from this turn's allowance. False once the allowance is used up. The count keeps
    /// going past the cap, which is harmless: only the first `MAX_AUTO_PER_TURN` takes succeed.
    fn take_auto_slot(&self) -> bool {
        self.auto_given.fetch_add(1, Ordering::SeqCst) < MAX_AUTO_PER_TURN
    }
}

impl ApprovalPolicy for RegexPolicy {
    fn decide(&self, tool: &str, args: &Value, nested: bool) -> Verdict {
        match self.classify(tool, args, nested) {
            Verdict::Auto(Reason::Rule) if !self.take_auto_slot() => Verdict::Ask(Reason::AutoCap),
            verdict => verdict,
        }
    }

    fn check_line(&self, line: &str) -> Option<Reason> {
        self.line_floor(line)
    }
}

/// What `send_keys_verdict` needs to know about one call. The caller works these out from the call, the shell on top, the
/// approve-all state and the memory of earlier writes (`SendKeysMemory`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendKeysCall {
    /// Only Ctrl-C, Ctrl-D or Esc, with no text (`KeyPlan::is_interrupt_only`).
    pub interrupt_only: bool,
    /// The call presses Enter (`KeyPlan::has_enter`).
    pub has_enter: bool,
    /// The command on top is a shell or login the app did not integrate (ssh, a shell, su, a container shell).
    pub shell_like: bool,
    /// Approve-all covers this call. The caller decides that: for a shell_like command it needs follow_local_in_nested.
    pub approve_all: bool,
    /// The record's first write was approved, and that approval has not been revoked (`SendKeysMemory::latched`).
    pub latched: bool,
    /// The line each Enter of the call submits, in order: None where the line is not known (`TypedLine::after`).
    pub lines: Vec<Option<String>>,
}

/// The verdict for one `send_keys` call. Only the order matters, and the first rule that decides wins:
/// 1. Ctrl-C, Ctrl-D or Esc alone (`Interrupt`), in every mode.
/// 2. Approve-all, when it covers the call (`ApproveAll`). It skips the line floor, as it does for run_command.
/// 3. An Enter in an un-integrated shell asks (`UnintegratedShell`), even when the call is latched.
/// 4. A line the floor refuses asks with the floor's reason; a line that is not known asks (`SendKeys`).
/// 5. A latched record runs without a card (`Latched`). Otherwise the first write asks (`SendKeys`).
///
/// Allow rules play no part: they never run a typed line by themselves.
pub fn send_keys_verdict(policy: &dyn ApprovalPolicy, call: &SendKeysCall) -> Verdict {
    if call.interrupt_only {
        return Verdict::Auto(Reason::Interrupt);
    }
    if call.approve_all {
        return Verdict::Auto(Reason::ApproveAll);
    }
    if call.shell_like && call.has_enter {
        return Verdict::Ask(Reason::UnintegratedShell);
    }
    for line in &call.lines {
        match line {
            None => return Verdict::Ask(Reason::SendKeys),
            // A blank line runs nothing (in a REPL or a pager it is just Enter), so the floor has nothing to judge.
            Some(line) if !line.trim().is_empty() => {
                if let Some(reason) = policy.check_line(line) {
                    return Verdict::Ask(reason);
                }
            }
            Some(_) => {}
        }
    }
    if call.latched { Verdict::Auto(Reason::Latched) } else { Verdict::Ask(Reason::SendKeys) }
}

/// What the agent may type into one command without a new card: the latch (the first write's approval) and the line typed so
/// far, both per record. The approval gate keeps one of these for the session.
///
/// A user keystroke revokes the latch: the user may have taken the terminal over. The next `send_keys` is then refused once,
/// and the one after it asks again. The line is unknown after a user keystroke, since their typing is not seen here.
#[derive(Debug, Default)]
pub struct SendKeysMemory {
    latch: Option<Latch>,
    line_record: Option<u64>,
    line: TypedLine,
    /// The user's keystroke count when this memory last looked.
    seen_user_writes: u64,
}

/// A first write's approval: the record it was given for, and the user's keystroke count at the time.
#[derive(Debug, Clone, Copy)]
struct Latch {
    record: u64,
    user_writes: u64,
}

impl SendKeysMemory {
    /// Called before each `send_keys` decision, with the record it targets and the user's keystroke count. Returns true when
    /// the user typed since the latch was approved: the latch is revoked, and the caller refuses this call.
    pub fn observe(&mut self, record: u64, user_writes: u64) -> bool {
        if self.line_record != Some(record) {
            self.line_record = Some(record);
            self.line = TypedLine::default();
        }
        if user_writes != self.seen_user_writes {
            self.seen_user_writes = user_writes;
            self.line.mark_unknown();
        }
        match self.latch {
            Some(latch) if latch.user_writes != user_writes => {
                self.latch = None;
                true
            }
            _ => false,
        }
    }

    /// True when the record's first write was approved, and nothing has revoked it since.
    pub fn latched(&self, record: u64) -> bool {
        self.latch.is_some_and(|latch| latch.record == record)
    }

    /// The line typed at the record so far, as far as it is known (`TypedLine::after` takes it).
    pub fn line(&self, record: u64) -> TypedLine {
        if self.line_record == Some(record) { self.line.clone() } else { TypedLine::default() }
    }

    /// Records a first write the user approved. `user_writes` is the keystroke count read before the card was shown.
    pub fn approve_first_write(&mut self, record: u64, user_writes: u64) {
        self.latch = Some(Latch { record, user_writes });
    }

    /// Records the line after a `send_keys` that was sent to the record.
    pub fn sent(&mut self, record: u64, line: TypedLine) {
        self.line_record = Some(record);
        self.line = line;
    }
}

impl RegexPolicy {
    /// The same floor as `check_line` on the trait: the hard gate, then the deny rules. Used for typed lines.
    fn line_floor(&self, line: &str) -> Option<Reason> {
        hard_gate(line).or_else(|| self.deny.iter().any(|rule| rule.is_match(line)).then_some(Reason::DenyRule))
    }

    /// The order of evaluation, first hit wins: not `run_command`; the hard gate; a deny rule; an allow rule (asks in a
    /// nested shell); else ask. Says `Auto` only for an allow rule. The turn's allowance is applied by `decide`.
    fn classify(&self, tool: &str, args: &Value, nested: bool) -> Verdict {
        if tool != "run_command" {
            return Verdict::Ask(Reason::SendKeys);
        }
        // A missing or non-string command is treated as empty, which the gate refuses.
        let command = args.get("command").and_then(Value::as_str).unwrap_or_default();
        if let Some(reason) = hard_gate(command) {
            return Verdict::Ask(reason);
        }
        // Rules match the whole command as it will run.
        if self.deny.iter().any(|rule| rule.is_match(command)) {
            return Verdict::Ask(Reason::DenyRule);
        }
        if self.allow.iter().any(|rule| rule.is_match(command)) {
            return if nested { Verdict::Ask(Reason::NestedLevel) } else { Verdict::Auto(Reason::Rule) };
        }
        Verdict::Ask(Reason::NoRule)
    }
}

/// The floor that no user rule can override. `None` when the command may go on to the rules.
fn hard_gate(command: &str) -> Option<Reason> {
    if command.trim().is_empty() || !command.chars().all(is_safe_char) {
        return Some(Reason::GateChars);
    }
    // The charset allows only spaces, so this is the first word. It must be a plain command name: an assignment prefix
    // (`FOO=1 rm`), an equals expansion (`=ls`), a path (`./x`) or a leading `-` would hide the program from the list.
    let first = command.split_whitespace().next().unwrap_or_default();
    if !first.starts_with(|c: char| c.is_ascii_alphanumeric()) || first.contains(['=', '/']) {
        return Some(Reason::GatePath);
    }
    // macOS file systems are case-insensitive, so `RM` runs `rm`.
    if BUILTIN_DENY.binary_search(&first.to_ascii_lowercase().as_str()).is_ok() {
        return Some(Reason::GateDenylist);
    }
    None
}

fn is_safe_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || SAFE_PUNCTUATION.contains(c)
}

/// The policy the saved settings describe. Manual mode asks for everything. Regex mode compiles the rules.
pub fn policy_for(approval: &ApprovalSettings) -> Result<Box<dyn ApprovalPolicy>, String> {
    match approval.mode {
        ApprovalMode::Manual => Ok(Box::new(ManualPolicy)),
        ApprovalMode::Regex => Ok(Box::new(RegexPolicy::new(approval)?)),
    }
}

/// The verdict for `command` under the saved settings, without running it. Approve-all is not part of it. Uses a new
/// policy, so the per-turn allowance is not used up by the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Preview {
    /// "auto" or "ask".
    pub verdict: &'static str,
    pub reason: &'static str,
}

pub fn preview(approval: &ApprovalSettings, command: &str) -> Result<Preview, String> {
    let verdict = policy_for(approval)?.decide("run_command", &json!({"command": command}), false);
    Ok(match verdict {
        Verdict::Auto(reason) => Preview { verdict: "auto", reason: reason.code() },
        Verdict::Ask(reason) => Preview { verdict: "ask", reason: reason.code() },
    })
}

/// Problems with the saved approval rules, one string each. Names the list and the rule number. Never echoes a rule.
pub fn validate_settings(approval: &ApprovalSettings) -> Vec<String> {
    let mut problems = Vec::new();
    validate_list("allow", &approval.allow, &mut problems);
    validate_list("deny", &approval.deny, &mut problems);
    problems
}

fn validate_list(list: &str, rules: &[String], problems: &mut Vec<String>) {
    if rules.len() > MAX_RULES {
        problems.push(format!("approval {list} list must have at most {MAX_RULES} rules"));
    }
    for (index, rule) in rules.iter().enumerate() {
        let number = index + 1;
        if rule.trim().is_empty() {
            problems.push(format!("approval {list} rule {number} must not be empty"));
        } else if rule.chars().count() > MAX_RULE_CHARS {
            problems.push(format!("approval {list} rule {number} must be at most {MAX_RULE_CHARS} characters"));
        } else if rule.chars().any(char::is_control) {
            problems.push(format!("approval {list} rule {number} must not contain a line break or control characters"));
        } else if compile_rule(rule).is_err() {
            problems.push(format!("approval {list} rule {number} is not a valid pattern"));
        }
    }
}

fn compile_all(rules: &[String]) -> Result<Vec<Regex>, String> {
    rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            compile_rule(rule).map_err(|_| format!("approval rule {} is not a valid pattern", index + 1))
        })
        .collect()
}

/// A rule compiles as the whole command: `^(?:rule)$`. The rule must also compile on its own, because an unbalanced
/// `)` or `(` could otherwise close the wrapper early and turn the match into a partial one.
fn compile_rule(rule: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(rule).size_limit(REGEX_SIZE_LIMIT).build()?;
    RegexBuilder::new(&format!("^(?:{rule})$")).size_limit(REGEX_SIZE_LIMIT).build()
}

/// The built-in deny list, for the settings UI.
pub fn builtin_deny() -> &'static [&'static str] {
    BUILTIN_DENY
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: ApprovalMode, allow: &[&str], deny: &[&str]) -> ApprovalSettings {
        ApprovalSettings {
            mode,
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
            nested: crate::core::settings::Nested::Ask,
        }
    }

    fn regex_policy(allow: &[&str], deny: &[&str]) -> RegexPolicy {
        RegexPolicy::new(&settings(ApprovalMode::Regex, allow, deny)).expect("rules compile")
    }

    fn run(policy: &dyn ApprovalPolicy, command: &str) -> Verdict {
        policy.decide("run_command", &json!({"command": command}), false)
    }

    /// The same, in a nested shell.
    fn run_nested(policy: &dyn ApprovalPolicy, command: &str) -> Verdict {
        policy.decide("run_command", &json!({"command": command}), true)
    }

    /// The hard gate's answer for a command.
    fn gate(command: &str) -> Option<Reason> {
        hard_gate(command)
    }

    #[test]
    fn builtin_deny_is_sorted_and_unique() {
        for pair in BUILTIN_DENY.windows(2) {
            assert!(pair[0] < pair[1], "not sorted or repeated: {:?} then {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn plain_commands_pass_the_hard_gate() {
        let commands = [
            "git status",
            "ls -la /tmp",
            "ls -la",
            "cat README.md",
            "cargo test --lib",
            "echo hi",
            "ls rm",
            "echo sudo",
        ];
        for command in commands {
            assert_eq!(gate(command), None, "{command:?} should pass the gate");
        }
    }

    #[test]
    fn every_forbidden_character_class_is_refused() {
        let cases = [
            "ls | wc",
            "ls; pwd",
            "ls && pwd",
            "ls > out",
            "ls < in",
            "(ls)",
            "{ ls; }",
            "ls [a]",
            "ls *",
            "ls ?",
            "ls !x",
            "ls # c",
            "ls ~",
            "echo $HOME",
            "echo `pwd`",
            "echo \"a\"",
            "echo 'a'",
            "ls \\ x",
            "ls\nrm",
            "ls\tx",
            "ls\r",
            "ls\u{a0}-l",
            "\u{ff4c}\u{ff53}",
            "echo caf\u{e9}",
            "echo hi\u{200b}",
        ];
        for command in cases {
            assert_eq!(gate(command), Some(Reason::GateChars), "{command:?}");
        }
    }

    #[test]
    fn empty_and_blank_commands_are_refused_as_chars() {
        for command in ["", "   "] {
            assert_eq!(gate(command), Some(Reason::GateChars), "{command:?}");
        }
    }

    #[test]
    fn a_path_as_the_first_word_is_refused() {
        for command in ["./x", "../x", "/bin/ls", "bin/ls -l", "./x --flag"] {
            assert_eq!(gate(command), Some(Reason::GatePath), "{command:?}");
        }
    }

    #[test]
    fn an_assignment_or_equals_expansion_as_the_first_word_is_refused() {
        for command in ["a=b", "FOO=1 rm x", "X=1 /bin/ls", "X=1 sudo ls", "=ls", "=rm x", "a=b=c"] {
            assert_eq!(gate(command), Some(Reason::GatePath), "{command:?}");
        }
    }

    #[test]
    fn a_first_word_that_does_not_start_with_a_letter_or_digit_is_refused() {
        for command in ["-", "-la", "--help", "_x", ".hidden", "+1"] {
            assert_eq!(gate(command), Some(Reason::GatePath), "{command:?}");
        }
    }

    #[test]
    fn each_deny_list_word_is_refused_as_the_first_word() {
        for word in BUILTIN_DENY {
            // A word that does not start with a letter or digit (only `.` is listed) fails the path rule first.
            let expected = if word.starts_with(|c: char| c.is_ascii_alphanumeric()) {
                Reason::GateDenylist
            } else {
                Reason::GatePath
            };
            assert_eq!(gate(&format!("{word} x")), Some(expected), "{word}");
            assert_eq!(gate(word), Some(expected), "{word} alone");
        }
    }

    #[test]
    fn the_deny_list_ignores_ascii_case() {
        assert_eq!(gate("RM victim"), Some(Reason::GateDenylist));
        assert_eq!(gate("SUDO ls"), Some(Reason::GateDenylist));
        assert_eq!(gate("Rm x"), Some(Reason::GateDenylist));
        assert_eq!(gate("Git status"), None, "only the deny list is case-insensitive");
    }

    #[test]
    fn zsh_precommand_modifiers_and_reserved_words_are_refused() {
        assert_eq!(gate("repeat 1 rm x"), Some(Reason::GateDenylist));
        assert_eq!(gate("noglob ls"), Some(Reason::GateDenylist));
        assert_eq!(gate("nocorrect ls"), Some(Reason::GateDenylist));
        assert_eq!(gate("coproc ls"), Some(Reason::GateDenylist));
        assert_eq!(gate("until false"), Some(Reason::GateDenylist));
        assert_eq!(gate("export X"), Some(Reason::GateDenylist));
        assert_eq!(gate("unset HOME"), Some(Reason::GateDenylist));
    }

    #[test]
    fn a_deny_list_word_that_is_not_first_is_fine() {
        assert_eq!(gate("ls rm"), None);
        assert_eq!(gate("git log --grep sudo"), None);
    }

    #[test]
    fn the_hard_gate_wins_over_an_allow_rule() {
        let policy = regex_policy(&[".*"], &[]);
        assert_eq!(run(&policy, "ls | wc"), Verdict::Ask(Reason::GateChars));
        assert_eq!(run(&policy, "./x"), Verdict::Ask(Reason::GatePath));
        assert_eq!(run(&policy, "sudo ls"), Verdict::Ask(Reason::GateDenylist));
    }

    #[test]
    fn a_deny_rule_wins_over_an_allow_rule() {
        let policy = regex_policy(&[".*"], &["cargo .*"]);
        assert_eq!(run(&policy, "cargo test"), Verdict::Ask(Reason::DenyRule));
        assert_eq!(run(&policy, "git status"), Verdict::Auto(Reason::Rule));
    }

    #[test]
    fn rules_match_the_whole_command_only() {
        let policy = regex_policy(&["git status"], &[]);
        assert_eq!(run(&policy, "git status"), Verdict::Auto(Reason::Rule));
        assert_eq!(run(&policy, "git status -s"), Verdict::Ask(Reason::NoRule));
        assert_eq!(run(&policy, "xgit status"), Verdict::Ask(Reason::NoRule));
        assert_eq!(run(&policy, " git status"), Verdict::Ask(Reason::NoRule), "no trimming before matching");
    }

    #[test]
    fn an_alternation_rule_is_still_anchored() {
        let policy = regex_policy(&["a|b"], &[]);
        assert_eq!(run(&policy, "a"), Verdict::Auto(Reason::Rule));
        assert_eq!(run(&policy, "b"), Verdict::Auto(Reason::Rule));
        assert_eq!(run(&policy, "ab"), Verdict::Ask(Reason::NoRule));
        assert_eq!(run(&policy, "xa"), Verdict::Ask(Reason::NoRule));
    }

    #[test]
    fn a_rule_with_an_unbalanced_paren_is_refused_so_it_cannot_escape_the_wrapper() {
        assert!(compile_rule("a)|(b").is_err());
        assert!(RegexPolicy::new(&settings(ApprovalMode::Regex, &["a)|(b"], &[])).is_err());
    }

    /// In a nested shell an allow rule asks instead, with its own reason. Nothing else changes: the gate and the deny
    /// rules give the reasons they give anywhere, and a call that asks there does not use up the turn's allowance.
    #[test]
    fn an_allow_rule_asks_in_a_nested_shell_and_the_rest_is_unchanged() {
        let policy = regex_policy(&["echo .*", "cargo test"], &["cargo .*"]);
        assert_eq!(run(&policy, "echo hi"), Verdict::Auto(Reason::Rule));
        assert_eq!(run_nested(&policy, "echo hi"), Verdict::Ask(Reason::NestedLevel));
        assert_eq!(run_nested(&policy, "ls | wc"), Verdict::Ask(Reason::GateChars));
        assert_eq!(run_nested(&policy, "cargo test"), Verdict::Ask(Reason::DenyRule), "a deny rule still wins");
        assert_eq!(run_nested(&policy, "git status"), Verdict::Ask(Reason::NoRule));
        for n in 0..(MAX_AUTO_PER_TURN * 2) {
            assert_eq!(run_nested(&policy, &format!("echo {n}")), Verdict::Ask(Reason::NestedLevel));
        }
        assert_eq!(run(&policy, "echo local"), Verdict::Auto(Reason::Rule), "nested asks used up no allowance");
    }

    #[test]
    fn manual_mode_asks_for_every_command_and_send_keys() {
        let policy = ManualPolicy;
        assert_eq!(run(&policy, "git status"), Verdict::Ask(Reason::ManualMode));
        assert_eq!(policy.decide("send_keys", &json!({"bytes": [13]}), false), Verdict::Ask(Reason::SendKeys));
    }

    #[test]
    fn send_keys_always_asks_under_a_regex_policy() {
        let policy = regex_policy(&[".*"], &[]);
        assert_eq!(policy.decide("send_keys", &json!({"bytes": [13]}), false), Verdict::Ask(Reason::SendKeys));
    }

    #[test]
    fn a_missing_command_is_refused_as_chars() {
        let policy = regex_policy(&[".*"], &[]);
        assert_eq!(policy.decide("run_command", &json!({}), false), Verdict::Ask(Reason::GateChars));
        assert_eq!(policy.decide("run_command", &json!({"command": 5}), false), Verdict::Ask(Reason::GateChars));
    }

    #[test]
    fn no_matching_rule_asks() {
        let policy = regex_policy(&["git status"], &[]);
        assert_eq!(run(&policy, "git log"), Verdict::Ask(Reason::NoRule));
    }

    #[test]
    fn the_turn_allows_eight_automatic_approvals_and_then_asks() {
        let policy = regex_policy(&["echo .*"], &[]);
        for n in 1..=MAX_AUTO_PER_TURN {
            assert_eq!(run(&policy, &format!("echo {n}")), Verdict::Auto(Reason::Rule), "approval {n}");
        }
        assert_eq!(run(&policy, "echo 9"), Verdict::Ask(Reason::AutoCap));
        assert_eq!(run(&policy, "echo 10"), Verdict::Ask(Reason::AutoCap), "the cap stays reached");
    }

    #[test]
    fn calls_that_ask_do_not_use_up_the_allowance() {
        let policy = regex_policy(&["echo .*"], &[]);
        for _ in 0..20 {
            assert_eq!(run(&policy, "ls | wc"), Verdict::Ask(Reason::GateChars));
            assert_eq!(run(&policy, "cargo test"), Verdict::Ask(Reason::NoRule));
        }
        assert_eq!(run(&policy, "echo ok"), Verdict::Auto(Reason::Rule));
    }

    #[test]
    fn a_new_policy_starts_a_new_allowance() {
        let first = regex_policy(&["echo .*"], &[]);
        for _ in 0..MAX_AUTO_PER_TURN {
            run(&first, "echo x");
        }
        assert_eq!(run(&first, "echo x"), Verdict::Ask(Reason::AutoCap));
        let next_message = regex_policy(&["echo .*"], &[]);
        assert_eq!(run(&next_message, "echo x"), Verdict::Auto(Reason::Rule));
    }

    #[test]
    fn reason_codes_are_the_wire_strings() {
        let codes = [
            (Reason::ApproveAll, "approve_all"),
            (Reason::Rule, "rule"),
            (Reason::ManualMode, "manual_mode"),
            (Reason::SendKeys, "send_keys"),
            (Reason::GateChars, "gate_chars"),
            (Reason::GatePath, "gate_path"),
            (Reason::GateDenylist, "gate_denylist"),
            (Reason::DenyRule, "deny_rule"),
            (Reason::NoRule, "no_rule"),
            (Reason::AutoCap, "auto_cap"),
            (Reason::NestedLevel, "nested_level"),
            (Reason::ApproveAllSuspended, "approve_all_suspended"),
            (Reason::Interrupt, "interrupt"),
            (Reason::Latched, "latched"),
            (Reason::UnintegratedShell, "unintegrated_shell"),
        ];
        for (reason, code) in codes {
            assert_eq!(reason.code(), code);
        }
    }

    /// A send_keys call of a command on a local shell, with nothing latched, no approve-all, and no line.
    fn keys_call() -> SendKeysCall {
        SendKeysCall {
            interrupt_only: false,
            has_enter: false,
            shell_like: false,
            approve_all: false,
            latched: false,
            lines: Vec::new(),
        }
    }

    fn keys(policy: &dyn ApprovalPolicy, call: SendKeysCall) -> Verdict {
        send_keys_verdict(policy, &call)
    }

    /// Ctrl-C alone is an interrupt in every mode, and it is the only thing that is.
    #[test]
    fn interrupt_only_is_auto_in_every_mode() {
        let call = SendKeysCall { interrupt_only: true, ..keys_call() };
        assert_eq!(keys(&ManualPolicy, call.clone()), Verdict::Auto(Reason::Interrupt));
        assert_eq!(keys(&regex_policy(&[], &[]), call), Verdict::Auto(Reason::Interrupt));
    }

    /// The first write to a command asks, and the latch then lets the rest through. Allow rules do not run typed lines.
    #[test]
    fn the_first_write_asks_and_the_latch_covers_the_rest() {
        let policy = regex_policy(&[".*"], &[]);
        assert_eq!(keys(&policy, keys_call()), Verdict::Ask(Reason::SendKeys));
        let latched = SendKeysCall { latched: true, ..keys_call() };
        assert_eq!(keys(&policy, latched.clone()), Verdict::Auto(Reason::Latched));
        let enter = SendKeysCall { has_enter: true, lines: vec![Some("ls -l".into())], ..latched };
        assert_eq!(keys(&policy, enter), Verdict::Auto(Reason::Latched), "an allow rule is not needed, and a plain line passes");
    }

    /// Approve-all covers a call that the policy would otherwise ask about, and skips the line floor as run_command does.
    #[test]
    fn approve_all_covers_the_call_and_skips_the_line_floor() {
        let policy = regex_policy(&[], &[]);
        let call = SendKeysCall { approve_all: true, has_enter: true, lines: vec![Some("sudo ls".into())], ..keys_call() };
        assert_eq!(keys(&policy, call), Verdict::Auto(Reason::ApproveAll));
    }

    /// Enter in an un-integrated shell asks every time, even when latched.
    #[test]
    fn an_enter_in_an_unintegrated_shell_asks_even_when_latched() {
        let call = SendKeysCall { shell_like: true, has_enter: true, latched: true, lines: vec![Some("ls".into())], ..keys_call() };
        assert_eq!(keys(&ManualPolicy, call.clone()), Verdict::Ask(Reason::UnintegratedShell));
        let no_enter = SendKeysCall { has_enter: false, lines: Vec::new(), ..call };
        assert_eq!(keys(&ManualPolicy, no_enter), Verdict::Auto(Reason::Latched), "without Enter the latch rule applies");
    }

    /// The floor judges each typed line: a deny rule and the hard gate refuse with their reasons, in regex mode, and the hard
    /// gate alone applies in manual mode. A blank line is nothing to judge.
    #[test]
    fn the_floor_judges_each_typed_line() {
        let policy = RegexPolicy::new(&ApprovalSettings {
            mode: ApprovalMode::Regex,
            allow: Vec::new(),
            deny: vec!["cargo .*".to_string()],
            ..Default::default()
        })
        .expect("the rules compile");
        let line = |text: &str| SendKeysCall { has_enter: true, latched: true, lines: vec![Some(text.to_string())], ..keys_call() };
        assert_eq!(keys(&policy, line("cargo test")), Verdict::Ask(Reason::DenyRule));
        assert_eq!(keys(&policy, line("rm x")), Verdict::Ask(Reason::GateDenylist), "the built-in list comes first");
        assert_eq!(keys(&policy, line("sudo x")), Verdict::Ask(Reason::GateDenylist));
        assert_eq!(keys(&policy, line("ls | wc")), Verdict::Ask(Reason::GateChars));
        assert_eq!(keys(&policy, line("ls")), Verdict::Auto(Reason::Latched));
        assert_eq!(keys(&ManualPolicy, line("cargo test")), Verdict::Auto(Reason::Latched), "manual mode has no deny rules");
        assert_eq!(keys(&ManualPolicy, line("sudo x")), Verdict::Ask(Reason::GateDenylist), "the hard gate applies in manual mode");
        assert_eq!(keys(&policy, line("   ")), Verdict::Auto(Reason::Latched), "a blank line is nothing to judge");
    }

    /// A line that is not known asks, latched or not: the floor cannot judge it.
    #[test]
    fn an_unknown_line_asks() {
        let call = SendKeysCall { has_enter: true, latched: true, lines: vec![None], ..keys_call() };
        assert_eq!(keys(&ManualPolicy, call), Verdict::Ask(Reason::SendKeys));
    }

    /// The latch belongs to one command, and a user keystroke after the approval revokes it once.
    #[test]
    fn the_memory_latches_per_command_and_a_keystroke_revokes_it_once() {
        let mut memory = SendKeysMemory::default();
        assert!(!memory.observe(7, 0));
        assert!(!memory.latched(7));
        memory.approve_first_write(7, 0);
        assert!(memory.latched(7) && !memory.latched(8), "the latch is for command 7");
        assert!(!memory.observe(7, 0), "nothing typed, nothing revoked");
        assert!(memory.observe(7, 1), "a keystroke after the approval revokes it");
        assert!(!memory.latched(7));
        assert!(!memory.observe(7, 1), "the revocation is not repeated");
    }

    /// The typed line follows each sent call per command, and a keystroke makes it unknown.
    #[test]
    fn the_memory_follows_the_typed_line_and_a_keystroke_makes_it_unknown() {
        let mut memory = SendKeysMemory::default();
        memory.observe(7, 0);
        let plan = crate::core::keys::KeyPlan { text: "rm -rf".into(), ..Default::default() };
        let (lines, next) = memory.line(7).after(&plan);
        assert!(lines.is_empty());
        memory.sent(7, next);
        memory.observe(7, 0);
        let enter = crate::core::keys::KeyPlan { keys: vec![crate::core::keys::Key::Enter], ..Default::default() };
        assert_eq!(memory.line(7).after(&enter).0, [Some("rm -rf".to_string())], "the line sent before is remembered");
        memory.observe(7, 1);
        assert_eq!(memory.line(7).after(&enter).0, [None], "a keystroke makes the line unknown");
        assert_eq!(memory.line(8).after(&enter).0, [Some(String::new())], "another command starts with an empty line");
    }

    #[test]
    fn valid_rules_pass_validation() {
        let allow = ["git status", "a|b", "(?i)cargo test( .*)?", "echo [a-z]+"];
        let ok = settings(ApprovalMode::Regex, &allow, &["rm .*"]);
        assert!(validate_settings(&ok).is_empty());
    }

    #[test]
    fn invalid_rules_name_their_list_and_number() {
        let cases: [(ApprovalSettings, &str); 6] = [
            (settings(ApprovalMode::Regex, &["git status", "a("], &[]), "approval allow rule 2 is not a valid pattern"),
            (settings(ApprovalMode::Regex, &[], &["a)|(b"]), "approval deny rule 1 is not a valid pattern"),
            (settings(ApprovalMode::Regex, &["ls\nrm"], &[]), "approval allow rule 1 must not contain a line break"),
            (settings(ApprovalMode::Regex, &["ls\tx"], &[]), "approval allow rule 1 must not contain a line break"),
            (settings(ApprovalMode::Regex, &[""], &[]), "approval allow rule 1 must not be empty"),
            (settings(ApprovalMode::Regex, &[], &["  "]), "approval deny rule 1 must not be empty"),
        ];
        for (approval, fragment) in cases {
            let problems = validate_settings(&approval);
            assert!(problems.iter().any(|p| p.contains(fragment)), "{fragment}: {problems:?}");
        }
    }

    #[test]
    fn a_rule_longer_than_the_limit_is_refused() {
        let long = "a".repeat(MAX_RULE_CHARS + 1);
        let problems = validate_settings(&settings(ApprovalMode::Regex, &[&long], &[]));
        let fragment = "approval allow rule 1 must be at most 200 characters";
        assert!(problems.iter().any(|p| p.contains(fragment)), "{problems:?}");
        let exact = "a".repeat(MAX_RULE_CHARS);
        assert!(validate_settings(&settings(ApprovalMode::Regex, &[&exact], &[])).is_empty());
    }

    #[test]
    fn too_many_rules_are_refused() {
        let many: Vec<String> = (0..=MAX_RULES).map(|n| format!("cmd{n}")).collect();
        let approval = ApprovalSettings { mode: ApprovalMode::Regex, allow: many, ..ApprovalSettings::default() };
        let problems = validate_settings(&approval);
        assert!(problems.iter().any(|p| p.contains("at most 50 rules")), "{problems:?}");
        let at_limit = ApprovalSettings {
            mode: ApprovalMode::Regex,
            allow: (0..MAX_RULES).map(|n| format!("cmd{n}")).collect(),
            ..ApprovalSettings::default()
        };
        assert!(validate_settings(&at_limit).is_empty());
    }

    #[test]
    fn the_preview_reports_the_verdict_without_using_a_turn() {
        let saved = settings(ApprovalMode::Regex, &["echo .*"], &[]);
        assert_eq!(preview(&saved, "echo hi"), Ok(Preview { verdict: "auto", reason: "rule" }));
        assert_eq!(preview(&saved, "ls | wc"), Ok(Preview { verdict: "ask", reason: "gate_chars" }));
        assert_eq!(preview(&saved, "git status"), Ok(Preview { verdict: "ask", reason: "no_rule" }));
        let manual = settings(ApprovalMode::Manual, &["echo .*"], &[]);
        assert_eq!(preview(&manual, "echo hi"), Ok(Preview { verdict: "ask", reason: "manual_mode" }));
    }

    #[test]
    fn policy_for_builds_the_policy_the_mode_names() {
        let manual = policy_for(&settings(ApprovalMode::Manual, &["echo .*"], &[])).unwrap();
        assert_eq!(run(manual.as_ref(), "echo hi"), Verdict::Ask(Reason::ManualMode));
        let regex = policy_for(&settings(ApprovalMode::Regex, &["echo .*"], &[])).unwrap();
        assert_eq!(run(regex.as_ref(), "echo hi"), Verdict::Auto(Reason::Rule));
        assert!(policy_for(&settings(ApprovalMode::Regex, &["a("], &[])).is_err());
    }
}
