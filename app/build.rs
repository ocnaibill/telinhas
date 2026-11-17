//! Puts `assets/telinhas.ico` inside the Windows executable. Explorer reads
//! the icon of an `.exe` from its resources, not from the window at run time.
//!
//! On Windows it also copies the FFmpeg DLLs (from `%FFMPEG_DIR%\bin`) next to
//! the executable, which the hardware encoder needs to start at all.

fn main() {
    println!("cargo:rerun-if-changed=assets/telinhas.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        copy_ffmpeg_dlls();
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

#[cfg(windows)]
fn copy_ffmpeg_dlls() {
    use std::path::PathBuf;

    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let Some(dir) = std::env::var_os("FFMPEG_DIR") else {
        return;
    };
    let bin = PathBuf::from(dir).join("bin");
    // OUT_DIR is target/<profile>/build/<package>/out: the executable sits
    // three levels up.
    let Some(target) = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .and_then(|out| out.ancestors().nth(3).map(PathBuf::from))
    else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&bin) else {
        println!("cargo:warning=no FFmpeg DLLs found in {}", bin.display());
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
            && let Some(name) = path.file_name()
            && let Err(error) = std::fs::copy(&path, target.join(name))
        {
            println!("cargo:warning=could not copy {}: {error}", path.display());
        }
    }
}
