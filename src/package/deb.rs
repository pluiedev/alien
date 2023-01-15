use std::{
	borrow::Cow,
	collections::HashMap,
	ffi::OsStr,
	fmt::Write as _,
	fs::File,
	io::{BufRead, BufReader, Cursor, Read, Write},
	os::unix::prelude::OpenOptionsExt,
	path::{Path, PathBuf},
};

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use fs_extra::dir::CopyOptions;
use simple_eyre::eyre::{bail, Context, Result};
use subprocess::Exec;
use time::{format_description::well_known::Rfc2822, OffsetDateTime};
use xz::read::XzDecoder;

use crate::{
	util::{ExecExt, Verbosity},
	Args,
};

use super::{common, Format, PackageBehavior, PackageInfo};

const PATCH_DIRS: &[&str] = &["/var/lib/alien", "/usr/share/alien/patches"];

pub struct Deb {
	info: PackageInfo,
	deb_file: PathBuf,
	data_tar: tar::Archive<Cursor<Vec<u8>>>,
	dpkg_deb: Option<PathBuf>,
	fix_perms: bool,
	patch_file: Option<PathBuf>,
	dir_map: HashMap<PathBuf, PathBuf>,
}
impl Deb {
	pub fn check_file(file: &Path) -> bool {
		match file.extension() {
			Some(o) => o.eq_ignore_ascii_case("deb"),
			None => false,
		}
	}

	pub fn new(deb_file: PathBuf, args: &Args) -> Result<Self> {
		let mut info = PackageInfo {
			distribution: "Debian".into(),
			original_format: Format::Deb,
			..Default::default()
		};

		let dpkg_deb = which::which("dpkg-deb").ok();

		let mut control_files = fetch_control_files(
			dpkg_deb.as_deref(),
			&deb_file,
			&[
				"control",
				"conffiles",
				"postinst",
				"postrm",
				"preinst",
				"prerm",
			],
		)?;
		dbg!(&control_files);
		let Some(control) = control_files.remove("control") else {
            bail!("Control file not found!");
        };

		let mut description = String::new();
		let mut field = String::new();
		for c in control.lines() {
			if c.starts_with(' ') && field == "description" {
				// Handle extended description
				let c = c.trim_start();
				if c != "." {
					description.push_str(c);
					description.push('\n');
				}
			} else if let Some((f, value)) = c.split_once(":") {
				let value = value.trim().to_owned();
				// Really old debs might have oddly capitalized field names.
				field = f.to_ascii_lowercase();

				match field.as_str() {
					"package" => info.name = value,
					"version" => info.set_version_and_release(&value)?,
					"architecture" => info.arch = value,
					"maintainer" => info.maintainer = value,
					"section" => info.group = value,
					"description" => info.summary = value,
					"depends" => info.depends = value.split(", ").map(|s| s.to_owned()).collect(),
					_ => { /* ignore */ }
				}
			}
		}

		info.description = description;
		info.copyright = format!("see /usr/share/doc/{}/copyright", info.name);
		if info.group.is_empty() {
			info.group.push_str("unknown");
		}
		info.binary_info = control;

		if let Some(conffiles) = control_files.remove("conffiles") {
			info.conffiles
				.extend(conffiles.lines().map(|s| s.trim().to_owned()));
		};

		let mut data_tar = fetch_data_tar(dpkg_deb.as_deref(), &deb_file)?;

		// In the tar file, the files are all prefixed with "./", but we want them
		// to be just "/". So, we gotta do this!
		info.file_list.extend(
			data_tar
				.entries()?
				.filter_map(|f| f.ok())
				.filter_map(|f| f.path().map(Cow::into_owned).ok())
				.map(|s| {
					std::iter::once(OsStr::new("/"))
						.chain(s.iter().skip_while(|&x| x == "."))
						.collect::<PathBuf>()
				}),
		);

		info.postinst = control_files.remove("postinst");
		info.postrm = control_files.remove("postrm");
		info.preinst = control_files.remove("preinst");
		info.prerm = control_files.remove("prerm");

		let patch_file = if args.nopatch {
			None
		} else {
			match &args.patch {
				Some(o) => Some(o.clone()),
				None => get_patch(&info, args.anypatch, PATCH_DIRS),
			}
		};

		dbg!(&info);

		Ok(Self {
			info,
			deb_file,
			data_tar,
			dpkg_deb,
			fix_perms: args.fixperms,
			patch_file,
			dir_map: HashMap::new(),
		})
	}
	fn save_script(&self, debian_dir: &Path, script: &str, mut data: String) -> Result<()> {
		if script == "postinst" {
			self.patch_post_inst(&mut data);
		}
		if data.chars().any(|c| !c.is_whitespace()) {
			std::fs::write(debian_dir.join(script), data)?;
		}
		Ok(())
	}
	fn patch_post_inst(&self, old: &mut String) {
		let PackageInfo {
			owninfo, modeinfo, ..
		} = self.info();

		if owninfo.is_empty() {
			return;
		}

		// If there is no postinst, let's make one up..
		if old.is_empty() {
			old.push_str("#!/bin/sh\n");
		}

		let index = old.find("\n").unwrap_or(old.len());
		let first_line = &old[..index];

		if let Some(s) = first_line.strip_prefix("#!") {
			let s = s.trim_start();
			if let "/bin/bash" | "/bin/sh" = s {
				eprintln!("warning: unable to add ownership fixup code to postinst as the postinst is not a shell script!");
				return;
			}
		}

		let mut injection = String::from("\n# alien added permissions fixup code");

		for (file, owi) in owninfo {
			// no single quotes in single quotes...
			let file = file.replace("'", r#"'"'"'"#);
			write!(injection, "\nchown '{owi}' '{file}'").unwrap();

			if let Some(mdi) = modeinfo.get(&file) {
				write!(injection, "\nchmod '{mdi}' '{file}'").unwrap();
			}
		}
		old.insert_str(index, &injection);
	}
}
impl PackageBehavior for Deb {
	fn info(&self) -> &PackageInfo {
		&self.info
	}
	fn info_mut(&mut self) -> &mut PackageInfo {
		&mut self.info
	}
	fn install(&mut self, file_name: &Path) -> Result<()> {
		Exec::cmd("dpkg")
			.args(&["--no-force-overwrite", "-i"])
			.arg(&file_name)
			.log_and_spawn(Verbosity::VeryVerbose)
			.wrap_err("Unable to install")?;
		Ok(())
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

	fn unpack(&mut self) -> Result<PathBuf> {
		let work_dir = common::make_unpack_work_dir(self.info())?;
		self.data_tar.unpack(&work_dir)?;
		Ok(work_dir)
	}

	fn prepare(&mut self, unpacked_dir: &Path) -> Result<()> {
		let debian_dir = unpacked_dir.join("debian");
		std::fs::create_dir(&debian_dir)?;

		// Use a patch file to debianize?
		if let Some(patch) = &self.patch_file {
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
			common::chmod(debian_dir.join("rules"), 0o755)?;

			if let Ok(changelog) = File::open(debian_dir.join("changelog")) {
				let mut changelog = BufReader::new(changelog);
				let mut line = String::new();
				changelog.read_line(&mut line)?;

				// find the version inside the parens.
				let Some((a, b)) = line.find("(").zip(line.find(")")) else {
					return Ok(());
				};
				// ensure no whitespace
				let version = line[a + 1..b].replace(|c: char| c.is_whitespace(), "");

				self.info.set_version_and_release(&version)?;
			}

			return Ok(());
		}

		// Automatic debianization.

		let PackageInfo {
			name,
			version,
			release,
			original_format,
			changelog_text,
			arch,
			depends,
			summary,
			copyright,
			binary_info,
			conffiles,
			use_scripts,
			postinst,
			postrm,
			preinst,
			prerm,
			..
		} = self.info();

		let realname = whoami::realname();
		let email = fetch_email_address()?;
		let date = OffsetDateTime::now_local()
			.unwrap_or_else(|_| OffsetDateTime::now_utc())
			.format(&Rfc2822)?;
		let alien_version = env!("CARGO_PKG_VERSION");

		{
			// Changelog file.
			let mut file = File::create(debian_dir.join("changelog"))?;
			#[rustfmt::skip]
            writeln!(
                file,
r#"{name} ({}-{release}) experimental; urgency=low

  * Converted from {original_format} format to .deb by alien version {alien_version}
  
  {changelog_text}

  -- {realname} <{email}>  {date}
"#,
				self.info.version()
            )?;
		}
		{
			// Control file.
			let mut file = File::create(debian_dir.join("control"))?;
			#[rustfmt::skip]
            writeln!(
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
{}
"#,
				self.info.description()
            )?;
		}
		{
			// Copyright file.
			let mut file = File::create(debian_dir.join("copyright"))?;
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
		}

		// Conffiles, if any. Note that debhelper takes care of files in /etc.
		let mut conffiles = conffiles
			.iter()
			.filter(|s| !s.starts_with("/etc"))
			.peekable();
		if conffiles.peek().is_some() {
			let mut file = File::create(debian_dir.join("conffiles"))?;
			for conffile in conffiles {
				writeln!(file, "{conffile}")?;
			}
		}

		// Use debhelper v7
		std::fs::write(debian_dir.join("compat"), b"7\n")?;

		// A minimal rules file.
		{
			let mut file = File::options()
				.write(true)
				.create(true)
				.truncate(true)
				// TODO: ignore this on windows
				.mode(0o755)
				.open(debian_dir.join("rules"))?;
			#[rustfmt::skip]
            writeln!(
				file,
r#"
#!/usr/bin/make -f
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
#   dh_strip
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
				if self.fix_perms { "" } else { "#" }
			)?;
		}
		if *use_scripts {
			if let Some(postinst) = postinst {
				self.save_script(&debian_dir, "postinst", postinst.clone())?;
			}
			if let Some(postrm) = postrm {
				self.save_script(&debian_dir, "postrm", postrm.clone())?;
			}
			if let Some(preinst) = preinst {
				self.save_script(&debian_dir, "preinst", preinst.clone())?;
			}
			if let Some(prerm) = prerm {
				self.save_script(&debian_dir, "prerm", prerm.clone())?;
			}
		} else {
			// There may be a postinst with permissions fixups even when scripts are disabled.
			self.save_script(&debian_dir, "postinst", String::new())?;
		}

		// Move files to FHS-compliant locations, if possible.
		// Note: no trailling slashes on these directory names!
		for old_dir in ["/usr/man", "/usr/info", "/usr/doc"] {
			let old_dir = debian_dir.join(old_dir);
			let mut new_dir = debian_dir.join("/usr/share/");
			new_dir.push(old_dir.file_name().unwrap());

			if old_dir.exists() && !new_dir.exists() {
				// Ignore failure..
				let dir_base = new_dir.parent().unwrap_or(&new_dir);
				Exec::cmd("install")
					.arg("-d")
					.arg(dir_base)
					.log_and_spawn(None)?;

				fs_extra::dir::move_dir(&old_dir, &new_dir, &CopyOptions::new())?;
				if old_dir.exists() {
					std::fs::remove_dir_all(&old_dir)?;
				}

				// store for cleantree
				self.dir_map.insert(old_dir, new_dir);
			}
		}
		Ok(())
	}

	fn build(&mut self, unpacked_dir: &Path) -> Result<PathBuf> {
		todo!()
	}

	fn check_file(&mut self, file_name: &str) -> bool {
		todo!()
	}
}

//= Utilties

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
			let p = format!("{}/{}_{}*.diff.gz", dir, info.name, info.version);
			glob::glob(&p).unwrap()
		}));

		if !patches.is_empty() && anypatch {
			// Fall back to anything that matches the name.
			patches.extend(dirs.iter().flat_map(|dir| {
				let p = format!("{}/{}_*.diff.gz", dir, info.name);
				glob::glob(&p).unwrap()
			}))
		}
	}

	// just get the first one
	patches.into_iter().find_map(|p| p.ok())
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

fn fetch_data_tar(
	dpkg_deb: Option<&Path>,
	deb_file: &Path,
) -> Result<tar::Archive<Cursor<Vec<u8>>>> {
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

	Ok(tar::Archive::new(Cursor::new(tar)))
}

fn fetch_email_address() -> Result<String> {
	// TODO: how can this possibly work on windows?
	// Also TODO: just ask the user for their email address. ffs.
	// I don't have EMAIL set, and nor do i have `/etc/mailname`,
	// so now I'm stuck with leah@procrastinator, which of course, is not a real email address.

	if let Ok(email) = std::env::var("EMAIL") {
		return Ok(email);
	}
	let mailname = match std::fs::read_to_string("/etc/mailname") {
		Ok(o) => o,
		Err(_) => Exec::cmd("hostname").log_and_output(None)?.stdout_str(),
	};
	Ok(format!("{}@{}", whoami::username(), mailname))
}

trait InfoExt {
	fn version(&self) -> String;
	fn set_version_and_release(&mut self, version: &str) -> Result<()>;
	fn description(&self) -> String;
}
impl InfoExt for PackageInfo {
	fn version(&self) -> String {
		// filter out some characters not allowed in debian versions
		// see lib/dpkg/parsehelp.c parseversion
		fn valid_version_characters(c: &char) -> bool {
			matches!(c, '-' | '.' | '+' | '~' | ':') || c.is_ascii_alphanumeric()
		}

		let iter = self.version.chars().filter(valid_version_characters);

		if !self.version.starts_with(|c: char| c.is_ascii_digit()) {
			// make sure the version contains a digit at the start, as required by dpkg-deb
			std::iter::once('0').chain(iter).collect()
		} else {
			iter.collect()
		}
	}
	fn set_version_and_release(&mut self, version: &str) -> Result<()> {
		let (version, release) = if let Some((version, release)) = version.split_once("-") {
			(version, release.parse()?)
		} else {
			(version, 1)
		};

		// Ignore epochs.
		let version = version.split_once(":").map(|t| t.1).unwrap_or(version);

		self.version = version.to_owned();
		self.release = release;
		Ok(())
	}
	fn description(&self) -> String {
		let mut ret = String::new();
		for line in self.description.lines() {
			let line = line.replace('\t', "        "); // change tabs to spaces
			let line = line.trim_end(); // remove trailing whitespace
			let line = if line.is_empty() { "." } else { line }; // empty lines become dots
			ret.push(' ');
			ret.push_str(line);
			ret.push('\n');
		}
		// remove leading blank lines
		let mut ret = String::from(ret.trim_start_matches('\n'));
		if !ret.is_empty() {
			ret.push_str(" .\n");
		}
		write!(
			ret,
			" (Converted from a {} package by alien version {}.)",
			self.original_format,
			env!("CARGO_PKG_VERSION")
		)
		.unwrap();

		ret
	}
}
