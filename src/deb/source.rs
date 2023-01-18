use std::{
	borrow::Cow,
	collections::HashMap,
	fmt::Debug,
	fs::File,
	io::{Cursor, Read},
	path::{Path, PathBuf},
};

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;

use eyre::{bail, Result};
use subprocess::Exec;

use xz::read::XzDecoder;

use crate::{
	util::{make_unpack_work_dir, ExecExt},
	Args, Format, PackageInfo, Script, SourcePackage,
};

pub struct DebSource {
	info: PackageInfo,
	data: Data,
}
impl DebSource {
	#[must_use]
	pub fn check_file(file: &Path) -> bool {
		match file.extension() {
			Some(o) => o.eq_ignore_ascii_case("deb"),
			None => false,
		}
	}

	pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
		let mut info = PackageInfo {
			file,
			distribution: "Debian".into(),
			original_format: Format::Deb,
			..Default::default()
		};

		let dpkg_deb = which::which("dpkg-deb").ok();

		let mut control_files = fetch_control_files(
			dpkg_deb.as_deref(),
			&info.file,
			&[
				"control",
				"conffiles",
				"postinst",
				"postrm",
				"preinst",
				"prerm",
			],
		)?;

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

		let mut data = Data::new(dpkg_deb.as_deref(), &info.file)?;

		info.files.extend(data.files()?);

		info.scripts = control_files
			.into_iter()
			.filter_map(|(k, v)| Script::from_deb_name(k).map(|k| (k, v)))
			.collect();

		if let Some(arch) = &args.target {
			info.arch = arch.clone();
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
	fn new(dpkg_deb: Option<&Path>, deb_file: &Path) -> Result<Self> {
		let tar = if let Some(dpkg_deb) = dpkg_deb {
			Exec::cmd(dpkg_deb)
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
					b"data.tar" => entry.read_to_end(&mut tar).unwrap(),
					_ => bail!("Unknown data member!"),
				};
				break;
			}
			if tar.is_empty() {
				bail!("Cannot find data member!");
			}
			tar
		};

		Ok(Self(tar::Archive::new(Cursor::new(tar))))
	}

	// In the tar file, the files are all prefixed with "./", but we want them
	// to be just "/". So, we gotta do this!
	fn files(&mut self) -> Result<impl Iterator<Item = PathBuf> + '_> {
		Ok(self
			.0
			.entries()?
			.filter_map(|f| f.ok())
			.filter_map(|f| f.path().map(Cow::into_owned).ok())
			.map(|s| {
				let s = s.to_string_lossy();
				let s = s.strip_prefix('.').unwrap_or(&s);
				PathBuf::from(s)
			}))
	}
	fn unpack(&mut self, dst: &Path) -> std::io::Result<()> {
		self.0.unpack(dst)
	}
}

fn fetch_control_files(
	dpkg_deb: Option<&Path>,
	deb_file: &Path,
	control_files: &[&'static str],
) -> Result<HashMap<&'static str, String>> {
	if let Some(dpkg_deb) = dpkg_deb {
		let mut map = HashMap::new();
		for file in control_files {
			let out = Exec::cmd(dpkg_deb)
				.arg("--info")
				.arg(deb_file)
				.arg(file)
				.log_and_output_without_checking(None)?;

			if out.success() {
				map.insert(*file, out.stdout_str());
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
				let Ok(path) = entry.path() else { continue; };
				let Some(name) = path.file_name() else { continue; };

				if let Some(cf) = control_files.iter().find(|&&s| s == name) {
					let mut data = String::new();
					entry.read_to_string(&mut data)?;
					map.insert(*cf, data);
				}
			}

			return Ok(map);
		}
		bail!("Cannot find control member!");
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
				info.description.push('\n');
			}
		} else if let Some((f, value)) = c.split_once(':') {
			let value = value.trim().to_owned();
			// Really old debs might have oddly capitalized field names.
			field = f.to_ascii_lowercase();

			match field.as_str() {
				"package" => info.name = value,
				"version" => super::set_version_and_release(info, &value),
				"architecture" => info.arch = value,
				"maintainer" => info.maintainer = value,
				"section" => info.group = value,
				"description" => info.summary = value,
				"depends" => info.dependencies = value.split(", ").map(|s| s.to_owned()).collect(),
				_ => { /* ignore */ }
			}
		}
	}
}
