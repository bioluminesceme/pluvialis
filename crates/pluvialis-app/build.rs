//! Embed the Windows application icon into the executable so the pinned
//! taskbar icon and Explorer show it (eframe's runtime icon only covers the
//! live window, not the .exe file icon).

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // Do not fail the build if no resource compiler is available; the
            // app still runs, only the pinned-taskbar icon stays generic.
            println!("cargo:warning=could not embed Windows icon: {e}");
        }
    }
}
