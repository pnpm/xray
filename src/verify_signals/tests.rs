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
