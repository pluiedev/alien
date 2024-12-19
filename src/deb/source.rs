use std::{
	collections::HashMap,
	fmt::Debug,
	fs::File,
	io::{Cursor, Read, Seek},
	path::{Path, PathBuf},
};

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use liblzma::read::XzDecoder;

use eyre::{bail, Result};
use subprocess::{Exec, NullFile};

use crate::{
	util::{make_unpack_work_dir, ExecExt, Verbosity},
	Args, Format, PackageInfo, Script, SourcePackage,
};

pub struct DebSource {
	info: PackageInfo,
	data: Data,
}
impl DebSource {
	#[must_use]
	pub fn check_file(file: &Path) -> bool {
		file.extension()
			.map_or(false, |o| o.eq_ignore_ascii_case("deb"))
	}

	pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
		let mut info = PackageInfo {
			file,
			distribution: "Debian".into(),
			original_format: Format::Deb,
			..Default::default()
		};

		let DebArchive {
			mut data,
			mut control_files,
		} = DebArchive::extract(&info.file)?;

		let Some(control) = control_files.remove("control") else {
			bail!("Control file not found!");
		};
		read_control(&mut info, &control);

		info.copyright = format!("see /usr/share/doc/{}/copyright", info.name);
		if info.group.is_empty() {
			info.group.push_str("unknown");
		}
		info.binary_info = control;

		if let Some(conffiles) = control_files.remove("conffiles") {
			info.conffiles.extend(conffiles.lines().map(PathBuf::from));
		};

		info.files.extend(data.files()?);

		info.scripts = control_files
			.into_iter()
			.filter_map(|(k, v)| Script::from_deb_name(k).map(|k| (k, v)))
			.collect();

		if let Some(arch) = &args.target {
			info.arch.clone_from(arch);
		}

		Ok(Self { info, data })
	}
}
impl SourcePackage for DebSource {
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
		self.data.unpack(&work_dir)?;
		Ok(work_dir)
	}
}
impl Debug for DebSource {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("DebSource")
			.field("info", &self.info)
			.finish()
	}
}

//= Utilties
struct Data(tar::Archive<Cursor<Vec<u8>>>);

impl Data {
	// In the tar file, the files are all prefixed with "./", but we want them
	// to be just "/". So, we gotta do this!
	fn files(&mut self) -> Result<impl Iterator<Item = PathBuf> + '_> {
		let entries = self.0.entries()?;

		Ok(entries.filter_map(|entry| {
			let entry = entry.ok()?;
			let path = entry.path().ok()?;
			Some(Path::new("/").join(path.strip_prefix(".").unwrap_or(&path)))
		}))
	}

	fn unpack(&mut self, dst: &Path) -> std::io::Result<()> {
		// to unpack tar files, apparently we have to rewind first...
		let mut inner =
			std::mem::replace(&mut self.0, tar::Archive::new(Cursor::new(vec![]))).into_inner();
		inner.rewind()?;
		tar::Archive::new(inner).unpack(dst)
	}
}

struct DebArchive {
	data: Data,
	control_files: HashMap<&'static str, String>,
}

impl DebArchive {
	const CONTROL_FILES: &[&'static str] = &[
		"control",
		"conffiles",
		"postinst",
		"postrm",
		"preinst",
		"prerm",
	];

	fn extract(deb_file: &Path) -> Result<Self> {
		if let Ok(dpkg_deb) = which::which("dpkg-deb") {
			Self::extract_with_dpkg_deb(&dpkg_deb, deb_file)
		} else {
			Self::extract_manually(File::open(deb_file)?)
		}
	}

	fn extract_with_dpkg_deb(dpkg_deb: &Path, deb_file: &Path) -> Result<Self> {
		// HACK(pluie): You can't query subprocess's stdout settings once set,
		// and we really don't want dpkg-deb spilling bytes from tar files
		// into readable stdout, so we want to limit the output to commands only,
		// even in very verbose mode.
		let mut verbosity = Verbosity::get();
		if verbosity == Verbosity::VeryVerbose {
			verbosity = Verbosity::Verbose;
		}

		let data = Exec::cmd(dpkg_deb)
			.arg("--fsys-tarfile")
			.arg(deb_file)
			.log_and_output(verbosity)?
			.stdout;

		let mut control_files = HashMap::new();

		for file in Self::CONTROL_FILES {
			let out = Exec::cmd(dpkg_deb)
				.arg("--info")
				.arg(deb_file)
				.arg(file)
				.stderr(NullFile)
				.log_and_output_without_checking(None)?;

			if out.success() {
				control_files.insert(*file, out.stdout_str());
			}
		}

		Ok(Self {
			data: Data(tar::Archive::new(Cursor::new(data))),
			control_files,
		})
	}

	fn extract_manually<R: Read>(source: R) -> Result<Self> {
		let mut ar = ar::Archive::new(source);
		let mut control = None;
		let mut data = None;

		while let Some(entry) = ar.next_entry() {
			let mut entry = entry?;

			if control.is_none() {
				control = Self::try_read_tar(&mut entry, "control.tar")?;
			}
			if data.is_none() {
				data = Self::try_read_tar(&mut entry, "data.tar")?;
			}
		}

		let Some(mut control) = control else {
			bail!("Malformed .deb archive - control.tar not found!")
		};
		let Some(data) = data else {
			bail!("Malformed .deb archive - data.tar not found!")
		};

		// Go through all entries, and if an entry has a path, and that path's
		// file name matches a control file we're looking for, then add that to the map.
		let mut control_files = HashMap::new();

		for entry in control.entries()? {
			let mut entry = entry?;

			let Ok(path) = entry.path() else {
				continue;
			};
			let Some(name) = path.file_name() else {
				continue;
			};

			if let Some(cf) = Self::CONTROL_FILES.iter().find(|&&s| s == name) {
				let mut data = String::new();
				entry.read_to_string(&mut data)?;
				control_files.insert(*cf, data);
			}
		}

		Ok(Self {
			data: Data(data),
			control_files,
		})
	}

	fn try_read_tar<R: Read>(
		entry: &mut ar::Entry<'_, R>,
		file: &str,
	) -> Result<Option<tar::Archive<Cursor<Vec<u8>>>>> {
		let id = entry.header().identifier();
		if let Some(ext) = id.strip_prefix(file.as_bytes()) {
			let mut tar = vec![];
			match ext {
				b".gz" => GzDecoder::new(entry).read_to_end(&mut tar)?,
				b".bz2" => BzDecoder::new(entry).read_to_end(&mut tar)?,
				b".xz" | b".lzma" => XzDecoder::new(entry).read_to_end(&mut tar)?,
				// it's already a tarball
				b"" => entry.read_to_end(&mut tar)?,
				_ => bail!(
					"{file} is compressed with unknown compression algorithm ({:?})!",
					std::str::from_utf8(ext)
				),
			};
			let tar = tar::Archive::new(Cursor::new(tar));
			Ok(Some(tar))
		} else {
			Ok(None)
		}
	}
}

fn read_control(info: &mut PackageInfo, control: &str) {
	let mut field = String::new();

	for c in control.lines() {
		if c.starts_with(' ') && field == "description" {
			// Handle extended description
			let c = c.trim_start();
			if c != "." {
				info.description.push_str(c);
			}
			info.description.push('\n');
		} else if let Some((f, value)) = c.split_once(':') {
			let value = value.trim().to_owned();
			field = f.to_ascii_lowercase();

			match field.as_str() {
				"package" => info.name = value,
				"version" => super::set_version_and_release(info, &value),
				"architecture" => info.arch = value,
				"maintainer" => info.maintainer = value,
				"section" => info.group = value,
				"description" => info.summary = value,
				// TODO: think more about handling dependencies
				// "depends" => info.dependencies = value.split(", ").map(|s| s.to_owned()).collect(),
				_ => { /* ignore */ }
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use eyre::Result;

	fn test_deb_archive() -> Result<Vec<u8>> {
		let control = b"
Package: xenomorph
Version: 0.1.0-2
Architecture: amd64
Maintainer: Leah Amelia Chen <hi@pluie.me>
Section: Utilities
Description:
  Morph between package formats
";

		let mut control_files = tar::Builder::new(vec![]);
		let mut header = tar::Header::new_gnu();
		header.set_size(control.len() as u64);
		header.set_cksum();
		control_files.append_data(&mut header, "control", &control[..])?;
		let control_tar = control_files.into_inner()?;

		let data_files = tar::Builder::new(vec![]);
		let data_tar = data_files.into_inner()?;

		let mut deb_archive = ar::Builder::new(vec![]);
		deb_archive.append(
			&ar::Header::new(b"control.tar".into(), control_tar.len() as u64),
			control_tar.as_slice(),
		)?;
		deb_archive.append(
			&ar::Header::new(b"data.tar".into(), data_tar.len() as u64),
			data_tar.as_slice(),
		)?;

		Ok(deb_archive.into_inner()?)
	}

	#[test]
	fn test_deb_archive_extract_manually() -> Result<()> {
		let deb_archive = super::DebArchive::extract_manually(test_deb_archive()?.as_slice())?;
		let control = deb_archive.control_files.get("control").unwrap();
		let mut info = crate::PackageInfo::default();
		super::read_control(&mut info, &control);

		assert_eq!(info.name, "xenomorph");
		assert_eq!(info.version, "0.1.0");
		assert_eq!(info.release, "2");
		assert_eq!(info.arch, "amd64");
		assert_eq!(info.maintainer, "Leah Amelia Chen <hi@pluie.me>");
		assert_eq!(info.group, "Utilities");
		assert_eq!(info.description, "Morph between package formats\n");

		Ok(())
	}
}
