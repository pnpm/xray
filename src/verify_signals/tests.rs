use super::run;
use std::{
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

#[test]
fn cancellation_stops_a_child_and_its_descendant_before_returning() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("ready");
    let late = temp.path().join("late");
    let script = r"const {spawn}=require('node:child_process');
const fs=require('node:fs');
spawn(process.execPath,['-e',`setTimeout(()=>require('node:fs').writeFileSync(process.argv[1],'late'),800)`,process.argv[2]],{stdio:'ignore'});
fs.writeFileSync(process.argv[1],'ready');
setInterval(()=>{},1000);";
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let waiter = thread::spawn(move || {
        for _ in 0..200 {
            if ready.exists() {
                flag.store(true, Ordering::SeqCst);
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        flag.store(true, Ordering::SeqCst);
        panic!("child did not start");
    });
    let status = run(
        Command::new("node").args(["-e", script]).arg(temp.path().join("ready")).arg(&late),
        &cancelled,
    )
    .unwrap();
    waiter.join().unwrap();
    assert!(!status.success());
    thread::sleep(Duration::from_secs(1));
    assert!(!late.exists(), "descendant continued after cancellation");
}

#[test]
fn uncancelled_child_status_is_preserved() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let status =
        run(Command::new("node").args(["-e", "process.exit(7)"]), &AtomicBool::new(false)).unwrap();
    assert_eq!(status.code(), Some(7));
}

#[cfg(unix)]
#[test]
fn a_child_stopped_for_terminal_access_does_not_hang_verification() {
    // Without a terminal the stopped child is killed and reported; with one
    // that Xray owns, the child gets the terminal and continues to exit 3.
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let result =
            run(Command::new("sh").args(["-c", "kill -TTIN $$; exit 3"]), &AtomicBool::new(false));
        sender.send(result.map(|status| status.code())).unwrap();
    });
    match receiver.recv_timeout(Duration::from_secs(20)).expect("verification hung") {
        Ok(code) => assert_eq!(code, Some(3)),
        Err(error) => assert!(error.to_string().contains("terminal access"), "{error:#}"),
    }
}

#[cfg(unix)]
#[test]
fn terminal_interrupts_are_recognized_from_child_status() {
    use std::{os::unix::process::ExitStatusExt as _, process::ExitStatus};
    assert!(super::unix::interrupted(ExitStatus::from_raw(libc::SIGINT)));
    assert!(super::unix::interrupted(ExitStatus::from_raw(130 << 8)));
    assert!(!super::unix::interrupted(ExitStatus::from_raw(1 << 8)));
    assert!(!super::unix::interrupted(ExitStatus::from_raw(libc::SIGKILL)));
}
