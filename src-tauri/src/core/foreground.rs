//! The foreground program hint: what the open command line says is running right now.
//!
//! While a command runs, the observer knows its command text from our own shell hook (the E event of the open record).
//! This module turns that text into a kind: a remote login (ssh), a nested shell, su, a shell in a container, a terminal
//! multiplexer, or anything else. A remote login or a nested shell has no shell integration, so the agent cannot run
//! commands in it. The hint lets the app say so.
//!
//! It is a HINT, never a safety decision. Nothing that approves or refuses a command reads it. It is guessed from the
//! words, so an alias or a script can hide what really runs.

use serde::{Deserialize, Serialize};

/// The stored command text is cut to this many characters.
pub const MAX_COMMAND_CHARS: usize = 1000;

/// What kind of program the hint says is in the foreground. The codes are the wire names (snake_case).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForegroundKind {
    /// ssh or mosh: a shell on another machine.
    Ssh,
    /// A shell by name (zsh, bash, ...): a nested shell.
    Shell,
    /// su, or sudo with -i or -s and nothing to run: a shell as another user.
    Su,
    /// `docker|podman|kubectl exec`, or `docker|podman run` with -i and -t: a shell or program in a container.
    ContainerExec,
    /// tmux or screen.
    Multiplexer,
    /// Anything else. The command still runs in the shell, so the hint only names it.
    Other,
}

impl ForegroundKind {
    /// The stable wire code. Same as the serialized form.
    pub fn code(self) -> &'static str {
        match self {
            ForegroundKind::Ssh => "ssh",
            ForegroundKind::Shell => "shell",
            ForegroundKind::Su => "su",
            ForegroundKind::ContainerExec => "container_exec",
            ForegroundKind::Multiplexer => "multiplexer",
            ForegroundKind::Other => "other",
        }
    }

    /// A short noun phrase for the agent and the user, as in "a shell on another machine, without shell integration".
    pub fn describe(self) -> &'static str {
        match self {
            ForegroundKind::Ssh => "a shell on another machine",
            ForegroundKind::Shell => "a nested shell",
            ForegroundKind::Su => "a shell as another user",
            ForegroundKind::ContainerExec => "a shell or program in a container",
            ForegroundKind::Multiplexer => "a terminal multiplexer",
            ForegroundKind::Other => "a program",
        }
    }
}

/// The hint for the program that is running: its kind, and its command text as one line (control characters replaced by
/// spaces, at most `MAX_COMMAND_CHARS` characters).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Foreground {
    pub kind: ForegroundKind,
    pub command: String,
}

/// Wrappers that run the words after them as the same program. Skipped before the program name is read.
const WRAPPERS: [&str; 7] = ["command", "exec", "builtin", "nohup", "time", "noglob", "env"];
/// Shells recognised by name. A path such as `/bin/zsh` counts by its last part.
const SHELLS: [&str; 9] = ["zsh", "bash", "sh", "fish", "dash", "ksh", "tcsh", "csh", "nu"];
/// sudo short flags that take a value (`-u root`, `-iu root`). The value is skipped, so it is not read as the program.
const SUDO_VALUE_LETTERS: [char; 9] = ['u', 'g', 'p', 'r', 't', 'U', 'C', 'D', 'T'];

/// The hint for one command line. Input is the local command text of the open record; it is trusted as text, but the kind
/// is only a guess from the words. Compound lines (`;`, `&`, `|`) are checked segment by segment, and the first segment
/// that names a program wins.
pub fn classify(command: &str) -> Foreground {
    let line = one_line(command);
    let kind = line
        .split([';', '&', '|'])
        .find_map(|segment| {
            let words: Vec<&str> = segment.split_whitespace().collect();
            kind_of_words(&words)
        })
        .unwrap_or(ForegroundKind::Other);
    Foreground { kind, command: line.chars().take(MAX_COMMAND_CHARS).collect() }
}

/// Control characters become spaces (so they cannot glue two words together), and runs of whitespace collapse to one space.
fn one_line(command: &str) -> String {
    let spaced: String = command.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The kind of the program these words start, after skipping assignments and wrappers. None when the program is not one
/// of the recognised kinds.
fn kind_of_words(words: &[&str]) -> Option<ForegroundKind> {
    let mut rest = words;
    while let Some((first, tail)) = rest.split_first() {
        if is_assignment(first) || WRAPPERS.contains(first) {
            rest = tail;
        } else {
            break;
        }
    }
    let (program, args) = rest.split_first()?;
    match basename(program) {
        "ssh" | "mosh" => Some(ForegroundKind::Ssh),
        name if SHELLS.contains(&name) => Some(ForegroundKind::Shell),
        "su" => Some(ForegroundKind::Su),
        "sudo" => sudo_kind(args),
        "docker" | "podman" => docker_kind(args),
        "kubectl" => (args.first() == Some(&"exec")).then_some(ForegroundKind::ContainerExec),
        "tmux" | "screen" => Some(ForegroundKind::Multiplexer),
        _ => None,
    }
}

/// sudo's flags come first. `-i`, `-s`, `--login` or `--shell` with nothing to run is a shell as another user. `sudo su` is
/// too. Otherwise the words after the flags are classified as the command sudo runs.
fn sudo_kind(args: &[&str]) -> Option<ForegroundKind> {
    let mut login = false;
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if matches!(*arg, "--login" | "--shell") {
            login = true;
        } else if let Some(flags) = arg.strip_prefix('-').filter(|f| !f.starts_with('-')) {
            login |= flags.contains(['i', 's']);
            if flags.contains(SUDO_VALUE_LETTERS) {
                i += 1;
            }
        } else {
            break;
        }
        i += 1;
    }
    let rest = &args[i.min(args.len())..];
    match rest.first() {
        None => login.then_some(ForegroundKind::Su),
        Some(first) if basename(first) == "su" => Some(ForegroundKind::Su),
        Some(_) => kind_of_words(rest),
    }
}

/// `docker|podman exec`, and `docker|podman run` with both -i and -t (as -it, -ti, -i -t, --interactive --tty).
fn docker_kind(args: &[&str]) -> Option<ForegroundKind> {
    let sub_at = args.iter().position(|a| !a.starts_with('-'))?;
    match args[sub_at] {
        "exec" => Some(ForegroundKind::ContainerExec),
        "run" => has_interactive_and_tty(&args[sub_at + 1..]).then_some(ForegroundKind::ContainerExec),
        _ => None,
    }
}

fn has_interactive_and_tty(words: &[&str]) -> bool {
    let (mut interactive, mut tty) = (false, false);
    for word in words {
        match *word {
            "--interactive" => interactive = true,
            "--tty" => tty = true,
            _ => {
                if let Some(cluster) = word.strip_prefix('-').filter(|c| !c.starts_with('-')) {
                    interactive |= cluster.contains('i');
                    tty |= cluster.contains('t');
                }
            }
        }
    }
    interactive && tty
}

/// `NAME=value`, where NAME is a shell identifier.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The last part of a path: `/usr/bin/ssh` is `ssh`.
fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(command: &str) -> ForegroundKind {
        classify(command).kind
    }

    #[test]
    fn ssh_and_mosh_are_remote_logins_with_or_without_paths_and_options() {
        assert_eq!(kind("ssh ubuntu@lidia"), ForegroundKind::Ssh);
        assert_eq!(kind("mosh ubuntu@lidia"), ForegroundKind::Ssh);
        assert_eq!(kind("/usr/bin/ssh -p 2222 -i ~/.ssh/key ubuntu@lidia"), ForegroundKind::Ssh);
        assert_eq!(kind("ssh -t host tmux attach"), ForegroundKind::Ssh);
        assert_eq!(kind("sshpass -p x ssh host"), ForegroundKind::Other, "only the program name counts");
    }

    #[test]
    fn the_command_text_is_kept_as_it_was_typed() {
        let fg = classify("ssh ubuntu@lidia");
        assert_eq!(fg.command, "ssh ubuntu@lidia");
        assert_eq!(fg.kind, ForegroundKind::Ssh);
    }

    #[test]
    fn assignments_and_wrappers_before_the_program_are_skipped() {
        assert_eq!(kind("LANG=C ssh host"), ForegroundKind::Ssh);
        assert_eq!(kind("FOO=1 BAR=two command ssh host"), ForegroundKind::Ssh);
        assert_eq!(kind("command ssh host"), ForegroundKind::Ssh);
        assert_eq!(kind("exec zsh"), ForegroundKind::Shell);
        assert_eq!(kind("nohup ssh host"), ForegroundKind::Ssh);
        assert_eq!(kind("env TERM=dumb ssh host"), ForegroundKind::Ssh);
        assert_eq!(kind("_X_1=a ssh host"), ForegroundKind::Ssh);
    }

    #[test]
    fn a_word_that_is_not_an_identifier_assignment_is_the_program() {
        assert_eq!(kind("1X=1 ssh host"), ForegroundKind::Other, "a digit cannot start a name");
        assert_eq!(kind("=x ssh host"), ForegroundKind::Other, "an empty name is not an assignment");
        assert_eq!(kind("a-b=1 ssh host"), ForegroundKind::Other, "a dash is not in a name");
        assert_eq!(kind("ssh=1 host"), ForegroundKind::Other, "an assignment named ssh is skipped, so the program is host");
    }

    #[test]
    fn shells_by_name_and_by_path_are_nested_shells() {
        for line in [
            "zsh",
            "zsh -f -i",
            "/bin/zsh -l",
            "bash",
            "bash --norc",
            "/usr/bin/bash",
            "sh -c 'echo hi'",
            "/bin/sh",
            "dash",
            "fish",
            "/usr/local/bin/fish",
            "ksh",
            "tcsh",
            "csh",
            "nu",
        ] {
            assert_eq!(kind(line), ForegroundKind::Shell, "{line}");
        }
    }

    #[test]
    fn a_program_whose_name_only_starts_like_a_shell_is_not_one() {
        assert_eq!(kind("bashful"), ForegroundKind::Other);
        assert_eq!(kind("zshx -f"), ForegroundKind::Other);
        assert_eq!(kind("shellcheck x.sh"), ForegroundKind::Other);
        assert_eq!(kind("cat zsh"), ForegroundKind::Other, "a shell name as an argument is not the program");
    }

    #[test]
    fn su_and_sudo_into_a_shell() {
        assert_eq!(kind("su"), ForegroundKind::Su);
        assert_eq!(kind("su - ubuntu"), ForegroundKind::Su);
        assert_eq!(kind("sudo -i"), ForegroundKind::Su);
        assert_eq!(kind("sudo -s"), ForegroundKind::Su);
        assert_eq!(kind("sudo su"), ForegroundKind::Su);
        assert_eq!(kind("sudo su -"), ForegroundKind::Su);
        assert_eq!(kind("sudo -iu root"), ForegroundKind::Su, "a cluster with i needs no value");
        assert_eq!(kind("sudo -u root -i"), ForegroundKind::Su, "-u takes its value, then -i counts");
        assert_eq!(kind("sudo -E -i"), ForegroundKind::Su);
        assert_eq!(kind("sudo --login"), ForegroundKind::Su);
        assert_eq!(kind("sudo --shell"), ForegroundKind::Su);
    }

    #[test]
    fn sudo_with_a_command_is_the_command_it_runs() {
        assert_eq!(kind("sudo ls /root"), ForegroundKind::Other);
        assert_eq!(kind("sudo ssh host"), ForegroundKind::Ssh);
        assert_eq!(kind("sudo -E zsh"), ForegroundKind::Shell);
        assert_eq!(kind("sudo docker exec -it web sh"), ForegroundKind::ContainerExec);
        assert_eq!(kind("sudo -u root ssh host"), ForegroundKind::Ssh);
        assert_eq!(kind("sudo -l"), ForegroundKind::Other, "-l alone lists rights, it is not a shell");
        assert_eq!(kind("sudo"), ForegroundKind::Other);
        assert_eq!(kind("sudo -u"), ForegroundKind::Other, "a value flag with no value does not panic");
    }

    #[test]
    fn docker_and_podman_exec_are_container_shells_or_programs() {
        assert_eq!(kind("docker exec -it web sh"), ForegroundKind::ContainerExec);
        assert_eq!(kind("docker exec web ls"), ForegroundKind::ContainerExec);
        assert_eq!(kind("podman exec x ls"), ForegroundKind::ContainerExec);
        assert_eq!(kind("docker --debug exec web ls"), ForegroundKind::ContainerExec, "global flags before the subcommand are skipped");
    }

    #[test]
    fn kubectl_exec_is_a_container_shell_and_other_kubectl_is_not() {
        assert_eq!(kind("kubectl exec -it pod -- bash"), ForegroundKind::ContainerExec);
        assert_eq!(kind("kubectl get pods"), ForegroundKind::Other);
        assert_eq!(kind("kubectl"), ForegroundKind::Other);
    }

    #[test]
    fn docker_and_podman_run_need_both_interactive_and_tty() {
        assert_eq!(kind("docker run -it alpine sh"), ForegroundKind::ContainerExec);
        assert_eq!(kind("docker run -ti alpine sh"), ForegroundKind::ContainerExec);
        assert_eq!(kind("docker run -i -t alpine sh"), ForegroundKind::ContainerExec);
        assert_eq!(kind("docker run --interactive --tty alpine sh"), ForegroundKind::ContainerExec);
        assert_eq!(kind("docker run --rm -e A=b -itd alpine"), ForegroundKind::ContainerExec, "a cluster among other flags");
        assert_eq!(kind("podman run -it fedora bash"), ForegroundKind::ContainerExec);

        assert_eq!(kind("docker run -i alpine cat"), ForegroundKind::Other, "interactive without a tty");
        assert_eq!(kind("docker run -t alpine ls"), ForegroundKind::Other, "a tty without interactive");
        assert_eq!(kind("docker run alpine ls"), ForegroundKind::Other);
        assert_eq!(kind("docker ps -it"), ForegroundKind::Other, "only run and exec count");
        assert_eq!(kind("docker"), ForegroundKind::Other);
        assert_eq!(kind("docker -"), ForegroundKind::Other, "a bare dash is not a subcommand");
    }

    #[test]
    fn tmux_and_screen_are_multiplexers() {
        assert_eq!(kind("tmux"), ForegroundKind::Multiplexer);
        assert_eq!(kind("tmux attach -t work"), ForegroundKind::Multiplexer);
        assert_eq!(kind("screen -S work"), ForegroundKind::Multiplexer);
        assert_eq!(kind("tmuxinator start"), ForegroundKind::Other);
    }

    #[test]
    fn anything_else_is_other_and_keeps_its_text() {
        let fg = classify("sleep 30");
        assert_eq!(fg.kind, ForegroundKind::Other);
        assert_eq!(fg.command, "sleep 30");
        assert_eq!(kind("cat"), ForegroundKind::Other);
        assert_eq!(kind("make test"), ForegroundKind::Other);
        assert_eq!(kind("git ssh"), ForegroundKind::Other);
        assert_eq!(kind("/"), ForegroundKind::Other, "an empty basename does not panic");
        assert_eq!(kind("   "), ForegroundKind::Other);
        assert_eq!(kind(""), ForegroundKind::Other);
    }

    #[test]
    fn compound_lines_use_the_first_segment_that_names_a_program() {
        assert_eq!(kind("cd /x && ssh host"), ForegroundKind::Ssh);
        assert_eq!(kind("ssh host; echo done"), ForegroundKind::Ssh);
        assert_eq!(kind("ls | less"), ForegroundKind::Other);
        assert_eq!(kind("echo hi &"), ForegroundKind::Other);
        assert_eq!(kind("make || zsh"), ForegroundKind::Shell);
        assert_eq!(kind("true; ls ; sleep 2"), ForegroundKind::Other);
    }

    #[test]
    fn control_characters_are_replaced_so_the_text_is_one_line() {
        let fg = classify("ssh\tubuntu@lidia\n");
        assert_eq!(fg.command, "ssh ubuntu@lidia");
        assert_eq!(fg.kind, ForegroundKind::Ssh);

        let fg = classify("ssh \x1b[31mhost\x07 \r\nx");
        assert_eq!(fg.command, "ssh [31mhost x");
        assert!(!fg.command.chars().any(char::is_control));

        assert_eq!(classify("ssh a\nb").command, "ssh a b", "a newline does not glue words");
        assert_eq!(kind("ssh\nhost"), ForegroundKind::Ssh);
    }

    #[test]
    fn the_stored_command_is_capped_at_1000_characters() {
        let long = format!("ssh {}", "a".repeat(1200));
        let fg = classify(&long);
        assert_eq!(fg.kind, ForegroundKind::Ssh);
        assert_eq!(fg.command.chars().count(), MAX_COMMAND_CHARS);
        assert!(fg.command.starts_with("ssh aaa"));

        // Characters, not bytes: a multi-byte name is cut on a character boundary.
        let wide = format!("ssh {}", "日".repeat(1200));
        let fg = classify(&wide);
        assert_eq!(fg.command.chars().count(), MAX_COMMAND_CHARS);
        assert_eq!(fg.kind, ForegroundKind::Ssh);
    }

    #[test]
    fn the_kind_is_read_from_the_whole_line_before_the_cap() {
        // The program is at the start, so a long line still names it.
        let long = format!("docker run -it {} alpine", "x".repeat(1200));
        assert_eq!(kind(&long), ForegroundKind::ContainerExec);
    }

    #[test]
    fn odd_inputs_never_panic() {
        for line in ["=", "==", "sudo -", "env", "command", "exec", "time", "docker run", "tmux\0", "\u{7f}", "ssh\u{0}"] {
            let _ = classify(line);
        }
        assert_eq!(kind("env"), ForegroundKind::Other);
        assert_eq!(kind("exec"), ForegroundKind::Other, "a wrapper with nothing after it is other");
    }

    #[test]
    fn the_wire_codes_are_snake_case_and_match_code() {
        let all = [
            (ForegroundKind::Ssh, "ssh"),
            (ForegroundKind::Shell, "shell"),
            (ForegroundKind::Su, "su"),
            (ForegroundKind::ContainerExec, "container_exec"),
            (ForegroundKind::Multiplexer, "multiplexer"),
            (ForegroundKind::Other, "other"),
        ];
        for (kind, code) in all {
            assert_eq!(serde_json::to_value(kind).unwrap(), code);
            assert_eq!(kind.code(), code);
        }
        let fg = classify("ssh ubuntu@lidia");
        assert_eq!(
            serde_json::to_value(&fg).unwrap(),
            serde_json::json!({"kind": "ssh", "command": "ssh ubuntu@lidia"})
        );
    }

    #[test]
    fn a_foreground_round_trips_through_its_wire_form() {
        let fg = classify("su - root");
        let back: Foreground = serde_json::from_value(serde_json::to_value(&fg).unwrap()).unwrap();
        assert_eq!(back, fg);
    }
}
