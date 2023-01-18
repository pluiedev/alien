pub use source::DebSource;
pub use target::DebTarget;

use crate::util::{ExecExt, Verbosity};
use eyre::Result;
use std::path::Path;
use subprocess::Exec;

pub mod source;
pub mod target;

pub fn install(deb: &Path) -> Result<()> {
	Exec::cmd("dpkg")
		.args(&["--no-force-overwrite", "-i"])
		.arg(deb)
		.log_and_spawn(Verbosity::VeryVerbose)
}

fn set_version_and_release(info: &mut super::PackageInfo, version: &str) {
	let (version, release) = if let Some((version, release)) = version.split_once('-') {
		(version, release)
	} else {
		(version, "1")
	};

	// Ignore epochs.
	let version = version.split_once(':').map_or(version, |t| t.1);

	info.version = version.to_owned();
	info.release = release.to_owned();
}
