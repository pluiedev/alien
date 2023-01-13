mod package;
#[cfg(test)]
mod test;
mod util;

use std::path::PathBuf;

use clap::Parser;
use fs_extra::dir::CopyOptions;
use package::{Format, Package, PackageBehavior};
use simple_eyre::{eyre::bail, Result};
use util::Verbosity;

#[derive(clap::Parser, Debug)]
pub struct Args {
    /// Generate a Debian deb package (default).
    #[arg(short = 'd', long)]
    to_deb: bool,

    // deb-specific settings
    /// Specify patch file to use instead of automatically looking for patch
    /// in /var/lib/alien.
    #[arg(long, requires = "to_deb", value_parser = patch_file_exists)]
    patch: Option<PathBuf>,
    /// Do not use patches.
    #[arg(long, requires = "to_deb", conflicts_with = "patch")]
    nopatch: bool,
    /// Use even old version os patches.
    #[arg(long, requires = "to_deb")]
    anypatch: bool,
    /// Like --generate, but do not create .orig directory.
    #[arg(short, long, requires = "to_deb")]
    single: bool,
    /// Munge/fix permissions and owners.
    #[arg(long, requires = "to_deb")]
    fixperms: bool,
    /// Test generated packages with lintian.
    #[arg(long, requires = "to_deb")]
    test: bool,
    // end deb-specific settings
    /// Generate a Red Hat rpm package.
    #[arg(short = 'r', long)]
    to_rpm: bool,
    /// Generate a Stampede slp package.
    #[arg(long)]
    to_slp: bool,
    /// Generate a LSB package.
    #[arg(short = 'l', long)]
    to_lsb: bool,
    /// Generate a Slackware tgz package.
    #[arg(short = 't', long)]
    to_tgz: bool,

    // tgx-specific settings
    /// Specify package description.
    #[arg(long, requires = "to_tgz")]
    description: Option<String>,

    // /// Specify package version.
    // #[arg(long, requires = "to_tgz", require_equals = true)]
    // version: Option<String>,

    // end tgx-specific settings
    /// Generate a Solaris pkg package.
    #[arg(short = 'p', long)]
    to_pkg: bool,
    /// Install generated package.
    #[arg(short, long, conflicts_with_all = ["generate", "single"])]
    install: bool,
    /// Generate build tree, but do not build package.
    #[arg(short, long)]
    generate: bool,
    /// Include scripts in package.
    #[arg(short = 'c', long)]
    scripts: bool,
    /// Set architecture of the generated package.
    #[arg(long)]
    target: Option<String>,
    /// Display each command alien runs.
    #[arg(short, long)]
    verbose: bool,
    /// Be verbose, and also display output of run commands.
    #[arg(long)]
    veryverbose: bool,

    // TODO: veryverbose
    /// Do not change version of generated package.
    #[arg(short, long)]
    keep_version: bool,
    /// Increment package version by this number.
    #[arg(long, default_value_t = 1)]
    bump: u32,

    /// Package file or files to convert.
    #[arg(required = true)]
    files: Vec<PathBuf>,
}

fn patch_file_exists(s: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(s);

    if path.exists() {
        Ok(path)
    } else {
        Err(format!("Specified patch file, \"{s}\" cannot be found."))
    }
}

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
        let mut package = Package::new(file.clone(), &args)?;

        if let Some(arch) = &args.target {
            package.set_arch(arch);
        }

        // TODO: usescript

        if !args.keep_version {
            package.increment_release(args.bump);
        }

        for format in formats {
            if args.generate || package.info().original_format != format {
                // Only unpack once.
                let tree = match &package.info().unpacked_tree {
                    Some(u) => {
                        package.clean_tree();
                        u.clone()
                    }
                    None => package.unpack(),
                };

                // Make .orig.tar.gz directory?
                if format == Format::Deb && !args.single && !args.generate {
                    let option = CopyOptions {
                        overwrite: true,
                        ..Default::default()
                    };
                    fs_extra::dir::copy(&tree, tree.with_extension("orig"), &option)?;
                }

                package.prepare();

                if args.generate {
                    let tree = tree.display();
                    if format == Format::Deb && !args.single {
                        println!("Directories {tree} and {tree}.orig prepared.");
                    } else {
                        println!("Directory {tree} prepared.");
                    }
                    // Make sure `package` does not wipe out the
                    // directory when it is destroyed.
                    package.info_mut().unpacked_tree = None;
                    continue;
                }

                let new_file = package.build();
                if args.test {
                    let results = package.test(&new_file)?;
                    if !results.is_empty() {
                        println!("Test results:");
                        for result in results {
                            println!("\t{result}");
                        }
                    }
                }
                if args.install {
                    package.install(&new_file)?;
                    std::fs::remove_file(&new_file)?;
                } else {
                    // Tell them where the package ended up.
                    println!("{} generated", new_file.display());
                }
            } else if args.install {
                // Don't convert the package, but do install it.
                package.install(&file)?;
                // Note I don't remove it. I figure that might annoy
                // people, since it was an input file.
            }
            package.revert();
        }
    }

    Ok(())
}
