//! Puts the icon into `sppy.exe`, so Explorer, the taskbar and shortcuts show it.

fn main() {
    println!("cargo::rerun-if-changed=../../assets/sppy.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/sppy.ico");
        if let Err(e) = res.compile() {
            panic!("could not put the icon into sppy.exe: {e}");
        }
    }
}
