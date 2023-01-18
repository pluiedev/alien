use std::path::{Path, PathBuf};

use eyre::Result;
use subprocess::Exec;

use crate::{util::ExecExt, Args};

use super::{
	rpm::{RpmSource, RpmTarget},
	Format, PackageInfo, SourcePackage, TargetPackage,
};

#[derive(Debug)]
pub struct LsbSource {
	rpm: RpmSource,
}

impl LsbSource {
	/// `lsb` files are `rpm`s with a lsb- prefix, that depend on
	/// a package called 'lsb' and nothing else.
	#[must_use]
	pub fn check_file(file: &Path) -> bool {
		let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
			return false;
		};
		if !stem.starts_with("lsb-") {
			return false;
		}
		let Some(ext) = file.extension() else {
			return false;
		};
		if ext != "rpm" {
			return false;
		}

		let Ok(deps) = Exec::cmd("rpm").env("LANG", "C").arg("-qRp").arg(file).log_and_output(None) else {
			return false;
		};

		deps.stdout_str().lines().any(|s| s.trim() == "lsb")
	}
	pub fn new(lsb_file: PathBuf, args: &Args) -> Result<Self> {
		let mut rpm = RpmSource::new(lsb_file, args)?;
		let info = rpm.info_mut();

		info.distribution = "Linux Standard Base".into();
		info.original_format = Format::Lsb;
		info.dependencies.push("lsb".into());
		info.use_scripts = true;

		Ok(Self { rpm })
	}
}
impl SourcePackage for LsbSource {
	fn info(&self) -> &PackageInfo {
		self.rpm.info()
	}
	fn info_mut(&mut self) -> &mut PackageInfo {
		self.rpm.info_mut()
	}
	fn into_info(self) -> PackageInfo {
		self.rpm.into_info()
	}

	fn unpack(&mut self) -> Result<PathBuf> {
		self.rpm.unpack()
	}

	/// LSB package versions are not changed.
	fn increment_release(&mut self, _bump: u32) {}
}

#[derive(Debug)]
pub struct LsbTarget {
	rpm: RpmTarget,
}
impl LsbTarget {
	/// Uses [`RpmTarget::new`] to generate the spec file.
	/// First though, the package's name is munged to make it LSB compliant (sorta)
	/// and `lsb` is added to its dependencies.
	pub fn new(mut info: PackageInfo, unpacked_dir: PathBuf) -> Result<Self> {
		if !info.name.starts_with("lsb-") {
			info.name.insert_str(0, "lsb-");
		}
		info.dependencies.push("lsb".into());

		let rpm = RpmTarget::new(info, unpacked_dir)?;

		Ok(Self { rpm })
	}
}
impl TargetPackage for LsbTarget {
	fn clean_tree(&mut self) -> Result<()> {
		self.rpm.clean_tree()
	}

	/// Uses [`RpmTarget::build`] to build the package, using `lsb-rpmbuild` if available.
	fn build(&mut self) -> Result<PathBuf> {
		if let Ok(lsb_rpmbuild) = which::which("lsb-rpmbuild") {
			self.rpm.build_with(&lsb_rpmbuild)
		} else {
			self.rpm.build()
		}
	}

	fn install(&mut self, file_name: &Path) -> Result<()> {
		self.rpm.install(file_name)
	}
}
