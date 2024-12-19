use std::{
	collections::HashMap,
	fs::File,
	io::{BufRead, BufReader},
	path::{Path, PathBuf},
};

use eyre::{bail, Context, Result};
use fs_extra::dir::CopyOptions;
use subprocess::Exec;

use crate::{
	util::{make_unpack_work_dir, mkdir, ExecExt},
	Format, PackageInfo, Script, SourcePackage,
};

#[derive(Debug)]
pub struct PkgSource {
	info: PackageInfo,
	pkgname: String,
	pkg_dir: PathBuf,
}
impl PkgSource {
	#[must_use]
	pub fn check_file(file: &Path) -> bool {
		let Ok(file) = File::open(file) else {
			return false;
		};
		let mut file = BufReader::new(file);

		let mut line = String::new();
		if file.read_line(&mut line).is_err() {
			return false;
		}

		line.contains("# PaCkAgE DaTaStReAm")
	}
	pub fn new(file: PathBuf) -> Result<Self> {
		// FIXME: Bad. Not everyone follows FHS.
		for tool in ["/usr/bin/pkginfo", "/usr/bin/pkgtrans"] {
			if !Path::new(tool).exists() {
				bail!("`xenomorph` needs {tool} to run!");
			}
		}

		let Some(name) = file.file_name().map(|s| s.to_string_lossy()) else {
			bail!("Cannot extract package name from Solaris pkg file name: {} doesn't have a file name?!", file.display());
		};
		let Some((name, _)) = name.split_once('-') else {
			bail!("Cannot extract package name from Solaris pkg file name: {name} does not contain hyphens!");
		};
		let name = name.to_owned();

		let mut reader = PkgReader::new(file)?;
		let copyright = reader.read_copyright()?;

		let mut info = PackageInfo {
			name,
			group: "unknown".into(), // FIXME
			summary: "Converted Solaris pkg package".into(),
			copyright,
			original_format: Format::Pkg,
			distribution: "Solaris".into(),
			binary_info: "unknown".into(), // FIXME
			..Default::default()
		};

		reader.read_pkg_info(&mut info)?;
		reader.read_pkg_map(&mut info)?;
		reader.cleanup()?;

		let PkgReader {
			file,
			pkg_dir,
			pkgname,
		} = reader;
		info.file = file;

		Ok(Self {
			info,
			pkgname,
			pkg_dir,
		})
	}
}
impl SourcePackage for PkgSource {
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

		Exec::cmd("/usr/bin/pkgtrans")
			.arg(&self.info.file)
			.arg(&work_dir)
			.arg(&self.pkgname)
			.log_and_spawn(None)?;

		let mut work_dir_1 = work_dir.clone().into_os_string();
		work_dir_1.push("_1");

		fs_extra::dir::move_dir(&self.pkg_dir, &work_dir_1, &CopyOptions::default())?;
		std::fs::remove_dir(&work_dir)?;
		fs_extra::dir::move_dir(&work_dir_1, &work_dir, &CopyOptions::default())?;

		Ok(work_dir)
	}
}

struct PkgReader {
	file: PathBuf,
	pkg_dir: PathBuf,
	pkgname: String,
}
impl PkgReader {
	pub fn new(file: PathBuf) -> Result<Self> {
		let mut tdir = PathBuf::from(format!("pkg-scan-tmp.{}", std::process::id()));
		mkdir(&tdir)?;

		let pkginfo = Exec::cmd("/usr/bin/pkginfo")
			.arg("-d")
			.arg(&file)
			.log_and_output(None)?
			.stdout_str();
		let Some(pkgname) = pkginfo.lines().next() else {
			bail!("pkginfo is empty!");
		};
		let pkgname = pkgname
			.trim_start_matches(|c: char| !c.is_whitespace())
			.trim_start()
			.trim_end()
			.to_owned();

		Exec::cmd("/usr/bin/pkgtrans")
			.arg("-i")
			.arg(&file)
			.arg(&tdir)
			.arg(&pkgname)
			.log_and_spawn(None)
			.wrap_err("Error running pkgtrans")?;

		tdir.push(&pkgname);
		Ok(Self {
			file,
			pkg_dir: tdir,
			pkgname,
		})
	}
	fn read_copyright(&mut self) -> Result<String> {
		self.pkg_dir.push("copyright");

		let copyright = if self.pkg_dir.is_file() {
			let mut copyright = self.file.join("install");
			copyright.push("copyright");
			std::fs::read_to_string(&copyright)?
		} else {
			"unknown".into()
		};

		self.pkg_dir.pop();
		Ok(copyright)
	}
	fn read_pkg_info(&mut self, info: &mut PackageInfo) -> Result<()> {
		self.pkg_dir.push("pkginfo");

		let pkginfo = std::fs::read_to_string(&self.pkg_dir)?;

		let mut info_map: HashMap<&str, Vec<&str>> = HashMap::new();
		let mut key = "";
		for line in pkginfo.lines() {
			let value = if let Some((k, v)) = line.split_once('=') {
				key = k;
				v
			} else {
				line
			};
			info_map.entry(key).or_default().push(value);
		}

		let Some(mut arch) = info_map.remove("ARCH") else {
			bail!("ARCH field missing in pkginfo!");
		};
		let Some(mut version) = info_map.remove("VERSION") else {
			bail!("VERSION field missing in pkginfo!");
		};
		let description = if let Some(desc) = info_map.remove("DESC") {
			desc.join("")
		} else {
			".".into()
		};

		info.arch = arch.swap_remove(0).to_owned();
		info.version = version.swap_remove(0).to_owned();
		info.description = description;

		self.pkg_dir.pop();
		Ok(())
	}
	fn read_pkg_map(&mut self, info: &mut PackageInfo) -> Result<()> {
		self.pkg_dir.push("pkgmap");

		let file_list = std::fs::read_to_string(&self.pkg_dir)?;
		for f in file_list.lines() {
			let mut split = f.split(' ');
			let Some("1") = split.next() else {
				continue;
			};
			let Some(ftype @ ("f" | "d" | "i")) = split.next() else {
				continue;
			};
			let Some(_) = split.next() else {
				continue;
			};
			let Some(path) = split.next() else {
				continue;
			};

			match ftype {
				"f" if path.starts_with("etc/") => {
					let mut buf = PathBuf::from("/");
					buf.push(path);
					info.conffiles.push(buf);
				}
				"f" | "d" => info.files.push(PathBuf::from(path)),
				"i" => {
					let Some(script) = Script::from_pkg_script_name(path) else {
						continue;
					};
					info.scripts
						.insert(script, std::fs::read_to_string(self.file.join(path))?);
				}
				_ => {}
			}
		}

		self.pkg_dir.pop();
		Ok(())
	}
	fn cleanup(&mut self) -> Result<()> {
		self.pkg_dir.pop();
		std::fs::remove_dir_all(&self.pkg_dir)?;
		Ok(())
	}
}
