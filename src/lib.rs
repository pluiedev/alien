#![forbid(unsafe_code)]
#![warn(rust_2018_idioms, clippy::pedantic)]
#![allow(
	clippy::redundant_closure_for_method_calls,
	clippy::module_name_repetitions,
	clippy::missing_errors_doc,
	clippy::missing_panics_doc
)]

use std::{
	collections::HashMap,
	fmt::Display,
	path::{Path, PathBuf},
};

use enum_dispatch::enum_dispatch;
use simple_eyre::eyre::{bail, Result};
use util::Args;

use deb::{DebSource, DebTarget};
use lsb::{LsbSource, LsbTarget};
use rpm::{RpmSource, RpmTarget};

pub mod deb;
pub mod lsb;
pub mod rpm;
pub mod util;

/// A source package that can be unpacked, queried and modified.
#[enum_dispatch]
pub trait SourcePackage {
	/// Gets an immutable reference to the package info.
	fn info(&self) -> &PackageInfo;

	/// Gets a mutable reference to the package info.
	fn info_mut(&mut self) -> &mut PackageInfo;

	/// Extracts the package info by value, consuming the package.
	fn into_info(self) -> PackageInfo;

	/// Unpacks the package into a temporary directory, whose path is then returned.
	fn unpack(&mut self) -> Result<PathBuf>;

	/// Increments the release field of the package by the specified bump value.
	///
	/// If the release field is not a valid number, then it is set to the bump value.
	fn increment_release(&mut self, bump: u32) {
		let release = &mut self.info_mut().release;

		*release = if let Ok(num) = release.parse::<u32>() {
			(num + bump).to_string()
		} else {
			// Perl's string-number addition thing is... cursed.
			// If a string doesn't parse to a number, then it is treated as 0.
			// So, we will just set the release to the bump here.
			bump.to_string()
		};
	}
}

/// A target package that can be built, tested and installed.
#[enum_dispatch]
pub trait TargetPackage {
	/// Cleans the unpacked directory of any side-effects caused by
	/// initialization and [building](Self::build).
	fn clean_tree(&mut self) -> Result<()> {
		Ok(())
	}

	/// Builds a package from the completed unpacked directory,
	/// which is then placed in the current directory.
	///
	/// Returns the path to the built package.
	fn build(&mut self) -> Result<PathBuf>;

	/// Tests the given package file, and returns the test results as a list of lines.
	#[allow(unused_variables)]
	fn test(&mut self, package: &Path) -> Result<Vec<String>> {
		Ok(vec![])
	}

	/// Installs the given package file.
	fn install(&mut self, package: &Path) -> Result<()>;
}

#[enum_dispatch(SourcePackage)]
pub enum AnySourcePackage {
	Lsb(LsbSource),
	Rpm(RpmSource),
	Deb(DebSource),
}
impl AnySourcePackage {
	pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
		// lsb > rpm > deb > tgz > slp > pkg

		if LsbSource::check_file(&file) {
			LsbSource::new(file, args).map(Self::Lsb)
		} else if RpmSource::check_file(&file) {
			RpmSource::new(file, args).map(Self::Rpm)
		} else if DebSource::check_file(&file) {
			DebSource::new(file, args).map(Self::Deb)
		} else {
			bail!("Unknown type of package, {}", file.display());
		}
	}
}

#[enum_dispatch(TargetPackage)]
pub enum AnyTargetPackage {
	Lsb(LsbTarget),
	Rpm(RpmTarget),
	Deb(DebTarget),
}
impl AnyTargetPackage {
	pub fn new(
		format: Format,
		info: PackageInfo,
		unpacked_dir: PathBuf,
		args: &Args,
	) -> Result<Self> {
		let target = match format {
			Format::Deb => Self::Deb(DebTarget::new(info, unpacked_dir, args)?),
			Format::Lsb => Self::Lsb(LsbTarget::new(info, unpacked_dir)?),
			Format::Pkg => todo!(),
			Format::Rpm => Self::Rpm(RpmTarget::new(info, unpacked_dir)?),
			Format::Slp => todo!(),
			Format::Tgz => todo!(),
		};
		Ok(target)
	}
}

/// Extracted information about a package.
#[derive(Debug, Default, Clone)]
pub struct PackageInfo {
	/// The path to the package.
	pub file: PathBuf,

	/// The package's name.
	pub name: String,
	/// The package's upstream version.
	pub version: String,
	/// The package's distribution-specific release number.
	pub release: String,
	/// The package's architecture, in the format used by Debian.
	pub arch: String,
	/// The package's maintainer.
	pub maintainer: String,
	/// The package's dependencies.
	///
	/// Only dependencies that should exist on all target distributions
	/// can be put in here though, such as `lsb`.
	pub dependencies: Vec<String>,
	/// The section the package is in.
	pub group: String,
	/// A one-line description of the package.
	pub summary: String,
	/// A longer description of the package.
	///
	/// May contain multiple paragraphs.
	pub description: String,
	/// A short statement of copyright.
	pub copyright: String,
	/// The format the package was originally in.
	pub original_format: Format,
	/// The distribution family the package originated from.
	pub distribution: String,
	/// Whatever the package's package tool says when
	/// told to display info about the package.
	pub binary_info: String,
	/// A list of all conffiles in the package.
	pub conffiles: Vec<PathBuf>,
	/// A list of all files in the package.
	pub files: Vec<PathBuf>,
	/// The text of the changelog.
	pub changelog: String,

	/// When generating the package, only use the [`Self::scripts`] field
	/// if this is set to a true value.
	pub use_scripts: bool,
	/// A map of all [scripts](Script) in the package.
	pub scripts: HashMap<Script, String>,
	/// A map of file paths to ownership and mode information.
	///
	/// Some files cannot be represented on the filesystem — typically, that is
	/// because the owners or groups just don't exist yet — so `alien` has to
	/// store to preserve their ownership information (as well as mode information
	/// for `setuid` files) externally in this map.
	pub file_info: HashMap<PathBuf, FileInfo>,
}

/// Special information about files. See [`PackageInfo::file_info`] for more.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileInfo {
	/// The owner of the file.
	owner: String,
	/// The original mode of the file. Set for `setuid` files.
	mode: Option<u32>,
}

/// Scripts that may be run in the build process. See [`PackageInfo::scripts`] for more.
///
/// Due to historical reasons, there are many names for these scripts across
/// different package managers. Here's a table linking all of them:
///
/// | `alien` Name              | Debian-style name | RPM scriptlet name | RPM query key |
/// |---------------------------|-------------------|--------------------|---------------|
/// | [`Self::BeforeInstall`]   | `preinst`         | `%pre`             | `%{PREIN}`    |
/// | [`Self::AfterInstall`]    | `postinst`        | `%post`            | `%{POSTIN}`   |
/// | [`Self::BeforeUninstall`] | `prerm`           | `%preun`           | `%{PREUN}`    |
/// | [`Self::AfterInstall`]    | `postrm`          | `%postun`          | `%{POSTUN}`   |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Script {
	/// Script that will be run before install.
	BeforeInstall,
	/// Script that will be run after install.
	AfterInstall,
	/// Script that will be run before uninstall.
	BeforeUninstall,
	/// Script that will be run after uninstall.
	AfterUninstall,
}
impl Script {
	/// All recognized scripts.
	pub const ALL: [Script; 4] = [
		Self::BeforeInstall,
		Self::AfterInstall,
		Self::BeforeInstall,
		Self::AfterUninstall,
	];

	/// Gets a script from its Debian-style name.
	///
	/// See the [type-level documentation](Self) for the mapping between
	/// Debian-style names and [`Script`] variants.
	#[must_use]
	pub fn from_deb_name(s: &str) -> Option<Self> {
		match s {
			"preinst" => Some(Self::BeforeInstall),
			"postinst" => Some(Self::AfterInstall),
			"prerm" => Some(Self::BeforeUninstall),
			"postrm" => Some(Self::AfterUninstall),
			_ => None,
		}
	}

	/// Returns the script's Debian-style name.
	///
	/// See the [type-level documentation](Self) for the mapping between
	/// Debian-style names and [`Script`] variants.
	#[must_use]
	pub fn deb_name(&self) -> &str {
		match self {
			Self::BeforeInstall => "preinst",
			Self::AfterInstall => "postinst",
			Self::BeforeUninstall => "prerm",
			Self::AfterUninstall => "postrm",
		}
	}
	/// Returns the script's RPM query key.
	///
	/// See the [type-level documentation](Self) for the mapping between
	/// RPM query keys and [`Script`] variants.
	#[must_use]
	pub fn rpm_query_key(&self) -> &str {
		match self {
			Self::BeforeInstall => "%{PREIN}",
			Self::AfterInstall => "%{POSTIN}",
			Self::BeforeUninstall => "%{PREUN}",
			Self::AfterUninstall => "%{POSTUN}",
		}
	}
	/// Returns the script's RPM scriptlet name.
	///
	/// See the [type-level documentation](Self) for the mapping between
	/// RPM scriptlet names and [`Script`] variants.
	#[must_use]
	pub fn rpm_scriptlet_name(&self) -> &str {
		match self {
			Self::BeforeInstall => "%pre",
			Self::AfterInstall => "%post",
			Self::BeforeUninstall => "%preun",
			Self::AfterUninstall => "%postun",
		}
	}
}

/// Format of a package.
#[enumflags2::bitflags]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
	/// The `.deb` format, used by `dpkg` and default for Debian-
	/// and Ubuntu-derived distributions.
	#[default]
	Deb,
	/// The package format used by Linux Standard Base.
	/// Basically an [`rpm` file](Self::Rpm) with a `lsb-` prefix
	/// and a dependency on the `lsb` package.
	Lsb,
	/// The `.pkg` format, used by Solaris.
	Pkg,
	/// The `.rpm` format, used by the RPM package manager prevalent
	/// on many distributions derived from Red Hat Linux,
	/// including RHEL, CentOS, openSUSE, Fedora, and more.
	Rpm,
	/// The `.slp` format, used by Stampede Linux.
	Slp,
	/// The `.tgz` format, used by Slackware.
	Tgz,
}
impl Display for Format {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Format::Deb => "deb",
			Format::Lsb => "lsb",
			Format::Pkg => "pkg",
			Format::Rpm => "rpm",
			Format::Slp => "slp",
			Format::Tgz => "tgz",
		})
	}
}
