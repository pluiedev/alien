pub mod source;
pub mod target;

pub use source::RpmSource;
pub use target::RpmTarget;

use crate::util::{ExecExt, Verbosity};
use eyre::Result;
use std::path::Path;
use subprocess::Exec;

pub fn install(deb: &Path) -> Result<()> {
	let mut cmd = Exec::cmd("rpm").arg("-ivh");

	if let Ok(args) = std::env::var("RPMINSTALLOPT") {
		for arg in args.split(" ") {
			cmd = cmd.arg(arg);
		}
	}

	cmd.arg(deb).log_and_output(Verbosity::VeryVerbose)?;
	Ok(())
}
