//! Embed the RustSeis Windows application icon (`assets/rustseis.ico`) into `rustseis-desktop.exe`.

fn main() {
    println!("cargo:rerun-if-changed=assets/rustseis.ico");
    println!("cargo:rerun-if-changed=assets/rustseis.rc");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let rc_path = manifest_dir.join("assets").join("rustseis.rc");
    let res_path = out_dir.join("rustseis.res");

    // Compile the Win32 resource script using rc.exe (available in MSVC / Windows SDK PATH).
    let status = std::process::Command::new("rc.exe")
        .arg("/nologo")
        .arg(format!("/fo{}", res_path.display()))
        .arg(&rc_path)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bin=rustseis-desktop={}", res_path.display());
        }
        Ok(s) => {
            panic!("rc.exe failed with status {s} while compiling {}", rc_path.display());
        }
        Err(e) => {
            println!("cargo:warning=Could not invoke rc.exe to embed Windows icon: {e}");
        }
    }
}
