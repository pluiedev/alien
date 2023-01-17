use std::fmt::Debug;

use once_cell::sync::OnceCell;
use simple_eyre::eyre::{bail, Context, Result};
use subprocess::{CaptureData, Exec, NullFile, Pipeline};

use crate::PackageInfo;

use std::{
	os::unix::prelude::PermissionsExt,
	path::{Path, PathBuf},
};

#[allow(clippy::struct_excessive_bools)]
#[derive(clap::Parser, Debug)]
pub struct Args {
	/// Generate a Debian deb package (default).
	#[arg(short = 'd', long)]
	pub to_deb: bool,

	// deb-specific settings
	/// Specify patch file to use instead of automatically looking for patch
	/// in /var/lib/alien.
	#[arg(long, requires = "to_deb", value_parser = patch_file_exists)]
	pub patch: Option<PathBuf>,
	/// Do not use patches.
	#[arg(long, requires = "to_deb", conflicts_with = "patch")]
	pub nopatch: bool,
	/// Use even old version os patches.
	#[arg(long, requires = "to_deb")]
	pub anypatch: bool,
	/// Like --generate, but do not create .orig directory.
	#[arg(short, long, requires = "to_deb")]
	pub single: bool,
	/// Munge/fix permissions and owners.
	#[arg(long, requires = "to_deb")]
	pub fixperms: bool,
	/// Test generated packages with lintian.
	#[arg(long, requires = "to_deb")]
	pub test: bool,
	// end deb-specific settings
	/// Generate a Red Hat rpm package.
	#[arg(short = 'r', long)]
	pub to_rpm: bool,
	/// Generate a Stampede slp package.
	#[arg(long)]
	pub to_slp: bool,
	/// Generate a LSB package.
	#[arg(short = 'l', long)]
	pub to_lsb: bool,
	/// Generate a Slackware tgz package.
	#[arg(short = 't', long)]
	pub to_tgz: bool,

	// tgx-specific settings
	/// Specify package description.
	#[arg(long, requires = "to_tgz")]
	pub description: Option<String>,

	// /// Specify package version.
	// #[arg(long, requires = "to_tgz", require_equals = true)]
	// version: Option<String>,

	// end tgx-specific settings
	/// Generate a Solaris pkg package.
	#[arg(short = 'p', long)]
	pub to_pkg: bool,
	/// Install generated package.
	#[arg(short, long, conflicts_with_all = ["generate", "single"])]
	pub install: bool,
	/// Generate build tree, but do not build package.
	#[arg(short, long)]
	pub generate: bool,
	/// Include scripts in package.
	#[arg(short = 'c', long)]
	pub scripts: bool,
	/// Set architecture of the generated package.
	#[arg(long)]
	pub target: Option<String>,
	/// Display each command alien runs.
	#[arg(short, long)]
	pub verbose: bool,
	/// Be verbose, and also display output of run commands.
	#[arg(long)]
	pub veryverbose: bool,

	// TODO: veryverbose
	/// Do not change version of generated package.
	#[arg(short, long)]
	pub keep_version: bool,
	/// Increment package version by this number.
	#[arg(long, default_value_t = 1)]
	pub bump: u32,

	/// Package file or files to convert.
	#[arg(required = true)]
	pub files: Vec<PathBuf>,
}

fn patch_file_exists(s: &str) -> Result<PathBuf, String> {
	let path = PathBuf::from(s);

	if path.exists() {
		Ok(path)
	} else {
		Err(format!("Specified patch file, \"{s}\" cannot be found."))
	}
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verbosity {
	Normal,
	Verbose,
	VeryVerbose,
}
impl Verbosity {
	pub fn set(args: &Args) {
		VERBOSITY
			.set(if args.veryverbose {
				Verbosity::VeryVerbose
			} else if args.verbose {
				Verbosity::Verbose
			} else {
				Verbosity::Normal
			})
			.unwrap();
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
		self,
		verbosity: impl Into<Option<Verbosity>>,
	) -> Result<CaptureData> {
		let verbosity = verbosity.into().unwrap_or_else(Verbosity::get);
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
pub(crate) fn chmod<P: AsRef<Path>>(path: P, mode: u32) -> std::io::Result<()> {
	fn _chmod(path: &Path, mode: u32) -> std::io::Result<()> {
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
	std::fs::create_dir(&work_dir).wrap_err_with(|| format!("unable to mkdir {work_dir}"))?;

	// If the parent directory is suid/guid, mkdir will make the root
	// directory of the package inherit those bits. That is a bad thing,
	// so explicitly force perms to 755.

	chmod(&work_dir, 0o755)?;
	Ok(PathBuf::from(work_dir))
}

pub(crate) fn fetch_email_address() -> Result<String> {
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
	Ok(format!("{}@{mailname}", whoami::username()))
}
