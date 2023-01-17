#![forbid(unsafe_code)]
#![warn(rust_2018_idioms, clippy::pedantic)]
#![allow(
	clippy::redundant_closure_for_method_calls,
	clippy::module_name_repetitions
)]

use std::path::PathBuf;

use alien::{
	deb::{DebSource, DebTarget},
	lsb::{LsbSource, LsbTarget},
	rpm::{RpmSource, RpmTarget},
	util::{Args, Verbosity},
	Format, PackageInfo, SourcePackage, TargetPackage,
};
use clap::Parser;
use enum_dispatch::enum_dispatch;
use simple_eyre::{eyre::bail, Result};

fn main() -> Result<()> {
	simple_eyre::install()?;

	let args = Args::parse();

	// TODO: find a way to do this natively in `clap`
	let formats = Format::new(&args);

	Verbosity::set(&args);

	if (args.generate || args.install) && formats.exactly_one().is_none() {
		bail!("--generate and --single may only be used when converting to a single format.");
	}

	// TODO: check targets, assume debian, and if generate and single are specified, disallow multiple targets.

	// Check if we're root.
	if !nix::unistd::geteuid().is_root() {
		if formats.contains(Format::Deb) && !args.generate && !args.single {
			bail!("Must run as root to convert to deb format (or you may use fakeroot).");
		}
		eprintln!("Warning: alien is not running as root!");
		eprintln!("Warning: Ownerships of files in the generated packages will probably be wrong.");
	}

	for file in &args.files {
		if !file.try_exists()? {
			bail!("File \"{}\" not found.", file.display());
		}
		let mut pkg = AnySourcePackage::new(file.clone(), &args)?;

		let scripts = &pkg.info().scripts;
		if !pkg.info().use_scripts && !scripts.is_empty() {
			if !args.scripts {
				eprint!(
					"Warning: Skipping conversion of scripts in package {}:",
					pkg.info().name,
				);
				for (k, v) in scripts {
					if !v.is_empty() {
						eprint!(" {}", k.deb_name());
					}
				}
				eprintln!(".");
				eprintln!("Warning: Use the --scripts parameter to include the scripts.");
			}
			pkg.info_mut().use_scripts = args.scripts;
		}

		if !args.keep_version {
			pkg.increment_release(args.bump);
		}

		let unpacked = pkg.unpack()?;
		let info = pkg.into_info();

		for format in formats {
			// Convert package
			if args.generate || info.original_format != format {
				let mut pkg = AnyTargetPackage::new(format, info.clone(), unpacked.clone(), &args)?;

				if args.generate {
					let tree = unpacked.display();
					if format == Format::Deb && !args.single {
						println!("Directories {tree} and {tree}.orig prepared.");
					} else {
						println!("Directory {tree} prepared.");
					}
					// Make sure `package` does not wipe out the
					// directory when it is destroyed.
					// unpacked.clear();
					continue;
				}

				let new_file = pkg.build()?;
				if args.test {
					let results = pkg.test(&new_file)?;
					if !results.is_empty() {
						println!("Test results:");
						for result in results {
							println!("\t{result}");
						}
					}
				}
				if args.install {
					// pkg.install(&new_file)?;
					std::fs::remove_file(&new_file)?;
				} else {
					// Tell them where the package ended up.
					println!("{} generated", new_file.display());
				}

				pkg.clean_tree()?;
			} else if args.install {
				// Don't convert the package, but do install it.
				// pkg.install(file)?;
				// Note I don't remove it. I figure that might annoy
				// people, since it was an input file.
			}
		}
	}

	Ok(())
}

#[enum_dispatch(SourcePackage)]
pub enum AnySourcePackage {
	Lsb(LsbSource),
	Rpm(RpmSource),
	Deb(DebSource),
}
impl AnySourcePackage {
	pub fn new(file: PathBuf, args: &Args) -> Result<Self> {
		// lsb > rpm > deb > tgz > slp > pkg

		if LsbSource::check_file(&file) {
			LsbSource::new(file, args).map(Self::Lsb)
		} else if RpmSource::check_file(&file) {
			RpmSource::new(file, args).map(Self::Rpm)
		} else if DebSource::check_file(&file) {
			DebSource::new(file, args).map(Self::Deb)
		} else {
			bail!("Unknown type of package, {}", file.display());
		}
	}
}

#[enum_dispatch(TargetPackage)]
pub enum AnyTargetPackage {
	Lsb(LsbTarget),
	Rpm(RpmTarget),
	Deb(DebTarget),
}
impl AnyTargetPackage {
	pub fn new(
		format: Format,
		info: PackageInfo,
		unpacked_dir: PathBuf,
		args: &Args,
	) -> Result<Self> {
		let target = match format {
			Format::Deb => Self::Deb(DebTarget::new(info, unpacked_dir, args)?),
			Format::Lsb => Self::Lsb(LsbTarget::new(info, unpacked_dir)?),
			Format::Pkg => todo!(),
			Format::Rpm => Self::Rpm(RpmTarget::new(info, unpacked_dir)?),
			Format::Slp => todo!(),
			Format::Tgz => todo!(),
		};
		Ok(target)
	}
}
