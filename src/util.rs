use std::fmt::Debug;

use once_cell::sync::OnceCell;
use simple_eyre::eyre::{bail, Result};
use subprocess::{CaptureData, Exec, NullFile};

use crate::Args;

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

pub trait ExecExt {
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
		let verbosity = verbosity.into().unwrap_or(Verbosity::get());
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
		let verbosity = verbosity.into().unwrap_or(Verbosity::get());
		let cmdline = self.to_cmdline_lossy();
		if verbosity != Verbosity::Normal {
			println!("\t{cmdline}");
		}
		let output = self.capture()?;

		if verbosity == Verbosity::VeryVerbose {
			let stdout = String::from_utf8_lossy(&output.stdout);
			println!("{stdout}")
		}
		Ok(output)
	}
}
