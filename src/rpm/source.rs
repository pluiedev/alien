use std::{
	collections::{HashMap, HashSet},
	path::{Component, Path, PathBuf},
};

use fs_extra::dir::CopyOptions;
use nix::unistd::{chown, geteuid, Gid, Group, Uid, User};
use simple_eyre::{
	eyre::{bail, Context},
	Result,
};
use subprocess::{Exec, NullFile, Redirection};

use crate::{
	util::{chmod, make_unpack_work_dir, ExecExt},
	Args, {FileInfo, Format, PackageInfo, Script, SourcePackage},
};

pub struct RpmSource {
	info: PackageInfo,
	prefixes: Option<PathBuf>,
}
impl RpmSource {
	#[must_use]
	pub fn check_file(file: &Path) -> bool {
		match file.extension() {
			Some(o) => o.eq_ignore_ascii_case("rpm"),
			None => false,
		}
	}
	pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
		let rpm = RpmReader::new(&file);

		let prefixes = rpm.read_field("%{PREFIXES}")?.map(PathBuf::from);

		let conffiles = rpm.read_file_list("-c")?;
		let file_list = rpm.read_file_list("-l")?;
		let binary_info = rpm.read("-qip")?;

		// Sanity check and sanitize fields.

		let description = rpm.read_field("%{DESCRIPTION}")?;

		let summary = if let Some(summary) = rpm.read_field("%{SUMMARY}")? {
			summary
		} else {
			// Older rpms will have no summary, but will have a description.
			// We'll take the 1st line out of the description, and use it for the summary.
			let description = description.as_deref().unwrap_or_default();
			let s = description.split_once('\n').map_or(description, |t| t.0);
			if s.is_empty() {
				// Fallback.
				"Converted RPM package".into()
			} else {
				s.to_owned()
			}
		};

		let description = description.unwrap_or_else(|| summary.clone());

		// Older rpms have no license tag, but have a copyright.
		let copyright = match rpm.read_field("%{LICENSE}")? {
			Some(o) => o,
			None => rpm
				.read_field("%{COPYRIGHT}")?
				.unwrap_or_else(|| "unknown".into()),
		};

		let Some(name) = rpm.read_field("%{NAME}")? else {
			bail!("Error querying rpm file: name not found!")
		};
		let Some(version) = rpm.read_field("%{VERSION}")? else {
			bail!("Error querying rpm file: version not found!")
		};
		let Some(release) = rpm.read_field("%{RELEASE}")?
			.and_then(|s| s.parse().ok())
		else {
			bail!("Error querying rpm file: release not found or invalid!")
		};

		let mut scripts = HashMap::new();
		for script in Script::ALL {
			let field = rpm.read_field(script.rpm_query_key())?;
			scripts.insert(script, sanitize_script(&prefixes, field));
		}

		let info = PackageInfo {
			name,
			version,
			release,
			arch: rpm.read_arch(args.target.as_deref())?,
			changelog: rpm.read_field("%{CHANGELOGTEXT}")?.unwrap_or_default(),
			summary,
			description,
			scripts,
			copyright,

			conffiles,
			files: file_list,
			binary_info,

			file,
			distribution: "Red Hat".into(),
			original_format: Format::Rpm,
			..Default::default()
		};

		Ok(Self { info, prefixes })
	}
}
impl SourcePackage for RpmSource {
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

		let rpm2cpio = || Exec::cmd("rpm2cpio").arg(&self.info.file);

		// Check if we need to use lzma to uncompress the cpio archive
		let cmd = rpm2cpio()
			| Exec::cmd("lzma")
				.arg("-tq")
				.stdout(NullFile)
				.stderr(NullFile);

		let decomp = if cmd.log_and_output(None)?.success() {
			|| Exec::cmd("lzma").arg("-dq")
		} else {
			|| Exec::cmd("cat")
		};

		let cpio = Exec::cmd("cpio")
			.cwd(&work_dir)
			.args(&[
				"--extract",
				"--make-directories",
				"--no-absolute-filenames",
				"--preserve-modification-time",
			])
			.stderr(Redirection::Merge);

		(rpm2cpio() | decomp() | cpio)
			.log_and_spawn(None)
			.wrap_err_with(|| format!("Unpacking of {} failed", self.info.file.display()))?;

		// `cpio` does not necessarily store all parent directories in an archive,
		// and so some directories, if it has to make them and has no permission info,
		// will come out with some random permissions.
		// Find those directories and make them mode 755, which is more reasonable.

		let cpio = Exec::cmd("cpio").args(&["-it", "--quiet"]);
		let seen_files: HashSet<_> = (rpm2cpio() | decomp() | cpio)
			.log_and_output(None)
			.wrap_err_with(|| format!("File list of {} failed", self.info.file.display()))?
			.stdout_str()
			.lines()
			.map(PathBuf::from)
			.collect();

		let cur_dir = std::env::current_dir()?;
		std::env::set_current_dir(&work_dir)?;
		// glob doesn't allow you to specify a cwd... annoying, but ok
		for file in glob::glob("**/*").unwrap() {
			let file = file?;
			let new_file = work_dir.join(&file);
			if !seen_files.contains(&file) && new_file.exists() && !new_file.is_symlink() {
				chmod(&new_file, 0o755)?;
			}
		}
		std::env::set_current_dir(cur_dir)?;

		// If the package is relocatable, we'd like to move it to be under the `self.prefixes` directory.
		// However, it's possible that that directory is in the package - it seems some rpm's are marked
		// as relocatable and unpack already in the directory they can relocate to, while some are marked
		// relocatable and the directory they can relocate to is removed from all filenames in the package.
		// I suppose this is due to some change between versions of rpm, but none of this is adequately documented,
		// so we'll just muddle through.

		if let Some(prefixes) = &self.prefixes {
			let w_prefixes = work_dir.join(prefixes);
			if !w_prefixes.exists() {
				let mut relocate = true;

				// Get the files to move.
				let pattern = work_dir.join("*");
				let file_list: Vec<_> = glob::glob(&pattern.to_string_lossy())
					.unwrap()
					.filter_map(|p| p.ok())
					.collect();

				// Now, make the destination directory.
				let mut dest = PathBuf::new();

				for comp in prefixes.components() {
					if comp == Component::CurDir {
						dest.push("/");
					}
					dest.push(comp);

					if dest.is_dir() {
						// The package contains a parent directory of the relocation directory.
						// Since it's impossible to move a parent directory into its child,
						// bail out and do nothing.
						relocate = false;
						break;
					}
					std::fs::create_dir(&dest)?;
				}

				if relocate {
					// Now move all files in the package to the directory we made.
					if !file_list.is_empty() {
						fs_extra::move_items(&file_list, &w_prefixes, &CopyOptions::new())?;
					}

					self.info.conffiles = self
						.info
						.conffiles
						.iter()
						.map(|f| prefixes.join(f))
						.collect();
				}
			}
		}

		// `rpm` files have two sets of permissions; the set in the cpio archive,
		// and the set in the control data, which override the set in the archive.
		// The set in the control data are more correct, so let's use those.
		// Some permissions setting may have to be postponed until the postinst.

		let out = Exec::cmd("rpm")
			.args(&[
				"--queryformat",
				r#"'[%{FILEMODES} %{FILEUSERNAME} %{FILEGROUPNAME} %{FILENAMES}\n]'"#,
				"-qp",
			])
			.arg(&self.info.file)
			.log_and_output(None)?
			.stdout_str();

		let mut owninfo: HashMap<PathBuf, FileInfo> = HashMap::new();

		for line in out.lines() {
			let mut line = line.split(' ');
			let Some(mode) = line.next() else { continue; };
			let Some(owner) = line.next() else { continue; };
			let Some(group) = line.next() else { continue; };
			let Some(file) = line.next() else { continue; };

			let mut mode: u32 = mode.parse()?;
			mode &= 0o7777; // remove filetype

			let file = PathBuf::from(file);
			let file_info = owninfo.entry(file.clone()).or_default();

			// TODO: this is not gonna work on windows, is it
			let user_id = match User::from_name(owner)? {
				Some(User { uid, .. }) if uid.is_root() => uid,
				_ => {
					file_info.owner = owner.to_owned();
					Uid::from_raw(0)
				}
			};
			let group_id = match Group::from_name(group)? {
				Some(Group { gid, .. }) if gid.as_raw() == 0 => gid,
				_ => {
					file_info.owner.push(':');
					file_info.owner.push_str(group);
					Gid::from_raw(0)
				}
			};

			// If this is a `setuid` file
			if !file_info.owner.is_empty() && mode & 0o7000 > 0 {
				file_info.mode = Some(mode);
			}

			// Note that ghost files exist in the metadata but not in the cpio archive,
			// so check that the file exists before trying to access it.
			let file = work_dir.join(file);
			if file.exists() {
				if geteuid().is_root() {
					chown(&file, Some(user_id), Some(group_id)).wrap_err_with(|| {
						format!("failed chowning {} to {user_id}:{group_id}", file.display())
					})?;
				}
				chmod(&file, mode)
					.wrap_err_with(|| format!("failed chowning {} to {mode}", file.display()))?;
			}
		}
		self.info.file_info = owninfo;
		Ok(work_dir)
	}
}

//= Utilities
struct RpmReader<'r> {
	file: &'r Path,
}
impl<'r> RpmReader<'r> {
	fn new(file: &'r Path) -> Self {
		Self { file }
	}

	fn rpm() -> Exec {
		Exec::cmd("rpm").env("LANG", "C")
	}

	fn read(&self, flags: &str) -> Result<String> {
		Ok(Self::rpm()
			.arg(flags)
			.arg(self.file)
			.log_and_output(None)?
			.stdout_str())
	}
	fn read_field(&self, name: &str) -> Result<Option<String>> {
		let res = Self::rpm()
			.arg("-qp")
			.arg("--queryformat")
			.arg(name)
			.arg(self.file)
			.log_and_output(None)?
			.stdout_str();

		Ok(if res == "(none)" { None } else { Some(res) })
	}
	fn read_arch(&self, target: Option<&str>) -> Result<String> {
		let arch = if let Some(arch) = &target {
			let arch = match arch.as_bytes() {
				// NOTE(pluie): do NOT ask me where these numbers came from.
				// I have NO clue.
				b"1" => "i386",
				b"2" => "alpha",
				b"3" => "sparc",
				b"6" => "m68k",
				b"noarch" => "all",
				b"ppc" => "powerpc",
				b"x86_64" | b"em64t" => "amd64",
				b"armv4l" => "arm",
				b"armv7l" => "armel",
				b"parisc" => "hppa",
				b"ppc64le" => "ppc64el",

				// Treat 486, 586, etc, and Pentium, as 386.
				o if o.eq_ignore_ascii_case(b"pentium") => "i386",
				&[b'i' | b'I', b'0'..=b'9', b'8', b'6'] => "i386",

				_ => arch,
			};
			arch.to_owned()
		} else {
			self.read_field("%{ARCH}")?.unwrap_or_default()
		};
		Ok(arch)
	}
	fn read_file_list(&self, flag: &str) -> Result<Vec<PathBuf>> {
		let mut files: Vec<_> = Self::rpm()
			.arg("-qp")
			.arg(flag)
			.arg(self.file)
			.log_and_output(None)?
			.stdout_str()
			.lines()
			.map(|s| PathBuf::from(s.trim()))
			.collect();
		if let Some(f) = files.first() {
			if f.as_os_str() == "(contains no files)" {
				files.clear();
			}
		}
		Ok(files)
	}
}

// rpm maintainer scripts are typically shell scripts,
// but often lack the leading shebang line.
// This can confuse dpkg, so add the shebang if it looks like
// there is no shebang magic already in place.
//
// Additionally, it's not uncommon for rpm maintainer scripts to
// contain bashisms, which can be triggered when they are run on
// systems where /bin/sh is not bash. To work around this,
// the shebang line of the scripts is changed to use bash.
//
// Also if the rpm is relocatable, the script could refer to
// RPM_INSTALL_PREFIX, which is to set by rpm at runtime.
// Deal with this by adding code to the script to set RPM_INSTALL_PREFIX.
fn sanitize_script(prefixes: &Option<PathBuf>, s: Option<String>) -> String {
	let prefix_code = prefixes
		.as_ref()
		.map(|p| {
			format!(
				"\nRPM_INSTALL_PREFIX={}\nexport RPM_INSTALL_PREFIX",
				p.display()
			)
		})
		.unwrap_or_default();

	if let Some(t) = &s {
		if let Some(t) = t.strip_prefix("#!") {
			let t = t.trim_start();
			if t.starts_with('/') {
				let mut t = t.replacen("/bin/sh", "#!/bin/bash", 1);
				if let Some(nl) = t.find('\n') {
					t.insert_str(nl, &prefix_code);
				}
				return t;
			}
		}
	}
	format!("#!/bin/bash\n{prefix_code}{}", s.unwrap_or_default())
}
