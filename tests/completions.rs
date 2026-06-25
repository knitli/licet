//! T046 — shell completion generation is wired and emits a non-empty script per shell.

mod common;
use common::Fixture;

#[test]
fn generates_completions_for_each_shell() {
    let f = Fixture::new();
    for (shell, needle) in [
        ("bash", "_licet()"),
        ("zsh", "#compdef licet"),
        ("fish", "complete -c licet"),
    ] {
        let out = f.licet().args(["completions", shell]).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{shell} completions exit 0");
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(
            stdout.contains(needle),
            "{shell} script should contain `{needle}`:\n{stdout}"
        );
    }
}

#[test]
fn unknown_shell_is_usage_error() {
    let f = Fixture::new();
    let out = f
        .licet()
        .args(["completions", "notashell"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "unknown shell → usage error");
}
