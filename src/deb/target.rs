use std::{
	collections::HashMap,
	fmt::Write as _,
	fs::File,
	io::{BufRead, BufReader, Read, Write},
	os::unix::prelude::OpenOptionsExt,
	path::{Path, PathBuf},
};

use eyre::{bail, Context, Result};
use flate2::read::GzDecoder;
use fs_extra::dir::CopyOptions;
use subprocess::{Exec, Redirection};
use time::{format_description::well_known::Rfc2822, OffsetDateTime};

use crate::{
	util::{chmod, fetch_email_address, mkdir, ExecExt, Verbosity},
	Args, PackageInfo, Script, TargetPackage,
};

const PATCH_DIRS: &[&str] = &["/var/lib/alien", "/usr/share/alien/patches"];

#[derive(Debug)]
pub struct DebTarget {
	info: PackageInfo,
	unpacked_dir: PathBuf,
	debian_dir: PathBuf,
	dir_map: HashMap<&'static Path, &'static Path>,
}
impl DebTarget {
	pub fn new(mut info: PackageInfo, unpacked_dir: PathBuf, args: &Args) -> Result<Self> {
		Self::sanitize_info(&mut info)?;

		// Make .orig.tar.gz directory?
		if !args.deb_args.single && !args.generate {
			let option = CopyOptions {
				overwrite: true,
				..Default::default()
			};
			let mut target = unpacked_dir.as_os_str().to_owned();
			target.push(".orig");
			let target = PathBuf::from(target);

			if !target.exists() {
				mkdir(&target)?;
			}
			fs_extra::dir::copy(&unpacked_dir, target, &option)?;
		}

		let patch_file = if args.deb_args.nopatch {
			None
		} else {
			match &args.deb_args.patch {
				Some(o) => Some(o.clone()),
				None => get_patch(&info, args.deb_args.anypatch, PATCH_DIRS),
			}
		};

		let debian_dir = unpacked_dir.join("debian");
		mkdir(&debian_dir)?;

		// Use a patch file to debianize?
		if let Some(patch) = &patch_file {
			return Self::patch(info, unpacked_dir, patch, debian_dir);
		}

		// Automatic debianization.
		let mut writer = DebWriter::new(debian_dir, info)?;

		writer.write_changelog()?;
		writer.write_control()?;
		writer.write_copyright()?;
		writer.write_conffiles()?;
		writer.write_compat(7)?; // Use debhelper v7
		writer.write_rules(args.deb_args.fixperms)?;
		writer.write_scripts()?;

		let DebWriter { info, dir, .. } = writer;

		// Move files to FHS-compliant locations, if possible.
		// Note: no trailing slashes on these directory names!
		let mut dir_map = HashMap::new();

		for (old_dir, new_dir) in [
			("/usr/man", "/usr/share/man"),
			("/usr/info", "/usr/share/info"),
			("/usr/doc", "/usr/doc/info"),
		] {
			let old_dir = Path::new(old_dir);
			let new_dir = Path::new(new_dir);
			let prefixed_old_dir = dir.join(old_dir);
			let prefixed_new_dir = dir.join(new_dir);

			if prefixed_old_dir.is_dir() && !prefixed_new_dir.exists() {
				// Ignore failure..
				let dir_base = dir.join(new_dir.parent().unwrap_or(new_dir));
				Exec::cmd("install")
					.arg("-d")
					.arg(dir_base)
					.log_and_spawn(None)?;

				fs_extra::dir::move_dir(&prefixed_old_dir, &prefixed_new_dir, &CopyOptions::new())?;
				if prefixed_old_dir.is_dir() {
					std::fs::remove_dir_all(&prefixed_old_dir)?;
				}

				// store for cleantree
				dir_map.insert(old_dir, new_dir);
			}
		}

		Ok(Self {
			info,
			unpacked_dir,
			debian_dir: dir,
			dir_map,
		})
	}

	fn patch(
		mut info: PackageInfo,
		unpacked_dir: PathBuf,
		patch: &Path,
		debian_dir: PathBuf,
	) -> Result<Self> {
		let mut data = vec![];
		let mut unzipped = GzDecoder::new(File::open(patch)?);
		unzipped.read_to_end(&mut data)?;

		Exec::cmd("patch")
			.arg("-p1")
			.cwd(&unpacked_dir)
			.stdin(data)
			.log_and_output(None)
			.wrap_err("Patch error")?;

		// If any .rej file exists, we dun goof'd
		if glob::glob("*.rej").unwrap().any(|_| true) {
			bail!("Patch failed with .rej files; giving up");
		}
		for orig in glob::glob("*.orig").unwrap() {
			std::fs::remove_file(orig?)?;
		}
		chmod(debian_dir.join("rules"), 0o755)?;

		if let Ok(changelog) = File::open(debian_dir.join("changelog")) {
			let mut changelog = BufReader::new(changelog);
			let mut line = String::new();
			changelog.read_line(&mut line)?;

			// find the version inside the parens.
			if let Some((a, b)) = line.find('(').zip(line.find(')')) {
				// ensure no whitespace
				let version = line[a + 1..b].replace(char::is_whitespace, "");
				super::set_version_and_release(&mut info, &version);
			};
		}

		Ok(Self {
			info,
			unpacked_dir,
			debian_dir,
			dir_map: HashMap::new(),
		})
	}
	fn sanitize_info(info: &mut PackageInfo) -> Result<()> {
		// Version

		// filter out some characters not allowed in debian versions
		// see lib/dpkg/parsehelp.c parseversion
		fn valid_version_characters(c: char) -> bool {
			matches!(c, '-' | '.' | '+' | '~' | ':') || c.is_ascii_alphanumeric()
		}

		let iter = info
			.version
			.chars()
			.filter(|&c| valid_version_characters(c));

		info.version = if info.version.starts_with(|c: char| c.is_ascii_digit()) {
			iter.collect()
		} else {
			// make sure the version contains a digit at the start, as required by dpkg-deb
			std::iter::once('0').chain(iter).collect()
		};

		// Release
		// Make sure the release contains digits.
		if info.release.parse::<u32>().is_err() {
			info.release.push_str("-1");
		}

		// Description

		let mut desc = String::new();
		for line in info.description.lines() {
			let line = line.replace('\t', "        "); // change tabs to spaces
			let line = line.trim_end(); // remove trailing whitespace
			let line = if line.is_empty() { "." } else { line }; // empty lines become dots
			desc.push(' ');
			desc.push_str(line);
			desc.push('\n');
		}
		// remove leading blank lines
		let mut desc = String::from(desc.trim_start_matches('\n'));
		if !desc.is_empty() {
			desc.push_str(" .\n");
		}
		write!(
			desc,
			" (Converted from a {} package by alien version {}.)",
			info.original_format,
			env!("CARGO_PKG_VERSION")
		)?;

		info.description = desc;

		Ok(())
	}
}
impl TargetPackage for DebTarget {
	fn clean_tree(&mut self) -> Result<()> {
		let dir = &self.unpacked_dir;
		for (old_dir, new_dir) in &self.dir_map {
			let prefixed_old_dir = dir.join(old_dir);
			let prefixed_new_dir = dir.join(new_dir);

			if !prefixed_old_dir.exists() && prefixed_new_dir.is_dir() {
				// Ignore failure.. (should I?)
				let dir_base = dir.join(old_dir.parent().unwrap_or(old_dir));
				Exec::cmd("install")
					.arg("-d")
					.arg(dir_base)
					.log_and_spawn(None)?;

				fs_extra::dir::move_dir(&prefixed_new_dir, &prefixed_old_dir, &CopyOptions::new())?;
				if prefixed_new_dir.is_dir() {
					std::fs::remove_dir_all(&prefixed_new_dir)?;
				}
			}
		}
		std::fs::remove_dir_all(&self.debian_dir)?;
		Ok(())
	}

	fn build(&mut self) -> Result<PathBuf> {
		let PackageInfo {
			arch,
			name,
			version,
			release,
			..
		} = &self.info;

		// Detect architecture mismatch and abort with a comprehensible error message.
		if arch != "all"
			&& !Exec::cmd("dpkg-architecture")
				.arg("-i")
				.arg(arch)
				.log_and_output_without_checking(None)
				.wrap_err("dpkg-architecture not found - have you installed dpkg-dev?")?
				.success()
		{
			bail!(
				"{} is for architecture {}; the package cannot be built on this system",
				self.info.file.display(),
				arch
			);
		}

		let log = Exec::cmd("debian/rules")
			.cwd(&self.unpacked_dir)
			.arg("binary")
			.stderr(Redirection::Merge)
			.log_and_output_without_checking(None)?;
		if !log.success() {
			if log.stderr.is_empty() {
				bail!("Package build failed; could not run generated debian/rules file.");
			}
			bail!(
				"Package build failed. Here's the log:\n{}",
				log.stderr_str()
			);
		}

		let path = format!("{name}_{version}-{release}_{arch}.deb");
		Ok(PathBuf::from(path))
	}
	fn test(&mut self, file_name: &Path) -> Result<Vec<String>> {
		let Ok(lintian) = which::which("lintian") else {
			return Ok(vec!["lintian not available, so not testing".into()]);
		};

		let output = Exec::cmd(lintian)
			.arg(file_name)
			.log_and_output(None)?
			.stdout;

		let strings = output
			.lines()
			.filter_map(|s| s.ok())
			// Ignore errors we don't care about
			.filter(|s| !s.contains("unknown-section alien"))
			.map(|s| s.trim().to_owned())
			.collect();

		Ok(strings)
	}
	fn install(&mut self, file_name: &Path) -> Result<()> {
		Exec::cmd("dpkg")
			.args(&["--no-force-overwrite", "-i"])
			.arg(file_name)
			.log_and_spawn(Verbosity::VeryVerbose)
			.wrap_err("Unable to install")?;
		Ok(())
	}
}

struct DebWriter {
	dir: PathBuf,
	info: PackageInfo,
	realname: String,
	email: String,
	date: String,
}
impl DebWriter {
	fn new(dir: PathBuf, info: PackageInfo) -> Result<Self> {
		let realname = whoami::realname();
		let email = fetch_email_address()?;
		let date = OffsetDateTime::now_local()
			.unwrap_or_else(|_| OffsetDateTime::now_utc())
			.format(&Rfc2822)?;

		Ok(Self {
			dir,
			info,
			realname,
			email,
			date,
		})
	}

	fn write_changelog(&mut self) -> Result<()> {
		let Self {
			dir,
			info,
			realname,
			email,
			date,
		} = self;
		let PackageInfo {
			name,
			version,
			release,
			original_format,
			changelog: changelog_text,
			..
		} = info;

		dir.push("changelog");
		let mut file = File::create(&dir)?;

		#[rustfmt::skip]
		writeln!(
			file,
r#"{name} ({version}-{release}) experimental; urgency=low

  * Converted from {original_format} format to .deb by alien version {alien_version}

  {changelog_text}

  -- {realname} <{email}>  {date}
"#,
			alien_version = env!("CARGO_PKG_VERSION")
		)?;

		dir.pop();
		Ok(())
	}

	fn write_control(&mut self) -> Result<()> {
		let Self {
			dir,
			info,
			realname,
			email,
			..
		} = self;
		let PackageInfo {
			name,
			arch,
			dependencies: depends,
			summary,
			description,
			..
		} = info;

		dir.push("control");
		let mut file = File::create(&dir)?;

		#[rustfmt::skip]
		write!(
			file,
r#"Source: {name}
Section: alien
Priority: extra
Maintainer: {realname} <{email}>

Package: {name}
Architecture: {arch}
Depends: ${{shlibs:Depends}}"#
	)?;
		for dep in depends {
			write!(file, ", {dep}")?;
		}
		#[rustfmt::skip]
		writeln!(
			file,
r#"
Description: {summary}
{description}
"#,
		)?;

		dir.pop();
		Ok(())
	}

	fn write_copyright(&mut self) -> Result<()> {
		let Self {
			dir, info, date, ..
		} = self;
		let PackageInfo {
			original_format,
			copyright,
			binary_info,
			..
		} = info;

		dir.push("copyright");
		let mut file = File::create(&dir)?;

		#[rustfmt::skip]
		writeln!(
			file,
r#"This package was debianized by the alien program by converting
a binary .{original_format} package on {date}

Copyright: {copyright}

Information from the binary package:
{binary_info}
"#
		)?;

		dir.pop();
		Ok(())
	}

	fn write_conffiles(&mut self) -> Result<()> {
		self.dir.push("conffiles");

		let mut conffiles = self
			.info
			.conffiles
			.iter()
			// `debhelper` takes care of files in /etc.
			.filter(|s| !s.starts_with("/etc"))
			.peekable();

		if conffiles.peek().is_some() {
			let mut file = File::create(&self.dir)?;
			for conffile in conffiles {
				writeln!(file, "{}", conffile.display())?;
			}
		}

		self.dir.pop();
		Ok(())
	}

	fn write_compat(&mut self, version: u32) -> Result<()> {
		self.dir.push("compat");

		let mut file = File::create(&self.dir)?;
		writeln!(file, "{version}")?;

		self.dir.pop();
		Ok(())
	}

	fn write_rules(&mut self, fix_perms: bool) -> Result<()> {
		self.dir.push("rules");

		let mut file = File::options()
			.write(true)
			.create(true)
			.truncate(true)
			// TODO: ignore this on windows
			.mode(0o755)
			.open(&self.dir)?;
		#[rustfmt::skip]
		writeln!(
			file,
r#"#!/usr/bin/make -f
# debian/rules for alien

PACKAGE = $(shell dh_listpackages)

build:
	dh_testdir

clean:
	dh_testdir
	dh_testroot
	dh_clean -d

binary-arch: build
	dh_testdir
	dh_testroot
	dh_prep
	dh_installdirs

	dh_installdocs
	dh_installchangelogs

# Copy the packages' files.
	find . -maxdepth 1 -mindepth 1 -not -name debian -print0 | \
	xargs -0 -r -i cp -a {{}} debian/$(PACKAGE)

#
# If you need to move files around in debian/$(PACKAGE) or do some
# binary patching, do it here
#


# This has been known to break on some wacky binaries.
#	dh_strip
	dh_compress
{}	dh_fixperms
	dh_makeshlibs
	dh_installdeb
	-dh_shlibdeps
	dh_gencontrol
	dh_md5sums
	dh_builddeb

binary: binary-indep binary-arch
.PHONY: build clean binary-indep binary-arch binary
"#,
			if fix_perms { "" } else { "#" }
		)?;

		self.dir.pop();
		Ok(())
	}
	fn write_scripts(&mut self) -> Result<()> {
		// There may be a postinst with permissions fixups even when scripts are disabled.
		self.write_script(Script::AfterInstall)?;

		if self.info.use_scripts {
			self.write_script(Script::BeforeInstall)?;
			self.write_script(Script::AfterUninstall)?;
			self.write_script(Script::BeforeUninstall)?;
		}
		Ok(())
	}
	fn write_script(&mut self, script: Script) -> Result<()> {
		let data = self.info.scripts.get(&script).cloned();

		let data = if script == Script::AfterInstall {
			let mut data = data.unwrap_or_default();
			self.patch_postinst(&mut data);
			data
		} else if let Some(data) = data {
			data
		} else {
			return Ok(());
		};

		if !data.trim().is_empty() {
			self.dir.push(script.deb_name());
			std::fs::write(&self.dir, data)?;
			self.dir.pop();
		}
		Ok(())
	}
	fn patch_postinst(&self, old: &mut String) {
		let PackageInfo { file_info, .. } = &self.info;

		if file_info.is_empty() {
			return;
		}

		// If there is no postinst, let's make one up..
		if old.is_empty() {
			old.push_str("#!/bin/sh\n");
		}

		let index = old.find('\n').unwrap_or(old.len());
		let first_line = &old[..index];

		if let Some(s) = first_line.strip_prefix("#!") {
			let s = s.trim_start();
			if let "/bin/bash" | "/bin/sh" = s {
				eprintln!("warning: unable to add ownership fixup code to postinst as the postinst is not a shell script!");
				return;
			}
		}

		let mut injection = String::from("\n# alien added permissions fixup code");

		for (file, file_info) in file_info {
			// no single quotes in single quotes...
			let escaped_file = file.to_string_lossy().replace('\'', r#"'"'"'"#);
			let own_info = &file_info.owner;
			write!(injection, "\nchown '{own_info}' '{escaped_file}'").unwrap();

			if let Some(mode_info) = file_info.mode {
				write!(injection, "\nchmod '{mode_info}' '{escaped_file}'").unwrap();
			}
		}
		old.insert_str(index, &injection);
	}
}

fn get_patch(info: &PackageInfo, anypatch: bool, dirs: &[&str]) -> Option<PathBuf> {
	let mut patches: Vec<_> = dirs
		.iter()
		.flat_map(|dir| {
			let p = format!(
				"{}/{}_{}-{}*.diff.gz",
				dir, info.name, info.version, info.release
			);
			glob::glob(&p).unwrap()
		})
		.collect();

	if patches.is_empty() {
		// Try not matching the release, see if that helps.
		patches.extend(dirs.iter().flat_map(|dir| {
			let p = format!("{dir}/{}_{}*.diff.gz", info.name, info.version);
			glob::glob(&p).unwrap()
		}));

		if !patches.is_empty() && anypatch {
			// Fall back to anything that matches the name.
			patches.extend(dirs.iter().flat_map(|dir| {
				let p = format!("{dir}/{}_*.diff.gz", info.name);
				glob::glob(&p).unwrap()
			}));
		}
	}

	// just get the first one
	patches.into_iter().find_map(|p| p.ok())
}
