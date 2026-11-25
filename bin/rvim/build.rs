//! Embed the multi-size Windows icon into rvim.exe.
//!
//! The icon is compiled with the Windows SDK rc.exe (MSVC) or windres (GNU)
//! when available. A missing resource compiler is a warning, not a build
//! failure: the executable still runs, it just keeps the generic icon.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let icon = manifest.join("../../assets/icon/rvim.ico");
    let manifest_file = manifest.join("../../assets/rvim.manifest");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", manifest_file.display());
    let Ok(icon) = icon.canonicalize() else {
        println!(
            "cargo:warning=icon {} missing; building without an application icon",
            icon.display()
        );
        return;
    };

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let rc_path = out.join("rvim.rc");
    // RC uses backslash escapes inside quoted strings; forward slashes are safe.
    let icon_text = icon.display().to_string().replace('\\', "/");
    let manifest_text = manifest_file.display().to_string().replace('\\', "/");
    if let Err(error) = fs::write(
        &rc_path,
        format!("1 ICON \"{icon_text}\"\n1 24 \"{manifest_text}\"\n"),
    ) {
        println!("cargo:warning=cannot write icon resource: {error}");
        return;
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        let Some(rc) = find_rc() else {
            println!("cargo:warning=rc.exe not found; building without an application icon");
            return;
        };
        let res = out.join("rvim.res");
        match Command::new(&rc)
            .arg("/nologo")
            .arg("/fo")
            .arg(&res)
            .arg(&rc_path)
            .status()
        {
            Ok(status) if status.success() => {
                // link.exe accepts a compiled .res file directly as an input.
                println!("cargo:rustc-link-arg-bins={}", res.display());
            }
            Ok(status) => println!("cargo:warning=rc.exe failed ({status}); no application icon"),
            Err(error) => println!("cargo:warning=cannot run rc.exe: {error}"),
        }
    } else {
        let Some(windres) = find_windres() else {
            println!("cargo:warning=windres not found; building without an application icon");
            return;
        };
        let obj = out.join("rvim-res.o");
        match Command::new(&windres)
            .args(["-O", "coff", "-o"])
            .arg(&obj)
            .arg(&rc_path)
            .status()
        {
            Ok(status) if status.success() => {
                println!("cargo:rustc-link-arg-bins={}", obj.display());
            }
            Ok(status) => println!("cargo:warning=windres failed ({status}); no application icon"),
            Err(error) => println!("cargo:warning=cannot run windres: {error}"),
        }
    }
}

fn find_rc() -> Option<PathBuf> {
    if let Ok(path) = env::var("RC") {
        if Path::new(&path).is_file() {
            return Some(PathBuf::from(path));
        }
    }
    // Already on PATH.
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }
    // Windows SDK layout: <sdk>\bin\<version>\<arch>\rc.exe.
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        _ => "x64",
    };
    let mut sdks = Vec::new();
    if let Ok(dir) = env::var("WindowsSdkDir") {
        sdks.push(PathBuf::from(dir));
    }
    sdks.push(PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10"));
    sdks.push(PathBuf::from(r"C:\Program Files\Windows Kits\10"));
    for sdk in sdks {
        let bin = sdk.join("bin");
        let Ok(entries) = fs::read_dir(&bin) else {
            continue;
        };
        let mut versions: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        versions.sort();
        for version in versions.iter().rev() {
            let candidate = version.join(arch).join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn find_windres() -> Option<PathBuf> {
    if let Ok(path) = env::var("WINDRES") {
        if Path::new(&path).is_file() {
            return Some(PathBuf::from(path));
        }
    }
    if Command::new("windres").arg("--version").output().is_ok() {
        return Some(PathBuf::from("windres"));
    }
    None
}
