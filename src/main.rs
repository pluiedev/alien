#![forbid(unsafe_code)]
#![warn(rust_2018_idioms, clippy::pedantic)]

use alien::{
	util::{args, Verbosity},
	AnySourcePackage, AnyTargetPackage, Format, SourcePackage, TargetPackage,
};

use bpaf::Parser;
use eyre::{bail, Result};

#[cfg(debug_assertions)]
fn eyre() -> Result<()> {
	color_eyre::install()
}
#[cfg(not(debug_assertions))]
fn eyre() -> Result<()> {
	simple_eyre::install()
}

fn main() -> Result<()> {
	eyre()?;

	let args = args()
		.guard(
			|a| !(a.install && (a.generate || a.deb_args.single)),
			"You cannot use --generate or --single with --install.",
		)
		.guard(
			|a| !(a.formats.exactly_one().is_none() && (a.generate || a.deb_args.single)),
			"--generate and --single may only be used when converting to a single format.",
		)
		.guard(
			|a| !(a.deb_args.nopatch && a.deb_args.patch.is_some()),
			"The options --nopatch and --patchfile cannot be used together.",
		)
		.to_options()
		.run();

	Verbosity::set(args.verbosity);

	// Check alien's working environment.
	if std::fs::write("test", "test").is_err() {
		bail!("Cannot write to current directory. Try moving to /tmp and re-running alien.");
	}

	// Check if we're root.
	if !nix::unistd::geteuid().is_root() {
		if args.formats.contains(Format::Deb) && !args.generate && !args.deb_args.single {
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

		for format in args.formats {
			// Convert package
			if args.generate || info.original_format != format {
				let mut pkg = AnyTargetPackage::new(format, info.clone(), unpacked.clone(), &args)?;

				if args.generate {
					let tree = unpacked.display();
					if format == Format::Deb && !args.deb_args.single {
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
				if args.deb_args.test {
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
