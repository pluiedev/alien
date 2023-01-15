use std::{
	os::unix::prelude::PermissionsExt,
	path::{Path, PathBuf},
};

use simple_eyre::eyre::{Context, Result};

use super::PackageInfo;

#[cfg(unix)]
pub fn chmod<P: AsRef<Path>>(path: P, mode: u32) -> std::io::Result<()> {
	_chmod(path.as_ref(), mode)
}
fn _chmod(path: &Path, mode: u32) -> std::io::Result<()> {
	let mut perms = std::fs::metadata(&path)?.permissions();
	perms.set_mode(mode);
	std::fs::set_permissions(&path, perms)?;
	Ok(())
}

#[cfg(not(unix))]
pub fn chmod(_path: &Path, _mode: u32) -> std::io::Result<()> {
	// do nothing :p
}

pub fn make_unpack_work_dir(info: &PackageInfo) -> Result<PathBuf> {
	let work_dir = format!("{}-{}", info.name, info.version);
	std::fs::create_dir(&work_dir).wrap_err_with(|| format!("unable to mkdir {work_dir}"))?;

	// If the parent directory is suid/guid, mkdir will make the root
	// directory of the package inherit those bits. That is a bad thing,
	// so explicitly force perms to 755.

	chmod(&work_dir, 0o755)?;
	Ok(PathBuf::from(work_dir))
}
