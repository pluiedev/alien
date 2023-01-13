use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsStr,
    fs::File,
    io::{BufRead, Read},
    path::{Path, PathBuf},
    process::Command,
};

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use simple_eyre::eyre::{bail, Context, Result};
use xz::read::XzDecoder;

use crate::{
    util::{CommandExt, Verbosity},
    Args,
};

use super::{Format, PackageBehavior, PackageInfo};

const PATCH_DIRS: &[&str] = &["/var/lib/alien", "/usr/share/alien/patches"];

pub struct Deb {
    info: PackageInfo,
    dpkg_deb: Option<PathBuf>,
    fix_perms: bool,
    patch_file: Option<PathBuf>,
}
impl Deb {
    pub fn check_file(file: &Path) -> bool {
        match file.extension() {
            Some(o) => o.eq_ignore_ascii_case("deb"),
            None => false,
        }
    }

    pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
        let mut info = PackageInfo {
            distribution: "Debian".into(),
            original_format: Format::Deb,
            ..Default::default()
        };

        let dpkg_deb = which::which("dpkg-deb").ok();

        let mut control_files = fetch_control_files(
            dpkg_deb.as_deref(),
            &file,
            &[
                "control",
                "conffiles",
                "postinst",
                "postrm",
                "preinst",
                "prerm",
            ],
        )?;
        dbg!(&control_files);
        let Some(control) = control_files.remove("control") else {
            bail!("Control file not found!");
        };

        let mut description = String::new();
        let mut field = String::new();
        for c in control.lines() {
            if c.starts_with(' ') && field == "description" {
                // Handle extended description
                let c = c.trim_start();
                if c != "." {
                    description.push_str(c);
                    description.push('\n');
                }
            } else if let Some((f, value)) = c.split_once(":") {
                let value = value.trim().to_owned();
                // Really old debs might have oddly capitalized field names.
                field = f.to_ascii_lowercase();

                match field.as_str() {
                    "package" => info.name = value,
                    "version" => info.version = value,
                    "architecture" => info.arch = value,
                    "maintainer" => info.maintainer = value,
                    "section" => info.group = value,
                    "description" => info.summary = value,
                    "depends" => info.depends = value,
                    _ => { /* ignore */ }
                }
            }
        }

        info.description = description;
        info.copyright = format!("see /usr/share/doc/{}/copyright", info.name);
        if info.group.is_empty() {
            info.group.push_str("unknown");
        }
        info.binary_info = control;

        if let Some(conffiles) = control_files.remove("conffiles") {
            info.conffiles
                .extend(conffiles.lines().map(|s| s.trim().to_owned()));
        };

        // In the tar file, the files are all prefixed with "./", but we want them
        // to be just "/". So, we gotta do this!
        info.file_list.extend(
            fetch_data_members(dpkg_deb.as_deref(), &file)?
                .into_iter()
                .map(|s| {
                    std::iter::once(OsStr::new("/"))
                        .chain(s.iter().skip_while(|&x| x == "."))
                        .collect::<PathBuf>()
                }),
        );

        info.postinst = control_files.remove("postinst");
        info.postrm = control_files.remove("postrm");
        info.preinst = control_files.remove("preinst");
        info.prerm = control_files.remove("prerm");

        let patch_file = if args.nopatch {
            None
        } else {
            match &args.patch {
                Some(o) => Some(o.clone()),
                None => get_patch(&info, args.anypatch, PATCH_DIRS),
            }
        };

        dbg!(&info);

        Ok(Self {
            info,
            dpkg_deb,
            fix_perms: args.fixperms,
            patch_file,
        })
    }
}
impl PackageBehavior for Deb {
    fn info(&self) -> &PackageInfo {
        &self.info
    }
    fn info_mut(&mut self) -> &mut PackageInfo {
        &mut self.info
    }
    fn install(&mut self, file_name: &Path) -> Result<()> {
        Command::new("dpkg")
            .args(["--no-force-overwrite", "-i"])
            .arg(file_name)
            .log_and_spawn(Verbosity::VeryVerbose)
            .wrap_err("Unable to install")?;
        Ok(())
    }

    fn test(&mut self, file_name: &Path) -> Result<Vec<String>> {
        let Ok(lintian) = which::which("lintian") else {
            return Ok(vec!["lintian not available, so not testing".into()]);
        };

        let strings = Command::new(lintian)
            .arg(file_name)
            .output()?
            .stdout
            .lines()
            .filter_map(|s| s.ok())
            // Ignore errors we don't care about
            .filter(|s| !s.contains("unknown-section alien"))
            .map(|s| s.trim().to_owned())
            .collect();

        Ok(strings)
    }

    fn unpack(&mut self) -> PathBuf {
        todo!()
    }

    fn prepare(&mut self) {
        todo!()
    }

    fn build(&mut self) -> PathBuf {
        todo!()
    }

    fn check_file(&mut self, file_name: &str) -> bool {
        todo!()
    }
}

//= Utilties

fn get_patch(info: &PackageInfo, anypatch: bool, dirs: &[&str]) -> Option<PathBuf> {
    let mut patches: Vec<_> = dirs
        .iter()
        .flat_map(|dir| {
            let p = format!(
                "{}/{}_{}-{}*.diff.gz",
                dir, info.name, info.version, info.release
            );
            glob::glob(&p).unwrap()
        })
        .collect();

    if patches.is_empty() {
        // Try not matching the release, see if that helps.
        patches.extend(dirs.iter().flat_map(|dir| {
            let p = format!("{}/{}_{}*.diff.gz", dir, info.name, info.version);
            glob::glob(&p).unwrap()
        }));

        if !patches.is_empty() && anypatch {
            // Fall back to anything that matches the name.
            patches.extend(dirs.iter().flat_map(|dir| {
                let p = format!("{}/{}_*.diff.gz", dir, info.name);
                glob::glob(&p).unwrap()
            }))
        }
    }

    // just get the first one
    patches.into_iter().find_map(|p| p.ok())
}

fn fetch_control_files(
    dpkg_deb: Option<&Path>,
    deb_file: &Path,
    control_files: &[&'static str],
) -> Result<HashMap<&'static str, String>> {
    if let Some(dpkg_deb) = dpkg_deb {
        let mut map = HashMap::new();
        for file in control_files {
            let out = Command::new(dpkg_deb)
                .arg("--info")
                .arg(deb_file)
                .arg(file)
                .log_and_output(None)?;

            if out.status.success() {
                map.insert(*file, String::from_utf8(out.stdout)?);
            }
        }
        Ok(map)
    } else {
        // Fallback - perform manual extraction if `dpkg-deb` is not installed.

        // Step 1: Open the deb file as an `ar` archive,
        // and locate `control.tar(.gz|.xz)?`.

        let mut ar = ar::Archive::new(File::open(deb_file)?);
        while let Some(entry) = ar.next_entry() {
            let mut entry = entry?;
            let id = entry.header().identifier();

            if !id.starts_with(b"control.tar") {
                continue;
            }

            // Load the control tar file, applying gzip/xz decompression if necessary.
            let mut tar = vec![];
            match id {
                b"control.tar.gz" => GzDecoder::new(entry).read_to_end(&mut tar).unwrap(),
                b"control.tar.xz" => XzDecoder::new(entry).read_to_end(&mut tar).unwrap(),
                // it's already a tarball
                b"control.tar" => entry.read_to_end(&mut tar).unwrap(),
                _ => bail!("Unknown control member!"),
            };

            // Find the actual control file we're looking for, inside the tar file.
            let mut tar = tar::Archive::new(tar.as_slice());

            // Go through all entries, and if an entry has a path, and that path's
            // file name matches a control file we're looking for, then add that to the map.
            let mut map = HashMap::new();
            for entry in tar.entries()? {
                let mut entry = entry?;

                // if-let-chains stable when
                if let Ok(path) = entry.path() {
                    if let Some(name) = path.file_name() {
                        if let Some(cf) = control_files.iter().find(|&&s| s == name) {
                            let mut data = String::new();
                            entry.read_to_string(&mut data)?;
                            map.insert(*cf, data);
                        }
                    }
                }
            }

            return Ok(map);
        }
        bail!("Cannot find control member!");
    }
}

fn fetch_data_members(dpkg_deb: Option<&Path>, deb_file: &Path) -> Result<Vec<PathBuf>> {
    let tar = if let Some(dpkg_deb) = dpkg_deb {
        Command::new(dpkg_deb)
            .arg("--fsys-tarfile")
            .arg(deb_file)
            .log_and_output(None)?
            .stdout
    } else {
        // Fallback - perform manual extraction if `dpkg-deb` is not installed.

        let mut tar = vec![];
        let mut ar = ar::Archive::new(File::open(deb_file)?);
        while let Some(entry) = ar.next_entry() {
            let mut entry = entry?;
            let id = entry.header().identifier();

            if !id.starts_with(b"data.tar") {
                continue;
            }
            match id {
                b"data.tar.gz" => GzDecoder::new(entry).read_to_end(&mut tar).unwrap(),
                b"data.tar.bz2" => BzDecoder::new(entry).read_to_end(&mut tar).unwrap(),
                b"data.tar.xz" | b"data.tar.lzma" => {
                    XzDecoder::new(entry).read_to_end(&mut tar).unwrap()
                }
                // it's already a tarball
                b"control.tar" => entry.read_to_end(&mut tar).unwrap(),
                _ => bail!("Unknown data member!"),
            };
            break;
        }
        if tar.is_empty() {
            bail!("Cannot find data member!");
        }
        tar
    };

    let mut tar = tar::Archive::new(tar.as_slice());
    Ok(tar
        .entries()?
        .filter_map(|f| f.ok())
        .filter_map(|f| f.path().map(Cow::into_owned).ok())
        .collect())
}
