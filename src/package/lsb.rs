use std::path::{Path, PathBuf};

use simple_eyre::Result;
use subprocess::Exec;

use crate::{util::ExecExt, Args};

use super::{
	rpm::{RpmSource, RpmTarget},
	Format, PackageInfo, SourcePackageBehavior, TargetPackageBehavior,
};
pub struct LsbSource {
	rpm: RpmSource,
}

impl LsbSource {
	/// `lsb` files are `rpm`s with a lsb- prefix, that depend on
	/// a package called 'lsb' and nothing else.
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
		info.depends.push("lsb".into());
		info.use_scripts = true;

		Ok(Self { rpm })
	}
}
impl SourcePackageBehavior for LsbSource {
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

pub struct LsbTarget {
	rpm: RpmTarget,
	original_name: String,
	original_depends: Vec<String>,
	original_use_scripts: bool,
}
impl LsbTarget {
	/// Uses [`Rpm::prepare`] to generate the spec file.
	/// First though, the package's name is munged to make it LSB compliant (sorta)
	/// and `lsb` is added to its dependencies.
	pub fn new(mut info: PackageInfo, unpacked_dir: PathBuf) -> Result<Self> {
		let PackageInfo {
			name,
			depends,
			use_scripts,
			..
		} = &mut info;

		let original_name = name.clone();
		let original_depends = depends.clone();
		let original_use_scripts = *use_scripts;

		if !name.starts_with("lsb-") {
			name.insert_str(0, "lsb-");
		}

		let rpm = RpmTarget::new(info, unpacked_dir)?;

		Ok(Self {
			rpm,
			original_name,
			original_depends,
			original_use_scripts,
		})
	}
}
impl TargetPackageBehavior for LsbTarget {
	fn clear_unpacked_dir(&mut self) {
		self.rpm.clear_unpacked_dir();
	}

	fn clean_tree(&mut self) -> Result<()> {
		self.rpm.clean_tree()
	}

	/// Uses [`Rpm::build`] to build the package, using `lsb-rpmbuild` if available.
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

	/// Undoes the changes made by [`Self::prepare`].
	fn revert(&mut self) {
		let info = &mut self.rpm.info;
		info.name = self.original_name.clone();
		info.depends = self.original_depends.clone();
		info.use_scripts = self.original_use_scripts;
	}
}
