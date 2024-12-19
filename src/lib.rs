#![forbid(unsafe_code)]
#![warn(rust_2018_idioms, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
	clippy::module_name_repetitions,
	clippy::missing_errors_doc,
	clippy::missing_panics_doc,
	clippy::redundant_closure_for_method_calls,
	clippy::struct_excessive_bools
)]

use std::{
	collections::HashMap,
	fmt::Display,
	path::{Path, PathBuf},
};

use enum_dispatch::enum_dispatch;
use eyre::{bail, Result};
use pkg::{PkgSource, PkgTarget};
use util::Args;

use deb::{DebSource, DebTarget};
use lsb::{LsbSource, LsbTarget};
use rpm::{RpmSource, RpmTarget};
use tgz::{TgzSource, TgzTarget};

pub mod deb;
pub mod lsb;
pub mod pkg;
pub mod rpm;
pub mod tgz;
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
}

#[enum_dispatch(SourcePackage)]
#[derive(Debug)]
pub enum AnySourcePackage {
	Lsb(LsbSource),
	Rpm(RpmSource),
	Deb(DebSource),
	Tgz(TgzSource),
	Pkg(PkgSource),
}
impl AnySourcePackage {
	pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
		if LsbSource::check_file(&file) {
			LsbSource::new(file, args).map(Self::Lsb)
		} else if RpmSource::check_file(&file) {
			RpmSource::new(file, args).map(Self::Rpm)
		} else if DebSource::check_file(&file) {
			DebSource::new(file, args).map(Self::Deb)
		} else if TgzSource::check_file(&file) {
			TgzSource::new(file).map(Self::Tgz)
		} else if PkgSource::check_file(&file) {
			PkgSource::new(file).map(Self::Pkg)
		} else {
			bail!("Unknown type of package, {}", file.display());
		}
	}
}

#[enum_dispatch(TargetPackage)]
#[derive(Debug)]
pub enum AnyTargetPackage {
	Lsb(LsbTarget),
	Rpm(RpmTarget),
	Deb(DebTarget),
	Tgz(TgzTarget),
	Pkg(PkgTarget),
}
impl AnyTargetPackage {
	pub fn new(
		format: Format,
		info: PackageInfo,
		unpacked_dir: PathBuf,
		args: &Args,
	) -> Result<Self> {
		let target = match format {
			Format::Lsb => Self::Lsb(LsbTarget::new(info, unpacked_dir)?),
			Format::Rpm => Self::Rpm(RpmTarget::new(info, unpacked_dir)?),
			Format::Deb => Self::Deb(DebTarget::new(info, unpacked_dir, args)?),
			Format::Tgz => Self::Tgz(TgzTarget::new(info, unpacked_dir)?),
			Format::Pkg => Self::Pkg(PkgTarget::new(info, unpacked_dir)?),
		};
		Ok(target)
	}
}

/// Extracted information about a package.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
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
	/// because the owners or groups just don't exist yet — so `xenomorph` has to
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
/// different package managers. Here's a table linking all of them together:
///
/// | `xenomorph` name          | Debian-style name | RPM scriptlet name | RPM query key | `tgz` script name | `pkg` script name |
/// |---------------------------|-------------------|--------------------|---------------|-------------------|-------------------|
/// | [`Self::BeforeInstall`]   | `preinst`         | `%pre`             | `%{PREIN}`    | `predoinst.sh`    | `preinstall`      |
/// | [`Self::AfterInstall`]    | `postinst`        | `%post`            | `%{POSTIN}`   | `doinst.sh`       | `postinstall`     |
/// | [`Self::BeforeUninstall`] | `prerm`           | `%preun`           | `%{PREUN}`    | `predelete.sh`    | `preremove`       |
/// | [`Self::AfterInstall`]    | `postrm`          | `%postun`          | `%{POSTUN}`   | `delete.sh`       | `postremove`      |
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
		Self::BeforeUninstall,
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
	/// Gets a script from its `tgz`-style script name.
	///
	/// See the [type-level documentation](Self) for the mapping between
	/// `tgz`-style script names and [`Script`] variants.
	#[must_use]
	pub fn from_tgz_script_name(s: &str) -> Option<Self> {
		match s {
			"predoinst.sh" => Some(Self::BeforeInstall),
			"doinst.sh" => Some(Self::AfterInstall),
			"predelete.sh" => Some(Self::BeforeUninstall),
			"delete.sh" => Some(Self::AfterUninstall),
			_ => None,
		}
	}
	/// Returns the script's `tgz`-style script name.
	///
	/// See the [type-level documentation](Self) for the mapping between
	/// `tgz`-style names and [`Script`] variants.
	#[must_use]
	pub fn tgz_script_name(&self) -> &str {
		match self {
			Self::BeforeInstall => "predoinst.sh",
			Self::AfterInstall => "doinst.sh",
			Self::BeforeUninstall => "predelete.sh",
			Self::AfterUninstall => "delete.sh",
		}
	}
	/// Gets a script from its `pkg`-style script name.
	///
	/// See the [type-level documentation](Self) for the mapping between
	/// `pkg`-style script names and [`Script`] variants.
	#[must_use]
	pub fn from_pkg_script_name(s: &str) -> Option<Self> {
		match s {
			"preinstall" => Some(Self::BeforeInstall),
			"postinstall" => Some(Self::AfterInstall),
			"preremove" => Some(Self::BeforeUninstall),
			"postremove" => Some(Self::AfterUninstall),
			_ => None,
		}
	}
	/// Returns the script's `pkg`-style script name.
	///
	/// See the [type-level documentation](Self) for the mapping between
	/// `pkg`-style script names and [`Script`] variants.
	#[must_use]
	pub fn pkg_script_name(&self) -> &str {
		match self {
			Self::BeforeInstall => "preinstall",
			Self::AfterInstall => "postinstall",
			Self::BeforeUninstall => "preremove",
			Self::AfterUninstall => "postremove",
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
	/// The `.tgz` format, used by Slackware.
	Tgz,
}
impl Format {
	pub fn install(self, path: &Path) -> Result<()> {
		match self {
			Format::Deb => deb::install(path),
			Format::Lsb | Format::Rpm => rpm::install(path),
			Format::Pkg => pkg::install(path),
			Format::Tgz => tgz::install(path),
		}
	}
}
impl Display for Format {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Format::Deb => "deb",
			Format::Lsb => "lsb",
			Format::Pkg => "pkg",
			Format::Rpm => "rpm",
			Format::Tgz => "tgz",
		})
	}
}
