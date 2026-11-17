//! Puts `assets/telinhas.ico` inside the Windows executable. Explorer reads
//! the icon of an `.exe` from its resources, not from the window at run time.

fn main() {
    println!("cargo:rerun-if-changed=assets/telinhas.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/telinhas.ico");
        resource.set("ProductName", "Telinhas");
        resource.set("FileDescription", "Telinhas");
        // A missing resource compiler should not stop the build, only the icon.
        if let Err(error) = resource.compile() {
            println!("cargo:warning=could not embed the icon: {error}");
        }
    }
}
