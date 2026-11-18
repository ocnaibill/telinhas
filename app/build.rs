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
    println!("cargo:rerun-if-env-changed=PATH");
    // OUT_DIR is target/<profile>/build/<package>/out: the executable sits
    // next to the `build` folder, so find that and go one up.
    let Some(target) = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .and_then(|out| {
            out.ancestors()
                .find(|dir| dir.file_name().is_some_and(|name| name == "build"))
                .and_then(|build| build.parent().map(PathBuf::from))
        })
    else {
        return;
    };
    // Where the DLLs may be: next to FFMPEG_DIR's headers, or on the PATH.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("FFMPEG_DIR") {
        candidates.push(PathBuf::from(&dir).join("bin"));
        candidates.push(PathBuf::from(dir));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path));
    }
    let is_ffmpeg_dll = |path: &std::path::Path| {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        name.ends_with(".dll")
            && [
                "avcodec",
                "avdevice",
                "avfilter",
                "avformat",
                "avutil",
                "swresample",
                "swscale",
                "postproc",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
    };
    let Some(dir) = candidates.into_iter().find(|dir| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .starts_with("avcodec")
            })
    }) else {
        println!(
            "cargo:warning=FFmpeg DLLs not found (set FFMPEG_DIR or put them on the PATH): the app will not start without them next to the executable"
        );
        return;
    };
    let mut copied = 0;
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if is_ffmpeg_dll(&path)
            && let Some(name) = path.file_name()
        {
            match std::fs::copy(&path, target.join(name)) {
                Ok(_) => copied += 1,
                Err(error) => println!("cargo:warning=could not copy {}: {error}", path.display()),
            }
        }
    }
    println!(
        "cargo:warning=copied {copied} FFmpeg DLLs from {}",
        dir.display()
    );
}
