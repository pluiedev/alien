use std::{
	collections::HashMap,
	fmt::Debug,
	fs::File,
	io::{Read, Seek},
	path::{Path, PathBuf},
};

use eyre::Result;
use subprocess::Exec;

use crate::{
	util::{make_unpack_work_dir, ExecExt},
	Format, PackageInfo, Script, SourcePackage,
};

pub struct TgzSource {
	info: PackageInfo,
	tar: tar::Archive<File>,
}
impl TgzSource {
	#[must_use]
	pub fn check_file(file: &Path) -> bool {
		let Some(f) = file.file_name() else { return false; };
		let f = f.to_string_lossy();

		let Some((rest, ext)) = f.rsplit_once('.') else { return false; };
		let ext = ext.to_ascii_lowercase();

		match ext.as_str() {
			"tgz" | "taz" => true,
			"gz" | "z" | "bz" | "bz2" => {
				if let Some((_, ext2)) = rest.rsplit_once('.') {
					ext2.eq_ignore_ascii_case("tar")
				} else {
					false
				}
			}
			_ => false,
		}
	}
	pub fn new(file: PathBuf) -> Result<Self> {
		let mut basename = if let Some(file_name) = file.file_name() {
			PathBuf::from(file_name)
		} else {
			file.clone()
		};
		basename.set_extension("");
		let basename = basename.to_string_lossy();

		let (name, version) = basename.rsplit_once('-').unwrap_or((&basename, "1"));
		let (name, version) = (name.to_owned(), version.to_owned());

		let binary_info = Exec::cmd("ls")
			.arg("-l")
			.arg(&file)
			.log_and_output(None)?
			.stdout_str();

		let mut conffiles = vec![];
		let mut files = vec![];
		let mut scripts = HashMap::new();

		let mut tar = tar::Archive::new(File::open(&file)?);
		for entry in tar.entries()? {
			let mut entry = entry?;
			let header = entry.header();
			let mut path = PathBuf::from("/");
			path.push(header.path()?);

			// Assume any regular file (non-directory) in /etc/ is a conffile.
			if path.starts_with("/etc/") && header.mode()? & 0o1000 == 0 {
				// If entry is just a regular file and not a directory

				conffiles.push(path.clone());
			} else if path.starts_with("/install/") {
				// It might be a script!

				let Some(name) = path.file_name() else { continue; };
				let name = name.to_string_lossy();
				let Some(script) = Script::from_tgz_script_name(&name) else { continue; };

				let mut content = String::new();
				entry.read_to_string(&mut content)?;
				scripts.insert(script, content);
			} else {
				// Regular old file
				files.push(path);
			}
		}

		let info = PackageInfo {
			file,
			name,
			version,
			release: "1".into(),
			arch: "all".into(),
			group: "unknown".into(),
			summary: "Converted tgz package".into(),
			description: "Converted tgz package".into(),
			copyright: "unknown".into(),
			original_format: Format::Tgz,
			distribution: "Slackware/tarball".into(),
			binary_info,
			conffiles,
			files,
			scripts,
			..Default::default()
		};

		// Rewind tar to
		let mut tar = tar.into_inner();
		tar.rewind()?;
		let tar = tar::Archive::new(tar);

		Ok(Self { info, tar })
	}
}
impl SourcePackage for TgzSource {
	fn info(&self) -> &PackageInfo {
		&self.info
	}
	fn info_mut(&mut self) -> &mut PackageInfo {
		&mut self.info
	}
	fn into_info(self) -> PackageInfo {
		self.info
	}
	fn unpack(&mut self) -> Result<PathBuf> {
		let work_dir = make_unpack_work_dir(&self.info)?;

		self.tar.unpack(&work_dir)?;

		// Delete the install directory that has slackware info in it.
		std::fs::remove_dir_all(work_dir.join("install"))?;

		Ok(work_dir)
	}
}
impl Debug for TgzSource {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("TgzSource")
			.field("info", &self.info)
			.finish()
	}
}
