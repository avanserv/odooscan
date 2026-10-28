use std::process::{Command, Output};

fn oscn(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oscn"))
        .args(args)
        .output()
        .expect("oscn runs")
}

#[test]
fn explain_prints_the_page_of_a_code() {
    let out = oscn(&["explain", "OD203"]);
    assert_eq!(out.status.code(), Some(0));
    let page = String::from_utf8(out.stdout).unwrap();
    assert!(page.starts_with("# OD203 unresolved-field-path\n"), "{page}");
    assert!(page.contains("## Example"));
}

#[test]
fn explain_accepts_a_name() {
    let out = oscn(&["explain", "orphan-super"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .starts_with("# OD211 orphan-super\n")
    );
}

#[test]
fn explain_rejects_an_unknown_code() {
    let out = oscn(&["explain", "OD999"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("error: no rule is called 'OD999'")
    );
}

#[test]
fn explain_suggests_rules_sharing_a_prefix() {
    let out = oscn(&["explain", "od21"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("OD210") && stderr.contains("OD211"), "{stderr}");
}
