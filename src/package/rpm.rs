pub mod source;
pub mod target;

pub use source::RpmSource;
pub use target::RpmTarget;

// RPM style script names.
const RPM_SCRIPT_NAMES: &[&str] = &["pre", "post", "preun", "postun"];
const RPM_SCRIPT_NAMES_TEMPLATE: &[&str] = &["%{PREIN}", "%{POSTIN}", "%{PREUN}", "%{POSTUN}"];
