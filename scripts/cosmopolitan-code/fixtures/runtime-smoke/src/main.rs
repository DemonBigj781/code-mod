use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn main() {
    if env::args().nth(1).as_deref() == Some("--child") {
        print!("{}", fs::read_to_string("renamed.txt").expect("child relative read"));
        return;
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let directory = Scratch(env::temp_dir().join(format!("cosmo-source-smoke-{}-{nonce}", std::process::id())));
    fs::create_dir(&directory.0).expect("create scratch directory");
    fs::write(directory.0.join("original.txt"), "source-runtime-ok").expect("write file");
    fs::rename(directory.0.join("original.txt"), directory.0.join("renamed.txt")).expect("rename file");
    assert_eq!(fs::read_to_string(directory.0.join("renamed.txt")).unwrap(), "source-runtime-ok");
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    assert_eq!(fs::read(directory.0.join("missing")).unwrap_err().kind(), ErrorKind::NotFound);
    assert_eq!(std::thread::spawn(|| 42).join().unwrap(), 42);

    let child = Command::new(env::current_exe().expect("executable path"))
        .arg("--child")
        .current_dir(&directory.0)
        .output()
        .expect("spawn portable child with captured stdout");
    assert!(child.status.success(), "portable child failed: {child:?}");
    assert_eq!(child.stdout, b"source-runtime-ok");
    if env::args().nth(1).as_deref() == Some("--native-windows") {
        let command = PathBuf::from(env::var_os("SystemRoot").expect("Windows system directory"))
            .join("System32").join("cmd.exe");
        let status = Command::new(command).args(["/D", "/C", "exit 7"]).status().expect("native Windows child");
        assert_eq!(status.code(), Some(7), "native exit code must not become a signal");
    }
    println!("Rust source-runtime file, thread, and subprocess checks: OK");
}
