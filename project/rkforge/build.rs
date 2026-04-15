use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEPS_STUB_ENV: &str = "RKFORGE_SANDBOX_DEPS_STUB";
const RUNTIME_LIB_DIR_ENV: &str = "RKFORGE_RUNTIME_LIB_DIR";
const RUNTIME_TARBALL_ENV: &str = "RKFORGE_RUNTIME_TARBALL";
const RUNTIME_URL_ENV: &str = "RKFORGE_RUNTIME_URL";
const LIBKRUN_SRC_DIR_ENV: &str = "RKFORGE_LIBKRUN_SRC_DIR";
const LIBKRUNFW_SRC_DIR_ENV: &str = "RKFORGE_LIBKRUNFW_SRC_DIR";
const GENERATED_FILE: &str = "sandbox_runtime_manifest.rs";
const DEFAULT_PREBUILT_BASE_URL: &str = "https://download.rk8s.dev/rk8s/releases";

fn main() {
    println!("cargo:rerun-if-env-changed={DEPS_STUB_ENV}");
    println!("cargo:rerun-if-env-changed={RUNTIME_LIB_DIR_ENV}");
    println!("cargo:rerun-if-env-changed={RUNTIME_TARBALL_ENV}");
    println!("cargo:rerun-if-env-changed={RUNTIME_URL_ENV}");
    println!("cargo:rerun-if-env-changed={LIBKRUN_SRC_DIR_ENV}");
    println!("cargo:rerun-if-env-changed={LIBKRUNFW_SRC_DIR_ENV}");
    println!("cargo:rerun-if-changed=runtime/current/lib");
    println!("cargo:rerun-if-changed=runtime/current.tar.gz");
    println!("cargo:rerun-if-changed=vendor/libkrun");
    println!("cargo:rerun-if-changed=vendor/libkrunfw");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"));
    let generated = out_dir.join(GENERATED_FILE);
    let mode = DepsMode::from_env();
    println!("cargo:warning=Sandbox runtime dependency mode: {:?}", mode);
    let contents = generate_manifest(&out_dir, &mode);
    fs::write(&generated, contents).expect("failed to write embedded sandbox runtime manifest");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DepsMode {
    Source,
    Stub,
    Prebuilt,
    System,
}

impl DepsMode {
    fn from_env() -> Self {
        match env::var(DEPS_STUB_ENV).ok().as_deref() {
            Some("1") => Self::Stub,
            Some("2") => Self::Prebuilt,
            Some("3") => Self::System,
            _ => {
                if discover_runtime_source_dirs().is_some() {
                    Self::Source
                } else {
                    Self::System
                }
            }
        }
    }
}

fn discover_runtime_lib_dir() -> Option<PathBuf> {
    if let Some(path) = env::var_os(RUNTIME_LIB_DIR_ENV).map(PathBuf::from)
        && path.is_dir()
    {
        return Some(path);
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);
    let candidate = manifest_dir.join("runtime/current/lib");
    candidate.is_dir().then_some(candidate)
}

fn generate_manifest(out_dir: &Path, mode: &DepsMode) -> String {
    let mut body =
        String::from("pub const EMBEDDED_RUNTIME_LIB_ASSETS: &[(&str, &[u8], u32)] = &[\n");

    let runtime_lib_dir = match mode {
        DepsMode::Stub => None,
        DepsMode::Prebuilt => {
            prepare_prebuilt_runtime_libs(out_dir).or_else(discover_runtime_lib_dir)
        }
        DepsMode::Source => {
            prepare_runtime_libs_from_source(out_dir).or_else(discover_runtime_lib_dir)
        }
        DepsMode::System => discover_runtime_lib_dir(),
    };

    if let Some(dir) = runtime_lib_dir.as_deref()
        && let Ok(entries) = collect_runtime_files(dir)
    {
        for path in entries {
            let relative = path
                .strip_prefix(dir)
                .expect("runtime asset path must be under runtime lib dir");
            let relative = format!("lib/{}", relative.to_string_lossy());
            let absolute = path.canonicalize().unwrap_or(path.clone());
            body.push_str(&format!(
                "    ({:?}, include_bytes!(r#\"{}\"#), 0o644),\n",
                relative,
                absolute.display()
            ));
        }
    } else if *mode == DepsMode::System
        && let Some(assets) = discover_system_runtime_assets()
    {
        for (relative, absolute) in assets {
            body.push_str(&format!(
                "    ({:?}, include_bytes!(r#\"{}\"#), 0o644),\n",
                relative,
                absolute.display()
            ));
        }
    } else {
        match mode {
            DepsMode::Stub => {
                println!("cargo:warning=Sandbox runtime embedding disabled in stub mode");
            }
            DepsMode::Prebuilt => {
                println!(
                    "cargo:warning=Prebuilt sandbox runtime requested, but no runtime/current/lib (or RKFORGE_RUNTIME_LIB_DIR) was found"
                );
            }
            DepsMode::Source => {
                println!(
                    "cargo:warning=Source sandbox runtime requested, but no vendored runtime sources were successfully prepared"
                );
            }
            DepsMode::System => {
                println!(
                    "cargo:warning=System sandbox runtime mode selected, but no discoverable libkrun/libkrunfw were found"
                );
            }
        }
    }

    body.push_str("];\n");
    body
}

fn prepare_runtime_libs_from_source(out_dir: &Path) -> Option<PathBuf> {
    let (libkrun_src, libkrunfw_src) = discover_runtime_source_dirs()?;
    let runtime_root = out_dir.join("sandbox-runtime-source");
    let libkrunfw_install = runtime_root.join("libkrunfw-install");
    let libkrun_install = runtime_root.join("libkrun-install");

    if !libkrunfw_install.join("lib64/libkrunfw.so").exists() {
        build_libkrunfw(&libkrunfw_src, &libkrunfw_install)
            .unwrap_or_else(|e| panic!("failed to build vendored libkrunfw: {e}"));
    }
    if !libkrun_install.join("lib64/libkrun.so").exists() {
        build_libkrun(&libkrun_src, &libkrun_install)
            .unwrap_or_else(|e| panic!("failed to build vendored libkrun: {e}"));
    }

    let lib_dir = libkrun_install.join("lib64");
    if !lib_dir.is_dir() {
        return None;
    }

    let fw_lib_dir = libkrunfw_install.join("lib64");
    if fw_lib_dir.is_dir() {
        stage_libkrunfw_into_libkrun_install(&fw_lib_dir, &lib_dir)
            .unwrap_or_else(|e| panic!("failed to stage vendored libkrunfw into runtime dir: {e}"));
    }

    Some(lib_dir)
}

fn prepare_prebuilt_runtime_libs(out_dir: &Path) -> Option<PathBuf> {
    if let Some(dir) = discover_runtime_lib_dir() {
        return Some(dir);
    }

    let tarball = discover_runtime_tarball()?;
    let extract_root = out_dir.join("sandbox-runtime-prebuilt");
    let runtime_root = extract_root.join("runtime");
    if !runtime_root.join("lib").is_dir() {
        if extract_root.exists() {
            fs::remove_dir_all(&extract_root).ok()?;
        }
        fs::create_dir_all(&extract_root).ok()?;
        extract_tarball(&tarball, &extract_root).ok()?;
    }
    runtime_root
        .join("lib")
        .is_dir()
        .then_some(runtime_root.join("lib"))
}

fn discover_runtime_source_dirs() -> Option<(PathBuf, PathBuf)> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);

    let libkrun_src = env::var_os(LIBKRUN_SRC_DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            let path = manifest_dir.join("vendor/libkrun");
            path.join("Makefile").exists().then_some(path)
        })?;

    let libkrunfw_src = env::var_os(LIBKRUNFW_SRC_DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            let path = manifest_dir.join("vendor/libkrunfw");
            path.join("Makefile").exists().then_some(path)
        })?;

    Some((libkrun_src, libkrunfw_src))
}

fn build_libkrunfw(source_dir: &Path, install_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(install_dir)?;
    run_make(source_dir, &[])?;
    run_make(
        source_dir,
        &[
            "install".to_string(),
            format!("PREFIX={}", install_dir.display()),
        ],
    )?;
    Ok(())
}

fn build_libkrun(source_dir: &Path, install_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(install_dir)?;
    run_make(source_dir, &["BLK=1".to_string()])?;
    run_make(
        source_dir,
        &[
            "install".to_string(),
            "BLK=1".to_string(),
            format!("PREFIX={}", install_dir.display()),
        ],
    )?;
    Ok(())
}

fn run_make(source_dir: &Path, args: &[String]) -> std::io::Result<()> {
    let jobs = std::thread::available_parallelism()
        .map(|n| n.get().to_string())
        .unwrap_or_else(|_| "4".to_string());
    let mut cmd = Command::new("make");
    cmd.arg(format!("-j{jobs}")).current_dir(source_dir);
    for arg in args {
        cmd.arg(arg);
    }
    let status = cmd.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "make failed in {} with status {:?}",
            source_dir.display(),
            status.code()
        )))
    }
}

fn stage_libkrunfw_into_libkrun_install(
    libkrunfw_lib_dir: &Path,
    target_lib_dir: &Path,
) -> std::io::Result<()> {
    fs::create_dir_all(target_lib_dir)?;
    for entry in fs::read_dir(libkrunfw_lib_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() && !fs::symlink_metadata(&path)?.file_type().is_symlink() {
            continue;
        }
        let dest = target_lib_dir.join(entry.file_name());
        if dest.exists() {
            continue;
        }
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &dest)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

fn collect_runtime_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() || fs::symlink_metadata(&path)?.file_type().is_symlink() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn discover_runtime_tarball() -> Option<PathBuf> {
    if let Some(path) = env::var_os(RUNTIME_TARBALL_ENV).map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }

    let url = env::var_os(RUNTIME_URL_ENV)
        .and_then(|value| value.into_string().ok())
        .or_else(default_runtime_url);
    if let Some(url) = url {
        let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
        let tarball_path = out_dir.join("sandbox-runtime-prebuilt.tar.gz");
        download_file(&url, &tarball_path).ok()?;
        return Some(tarball_path);
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);
    let candidates = [
        manifest_dir.join("runtime/current.tar.gz"),
        manifest_dir.join("runtime/prebuilt/runtime.tar.gz"),
    ];
    candidates.into_iter().find(|path| path.is_file())
}

fn default_runtime_url() -> Option<String> {
    let target = env::var("TARGET").ok()?;
    let version = env::var("CARGO_PKG_VERSION").ok()?;
    Some(format!(
        "{DEFAULT_PREBUILT_BASE_URL}/v{version}/rkforge-sandbox-runtime-v{version}-{target}.tar.gz"
    ))
}

fn download_file(url: &str, dest: &Path) -> std::io::Result<()> {
    let status = Command::new("curl")
        .args(["-fsSL", "-o", dest.to_str().unwrap_or_default(), url])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "failed to download prebuilt runtime from {url}"
        )))
    }
}

fn extract_tarball(tarball: &Path, dest: &Path) -> std::io::Result<()> {
    let status = Command::new("tar")
        .args([
            "-xzf",
            tarball.to_str().unwrap_or_default(),
            "-C",
            dest.to_str().unwrap_or_default(),
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "failed to extract prebuilt runtime tarball {}",
            tarball.display()
        )))
    }
}

fn discover_system_runtime_assets() -> Option<Vec<(String, PathBuf)>> {
    let libkrun = discover_library(
        "RKFORGE_LIBKRUN_LIBRARY",
        "RKFORGE_LIBKRUN_DIR",
        "libkrun.so",
    )?;
    let libkrunfw = discover_library(
        "RKFORGE_LIBKRUNFW_PATH",
        "RKFORGE_LIBKRUNFW_DIR",
        "libkrunfw.so",
    )?;

    let mut assets = Vec::new();
    assets.extend(runtime_asset_entries("libkrun.so", &libkrun));
    assets.extend(runtime_asset_entries("libkrunfw.so", &libkrunfw));
    Some(assets)
}

fn runtime_asset_entries(base_name: &str, source: &Path) -> Vec<(String, PathBuf)> {
    let resolved = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    let actual_name = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(base_name)
        .to_string();

    let mut names = vec![actual_name.clone(), base_name.to_string()];
    if let Some(major_alias) = major_alias_name(base_name, &actual_name)
        && !names.iter().any(|name| name == &major_alias)
    {
        names.push(major_alias);
    }
    names.sort();
    names.dedup();

    names
        .into_iter()
        .map(|name| (format!("lib/{name}"), resolved.clone()))
        .collect()
}

fn major_alias_name(base_name: &str, actual_name: &str) -> Option<String> {
    let suffix = actual_name.strip_prefix(&format!("{base_name}."))?;
    let major = suffix.split('.').next()?;
    Some(format!("{base_name}.{major}"))
}

fn discover_library(file_env: &str, dir_env: &str, base_name: &str) -> Option<PathBuf> {
    if let Some(path) = env::var_os(file_env).map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    if let Some(dir) = env::var_os(dir_env).map(PathBuf::from)
        && dir.is_dir()
        && let Some(path) = find_library_in_dir(&dir, base_name)
    {
        return Some(path);
    }

    for dir in standard_library_dirs() {
        if let Some(path) = find_library_in_dir(&dir, base_name) {
            return Some(path);
        }
    }
    None
}

fn standard_library_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/local/lib64"),
        PathBuf::from("/usr/local/lib"),
        PathBuf::from("/usr/lib64"),
        PathBuf::from("/usr/lib"),
        PathBuf::from("/lib64"),
        PathBuf::from("/lib"),
    ];
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/lib64"));
        dirs.push(home.join(".local/lib"));
    }
    dirs
}

fn find_library_in_dir(dir: &Path, base_name: &str) -> Option<PathBuf> {
    let direct = dir.join(base_name);
    if direct.is_file() {
        return Some(direct);
    }

    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(base_name) && path.is_file() {
            return Some(path);
        }
    }
    None
}
