use std::{fs::File, path::PathBuf};

use eyre::Result;

use crate::{
	util::{chmod, mkdir},
	PackageInfo, TargetPackage,
};

#[derive(Debug)]
pub struct TgzTarget {
	info: PackageInfo,
	unpacked_dir: PathBuf,
}
impl TgzTarget {
	pub fn new(info: PackageInfo, unpacked_dir: PathBuf) -> Result<Self> {
		let mut created_install_folder = false;
		if info.use_scripts {
			for (script, data) in &info.scripts {
				if data.chars().all(char::is_whitespace) {
					continue;
				}

				let mut out = unpacked_dir.join("install");
				if !created_install_folder {
					mkdir(&out)?;
					chmod(&out, 0o755)?;
					created_install_folder = true;
				}
				out.push(script.tgz_script_name());

				std::fs::write(&out, data)?;
				chmod(&out, 0o755)?;
			}
		}

		Ok(Self { info, unpacked_dir })
	}
}
impl TargetPackage for TgzTarget {
	fn build(&mut self) -> Result<PathBuf> {
		let path = format!("{}-{}.tgz", self.info.name, self.info.version);
		let path = PathBuf::from(path);

		let mut tgz = tar::Builder::new(File::create(&path)?);
		tgz.append_dir_all(".", &self.unpacked_dir)?;

		Ok(path)
	}
}
