pub use source::RpmSource;
pub use target::RpmTarget;

use crate::util::{ExecExt, Verbosity};
use eyre::Result;
use std::path::Path;
use subprocess::Exec;

pub mod source;
pub mod target;

pub fn install(rpm: &Path) -> Result<()> {
	let mut cmd = Exec::cmd("rpm").arg("-ivh");

	if let Ok(args) = std::env::var("RPMINSTALLOPT") {
		for arg in args.split(' ') {
			cmd = cmd.arg(arg);
		}
	}

	cmd.arg(rpm).log_and_spawn(Verbosity::VeryVerbose)
}
