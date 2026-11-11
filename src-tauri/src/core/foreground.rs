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

/// True when the command line opens a shell that the user types into, and that shell has no shell integration unless the app
/// integrates it. That is: ssh or mosh with no remote command; su with no command; a bare shell (no `-c`, no script); `sudo -i`
/// or `sudo -s` with nothing to run; and a container `exec` or `run -it` whose last word is a shell. Anything that runs a
/// command or a script instead (a remote command, `-c`, a script), the ssh options that never leave a shell (`-N`, `-W`,
/// `-O`, `-f`, `-G`, `-V`, `-T`), and any option this does not know, makes it not a login. So the bias is to no early return.
/// Compound lines are checked segment by segment, as in `classify`. Quoted words are not unquoted: a quoted value with a
/// space reads as a word of its own, which can only make the answer "not a login".
pub fn is_login(command: &str) -> bool {
    one_line(command)
        .split([';', '&', '|'])
        .find_map(|segment| {
            let words: Vec<&str> = segment.split_whitespace().collect();
            login_of_words(&words)
        })
        .unwrap_or(false)
}

/// The words after any assignments and wrappers (see WRAPPERS): the program, then its arguments.
fn program_words<'w, 'a>(words: &'w [&'a str]) -> &'w [&'a str] {
    let mut rest = words;
    while let Some((first, tail)) = rest.split_first() {
        if is_assignment(first) || WRAPPERS.contains(first) {
            rest = tail;
        } else {
            break;
        }
    }
    rest
}

/// Whether the program these words start is a login, for the programs `kind_of_words` recognises. None for the others, so
/// the same segment is found as the one `classify` reads.
fn login_of_words(words: &[&str]) -> Option<bool> {
    let (program, args) = program_words(words).split_first()?;
    match basename(program) {
        "ssh" => Some(ssh_login(args)),
        "mosh" => Some(mosh_login(args)),
        name if SHELLS.contains(&name) => Some(shell_login(args)),
        "su" => Some(su_login(args)),
        "sudo" => sudo_login(args),
        "docker" | "podman" => docker_kind(args).map(|_| container_shell_login(args)),
        "kubectl" => (args.first() == Some(&"exec")).then(|| container_shell_login(args)),
        "tmux" | "screen" => Some(false),
        _ => None,
    }
}

/// The ssh options that take a value (OpenSSH's list). A value is the rest of its word when it has one (`-p2222`), else the
/// next word.
const SSH_VALUE_LETTERS: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// `ssh [options] host [command...]`. A login has a host and nothing after it, or `-t host <bare shell with flags>`: with a
/// tty, a shell is what the remote side runs. `host <anything else>` runs that command, not a shell.
fn ssh_login(args: &[&str]) -> bool {
    let mut tty = false;
    let mut host_at = None;
    let mut i = 0;
    while i < args.len() {
        let word = args[i];
        i += 1;
        if word == "--" {
            continue;
        }
        let Some(letters) = word.strip_prefix('-') else {
            host_at = Some(i - 1);
            break;
        };
        if letters.is_empty() || letters.starts_with('-') {
            return false;
        }
        let mut takes_next = false;
        for (at, c) in letters.char_indices() {
            match c {
                'N' | 'W' | 'O' | 'f' | 'G' | 'V' | 'T' => return false,
                't' => tty = true,
                c if SSH_VALUE_LETTERS.contains(c) => {
                    takes_next = at + c.len_utf8() == letters.len();
                    break;
                }
                _ => {}
            }
        }
        if takes_next {
            if i >= args.len() {
                return false;
            }
            i += 1;
        }
    }
    let Some(host) = host_at else {
        return false;
    };
    match args[host + 1..].split_first() {
        None => true,
        Some((first, flags)) => tty && SHELLS.contains(&basename(first)) && shell_login(flags),
    }
}

/// `mosh [options] host`: only `--name=value` options before the host, and nothing after it. Any other option is not read,
/// so the answer is "not a login" for it.
fn mosh_login(args: &[&str]) -> bool {
    let options = args.iter().take_while(|w| w.starts_with('-')).count();
    let (opts, rest) = args.split_at(options);
    opts.iter().all(|w| w.starts_with("--") && w.contains('=')) && rest.len() == 1
}

/// A shell started with no command and no script: only flags, none of them a `-c` (`-c`, `-lc`). Any other word (a script,
/// the value of an option such as `-o`) makes it not a login.
fn shell_login(args: &[&str]) -> bool {
    args.iter().all(|word| match *word {
        "--login" | "--norc" | "--noprofile" | "--interactive" => true,
        long if long.starts_with("--") => false,
        short if short.len() > 1 && (short.starts_with('-') || short.starts_with('+')) => !short.contains('c'),
        _ => false,
    })
}

/// `su [-] [-l] [user]`, with nothing else: `-c`, `-s`, and a second word (a command) are not a login.
fn su_login(args: &[&str]) -> bool {
    let mut user = false;
    for word in args {
        match *word {
            "-" | "-l" | "--login" | "-m" | "-p" | "--preserve-environment" => {}
            name if !name.starts_with('-') && !user => user = true,
            _ => return false,
        }
    }
    true
}

/// The container shell of `docker|podman exec|run` and `kubectl exec`: its last word is a shell, and that shell is not given
/// a `-c` (the word before it is not `-c` or a cluster with `c`, such as `-lc`).
fn container_shell_login(args: &[&str]) -> bool {
    match args {
        [.., before, last] => SHELLS.contains(&basename(last)) && !is_c_flag(before),
        _ => false,
    }
}

/// A short option cluster that has a `c` in it (`-c`, `-lc`). Long options are not.
fn is_c_flag(word: &str) -> bool {
    word.strip_prefix('-').is_some_and(|c| !c.starts_with('-') && c.contains('c'))
}

/// `sudo`: a login when it has nothing to run and a login flag (`-i`, `-s`, `--login`, `--shell`), or when it runs a login
/// (`sudo su`, `sudo zsh`). None when it runs something that is not one of the recognised programs.
fn sudo_login(args: &[&str]) -> Option<bool> {
    let (login, at) = sudo_flags(args);
    let rest = &args[at..];
    match rest.first() {
        None => login.then_some(true),
        Some(first) if basename(first) == "su" => Some(su_login(&rest[1..])),
        Some(_) => login_of_words(rest),
    }
}

/// The kind of the program these words start, after skipping assignments and wrappers. None when the program is not one
/// of the recognised kinds.
fn kind_of_words(words: &[&str]) -> Option<ForegroundKind> {
    let (program, args) = program_words(words).split_first()?;
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
    let (login, at) = sudo_flags(args);
    let rest = &args[at..];
    match rest.first() {
        None => login.then_some(ForegroundKind::Su),
        Some(first) if basename(first) == "su" => Some(ForegroundKind::Su),
        Some(_) => kind_of_words(rest),
    }
}

/// The sudo flags before the command: whether they ask for a login shell, and where the command starts.
fn sudo_flags(args: &[&str]) -> (bool, usize) {
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
    (login, i.min(args.len()))
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

    fn login(command: &str) -> bool {
        is_login(command)
    }

    #[test]
    fn ssh_with_no_remote_command_is_a_login() {
        assert!(login("ssh ubuntu@lidia"));
        assert!(login("ssh -p 2222 user@host"));
        assert!(login("ssh -p2222 user@host"), "an attached value");
        assert!(login("ssh -i ~/.ssh/key -l ubuntu host"));
        assert!(login("ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null host"));
        assert!(login("ssh -J bastion host"), "-J takes a value");
        assert!(login("ssh -L 8080:localhost:80 host"), "a forward still gives a shell");
        assert!(login("ssh -tt host"), "a forced tty and no command");
        assert!(login("ssh -4 -A -v host"), "flags with no value");
        assert!(login("ssh -- host"), "the end of options");
        assert!(login("/usr/bin/ssh host"));
        assert!(login("sudo -u root ssh host"));
        assert!(login("LANG=C ssh host"));
        assert!(login("ssh host; echo done"), "the command ends at the ;");
        assert!(login("cd x && ssh host"));
        assert!(login("ssh -p 2222 -i ~/.ssh/key ubuntu@lidia"));
    }

    #[test]
    fn ssh_with_a_remote_command_is_not_a_login() {
        assert!(!login("ssh host make"), "a program on the remote host");
        assert!(!login("ssh host bash"), "a shell with no tty runs as a command, not a login");
        assert!(!login("ssh host bash -l"));
        assert!(!login("ssh -p 2222 user@host ls -la"));
        assert!(!login("ssh -i key host 'cat /etc/hosts'"));
        assert!(!login("ssh host \"make\""));
        assert!(!login("ssh host ls"));
        assert!(!login("ssh host -p 22"), "options after the host are read as a command, so no login");
        assert!(!login("ssh -t host ls"), "a tty does not make a non-shell command a login");
    }

    #[test]
    fn ssh_with_t_and_a_bare_shell_is_a_login() {
        assert!(login("ssh -t host bash"));
        assert!(login("ssh -t host zsh -f -i"), "a bare shell with flags");
        assert!(login("ssh -tt host /bin/bash"));
        assert!(login("ssh -t -p 2222 user@host zsh"));
        assert!(!login("ssh -t host bash -c 'make'"), "a shell given a command");
        assert!(!login("ssh -t host \"bash\""), "a quoted shell is not read as one: conservative");
        assert!(!login("ssh -t host tmux attach"));
        assert!(!login("ssh -t host bash script.sh"), "a shell given a script");
        assert!(!login("ssh -T host bash"), "-T means no tty: no interactive shell");
        assert!(!login("ssh -tT host bash"), "-T wins over -t: conservative");
    }

    #[test]
    fn ssh_options_that_never_leave_a_shell_are_not_a_login() {
        assert!(!login("ssh -N host"), "no shell at all");
        assert!(!login("ssh -fN -L 8080:x:80 host"));
        assert!(!login("ssh -f host"), "backgrounds after authentication");
        assert!(!login("ssh -W host:22 bastion"), "a stdio forward");
        assert!(!login("ssh -O check host"), "a control command");
        assert!(!login("ssh -G host"), "prints the config");
        assert!(!login("ssh -V"));
        assert!(!login("ssh -T host"), "no tty");
        assert!(!login("ssh --help"), "a long option is not read");
        assert!(!login("ssh"), "no host");
        assert!(!login("ssh -p"), "a value flag with no value and no host");
        assert!(!login("ssh -"), "a bare dash");
        assert!(!login("ssh -i"), "the identity value is missing");
    }

    #[test]
    fn ssh_value_words_are_not_read_as_options_or_hosts() {
        // The value of -o contains an N and a W, which must not count as the options that end a login.
        assert!(login("ssh -o NumberOfPasswordPrompts=3 host"));
        assert!(login("ssh -o ServerAliveInterval=30 host"));
        assert!(login("ssh -i ~/keys/N-key host"), "the value N-key is a path, not an option");
        assert!(!login("ssh -o \"ProxyCommand=ssh -W %h:%p bastion\" host"), "quoted with a space: the -W reads as the host's args");
    }

    #[test]
    fn mosh_with_no_remote_command_is_a_login() {
        assert!(login("mosh ubuntu@lidia"));
        assert!(login("mosh --server=/usr/bin/mosh-server host"), "a long option with =");
        assert!(!login("mosh --ssh=\"ssh -p 2222\" host"), "a quoted value with a space splits into words: conservative");
        assert!(!login("mosh host -- ls"), "a command after the host");
        assert!(!login("mosh host ls"));
        assert!(!login("mosh -a host"), "a short option is not read: conservative");
        assert!(!login("mosh -- host"), "the separator before the host");
        assert!(!login("mosh"), "no host");
    }

    #[test]
    fn a_bare_shell_is_a_login_and_one_with_a_command_or_script_is_not() {
        for line in ["zsh", "zsh -f -i", "/bin/zsh -l", "bash", "bash --norc", "sh", "dash", "fish", "ksh", "exec zsh", "nohup bash"] {
            assert!(login(line), "{line}");
        }
        for line in [
            "bash -c 'make'",
            "sh -c 'sleep 2'",
            "zsh -lc 'ls'",
            "bash -ic 'echo'",
            "bash script.sh",
            "bash -o pipefail",
            "bash --rcfile x",
            "fish -c 'ls'",
            "bash -",
            "bash --command ls",
            "bash +c",
        ] {
            assert!(!login(line), "{line}");
        }
    }

    #[test]
    fn su_into_a_shell_is_a_login_and_su_with_a_command_is_not() {
        assert!(login("su"));
        assert!(login("su -"));
        assert!(login("su - ubuntu"));
        assert!(login("su -l root"));
        assert!(login("su --login ubuntu"));
        assert!(login("sudo -i"));
        assert!(login("sudo -s"));
        assert!(login("sudo su -"));
        assert!(login("sudo --login"));
        assert!(!login("su -c ls"));
        assert!(!login("su root -c ls"));
        assert!(!login("su -s /bin/zsh ubuntu"), "a shell option is not read: conservative");
        assert!(!login("su root ls"), "a second word is a command");
        assert!(!login("sudo -i ls"), "sudo -i with a command");
        assert!(!login("sudo ls /root"));
        assert!(!login("sudo -l"), "lists rights, not a shell");
        assert!(!login("sudo"));
    }

    #[test]
    fn a_container_exec_or_run_is_a_login_when_it_ends_in_a_shell() {
        assert!(login("docker exec -it web sh"));
        assert!(login("docker exec -it web bash"));
        assert!(login("docker exec web /bin/zsh"));
        assert!(login("podman exec -it x bash"));
        assert!(login("docker --debug exec web bash"), "a global flag before exec");
        assert!(login("docker run -it alpine sh"));
        assert!(login("docker run -ti --rm ubuntu bash"));
        assert!(login("kubectl exec -it pod -- bash"));
        assert!(login("kubectl exec -it pod -c app -- sh"), "-c for kubectl is a container name, not a shell flag");
        assert!(login("sudo docker exec -it web sh"));

        assert!(!login("docker exec web ls"), "the last word is not a shell");
        assert!(!login("docker exec -it web bash -c 'make'"), "a shell with -c");
        assert!(!login("docker exec web sh -c bash"), "the word before the shell is -c");
        assert!(!login("docker exec -it web bash -l"), "a flag after the shell: conservative");
        assert!(!login("docker run -it alpine"), "no command");
        assert!(!login("docker run -i alpine cat"), "not a tty");
        assert!(!login("kubectl exec -it pod -- ls"));
        assert!(!login("kubectl get pods"));
    }

    #[test]
    fn multiplexers_and_plain_programs_are_not_logins() {
        assert!(!login("tmux"), "a multiplexer is not a shell the app can integrate");
        assert!(!login("tmux attach -t work"));
        assert!(!login("screen -S work"));
        assert!(!login("sleep 30"));
        assert!(!login("make test"));
        assert!(!login("cat zsh"), "a shell name as an argument is not the program");
        assert!(!login("bashful"));
        assert!(!login(""));
        assert!(!login("   "));
        assert!(!login("/"));
        assert!(!login("env"));
    }

    #[test]
    fn compound_lines_use_the_first_segment_that_names_a_program_as_classify_does() {
        assert!(login("make || zsh"));
        assert!(!login("true; ls ; sleep 2"));
        assert!(!login("ssh host make; echo x"), "the first program is ssh with a command");
        assert!(login("ssh host; zsh -c ls"), "the first program is ssh with no command; the second segment is not read");
    }

    /// Every login is a kind the app can integrate (ssh, shell, su, container exec). A login is never an `other` or a
    /// multiplexer, so the two readings agree on what a login is.
    #[test]
    fn a_login_is_always_a_kind_that_can_be_integrated() {
        for line in [
            "ssh host",
            "ssh -t host bash",
            "mosh host",
            "zsh -f -i",
            "su -",
            "sudo -i",
            "docker exec -it web sh",
            "kubectl exec -it p -- bash",
            "ssh host make",
            "tmux",
            "sleep 3",
            "docker run -it alpine",
        ] {
            if login(line) {
                assert!(
                    matches!(
                        classify(line).kind,
                        ForegroundKind::Ssh | ForegroundKind::Shell | ForegroundKind::Su | ForegroundKind::ContainerExec
                    ),
                    "{line} is a login but classifies as {:?}",
                    classify(line).kind
                );
            }
        }
    }

    /// The sudo flag parse is shared by the kind and the login: the same `sudo -u root -i` is a shell both ways.
    #[test]
    fn sudo_flags_are_read_the_same_for_the_kind_and_the_login() {
        for line in ["sudo -u root -i", "sudo -iu root", "sudo -E -i", "sudo --login", "sudo -s"] {
            assert_eq!(kind(line), ForegroundKind::Su, "{line}");
            assert!(login(line), "{line}");
        }
        assert!(!login("sudo -u root ls"));
        assert_eq!(kind("sudo -u root ls"), ForegroundKind::Other);
    }

    #[test]
    fn odd_inputs_are_never_a_login_and_never_panic() {
        for line in ["=", "==", "sudo -", "env", "command", "exec", "time", "docker run", "tmux\0", "\u{7f}", "ssh\u{0}", "ssh -", "ssh --", "docker exec", "kubectl exec", "su -c"] {
            let _ = login(line);
        }
        assert!(!login("ssh -"));
        assert!(!login("docker exec"));
        assert!(!login("kubectl exec"));
        assert!(!login("su -c"));
    }
}
