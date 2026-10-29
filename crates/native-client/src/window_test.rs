//! Run each real-window test in a fresh process: winit permits one event loop
//! per process, even if the earlier loop has already been dropped.

pub fn isolated(name: &str) -> bool {
    const MARKER: &str = "RE_MINECRAFT_WINDOW_TEST_CHILD";
    if std::env::var(MARKER).as_deref() == Ok(name) {
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env(MARKER, name)
        .output()
        .expect("cannot start the isolated window test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    print!("{stdout}");
    eprint!("{stderr}");
    assert!(
        output.status.success(),
        "isolated window test {name} failed"
    );
    assert!(
        stdout.contains("1 passed;"),
        "isolated window test {name} did not run"
    );
    false
}
