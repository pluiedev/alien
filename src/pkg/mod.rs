pub use source::PkgSource;
pub use target::PkgTarget;

use crate::util::{ExecExt, Verbosity};
use eyre::{bail, Context, Result};
use std::path::Path;
use subprocess::Exec;

pub mod source;
pub mod target;

/// Install a pkg with pkgadd. Pass in the filename of the pkg to install.
pub fn install(pkg: &Path) -> Result<()> {
	if Path::new("/usr/sbin/pkgadd").exists() {
		Exec::cmd("/usr/sbin/pkgadd")
			.arg("-d")
			.arg(".")
			.arg(pkg)
			.log_and_spawn(Verbosity::VeryVerbose)
			.wrap_err("Unable to install")
	} else {
		bail!("Sorry, I cannot install the generated .pkg file because /usr/sbin/pkgadd is not present.")
	}
}
