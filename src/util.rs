use std::fmt::Debug;

use bpaf::{construct, long, Parser};
use enumflags2::BitFlags;
use eyre::{bail, Context, Result};
use once_cell::sync::OnceCell;
use subprocess::{CaptureData, Exec, NullFile, Pipeline, Redirection};

use crate::{Format, PackageInfo};

use std::{
	os::unix::prelude::PermissionsExt,
	path::{Path, PathBuf},
};

#[allow(clippy::struct_excessive_bools)]
#[derive(bpaf::Bpaf, Debug)]
pub struct Args {
	#[bpaf(external)]
	pub formats: BitFlags<Format>,

	#[bpaf(external, group_help("deb-specific options:"))]
	pub deb_args: DebArgs,

	#[bpaf(external, group_help("tgz-specific options:"))]
	pub tgz_args: TgzArgs,

	/// Install generated package.
	#[bpaf(short, long, group_help(""))] // have to forcibly break the group for some reason
	pub install: bool,

	/// Generate build tree, but do not build package.
	#[bpaf(short, long)]
	pub generate: bool,

	/// Include scripts in package.
	#[bpaf(short('c'), long)]
	pub scripts: bool,

	/// Set architecture of the generated package.
	#[bpaf(argument("arch"))]
	pub target: Option<String>,

	/// Display each command alien runs.
	#[bpaf(external)]
	pub verbosity: Verbosity,

	/// Do not change version of generated package.
	#[bpaf(short, long)]
	pub keep_version: bool,

	/// Increment package version by this number.
	#[bpaf(argument("number"), fallback(1))]
	pub bump: u32,

	/// Package file or files to convert.
	#[bpaf(positional("FILES"), some("You must specify a file to convert."))]
	pub files: Vec<PathBuf>,
}
#[derive(Debug, bpaf::Bpaf)]
pub struct DebArgs {
	/// Specify patch file to use instead of automatically looking for patch
	/// in /var/lib/alien.
	#[bpaf(
		argument("patch"),
		guard(patch_file_exists, "Specified patch file cannot be found")
	)]
	pub patch: Option<PathBuf>,
	/// Do not use patches.
	pub nopatch: bool,
	/// Use even old version os patches.
	pub anypatch: bool,
	/// Like --generate, but do not create .orig directory.
	#[bpaf(short, long)]
	pub single: bool,
	/// Munge/fix permissions and owners.
	pub fixperms: bool,
	/// Test generated packages with lintian.
	pub test: bool,
}

#[derive(Debug, bpaf::Bpaf)]
pub struct TgzArgs {
	/// Specify package description.
	#[bpaf(argument("desc"))]
	pub description: Option<String>,

	#[bpaf(argument("version"))]
	/// Specify package version.
	pub version: Option<String>,
}

fn formats() -> impl Parser<BitFlags<Format>> {
	let to_deb = long("to-deb")
		.short('d')
		.help("Generate a Debian deb package (default).")
		.switch();
	let to_rpm = long("to-rpm")
		.short('r')
		.help("Generate a Red Hat rpm package.")
		.switch();
	let to_slp = long("to-slp")
		.help("Generate a Stampede slp package.")
		.switch();
	let to_lsb = long("to-lsb")
		.short('l')
		.help("Generate a LSB package.")
		.switch();
	let to_tgz = long("to-tgz")
		.short('t')
		.help("Generate a Slackware tgz package.")
		.switch();
	let to_pkg = long("to-pkg")
		.short('p')
		.help("Generate a Solaris pkg package.")
		.switch();

	construct!(to_deb, to_rpm, to_slp, to_lsb, to_tgz, to_pkg,).map(|(d, r, s, l, t, p)| {
		let mut formats = BitFlags::empty();

		#[rustfmt::skip]
		let _ = {
			if d { formats |= Format::Deb; }
			if r { formats |= Format::Rpm; }
			if s { formats |= Format::Slp; }
			if l { formats |= Format::Lsb; }
			if t { formats |= Format::Tgz; }
			if p { formats |= Format::Pkg; }
		};

		if formats.is_empty() {
			// Default to deb
			formats |= Format::Deb;
		}
		formats
	})
}

fn patch_file_exists(s: &Option<PathBuf>) -> bool {
	s.as_ref().map_or(true, |s| s.exists())
}

fn verbosity() -> impl Parser<Verbosity> {
	let verbose = long("verbose")
		.short('v')
		.help("Display each command alien runs.")
		.switch();
	let very_verbose = long("veryverbose")
		.help("Be verbose, and also display output of run commands.")
		.switch();

	construct!(verbose, very_verbose).map(|(v, vv)| {
		if vv {
			Verbosity::VeryVerbose
		} else if v {
			Verbosity::Verbose
		} else {
			Verbosity::Normal
		}
	})
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verbosity {
	Normal,
	Verbose,
	VeryVerbose,
}
impl Verbosity {
	pub fn set(self) {
		VERBOSITY.set(self).unwrap();
	}
	pub fn get() -> Verbosity {
		*VERBOSITY.get().unwrap()
	}
}
static VERBOSITY: OnceCell<Verbosity> = OnceCell::new();

pub(crate) trait ExecExt {
	type Output;

	fn log_and_spawn(self, verbosity: impl Into<Option<Verbosity>>) -> Result<()>;
	fn log_and_output(self, verbosity: impl Into<Option<Verbosity>>) -> Result<CaptureData>;
	fn log_and_output_without_checking(
		self,
		verbosity: impl Into<Option<Verbosity>>,
	) -> Result<CaptureData>;
}
impl ExecExt for Exec {
	type Output = CaptureData;

	fn log_and_spawn(mut self, verbosity: impl Into<Option<Verbosity>>) -> Result<()> {
		let verbosity = verbosity.into().unwrap_or_else(Verbosity::get);
		let cmdline = self.to_cmdline_lossy();
		if verbosity != Verbosity::Normal {
			println!("\t{cmdline}");
		}
		if verbosity != Verbosity::VeryVerbose {
			self = self.stdout(NullFile);
		}
		let capture = self.capture()?;
		if !capture.success() {
			bail!(
				"Error executing command - stderr:\n{}",
				capture.stderr_str()
			)
		}
		Ok(())
	}

	fn log_and_output(self, verbosity: impl Into<Option<Verbosity>>) -> Result<CaptureData> {
		let out = self.log_and_output_without_checking(verbosity)?;
		if !out.success() {
			bail!("Error executing command - stderr:\n{}", out.stderr_str())
		}
		Ok(out)
	}
	fn log_and_output_without_checking(
		mut self,
		verbosity: impl Into<Option<Verbosity>>,
	) -> Result<CaptureData> {
		let verbosity = verbosity.into().unwrap_or_else(Verbosity::get);
		self = self.stdout(Redirection::Pipe);

		let cmdline = self.to_cmdline_lossy();
		if verbosity != Verbosity::Normal {
			println!("\t{cmdline}");
		}
		let output = self.capture()?;

		if verbosity == Verbosity::VeryVerbose {
			let stdout = String::from_utf8_lossy(&output.stdout);
			println!("{stdout}");
		}
		Ok(output)
	}
}

impl ExecExt for Pipeline {
	type Output = CaptureData;

	fn log_and_spawn(mut self, verbosity: impl Into<Option<Verbosity>>) -> Result<()> {
		let verbosity = verbosity.into().unwrap_or_else(Verbosity::get);
		if verbosity != Verbosity::Normal {
			println!("\t{self:?}");
		}
		if verbosity != Verbosity::VeryVerbose {
			self = self.stdout(NullFile);
		}
		let capture = self.capture()?;
		if !capture.success() {
			bail!(
				"Error executing command - stderr:\n{}",
				capture.stderr_str()
			)
		}
		Ok(())
	}

	fn log_and_output(self, verbosity: impl Into<Option<Verbosity>>) -> Result<CaptureData> {
		let out = self.log_and_output_without_checking(verbosity)?;
		if !out.success() {
			bail!("Error executing command - stderr:\n{}", out.stderr_str())
		}
		Ok(out)
	}
	fn log_and_output_without_checking(
		self,
		verbosity: impl Into<Option<Verbosity>>,
	) -> Result<CaptureData> {
		let verbosity = verbosity.into().unwrap_or_else(Verbosity::get);
		if verbosity != Verbosity::Normal {
			println!("\t{self:?}");
		}
		let output = self.capture()?;

		if verbosity == Verbosity::VeryVerbose {
			let stdout = String::from_utf8_lossy(&output.stdout);
			println!("{stdout}");
		}
		Ok(output)
	}
}

#[cfg(unix)]
pub(crate) fn mkdir<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
	fn _mkdir(path: &Path) -> std::io::Result<()> {
		if let Some(Verbosity::Verbose) = VERBOSITY.get() {
			println!("\tmkdir {}", path.display());
		}

		std::fs::create_dir(path)
	}
	_mkdir(path.as_ref())
}

#[cfg(unix)]
pub(crate) fn chmod<P: AsRef<Path>>(path: P, mode: u32) -> std::io::Result<()> {
	fn _chmod(path: &Path, mode: u32) -> std::io::Result<()> {
		if let Some(Verbosity::Verbose) = VERBOSITY.get() {
			println!("\tchmod {mode:o} {}", path.display());
		}

		let mut perms = std::fs::metadata(path)?.permissions();
		perms.set_mode(mode);
		std::fs::set_permissions(path, perms)?;
		Ok(())
	}
	_chmod(path.as_ref(), mode)
}

#[cfg(not(unix))]
pub(crate) fn chmod(_path: &Path, _mode: u32) -> std::io::Result<()> {
	// do nothing :p
}

pub(crate) fn make_unpack_work_dir(info: &PackageInfo) -> Result<PathBuf> {
	let work_dir = format!("{}-{}", info.name, info.version);
	mkdir(&work_dir).wrap_err_with(|| format!("unable to mkdir {work_dir}"))?;

	// If the parent directory is suid/guid, mkdir will make the root
	// directory of the package inherit those bits. That is a bad thing,
	// so explicitly force perms to 755.

	chmod(&work_dir, 0o755)?;
	Ok(PathBuf::from(work_dir))
}

pub(crate) fn fetch_email_address() -> String {
	// TODO: how can this possibly work on windows?
	// Also TODO: just ask the user for their email address. ffs.
	// I don't have EMAIL set, and nor do i have `/etc/mailname`,
	// so now I'm stuck with leah@procrastinator, which of course, is not a real email address.

	if let Ok(email) = std::env::var("EMAIL") {
		email
	} else {
		let mailname = match std::fs::read_to_string("/etc/mailname") {
			Ok(o) => o,
			Err(_) => whoami::hostname(),
		};
		format!("{}@{mailname}", whoami::username())
	}
}
