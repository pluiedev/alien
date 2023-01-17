use std::{
	collections::HashMap,
	fmt::Display,
	path::{Path, PathBuf},
};

use enum_dispatch::enum_dispatch;
use enumflags2::BitFlags;
use simple_eyre::eyre::{bail, Result};

use crate::Args;

use self::{
	deb::{DebSource, DebTarget},
	lsb::{LsbSource, LsbTarget},
	rpm::{RpmSource, RpmTarget},
};

pub mod deb;
pub mod lsb;
pub mod rpm;

#[enum_dispatch]
pub trait SourcePackageBehavior {
	fn info(&self) -> &PackageInfo;
	fn info_mut(&mut self) -> &mut PackageInfo;
	fn into_info(self) -> PackageInfo;

	fn unpack(&mut self) -> Result<PathBuf>;

	fn increment_release(&mut self, bump: u32) {
		let release = &mut self.info_mut().release;

		*release = if let Ok(num) = release.parse::<u32>() {
			(num + bump).to_string()
		} else {
			// Perl's string-number addition thing is... cursed.
			// If a string doesn't parse to a number, then it is treated as 0.
			// So, we will just set the release to the bump here.
			bump.to_string()
		};
	}
}
#[enum_dispatch]
pub trait TargetPackageBehavior {
	fn clear_unpacked_dir(&mut self);

	fn clean_tree(&mut self) -> Result<()> {
		Ok(())
	}
	fn build(&mut self) -> Result<PathBuf>;
	fn test(&mut self, _file_name: &Path) -> Result<Vec<String>> {
		Ok(vec![])
	}
	fn install(&mut self, file_name: &Path) -> Result<()>;
	fn revert(&mut self) {}
}

#[enum_dispatch(SourcePackageBehavior)]
pub enum SourcePackage {
	Lsb(LsbSource),
	Rpm(RpmSource),
	Deb(DebSource),
}
impl SourcePackage {
	pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
		// lsb > rpm > deb > tgz > slp > pkg

		if LsbSource::check_file(&file) {
			LsbSource::new(file, args).map(Self::Lsb)
		} else if RpmSource::check_file(&file) {
			RpmSource::new(file, args).map(Self::Rpm)
		} else if DebSource::check_file(&file) {
			DebSource::new(file, args).map(Self::Deb)
		} else {
			bail!("Unknown type of package, {}", file.display());
		}
	}
}
#[enum_dispatch(TargetPackageBehavior)]
pub enum TargetPackage {
	Lsb(LsbTarget),
	Rpm(RpmTarget),
	Deb(DebTarget),
}
impl TargetPackage {
	pub fn new(
		format: Format,
		info: PackageInfo,
		unpacked_dir: PathBuf,
		args: &Args,
	) -> Result<Self> {
		let target = match format {
			Format::Deb => Self::Deb(DebTarget::new(info, unpacked_dir, args)?),
			Format::Lsb => Self::Lsb(LsbTarget::new(info, unpacked_dir)?),
			Format::Pkg => todo!(),
			Format::Rpm => Self::Rpm(RpmTarget::new(info, unpacked_dir)?),
			Format::Slp => todo!(),
			Format::Tgz => todo!(),
		};
		Ok(target)
	}
}

#[derive(Debug, Default, Clone)]
pub struct PackageInfo {
	pub file: PathBuf,

	pub name: String,
	pub version: String,
	pub release: String,
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
	pub conffiles: Vec<PathBuf>,
	pub file_list: Vec<PathBuf>,
	pub changelog_text: String,

	pub use_scripts: bool,
	pub scripts: HashMap<&'static str, String>,
	pub owninfo: HashMap<PathBuf, String>,
	pub modeinfo: HashMap<PathBuf, u32>,
}
impl PackageInfo {
	pub const SCRIPTS: &'static [&'static str] = &["preinst", "postinst", "prerm", "postrm"];
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
