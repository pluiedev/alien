pub mod source;
pub mod target;

pub use source::DebSource;
pub use target::DebTarget;

use super::PackageInfo;

fn set_version_and_release(info: &mut PackageInfo, version: &str) {
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