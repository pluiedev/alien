use std::{fs::File, io::Write, path::PathBuf};

use eyre::{Context, Result};
use subprocess::Exec;

use crate::{
	util::{chmod, mkdir, ExecExt},
	PackageInfo, TargetPackage,
};

#[derive(Debug)]
pub struct PkgTarget {
	info: PackageInfo,
	unpacked_dir: PathBuf,
	converted_name: String,
}
impl PkgTarget {
	pub fn new(mut info: PackageInfo, mut unpacked_dir: PathBuf) -> Result<Self> {
		let pwd = std::env::current_dir()?;
		std::env::set_current_dir(&unpacked_dir)?;

		let mut file_list = String::new();
		for file in glob::glob("**/*").unwrap() {
			let file = file?;
			let Some(name) = file.file_name() else {
				continue;
			};
			let name = name.to_string_lossy();
			if name != "prototype" {
				file_list.push_str(&name);
				file_list.push('\n');
			}
		}

		let mut pkgproto = File::create("./prototype")?;
		Exec::cmd("pkgproto")
			.stdin(file_list.as_str())
			.stdout(pkgproto.try_clone()?)
			.log_and_spawn(None)?;
		std::env::set_current_dir(pwd)?;

		let PackageInfo {
			name,
			arch,
			version,
			description,
			copyright,
			scripts,
			..
		} = &mut info;
		let mut converted_name = name.clone();
		Self::convert_name(&mut converted_name);

		unpacked_dir.push("pkginfo");
		let mut pkginfo = File::create(&unpacked_dir)?;
		#[rustfmt::skip]
		writeln!(
			pkginfo,
r#"PKG="{converted_name}"
NAME="{name}"
ARCH="{arch}"
VERSION="{version}"
CATEGORY="application"
VENDOR="Alien-converted package"
EMAIL=
PSTAMP=alien
MAXINST=1000
BASEDIR="/"
CLASSES="none"
DESC="{description}"
"#)?;
		unpacked_dir.pop();
		writeln!(pkgproto, "i pkginfo=./pkginfo")?;

		unpacked_dir.push("install");
		mkdir(&unpacked_dir)?;

		unpacked_dir.push("copyright");
		std::fs::write(&unpacked_dir, copyright)?;
		writeln!(pkgproto, "i copyright=./install/copyright")?;
		unpacked_dir.pop();

		for (script, data) in scripts {
			let name = script.pkg_script_name();
			unpacked_dir.push(name);
			if !data.trim().is_empty() {
				std::fs::write(&unpacked_dir, data)?;
				chmod(&unpacked_dir, 0o755)?;
				writeln!(pkgproto, "i {name}={}", unpacked_dir.display())?;
			}
			unpacked_dir.pop();
		}
		unpacked_dir.pop();

		Ok(Self {
			info,
			unpacked_dir,
			converted_name,
		})
	}

	fn convert_name(name: &mut String) {
		if name.starts_with("lib") {
			name.replace_range(.."lib".len(), "l");
		}
		if name.ends_with("-perl") {
			let index = name.len() - "-perl".len();
			name.replace_range(index.., "p");
		}
		if name.starts_with("perl-") {
			name.replace_range(.."perl-".len(), "pl");
		}
	}
}
impl TargetPackage for PkgTarget {
	fn build(&mut self) -> Result<PathBuf> {
		Exec::cmd("pkgmk")
			.args(&["-r", "/", "-d", "."])
			.cwd(&self.unpacked_dir)
			.log_and_spawn(None)
			.wrap_err("Error during pkgmk")?;
		let name = format!("{}-{}.pkg", self.info.name, self.info.version);

		Exec::cmd("pkgtrans")
			.arg(&self.unpacked_dir)
			.arg(&name)
			.arg(&self.converted_name)
			.log_and_spawn(None)
			.wrap_err("Error during pkgtrans")?;

		std::fs::rename(self.unpacked_dir.join(&name), &name)?;

		Ok(PathBuf::from(name))
	}
}
