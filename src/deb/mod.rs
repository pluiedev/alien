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
	let version = version
		.split_once(':')
		.map_or(version, |(_epoch, version)| version);

	info.version = version.to_owned();
	info.release = release.to_owned();
}

#[cfg(test)]
mod tests {
	#[test]
	fn test_set_version_and_release() {
		let mut info = crate::PackageInfo::default();

		super::set_version_and_release(&mut info, "1.0.0");
		assert_eq!(info.version, "1.0.0");
		assert_eq!(info.release, "1");

		// With revision
		super::set_version_and_release(&mut info, "1.0.0-2");
		assert_eq!(info.version, "1.0.0");
		assert_eq!(info.release, "2");

		// With epoch
		super::set_version_and_release(&mut info, "3:1.0.0-2");
		assert_eq!(info.version, "1.0.0");
		assert_eq!(info.release, "2");
	}
}
