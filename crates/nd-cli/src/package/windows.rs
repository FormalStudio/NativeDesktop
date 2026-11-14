//! Windows: a branded Velopack `Setup.exe` (name, icon, splash, publisher, accent and shortcuts
//! from `[windows]`), an MSI on request, signed with the identity `[windows]` names. `files` are
//! copied beside the executable, and so is Chromium's runtime when the executable links
//! `libcef.dll`.
//!
//! Signing runs only on a Windows host (`vpk` signs through signtool there), with Azure Artifact
//! Signing (`[windows.azure]`) or a certificate from the store (`[windows.certificate]`). A
//! cross pack from macOS or Linux signs only through `ND_WINDOWS_SIGN_TEMPLATE`, a command with
//! `{{file}}` (for example jsign), and otherwise refuses rather than ship unsigned by accident.
//! File associations and URL schemes are not registered by Setup.exe yet.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nd_config::{Shortcut, WindowsSigning};

use super::{Options, Packed, icon};
use crate::project::{Project, copy_files, fresh_dir};
use crate::vpk;

pub fn package(project: &Project, opts: &Options) -> Result<Packed> {
    let work = project.nd_dir().join("package/windows");
    fresh_dir(&work)?;
    let exe = match &opts.binary {
        Some(path) => path.clone(),
        None if cfg!(windows) => project.build_release(None)?,
        None => bail!("nd package windows off Windows needs --binary <a Windows build of {}.exe>", project.bin),
    };
    let dir = work.join("app");
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(&exe, dir.join(format!("{}.exe", project.bin)))
        .with_context(|| format!("copying {}", exe.display()))?;
    copy_files(project, "windows", &project.config.windows.files, &dir)?;
    stage_cef(project, &exe, &dir)?;
    let ico = match &project.config.app.icon {
        Some(source) => {
            let path = work.join("app.ico");
            icon::write_ico(&icon::load(&project.resolve(source))?, &path)?;
            Some(path)
        }
        None => None,
    };
    let signing = Signing::from_config(project, &work, cfg!(windows))?;
    let vpk = vpk::locate()?;
    std::fs::create_dir_all(&opts.releases)?;
    vpk::run(&vpk, &pack_args(project, opts, &dir, ico.as_deref(), &signing, cfg!(windows)))?;
    let setup = opts.releases.join(format!("{}-{}-Setup.exe", project.id(), opts.channel));
    Ok(Packed { app: dir, installer: setup.exists().then_some(setup), releases: opts.releases.clone() })
}

/// Chromium's runtime, from beside the built executable, when the executable links it.
fn stage_cef(project: &Project, exe: &Path, dir: &Path) -> Result<()> {
    if !super::cef::links_cef(exe, "libcef.dll")? {
        if super::cef::enabled(project) {
            bail!(
                "[webview] engine = \"cef\", but {} does not link libcef.dll: enable nd's `cef` feature",
                exe.display()
            );
        }
        return Ok(());
    }
    let built = exe.parent().context("the executable has no directory")?;
    super::cef::stage(&super::cef::WINDOWS_RUNTIME, built, dir)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signing {
    None,
    /// `metadata.json` for Azure Artifact Signing, written from `[windows.azure]`.
    Azure(PathBuf),
    /// signtool's parameters for `[windows.certificate]`.
    Params(String),
    /// A signing command with `{{file}}` (`ND_WINDOWS_SIGN_TEMPLATE`).
    Template(String),
}

impl Signing {
    fn from_config(project: &Project, work: &Path, on_windows: bool) -> Result<Self> {
        let win = &project.config.windows;
        let Some(method) = win.signing else { return Ok(Self::None) };
        if !on_windows {
            let template = std::env::var("ND_WINDOWS_SIGN_TEMPLATE").ok().filter(|t| !t.is_empty());
            return template
                .map(Self::Template)
                .context("windows.signing signs on a Windows host; cross-packing needs ND_WINDOWS_SIGN_TEMPLATE");
        }
        match method {
            WindowsSigning::Azure => {
                let azure = win.azure.as_ref().context("windows.signing = \"azure\" needs [windows.azure]")?;
                let metadata = work.join("metadata.json");
                let json = serde_json::json!({
                    "Endpoint": azure.endpoint,
                    "CodeSigningAccountName": azure.account,
                    "CertificateProfileName": azure.profile,
                });
                std::fs::write(&metadata, serde_json::to_vec_pretty(&json)?)
                    .with_context(|| format!("writing {}", metadata.display()))?;
                Ok(Self::Azure(metadata))
            }
            WindowsSigning::Certificate => {
                let cert = win
                    .certificate
                    .as_ref()
                    .context("windows.signing = \"certificate\" needs [windows.certificate]")?;
                Ok(Self::Params(format!("/sha1 {} /fd sha256 /tr {} /td sha256", cert.thumbprint, cert.timestamp_url)))
            }
        }
    }
}

pub fn pack_args(
    project: &Project,
    opts: &Options,
    dir: &Path,
    ico: Option<&Path>,
    signing: &Signing,
    on_windows: bool,
) -> Vec<String> {
    let win = &project.config.windows;
    let mut args: Vec<String> = Vec::new();
    if !on_windows {
        args.push("[win]".into());
    }
    args.extend([
        "pack".into(),
        "--runtime".into(),
        opts.runtime.clone().unwrap_or_else(|| "win-x64".into()),
        "--packId".into(),
        project.id().into(),
        "--packVersion".into(),
        project.version.clone(),
        "--packDir".into(),
        dir.display().to_string(),
        "--mainExe".into(),
        format!("{}.exe", project.bin),
        "--packTitle".into(),
        project.display_name().into(),
        "--channel".into(),
        opts.channel.clone(),
        "--outputDir".into(),
        opts.releases.display().to_string(),
    ]);
    if let Some(publisher) = &win.publisher {
        args.extend(["--packAuthors".into(), publisher.clone()]);
    }
    if let Some(ico) = ico {
        args.extend(["--icon".into(), ico.display().to_string()]);
    }
    if let Some(splash) = &win.installer.splash {
        args.extend(["--splashImage".into(), project.resolve(splash).display().to_string()]);
    }
    if let Some(accent) = &win.accent {
        args.extend(["--splashProgressColor".into(), accent.clone()]);
    }
    if !win.installer.shortcuts.is_empty() {
        let locations: Vec<&str> = win
            .installer
            .shortcuts
            .iter()
            .map(|s| match s {
                Shortcut::Start => "StartMenuRoot",
                Shortcut::Desktop => "Desktop",
            })
            .collect();
        args.extend(["--shortcuts".into(), locations.join(",")]);
    }
    if win.installer.msi && on_windows {
        args.extend(["--msi".into(), "--instLocation".into(), "PerUser".into()]);
    }
    match signing {
        Signing::None => {}
        Signing::Azure(metadata) => args.extend(["--azureTrustedSignFile".into(), metadata.display().to_string()]),
        Signing::Params(params) => args.extend(["--signParams".into(), params.clone()]),
        Signing::Template(template) => args.extend(["--signTemplate".into(), template.clone()]),
    }
    if let Some(notes) = &opts.notes {
        args.extend(["--releaseNotes".into(), notes.display().to_string()]);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::tests::project;

    const BRANDED: &str = r##"
[app]
id = "dev.formalsnake.notes"
display_name = "Notes"
[windows]
publisher = "FormalSnake"
accent = "#3366ff"
installer = { splash = "assets/splash.png", shortcuts = ["start", "desktop"], msi = true }
signing = "azure"
azure = { endpoint = "https://weu.codesigning.azure.net", account = "formalsnake", profile = "notes" }
"##;

    #[test]
    fn setup_is_branded_from_the_config() {
        let p = project(BRANDED);
        let opts = Options::for_test("stable");
        let args = pack_args(&p, &opts, Path::new("app"), Some(Path::new("app.ico")), &Signing::None, false);
        let joined = args.join(" ");
        assert!(joined.starts_with("[win] pack --runtime win-x64 --packId dev.formalsnake.notes"), "{joined}");
        for needle in [
            "--mainExe demo.exe",
            "--packTitle Notes",
            "--packAuthors FormalSnake",
            "--icon app.ico",
            "--splashProgressColor #3366ff",
            "--shortcuts StartMenuRoot,Desktop",
        ] {
            assert!(joined.contains(needle), "missing {needle} in {joined}");
        }
        assert!(joined.contains("assets/splash.png"));
        assert!(!joined.contains("--msi"), "MSI only builds on a Windows host");

        let native = pack_args(&p, &opts, Path::new("app"), None, &Signing::Azure("m.json".into()), true).join(" ");
        assert!(native.starts_with("pack "));
        assert!(native.contains("--msi --instLocation PerUser"));
        assert!(native.contains("--azureTrustedSignFile m.json"));
    }

    #[test]
    fn files_land_beside_the_executable() {
        let p = project("[app]\nid = \"a.b\"\n[windows]\nfiles = [\"bin/helper.exe\"]\n");
        std::fs::create_dir_all(p.root.join("bin")).unwrap();
        std::fs::write(p.root.join("bin/helper.exe"), b"helper").unwrap();
        let dir = p.root.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        copy_files(&p, "windows", &p.config.windows.files, &dir).unwrap();
        assert_eq!(std::fs::read(dir.join("helper.exe")).unwrap(), b"helper");

        let missing = project("[app]\nid = \"a.b\"\n[windows]\nfiles = [\"gone.exe\"]\n");
        let err = copy_files(&missing, "windows", &missing.config.windows.files, &missing.root).unwrap_err();
        assert!(format!("{err:#}").contains("gone.exe"));
    }

    #[test]
    fn signing_off_windows_needs_a_template() {
        let p = project(BRANDED);
        // SAFETY: tests in this module do not read the variable concurrently.
        unsafe { std::env::remove_var("ND_WINDOWS_SIGN_TEMPLATE") };
        let err = Signing::from_config(&p, &p.root, false).unwrap_err().to_string();
        assert!(err.contains("ND_WINDOWS_SIGN_TEMPLATE"), "{err}");
        let unsigned = project("[app]\nid = \"a.b\"\n");
        assert_eq!(Signing::from_config(&unsigned, &unsigned.root, false).unwrap(), Signing::None);
    }

    #[test]
    fn signing_identity_comes_from_the_config() {
        let p = project(BRANDED);
        let Signing::Azure(metadata) = Signing::from_config(&p, &p.root, true).unwrap() else { panic!() };
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(metadata).unwrap()).unwrap();
        assert_eq!(json["Endpoint"], "https://weu.codesigning.azure.net");
        assert_eq!(json["CodeSigningAccountName"], "formalsnake");
        assert_eq!(json["CertificateProfileName"], "notes");

        let cert = project(
            "[app]\nid = \"a.b\"\n[windows]\nsigning = \"certificate\"\n\
             certificate = { thumbprint = \"ab12\", timestamp_url = \"http://ts.example\" }\n",
        );
        let signing = Signing::from_config(&cert, &cert.root, true).unwrap();
        assert_eq!(signing, Signing::Params("/sha1 ab12 /fd sha256 /tr http://ts.example /td sha256".into()));
        let args = pack_args(&cert, &Options::for_test("stable"), Path::new("app"), None, &signing, true).join(" ");
        assert!(args.contains("--signParams /sha1 ab12"), "{args}");
    }

    #[test]
    fn a_cef_app_carries_the_runtime() {
        let p = project("[app]\nid = \"a.b\"\n[webview]\nengine = \"cef\"\n");
        let built = p.root.join("built");
        std::fs::create_dir_all(built.join("locales")).unwrap();
        for name in super::super::cef::WINDOWS_RUNTIME.iter().filter(|n| **n != "locales") {
            std::fs::write(built.join(name), b"x").unwrap();
        }
        let app = p.root.join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(built.join("plain.exe"), b"MZ plain").unwrap();
        assert!(stage_cef(&p, &built.join("plain.exe"), &app).unwrap_err().to_string().contains("libcef.dll"));
        std::fs::write(built.join("demo.exe"), b"MZ libcef.dll\0").unwrap();
        stage_cef(&p, &built.join("demo.exe"), &app).unwrap();
        assert!(app.join("libcef.dll").exists() && app.join("locales").is_dir());
    }
}
