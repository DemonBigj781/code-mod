use std::env;
use std::ffi::{c_char, c_int, c_void};
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

#[repr(C)]
struct DlInfo {
    filename: *const c_char,
    base: *mut c_void,
    symbol_name: *const c_char,
    symbol_address: *mut c_void,
}

unsafe extern "C" {
    fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int;
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

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

    let mut info = DlInfo {
        filename: std::ptr::null(), base: std::ptr::null_mut(),
        symbol_name: std::ptr::null(), symbol_address: std::ptr::null_mut(),
    };
    // Exercise the address adapter together with the runtime's loader stubs.
    unsafe {
        if dladdr(main as *const () as *const c_void, &mut info) != 0 {
            assert!(!info.filename.is_null());
        }
        assert!(dlopen(std::ptr::null(), 0).is_null());
        assert!(dlsym(std::ptr::null_mut(), c"missing".as_ptr()).is_null());
    }
    assert_eq!(Command::new(directory.0.join("missing-child")).output().unwrap_err().kind(), ErrorKind::NotFound);

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
