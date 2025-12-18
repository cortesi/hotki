//! App bundle build helpers.

use std::{
    ffi::OsString,
    fs,
    io::{Error as IoError, ErrorKind},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use clap::Args;

use crate::{
    Error, Result,
    cmd::{run_status_quiet, run_status_streaming},
    workspace::workspace_version,
};

/// Arguments for `cargo xtask bundle`.
#[derive(Debug, Args)]
pub struct BundleArgs {
    /// Bundle application name (also used as `.app` directory name).
    #[arg(long, default_value = "Hotki")]
    app_name: String,
    /// Cargo binary name (also used as the bundle executable name).
    #[arg(long, default_value = "hotki")]
    bin_name: String,
    /// Bundle identifier.
    #[arg(long, default_value = "si.corte.hotki")]
    bundle_id: String,
    /// Output directory (relative to workspace root).
    #[arg(long, default_value = "target/bundle")]
    out_dir: PathBuf,
    /// Source icon PNG (relative to workspace root).
    #[arg(long, default_value = "crates/hotki/assets/logo.png")]
    icon_src: PathBuf,
}

/// Arguments for `cargo xtask bundle-dev`.
#[derive(Debug, Args)]
pub struct BundleDevArgs {
    /// Bundle application name (also used as `.app` directory name).
    #[arg(long, default_value = "Hotki-Dev")]
    app_name: String,
    /// Cargo binary name (also used as the bundle executable name).
    #[arg(long, default_value = "hotki")]
    bin_name: String,
    /// Bundle identifier.
    #[arg(long, default_value = "si.corte.hotki.dev")]
    bundle_id: String,
    /// Cargo feature flags (passed to `cargo build --features`).
    #[arg(long)]
    features: Vec<String>,
    /// Output directory (relative to workspace root).
    #[arg(long, default_value = "target/bundle-dev")]
    out_dir: PathBuf,
    /// Source icon PNG (relative to workspace root).
    #[arg(long, default_value = "crates/hotki/assets/logo-dev.png")]
    icon_src: PathBuf,
}

/// Build a release `.app` bundle.
pub fn bundle_release(root_dir: &Path, args: &BundleArgs) -> Result<()> {
    let version = workspace_version(root_dir)?;
    println!(
        "==> Building release bundle ({}, {})",
        args.app_name, version
    );

    run_status_streaming(
        root_dir,
        "cargo",
        ["build", "--release", "-p", "hotki", "--bin", &args.bin_name],
    )?;

    let bin_path = root_dir.join("target/release").join(&args.bin_name);
    if !bin_path.is_file() {
        return Err(Error::Io {
            path: bin_path,
            source: io_not_found("release binary missing after cargo build"),
        });
    }

    let out_dir = root_dir.join(&args.out_dir);
    let app_dir = out_dir.join(format!("{}.app", args.app_name));
    let contents_dir = app_dir.join("Contents");
    let macos_dir = contents_dir.join("MacOS");
    let res_dir = contents_dir.join("Resources");
    let iconset_dir = out_dir.join("icon.iconset");
    let icns_file = res_dir.join(format!("{}.icns", args.bin_name));
    let icon_src = root_dir.join(&args.icon_src);

    ensure_file_exists(&icon_src)?;
    prepare_bundle_dirs(&app_dir, &iconset_dir, &macos_dir, &res_dir)?;
    copy_themes(root_dir, &res_dir)?;

    generate_iconset(root_dir, &icon_src, &iconset_dir, IconsetProfile::Release)?;
    run_status_streaming(
        root_dir,
        "iconutil",
        iconutil_args(&iconset_dir, &icns_file),
    )?;

    let plist_path = contents_dir.join("Info.plist");
    write_file(
        &plist_path,
        release_plist(&args.app_name, &args.bin_name, &args.bundle_id, &version),
    )?;

    let bundle_bin_path = macos_dir.join(&args.bin_name);
    fs::copy(&bin_path, &bundle_bin_path).map_err(|source| Error::Io {
        path: bundle_bin_path.clone(),
        source,
    })?;
    chmod_executable(&bundle_bin_path)?;

    println!("==> Bundle ready: {}", app_dir.display());
    println!("    Run with: open \"{}\"", app_dir.display());
    Ok(())
}

/// Build a debug `.app` bundle (dev identifiers + icon).
pub fn bundle_dev(root_dir: &Path, args: &BundleDevArgs) -> Result<()> {
    let version = workspace_version(root_dir)?;
    println!("==> Building debug bundle ({}, {})", args.app_name, version);

    let mut cargo_args = vec![
        "build".to_string(),
        "-p".to_string(),
        "hotki".to_string(),
        "--bin".to_string(),
        args.bin_name.clone(),
    ];
    if !args.features.is_empty() {
        cargo_args.push("--features".to_string());
        cargo_args.push(args.features.join(","));
    }
    run_status_streaming(root_dir, "cargo", cargo_args.iter().map(String::as_str))?;

    let bin_path = root_dir.join("target/debug").join(&args.bin_name);
    if !bin_path.is_file() {
        return Err(Error::Io {
            path: bin_path,
            source: io_not_found("debug binary missing after cargo build"),
        });
    }

    let out_dir = root_dir.join(&args.out_dir);
    let app_dir = out_dir.join(format!("{}.app", args.app_name));
    let contents_dir = app_dir.join("Contents");
    let macos_dir = contents_dir.join("MacOS");
    let res_dir = contents_dir.join("Resources");
    let iconset_dir = out_dir.join("icon.iconset");
    let icns_file = res_dir.join(format!("{}.icns", args.bin_name));
    let icon_src = root_dir.join(&args.icon_src);

    ensure_file_exists(&icon_src)?;
    prepare_bundle_dirs(&app_dir, &iconset_dir, &macos_dir, &res_dir)?;
    copy_themes(root_dir, &res_dir)?;

    generate_iconset(root_dir, &icon_src, &iconset_dir, IconsetProfile::Dev)?;
    run_status_streaming(
        root_dir,
        "iconutil",
        iconutil_args(&iconset_dir, &icns_file),
    )?;
    remove_dir_all_if_exists(&iconset_dir)?;

    let plist_path = contents_dir.join("Info.plist");
    write_file(
        &plist_path,
        dev_plist(&args.app_name, &args.bin_name, &args.bundle_id, &version),
    )?;

    let bundle_bin_path = macos_dir.join(&args.bin_name);
    fs::copy(&bin_path, &bundle_bin_path).map_err(|source| Error::Io {
        path: bundle_bin_path.clone(),
        source,
    })?;
    chmod_executable(&bundle_bin_path)?;

    println!("==> Dev bundle ready: {}", app_dir.display());
    Ok(())
}

/// Recreate the bundle directory structure and the iconset directory.
fn prepare_bundle_dirs(
    app_dir: &Path,
    iconset_dir: &Path,
    macos_dir: &Path,
    res_dir: &Path,
) -> Result<()> {
    remove_dir_all_if_exists(app_dir)?;
    remove_dir_all_if_exists(iconset_dir)?;
    fs::create_dir_all(macos_dir).map_err(|source| Error::Io {
        path: macos_dir.to_path_buf(),
        source,
    })?;
    fs::create_dir_all(res_dir).map_err(|source| Error::Io {
        path: res_dir.to_path_buf(),
        source,
    })?;
    fs::create_dir_all(iconset_dir).map_err(|source| Error::Io {
        path: iconset_dir.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Remove a directory tree if it exists.
fn remove_dir_all_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    fs::remove_dir_all(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Copy bundled themes into the app bundle resources.
fn copy_themes(root_dir: &Path, res_dir: &Path) -> Result<()> {
    let themes_src = root_dir.join("themes");
    if !themes_src.is_dir() {
        return Ok(());
    }
    let themes_dst = res_dir.join("themes");
    remove_dir_all_if_exists(&themes_dst)?;
    copy_dir_recursive(&themes_src, &themes_dst)
}

/// Copy a directory tree recursively.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).map_err(|source| Error::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    for entry in fs::read_dir(src).map_err(|source| Error::Io {
        path: src.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| Error::Io {
            path: entry.path(),
            source,
        })?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path).map_err(|source| Error::Io {
                path: dst_path,
                source,
            })?;
        }
    }
    Ok(())
}

/// Ensure a required input file exists.
fn ensure_file_exists(path: &Path) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    Err(Error::Io {
        path: path.to_path_buf(),
        source: io_not_found("file does not exist"),
    })
}

/// Ensure the bundle binary is executable.
fn chmod_executable(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut perms = metadata.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Write a UTF-8 file.
fn write_file(path: &Path, contents: String) -> Result<()> {
    fs::write(path, contents).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Generate the `Info.plist` for the release bundle.
fn release_plist(app_name: &str, bin_name: &str, bundle_id: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>{bin_name}</string>
  <key>CFBundleIconFile</key>
  <string>{bin_name}</string>
  <key>CFBundleIdentifier</key>
  <string>{bundle_id}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>{app_name}</string>
  <key>CFBundleDisplayName</key>
  <string>{app_name}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>{version}</string>
  <key>CFBundleVersion</key>
  <string>{version}</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
"#
    )
}

/// Generate the `Info.plist` for the debug/dev bundle.
fn dev_plist(app_name: &str, bin_name: &str, bundle_id: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>{bin_name}</string>
    <key>CFBundleIconFile</key>
    <string>{bin_name}</string>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{app_name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
"#
    )
}

#[derive(Clone, Copy, Debug)]
/// Icon size profile for iconset generation.
enum IconsetProfile {
    /// A more complete iconset used by dev bundling.
    Dev,
    /// A minimal iconset used for release bundling.
    Release,
}

/// Generate an iconset directory suitable for `iconutil -c icns`.
fn generate_iconset(
    root_dir: &Path,
    icon_src: &Path,
    iconset_dir: &Path,
    profile: IconsetProfile,
) -> Result<()> {
    let sizes: &[(u32, u32)] = match profile {
        IconsetProfile::Dev => &[
            (16, 32),
            (32, 64),
            (64, 128),
            (128, 256),
            (256, 512),
            (512, 1024),
        ],
        IconsetProfile::Release => &[(16, 32), (32, 64), (128, 256), (256, 512), (512, 1024)],
    };

    for (base, two) in sizes {
        let base_out = iconset_dir.join(format!("icon_{base}x{base}.png"));
        let two_out = iconset_dir.join(format!("icon_{base}x{base}@2x.png"));

        let base = base.to_string();
        let two = two.to_string();
        run_status_quiet(root_dir, "sips", sips_args(&base, icon_src, &base_out))?;
        run_status_quiet(root_dir, "sips", sips_args(&two, icon_src, &two_out))?;
    }

    Ok(())
}

/// Create a `NotFound` IO error for internal checks.
fn io_not_found(message: &str) -> IoError {
    IoError::new(ErrorKind::NotFound, message)
}

/// Construct `iconutil` arguments.
fn iconutil_args(iconset_dir: &Path, icns_file: &Path) -> [OsString; 5] {
    [
        OsString::from("-c"),
        OsString::from("icns"),
        OsString::from("-o"),
        icns_file.as_os_str().to_os_string(),
        iconset_dir.as_os_str().to_os_string(),
    ]
}

/// Construct `sips` arguments for resizing a square icon.
fn sips_args(size: &str, icon_src: &Path, out_path: &Path) -> [OsString; 6] {
    [
        OsString::from("-z"),
        OsString::from(size),
        OsString::from(size),
        icon_src.as_os_str().to_os_string(),
        OsString::from("--out"),
        out_path.as_os_str().to_os_string(),
    ]
}
