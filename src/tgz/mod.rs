use crate::util::ExecExt;
use eyre::{bail, Context, Result};
use std::path::Path;
use subprocess::Exec;

pub mod source;
pub mod target;

/// Install a tgz with installpkg. Pass in the filename of the tgz to install.
///
/// installpkg (a slackware program) is used because I'm not sanguine about
/// just untarring a tgz file — it might trash a system.
pub fn install(tgz: &Path) -> Result<()> {
	if Path::new("/sbin/installpkg").exists() {
		Exec::cmd("/sbin/installpkg")
			.arg(tgz)
			.log_and_spawn(None)
			.wrap_err("Unable to install")
	} else {
		bail!("Sorry, I cannot install the generated .tgz file because /sbin/installpkg is not present. You can use tar to install it yourself.")
	}
}
