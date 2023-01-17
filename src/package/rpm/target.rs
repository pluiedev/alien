use std::{
	fmt::Write as _,
	fs::File,
	io::Write,
	path::{Path, PathBuf},
};

use base64::Engine;
use simple_eyre::{
	eyre::{bail, Context},
	Result,
};
use subprocess::{Exec, Redirection};

use crate::{
	package::{PackageInfo, TargetPackageBehavior},
	util::{ExecExt, Verbosity},
};

pub struct RpmTarget {
	pub(crate) info: PackageInfo,
	unpacked_dir: PathBuf,
	spec: PathBuf,
}
impl RpmTarget {
	pub fn new(mut info: PackageInfo, unpacked_dir: PathBuf) -> Result<Self> {
		Self::sanitize_info(&mut info);

		let mut file_list = String::new();
		for filename in &info.file_list {
			// DIFFERENCE WITH THE PERL VERSION:
			// `snailquote` doesn't escape the same characters as Perl, but that difference
			// is negligible at best - feel free to implement Perl-style escaping if you want to.
			// The list of escape sequences is in `perlop`.

			// Unquote any escaped characters in filenames - needed for non ascii characters.
			// (eg. iso_8859-1 latin set)
			let unquoted = snailquote::unescape(&filename.to_string_lossy())?;

			if unquoted.ends_with('/') {
				file_list.push_str("%dir ");
			} else if info
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
			scripts,
			description,
			original_format,
			..
		} = &info;

		let spec = PathBuf::from(format!(
			"{}/{name}-{version}-{release}.spec",
			unpacked_dir.display()
		));
		let mut spec_file = File::create(&spec)?;

		let mut build_root = std::env::current_dir()?;
		build_root.push(&unpacked_dir);

		#[rustfmt::skip]
		write!(
			spec_file,
r#"Buildroot: {build_root}
Name: {name}
Version: {version}
Release: {release}
"#,
			build_root = build_root.display(),
		)?;

		if let [first, rest @ ..] = &depends[..] {
			write!(spec_file, "Requires: {first}",)?;
			for dep in rest {
				write!(spec_file, ", {dep}")?;
			}
			writeln!(spec_file)?;
		}

		#[rustfmt::skip]
		write!(
			spec_file,
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
			for (name, script) in super::RPM_SCRIPT_NAMES
				.iter()
				.zip(PackageInfo::SCRIPTS)
			{
				let Some(script) = scripts.get(script) else { continue; };
				write!(spec_file, "%{name}\n{script}\n\n")?;
			}
		}
		#[rustfmt::skip]
		write!(
			spec_file,
r#"%description
{description}

(Converted from a {original_format} package by alien version {alien_version}.)

%files
{file_list}"#,
			alien_version = env!("CARGO_PKG_VERSION")
		)?;

		Ok(Self {
			info,
			unpacked_dir,
			spec,
		})
	}

	pub(crate) fn build_with(&mut self, cmd: &Path) -> Result<PathBuf> {
		let rpmdir = Exec::cmd("rpm")
			.arg("--showrc")
			.log_and_output(None)?
			.stdout_str()
			.lines()
			.find_map(|l| {
				if let Some(l) = l.strip_prefix("rpmdir") {
					let path = l.trim_start().trim_start_matches(':').trim_start();
					Some(PathBuf::from(path))
				} else {
					None
				}
			});

		let PackageInfo {
			name,
			version,
			release,
			arch,
			..
		} = &self.info;

		let rpm = format!("{name}-{version}-{release}.{arch}.rpm");

		let (rpm, arch_flag) = if let Some(rpmdir) = rpmdir {
			// Old versions of rpm toss it off in te middle of nowhere.
			let mut r = rpmdir.join(arch);
			r.push(&rpm);
			(r, "--buildarch")
		} else {
			// Presumably we're dealing with rpm 3.0 or above, which doesn't
			// output rpmdir in any format I'd care to try to parse.
			// Instead, rpm is now of a late enough version to notice the
			// %define's in the spec file, which will make the file end up
			// in the directory we started in.
			// Anyway, let's assume this is version 3 or above.

			// This is the new command line argument to set the arch rpms.
			// It appeared in rpm version 3.
			(PathBuf::from(rpm), "--target")
		};

		let mut build_root = std::env::current_dir()?;
		build_root.push(&self.unpacked_dir);

		let mut cmd = Exec::cmd(cmd)
			.cwd(&self.unpacked_dir)
			.stderr(Redirection::Merge)
			.arg("--buildroot")
			.arg(build_root)
			.arg("-bb")
			.arg(arch_flag)
			.arg(arch);

		if let Ok(opt) = std::env::var("RPMBUILDOPT") {
			let opt: Vec<_> = opt.split(' ').collect();
			cmd = cmd.args(&opt);
		}

		let spec = format!("{name}-{version}-{release}.spec");

		let cmdline = cmd.to_cmdline_lossy();
		let out = cmd.arg(&spec).log_and_output_without_checking(None)?;

		if !out.success() {
			bail!(
				"Package build failed. Here's the log of the command ({cmdline}):\n{}",
				out.stdout_str()
			);
		}

		Ok(rpm)
	}

	fn sanitize_info(info: &mut PackageInfo) {
		// When retrieving scripts for building, we have to do some truly sick mangling.
		// Since debian/slackware scripts can be anything -- perl programs or binary files --
		// and rpm is limited to only shell scripts, we need to encode the files and add a
		// scrap of shell script to make it unextract and run on the fly.

		for script in PackageInfo::SCRIPTS {
			let Some(script) = info.scripts.get_mut(script) else { return; };

			if script.chars().all(char::is_whitespace) {
				return; // it's blank.
			}

			if let Some(s) = script.strip_prefix("#!") {
				if s.trim_start().starts_with("/bin/sh") {
					return; // looks like a shell script already
				}
			}
			// The original used uuencoding. That is cursed. We don't do that here
			let encoded = base64::engine::general_purpose::STANDARD.encode(&script);

			#[rustfmt::skip]
			let patched = format!(
r#"#!/bin/sh
set -e
mkdir /tmp/alien.$$
echo '{encoded}' | base64 -d > /tmp/alien.$$/script
chmod 755 /tmp/alien.$$/script
/tmp/alien.$$/script "$@"
rm -f /tmp/alien.$$/script
rmdir /tmp/alien.$$
"#
			);
			*script = patched;
		}

		info.version = info.version.replace('-', "_");

		let arch = match info.arch.as_str() {
			"amd64" => Some("x86_64"),
			"powerpc" => Some("ppc"), // XXX is this the canonical name for powerpc on rpm systems?
			"hppa" => Some("parisc"),
			"all" => Some("noarch"),
			"ppc64el" => Some("ppc64le"),
			_ => None,
		};
		if let Some(arch) = arch {
			info.arch = arch.to_owned();
		}
	}
}

impl TargetPackageBehavior for RpmTarget {
	fn clear_unpacked_dir(&mut self) {
		self.unpacked_dir.clear();
	}
	fn clean_tree(&mut self) -> Result<()> {
		std::fs::remove_file(&self.spec)?;
		Ok(())
	}
	fn build(&mut self) -> Result<PathBuf> {
		self.build_with(Path::new("rpmbuild"))
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
}
