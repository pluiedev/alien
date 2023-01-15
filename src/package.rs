pub(crate) mod common;
pub mod deb;

use std::{path::{Path, PathBuf}, fmt::Display, collections::HashMap};

use enum_dispatch::enum_dispatch;
use enumflags2::BitFlags;
use simple_eyre::eyre::{bail, Result};

use deb::Deb;

use crate::Args;

#[enum_dispatch]
pub trait PackageBehavior {
    fn info(&self) -> &PackageInfo;
    fn info_mut(&mut self) -> &mut PackageInfo;

    fn install(&mut self, file_name: &Path) -> Result<()>;
    fn test(&mut self, file_name: &Path) -> Result<Vec<String>>;
    fn unpack(&mut self) -> Result<PathBuf>;
    fn prepare(&mut self, unpacked_dir: &Path) -> Result<()>;
    fn sanitize_info(&mut self) -> Result<()>;
    fn build(&mut self, unpacked_dir: &Path) -> Result<PathBuf>;
    fn revert(&mut self) {}

    fn increment_release(&mut self, bump: u32) {
        self.info_mut().release += bump;
    }
    fn set_arch(&mut self, arch: String) {
        self.info_mut().arch = arch;
    }
}

#[enum_dispatch(PackageBehavior)]
pub enum Package {
    // Rpm,
    Deb,
}
impl Package {
    pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
        // lsb > rpm > deb > tgz > slp > pkg

        if Deb::check_file(&file) {
            Deb::new(file, args).map(Package::Deb)
        } else {
            bail!("Unknown type of package, {}", file.display());
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub arch: String,
    pub maintainer: String,
    pub depends: Vec<String>,
    pub group: String,
    pub summary: String,
    pub description: String,
    pub copyright: String,
    pub original_format: Format,
    pub distribution: String,
    pub binary_info: String,
    pub conffiles: Vec<String>,
    pub file_list: Vec<PathBuf>,
    pub changelog_text: String,

    pub use_scripts: bool,
    pub preinst: Option<String>,
    pub prerm: Option<String>,
    pub postinst: Option<String>,
    pub postrm: Option<String>,
    pub owninfo: HashMap<String, String>,
    pub modeinfo: HashMap<String, String>,
}
impl PackageInfo {
    pub fn scripts(&self) -> Vec<&str> {
        [&self.postinst, &self.postrm, &self.preinst, &self.prerm]
            .into_iter()
            .filter_map(|o| o.as_deref())
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct OwnInfo {}

#[derive(Debug, Clone, Default)]
pub struct ModeInfo {}

#[enumflags2::bitflags]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Deb,
    Lsb,
    Pkg,
    Rpm,
    Slp,
    Tgz,
}
impl Format {
    pub fn new(args: &Args) -> BitFlags<Self> {
        let mut set = BitFlags::empty();
        if args.to_deb {
            set |= Self::Deb;
        }
        if args.to_lsb {
            set |= Self::Lsb;
        }
        if args.to_pkg {
            set |= Self::Pkg;
        }
        if args.to_rpm {
            set |= Self::Rpm;
        }
        if args.to_slp {
            set |= Self::Slp;
        }
        if args.to_tgz {
            set |= Self::Tgz;
        }

        if set.is_empty() {
            // Default to deb
            set |= Self::Deb;
        }
        set
    }
}
impl Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Format::Deb => "deb",
            Format::Lsb => "lsb",
            Format::Pkg => "pkg",
            Format::Rpm => "rpm",
            Format::Slp => "slp",
            Format::Tgz => "tgz",
        })
    }
}