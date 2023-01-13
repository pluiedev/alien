pub mod deb;

use std::path::{Path, PathBuf};

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
    fn unpack(&mut self) -> PathBuf;
    fn prepare(&mut self);
    fn clean_tree(&self) {}
    fn build(&mut self) -> PathBuf;
    fn revert(&mut self) {}

    fn check_file(&mut self, file_name: &str) -> bool;

    fn increment_release(&mut self, bump: u32) {
        self.info_mut().release += bump;
    }

    fn set_arch(&mut self, arch: &str) {}
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
    pub depends: String, // vec?
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

    pub unpacked_tree: Option<PathBuf>,
    // pub owninfo: Option<HashMap<String, OwnInfo>>,
    // pub modeinfo: Option<HashMap<String, ModeInfo>>,
}

pub struct OwnInfo {}
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
