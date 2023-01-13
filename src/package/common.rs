use std::{os::unix::prelude::PermissionsExt, path::PathBuf};

use simple_eyre::eyre::{Context, Result};

use super::PackageInfo;

pub fn make_unpack_work_dir(info: &PackageInfo) -> Result<PathBuf> {
	let work_dir = format!("{}-{}", info.name, info.version);
	std::fs::create_dir(&work_dir).wrap_err_with(|| format!("unable to mkdir {work_dir}"))?;

	// If the parent directory is suid/guid, mkdir will make the root
	// directory of the package inherit those bits. That is a bad thing,
	// so explicitly force perms to 755.

	// TODO: make this portable - modes only exist on *nix
	let mut perms = std::fs::metadata(&work_dir)?.permissions();
	perms.set_mode(0o755);
	std::fs::set_permissions(&work_dir, perms)?;

	Ok(PathBuf::from(work_dir))
}
