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
	util::{make_unpack_work_dir, ExecExt},
	Format, PackageInfo, Script, SourcePackage,
};

#[derive(Debug)]
pub struct PkgSource {
	info: PackageInfo,
	pkgname: String,
	pkg_dir: PathBuf,

	pkgtrans: PathBuf,
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
		let pkginfo = which::which("pkginfo")
			.wrap_err("`pkginfo` needs to be installed in order to convert from Solaris pkgs")?;
		let pkgtrans = which::which("pkgtrans")
			.wrap_err("`pkgtrans` needs to be installed in order to convert from Solaris pkgs")?;

		let Some(name) = file.file_name().map(|s| s.to_string_lossy()) else {
			bail!("Cannot extract package name from Solaris pkg file name: {} doesn't have a file name?!", file.display());
		};
		let Some((name, _)) = name.split_once('-') else {
			bail!("Cannot extract package name from Solaris pkg file name: {name} does not contain hyphens!");
		};
		let name = name.to_owned();

		let mut reader = PkgReader::new(file, &pkginfo, &pkgtrans)?;
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
			..
		} = reader;
		info.file = file;

		Ok(Self {
			info,
			pkgname,
			pkg_dir,
			pkgtrans,
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

		Exec::cmd(&self.pkgtrans)
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
	pub fn new(file: PathBuf, pkginfo: &Path, pkgtrans: &Path) -> Result<Self> {
		let tdir = tempfile::tempdir()?.into_path();

		let pkginfo = Exec::cmd(pkginfo)
			.arg("-d")
			.arg(&file)
			.log_and_output(None)?
			.stdout_str();
		let Some(pkgname) = pkginfo.lines().next() else {
			bail!("Received empty output from pkginfo");
		};
		let pkgname = pkgname
			.trim_start_matches(|c: char| !c.is_whitespace())
			.trim_start()
			.trim_end()
			.to_owned();

		Exec::cmd(pkgtrans)
			.arg("-i")
			.arg(&file)
			.arg(&tdir)
			.arg(&pkgname)
			.log_and_spawn(None)
			.wrap_err("Error running pkgtrans")?;

		Ok(Self {
			file,
			pkg_dir: tdir.join(&pkgname),
			pkgname,
		})
	}
	fn read_pkg_info(&mut self, info: &mut PackageInfo) -> Result<()> {
		let pkginfo = std::fs::read_to_string(&self.pkg_dir.join("pkginfo"))?;
		parse_pkg_info(info, &pkginfo)
	}
	fn read_pkg_map(&mut self, info: &mut PackageInfo) -> Result<()> {
		let pkgmap = std::fs::read_to_string(&self.pkg_dir.join("pkgmap"))?;
		parse_pkg_map(info, &pkgmap, &self.file)
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

	fn cleanup(&mut self) -> Result<()> {
		self.pkg_dir.pop();
		std::fs::remove_dir_all(&self.pkg_dir)?;
		Ok(())
	}
}

fn parse_pkg_info(info: &mut PackageInfo, content: &str) -> Result<()> {
	// See https://docs.oracle.com/cd/E36784_01/html/E36882/pkginfo-4.html
	let mut info_map: HashMap<&str, &str> = HashMap::new();
	let mut key = "";
	for line in content.lines() {
		let value = if let Some((k, v)) = line.split_once('=') {
			key = k;
			v
		} else {
			line
		};
		*info_map.entry(key).or_default() = value;
	}

	let Some(arch) = info_map.remove("ARCH") else {
		bail!("ARCH field missing in pkginfo!");
	};
	let Some(version) = info_map.remove("VERSION") else {
		bail!("VERSION field missing in pkginfo!");
	};

	info.arch = arch.trim_matches('"').to_owned();
	info.version = version.trim_matches('"').to_owned();
	info.description = info_map
		.remove("DESC")
		.map(|d| d.trim_matches('"').to_owned())
		.unwrap_or_default();

	Ok(())
}

fn parse_pkg_map(info: &mut PackageInfo, content: &str, file: &Path) -> Result<()> {
	// See https://docs.oracle.com/cd/E36784_01/html/E36882/pkgmap-4.html

	// Skip the preamble line
	for f in content.lines().skip(1) {
		let mut split = f.split(' ');

		// TODO: allow other part numbers
		let Some("1") = split.next() else {
			continue;
		};
		let Some(ftype) = split.next() else {
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
					.insert(script, std::fs::read_to_string(file.join(path))?);
			}
			_ => { /* TODO handle other ftypes */ }
		}
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	#[test]
	fn test_parse_pkg_info() -> eyre::Result<()> {
		let mut info = crate::PackageInfo::default();

		super::parse_pkg_info(
			&mut info,
			r#"
SUNW_PRODNAME="SunOS"
SUNW_PRODVERS="5.5"
SUNW_PKGTYPE="usr"
SUNW_PKG_ALLZONES=false
SUNW_PKG_HOLLOW=false
PKG="SUNWesu"
NAME="Extended System Utilities"
VERSION="11.5.1"
ARCH="sparc"
DESC="Have a nice Sun-day!"
VENDOR="Sun Microsystems, Inc."
HOTLINE="Please contact your local service provider"
EMAIL=""
VSTOCK="0122c3f5566"
CATEGORY="system"
ISTATES="S 2"
RSTATES="S 2"
			"#,
		)?;

		assert_eq!(info.arch, "sparc");
		assert_eq!(info.version, "11.5.1");
		assert_eq!(info.description, "Have a nice Sun-day!");

		Ok(())
	}

	#[test]
	fn test_parse_pkg_map() -> eyre::Result<()> {
		let mut info = crate::PackageInfo::default();

		super::parse_pkg_map(
			&mut info,
			r#"
: 2 500
1 i pkginfo 237 1179 541296672
1 d none bin 0755 root bin
1 f none bin/INSTALL 0755 root bin 11103 17954 541295535
1 f none bin/REMOVE 0755 root bin 3214 50237 541295541
1 l none bin/UNINSTALL=bin/REMOVE
1 f none bin/cmda 0755 root bin 3580 60325 541295567
1 f none bin/cmdb 0755 root bin 49107 51255 541438368
1 f class1 bin/cmdc 0755 root bin 45599 26048 541295599
1 f class1 etc/cmdd 0755 root bin 4648 8473 541461238
1 f none etc/cmde 0755 root bin 40501 1264 541295622
1 f class2 etc/cmdf 0755 root bin 2345 35889 541295574
1 f none etc/cmdg 0755 root bin 41185 47653 541461242
2 d class2 data 0755 root bin
2 p class1 data/apipe 0755 root other
2 d none log 0755 root bin
2 v none log/logfile 0755 root bin 41815 47563 541461333
2 d none save 0755 root bin
2 d none spool 0755 root bin
2 d none tmp 0755 root bin
			"#,
			Path::new(""),
		)?;

		assert_eq!(
			info.files,
			vec![
				Path::new("bin"),
				Path::new("bin/INSTALL"),
				Path::new("bin/REMOVE"),
				Path::new("bin/cmda"),
				Path::new("bin/cmdb"),
				Path::new("bin/cmdc"),
			]
		);
		assert_eq!(
			info.conffiles,
			vec![
				Path::new("/etc/cmdd"),
				Path::new("/etc/cmde"),
				Path::new("/etc/cmdf"),
				Path::new("/etc/cmdg"),
			]
		);

		Ok(())
	}
}
