use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use thiserror::Error;

use super::model::{RuleSet, validate_rule_set};

const MAX_RULE_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Error)]
pub enum RuleLoadError {
    #[error("rules directory does not exist: {0}")]
    MissingDirectory(PathBuf),
    #[error("cannot inspect {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("rules path is not a regular directory or file: {0}")]
    InvalidFileType(PathBuf),
    #[error("rules path must not be a symbolic link: {0}")]
    Symlink(PathBuf),
    #[error("rules file is larger than {MAX_RULE_FILE_BYTES} bytes: {0}")]
    TooLarge(PathBuf),
    #[error("cannot parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid rules file {path}: {reason}")]
    InvalidRules { path: PathBuf, reason: String },
    #[error("duplicate product id {product_id:?} in {path}")]
    DuplicateProduct { product_id: String, path: PathBuf },
    #[error("no rules.json files found under {0}")]
    NoRules(PathBuf),
}

pub fn load_rules(directory: &Path) -> Result<Vec<RuleSet>, RuleLoadError> {
    let root_meta = fs::symlink_metadata(directory).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            RuleLoadError::MissingDirectory(directory.to_owned())
        } else {
            RuleLoadError::Io {
                path: directory.to_owned(),
                source,
            }
        }
    })?;
    if root_meta.file_type().is_symlink() {
        return Err(RuleLoadError::Symlink(directory.to_owned()));
    }
    if !root_meta.is_dir() {
        return Err(RuleLoadError::InvalidFileType(directory.to_owned()));
    }

    let entries = fs::read_dir(directory).map_err(|source| RuleLoadError::Io {
        path: directory.to_owned(),
        source,
    })?;
    let mut product_directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| RuleLoadError::Io {
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| RuleLoadError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(RuleLoadError::Symlink(path));
        }
        if metadata.is_dir() {
            product_directories.push(path);
        }
    }
    product_directories.sort();

    let mut rulesets = Vec::with_capacity(product_directories.len());
    let mut products = std::collections::BTreeSet::new();
    for product_directory in product_directories {
        let path = product_directory.join("rules.json");
        let Some(file) = open_rule_file(&path)? else {
            continue;
        };
        let mut bytes = Vec::new();
        file.take(MAX_RULE_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| RuleLoadError::Io {
                path: path.clone(),
                source,
            })?;
        if bytes.len() as u64 > MAX_RULE_FILE_BYTES {
            return Err(RuleLoadError::TooLarge(path));
        }
        let ruleset: RuleSet =
            serde_json::from_slice(&bytes).map_err(|source| RuleLoadError::Parse {
                path: path.clone(),
                source,
            })?;
        validate_rule_set(&ruleset).map_err(|reason| RuleLoadError::InvalidRules {
            path: path.clone(),
            reason,
        })?;
        if !products.insert(ruleset.product.id.clone()) {
            return Err(RuleLoadError::DuplicateProduct {
                product_id: ruleset.product.id,
                path,
            });
        }
        rulesets.push(ruleset);
    }
    if rulesets.is_empty() {
        return Err(RuleLoadError::NoRules(directory.to_owned()));
    }
    rulesets.sort_by(|left, right| left.product.id.cmp(&right.product.id));
    Ok(rulesets)
}

pub fn load_bundled_rules() -> Result<Vec<RuleSet>, RuleLoadError> {
    const BUNDLED: [(&str, &str); 3] = [
        ("muxy", include_str!("../../rules/muxy/rules.json")),
        ("paseo", include_str!("../../rules/paseo/rules.json")),
        ("superset", include_str!("../../rules/superset/rules.json")),
    ];

    let mut rulesets = Vec::with_capacity(BUNDLED.len());
    let mut products = std::collections::BTreeSet::new();
    for (bundle_id, contents) in BUNDLED {
        let path = PathBuf::from(format!("<bundled>/{bundle_id}/rules.json"));
        let ruleset: RuleSet =
            serde_json::from_str(contents).map_err(|source| RuleLoadError::Parse {
                path: path.clone(),
                source,
            })?;
        validate_rule_set(&ruleset).map_err(|reason| RuleLoadError::InvalidRules {
            path: path.clone(),
            reason,
        })?;
        if !products.insert(ruleset.product.id.clone()) {
            return Err(RuleLoadError::DuplicateProduct {
                product_id: ruleset.product.id,
                path,
            });
        }
        rulesets.push(ruleset);
    }
    rulesets.sort_by(|left, right| left.product.id.cmp(&right.product.id));
    Ok(rulesets)
}

fn open_rule_file(path: &Path) -> Result<Option<File>, RuleLoadError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(RuleLoadError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(RuleLoadError::Symlink(path.to_owned()));
    }
    if !metadata.is_file() {
        return Err(RuleLoadError::InvalidFileType(path.to_owned()));
    }
    if metadata.len() > MAX_RULE_FILE_BYTES {
        return Err(RuleLoadError::TooLarge(path.to_owned()));
    }
    File::open(path)
        .map(Some)
        .map_err(|source| RuleLoadError::Io {
            path: path.to_owned(),
            source,
        })
}
