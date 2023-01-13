use std::{
    fmt::Debug,
    process::{Command, Output, Stdio},
};

use once_cell::sync::OnceCell;
use simple_eyre::eyre::{Context, Result};

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

pub trait CommandExt {
    fn log_and_spawn(&mut self, verbosity: impl Into<Option<Verbosity>>) -> Result<()>;
    fn log_and_output(&mut self, verbosity: impl Into<Option<Verbosity>>) -> Result<Output>;
}
impl CommandExt for Command {
    fn log_and_spawn(&mut self, verbosity: impl Into<Option<Verbosity>>) -> Result<()> {
        let verbosity = verbosity.into().unwrap_or(Verbosity::get());
        if verbosity != Verbosity::Normal {
            println!("\t{self:?}");
        }
        if verbosity != Verbosity::VeryVerbose {
            self.stdout(Stdio::null());
        }
        self.spawn()
            .wrap_err_with(|| format!("Error executing \"{self:?}\""))?;
        Ok(())
    }

    fn log_and_output(&mut self, verbosity: impl Into<Option<Verbosity>>) -> Result<Output> {
        let verbosity = verbosity.into().unwrap_or(Verbosity::get());
        if verbosity != Verbosity::Normal {
            println!("\t{self:?}");
        }
        let output = self
            .output()
            .wrap_err_with(|| format!("Error executing \"{self:?}\""))?;

        if verbosity == Verbosity::VeryVerbose {
            let stdout = String::from_utf8_lossy(&output.stdout);
            println!("{stdout}")
        }
        Ok(output)
    }
}
#[macro_export]
#[doc(hidden)]
macro_rules! args {
    [$($arg:expr),+] => {
        &[$(::std::ffi::OsStr::new($arg)),+]
    };
}
