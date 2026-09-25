mod report;
mod safe_read;
mod scan;

pub use report::{Attribution, Coverage, Diagnostic, Finding, Integrity, ScanReport};
pub use scan::scan;

#[cfg(test)]
pub(crate) use scan::scan_with_inventory;

#[cfg(test)]
mod tests;
