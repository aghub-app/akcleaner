use std::{
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

pub(super) const MAX_MANIFEST_BYTES: u64 = 5 * 1024 * 1024;
pub(super) const MAX_MANAGED_FILE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SafeReadError {
    Symlink,
    Missing,
    NotDirectory,
    NotRegularFile,
    TooLarge,
    InvalidPath,
    Io,
}

pub(super) fn verify_directory_under(
    approved_home: &Path,
    root: &Path,
) -> Result<(), SafeReadError> {
    let relative_root = root
        .strip_prefix(approved_home)
        .map_err(|_| SafeReadError::InvalidPath)?;
    let mut current = approved_home.to_owned();
    for component in relative_root.components() {
        let Component::Normal(part) = component else {
            return Err(SafeReadError::InvalidPath);
        };
        current.push(part);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SafeReadError::Missing
            } else {
                SafeReadError::Io
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(SafeReadError::Symlink);
        }
        if !metadata.is_dir() {
            return Err(SafeReadError::NotDirectory);
        }
    }
    Ok(())
}

pub(super) fn read_under(
    approved_home: &Path,
    root: &Path,
    relative: &str,
    max_bytes: u64,
) -> Result<Option<(PathBuf, Vec<u8>)>, SafeReadError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || relative
            .replace('\\', "/")
            .split('/')
            .any(|part| part == "..")
        || relative.starts_with(['/', '\\'])
        || (relative.as_bytes().get(1) == Some(&b':')
            && relative.as_bytes()[0].is_ascii_alphabetic())
    {
        return Err(SafeReadError::InvalidPath);
    }
    verify_directory_under(approved_home, root)?;

    let components: Vec<_> = relative_path.components().collect();
    if components.is_empty() {
        return Err(SafeReadError::InvalidPath);
    }
    let mut path = root.to_owned();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(part) = component else {
            return Err(SafeReadError::InvalidPath);
        };
        path.push(part);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(_) => return Err(SafeReadError::Io),
        };
        if metadata.file_type().is_symlink() {
            return Err(SafeReadError::Symlink);
        }
        let is_last = index + 1 == components.len();
        if is_last {
            if !metadata.is_file() {
                return Err(SafeReadError::NotRegularFile);
            }
            if metadata.len() > max_bytes {
                return Err(SafeReadError::TooLarge);
            }
        } else if !metadata.is_dir() {
            return Err(SafeReadError::NotRegularFile);
        }
    }

    let file = File::open(&path).map_err(|_| SafeReadError::Io)?;
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SafeReadError::Io)?;
    if bytes.len() as u64 > max_bytes {
        return Err(SafeReadError::TooLarge);
    }
    Ok(Some((path, bytes)))
}
