use std::{
	collections::{HashMap, HashSet},
	fmt::Write as _,
	fs::File,
	io::Write,
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
	package::Format,
	util::{ExecExt, Verbosity},
};

use super::{
	common::{self, chmod},
	PackageBehavior, PackageInfo,
};

pub struct Rpm {
	info: PackageInfo,
	rpm_file: PathBuf,
	prefixes: Option<PathBuf>,
}
impl Rpm {
	pub fn check_file(file: &Path) -> bool {
		match file.extension() {
			Some(o) => o.eq_ignore_ascii_case("rpm"),
			None => false,
		}
	}
	pub fn new(rpm_file: PathBuf) -> Result<Self> {
		// I'm lazy.
		fn rpm() -> Exec {
			Exec::cmd("rpm").env("LANG", "C")
		}
		let read_field = |name: &str| -> Option<String> {
			let res = rpm()
				.arg("-qp")
				.arg("--queryformat")
				.arg(name)
				.arg(&rpm_file)
				.log_and_output(None)
				.ok()?
				.stdout_str();

			if res == "(none)" {
				None
			} else {
				Some(res)
			}
		};

		let mut conffiles: Vec<_> = rpm()
			.arg("-qcp")
			.arg(&rpm_file)
			.log_and_output(None)?
			.stdout_str()
			.lines()
			.map(|s| PathBuf::from(s.trim()))
			.collect();
		if let Some(f) = conffiles.first() {
			if f.as_os_str() == "(contains no files)" {
				conffiles.clear();
			}
		}

		let mut file_list: Vec<_> = rpm()
			.arg("-qlp")
			.arg(&rpm_file)
			.log_and_output(None)?
			.stdout_str()
			.lines()
			.map(|s| PathBuf::from(s.trim()))
			.collect();
		if let Some(f) = file_list.first() {
			if f.as_os_str() == "(contains no files)" {
				file_list.clear();
			}
		}

		let binary_info = rpm()
			.arg("-qip")
			.arg(&rpm_file)
			.log_and_output(None)?
			.stdout_str();

		// Sanity check and sanitize fields.

		let description = read_field("%{DESCRIPTION}");

		let summary = if let Some(summary) = read_field("%{SUMMARY}") {
			summary
		} else {
			// Older rpms will have no summary, but will have a description.
			// We'll take the 1st line out of the description, and use it for the summary.
			let description = description.as_deref().unwrap_or("");
			let s = description
				.split_once('\n')
				.map(|t| t.0)
				.unwrap_or(description);
			if s.is_empty() {
				// Fallback.
				"Converted RPM package".into()
			} else {
				s.to_owned()
			}
		};

		let description = description.unwrap_or_else(|| summary.clone());

		// Older rpms have no license tag, but have a copyright.
		let copyright = read_field("%{LICENSE}")
			.or_else(|| read_field("%{COPYRIGHT}"))
			.unwrap_or_else(|| "unknown".into());

		let Some(name) = read_field("%{NAME}") else {
			bail!("Error querying rpm file: name not found!")
		};
		let Some(version) = read_field("%{VERSION}") else {
			bail!("Error querying rpm file: version not found!")
		};
		let Some(release) = read_field("%{RELEASE}")
			.and_then(|s| s.parse().ok())
		else {
			bail!("Error querying rpm file: release not found or invalid!")
		};

		let info = PackageInfo {
			name,
			version,
			release,
			arch: read_field("%{ARCH}").unwrap_or_default(),
			changelog_text: read_field("%{CHANGELOGTEXT}").unwrap_or_default(),
			summary,
			description,
			preinst: read_field("%{PREIN}"),
			postinst: read_field("%{POSTIN}"),
			prerm: read_field("%{PREUN}"),
			postrm: read_field("%{POSTUN}"),
			copyright,

			conffiles,
			file_list,
			binary_info,

			distribution: "Red Hat".into(),
			original_format: Format::Rpm,
			..Default::default()
		};
		let prefixes = read_field("%{PREFIXES}").map(PathBuf::from);

		Ok(Self {
			info,
			rpm_file,
			prefixes,
		})
	}
}
impl PackageBehavior for Rpm {
	fn info(&self) -> &PackageInfo {
		&self.info
	}
	fn info_mut(&mut self) -> &mut PackageInfo {
		&mut self.info
	}

	fn install(&mut self, file_name: &Path) -> Result<()> {
		let cmd = Exec::cmd("rpm").arg("-ivh");
		let cmd = if let Some(opt) = std::env::var_os("RPMINSTALLOPT") {
			let mut path = PathBuf::from(opt);
			path.push(file_name);
			cmd.arg(path)
		} else {
			cmd.arg(file_name)
		};
		cmd.log_and_output(Verbosity::VeryVerbose)
			.wrap_err("Unable to install")?;
		Ok(())
	}
	fn unpack(&mut self) -> Result<PathBuf> {
		let work_dir = common::make_unpack_work_dir(&self.info)?;

		let rpm2cpio = || Exec::cmd("rpm2cpio").arg(&self.rpm_file);

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
			.wrap_err_with(|| format!("Unpacking of {} failed", self.rpm_file.display()))?;

		// `cpio` does not necessarily store all parent directories in an archive,
		// and so some directories, if it has to make them and has no permission info,
		// will come out with some random permissions.
		// Find those directories and make them mode 755, which is more reasonable.

		let cpio = Exec::cmd("cpio").args(&["-it", "--quiet"]);
		let seen_files: HashSet<_> = (rpm2cpio() | decomp() | cpio)
			.log_and_output(None)
			.wrap_err_with(|| format!("File list of {} failed", self.rpm_file.display()))?
			.stdout_str()
			.lines()
			.map(PathBuf::from)
			.collect();

		let cwd = std::env::current_dir()?;
		std::env::set_current_dir(&work_dir)?;
		// glob doesn't allow you to specify a cwd... annoying, but ok
		for file in glob::glob("**/*").unwrap() {
			let file = file?;
			let new_file = work_dir.join(&file);
			if !seen_files.contains(&file) && new_file.exists() && !new_file.is_symlink() {
				chmod(&new_file, 0o755)?;
			}
		}
		std::env::set_current_dir(cwd)?;

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
			.arg(&self.rpm_file)
			.log_and_output(None)?
			.stdout_str();

		let mut owninfo = HashMap::new();
		let mut modeinfo = HashMap::new();

		for line in out.lines() {
			let mut line = line.split(' ');
			let Some(mode) = line.next() else { continue; };
			let Some(owner) = line.next() else { continue; };
			let Some(group) = line.next() else { continue; };
			let Some(file) = line.next() else { continue; };

			let mut mode: u32 = mode.parse()?;
			mode &= 0o7777; // remove filetype

			let file = PathBuf::from(file);

			// TODO: this is not gonna work on windows, is it
			let uid = match User::from_name(owner)? {
				Some(User { uid, .. }) if uid.is_root() => uid,
				_ => {
					owninfo.insert(file.clone(), owner.to_owned());
					Uid::from_raw(0)
				}
			};
			let gid = match Group::from_name(group)? {
				Some(Group { gid, .. }) if gid.as_raw() == 0 => gid,
				_ => {
					let s = owninfo.entry(file.clone()).or_default();
					s.push(':');
					s.push_str(group);
					Gid::from_raw(0)
				}
			};

			if owninfo.contains_key(&file) && mode & 0o7000 > 0 {
				modeinfo.insert(file.clone(), mode);
			}

			// Note that ghost files exist in the metadata but not in the cpio archive,
			// so check that the file exists before trying to access it.
			let file = work_dir.join(file);
			if file.exists() {
				if geteuid().is_root() {
					chown(&file, Some(uid), Some(gid)).wrap_err_with(|| {
						format!("failed chowning {} to {uid}:{gid}", file.display())
					})?;
				}
				chmod(&file, mode)
					.wrap_err_with(|| format!("failed chowning {} to {mode}", file.display()))?;
			}
		}
		self.info.owninfo = owninfo;
		self.info.modeinfo = modeinfo;

		Ok(work_dir)
	}

	fn prepare(&mut self, unpacked_dir: &Path) -> Result<()> {
		let mut file_list = String::new();
		for filename in &self.info.file_list {
			// DIFFERENCE WITH THE PERL VERSION:
			// `snailquote` doesn't escape the same characters as Perl, but that difference
			// is negligible at best - feel free to implement Perl-style escaping if you want to.
			// The list of escape sequences is in `perlop`.

			// Unquote any escaped characters in filenames - needed for non ascii characters.
			// (eg. iso_8859-1 latin set)
			let unquoted = snailquote::unescape(&filename.to_string_lossy())?;

			if unquoted.ends_with('/') {
				file_list.push_str("%dir ");
			} else if self
				.info
				.conffiles
				.iter()
				.any(|f| f.as_os_str() == unquoted.as_str())
			{
				// it's a conffile
				file_list.push_str("%config ");
			}
			// Note all filenames are quoted in case they contain spaces.
			writeln!(file_list, r#""{unquoted}""#)?;
		}

		let PackageInfo {
			name,
			version,
			release,
			depends,
			summary,
			copyright,
			distribution,
			group,
			use_scripts,
			preinst,
			postinst,
			prerm,
			postrm,
			description,
			original_format,
			..
		} = &self.info;

		let mut spec = File::create(format!(
			"{}/{name}-{version}-{release}.spec",
			unpacked_dir.display()
		))?;

		let mut build_root = std::env::current_dir()?;
		build_root.push(unpacked_dir);

		#[rustfmt::skip]
		write!(
			spec,
r#"Buildroot: {build_root}
Name: {name}
Version: {version}
Release: {release}
"#,
			build_root = build_root.display(),
		)?;

		if let [first, rest @ ..] = &depends[..] {
			write!(spec, "Requires: {first}",)?;
			for dep in rest {
				write!(spec, ", {dep}")?;
			}
			writeln!(spec)?;
		}

		#[rustfmt::skip]
		write!(
			spec,
r#"Summary: {summary}
License: {copyright}
Distribution: {distribution}
Group: Converted/{group}

%define _rpmdir ../
%define _rpmfilename %%{{NAME}}-%%{{VERSION}}-%%{{RELEASE}}.%%{{ARCH}}.rpm
%define _unpackaged_files_terminate_build 0

"#,
		)?;

		if *use_scripts {
			if let Some(preinst) = preinst {
				write!(spec, "%pre\n{preinst}\n\n")?;
			}
			if let Some(postinst) = postinst {
				write!(spec, "%pre\n{postinst}\n\n")?;
			}
			if let Some(prerm) = prerm {
				write!(spec, "%pre\n{prerm}\n\n")?;
			}
			if let Some(postrm) = postrm {
				write!(spec, "%pre\n{postrm}\n\n")?;
			}
		}
		#[rustfmt::skip]
		write!(
			spec,
r#"%description
{description}

(Converted from a {original_format} package by alien version {alien_version}.)

%files
{file_list}"#,
			alien_version = env!("CARGO_PKG_VERSION")
		)?;

		Ok(())
	}

	fn sanitize_info(&mut self) -> Result<()> {
		todo!()
	}

	fn build(&mut self, unpacked_dir: &Path) -> Result<PathBuf> {
		todo!()
	}
}
