use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    os::{unix::ffi::OsStrExt, unix::fs::MetadataExt},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};

use rustix::{
    fs::{self, AtFlags, Mode, OFlags},
    io::Errno,
};

const PRIVATE_DIR_MODE: Mode = Mode::from_raw_mode(0o700);
const PRIVATE_FILE_MODE: Mode = Mode::from_raw_mode(0o600);
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct ReadFile {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

pub(super) fn read_file(home: &Path, relative: &Path, max_bytes: u64) -> io::Result<ReadFile> {
    let (parent, name) = open_parent(home, relative)?;
    let fd = fs::openat(
        &parent,
        &name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(to_io)?;
    let file = File::from(fd);
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "target is not a regular file",
        ));
    }
    if metadata.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "target exceeds the supported size",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "target exceeds the supported size",
        ));
    }
    Ok(ReadFile {
        bytes,
        mode: metadata.mode() & 0o7777,
    })
}

pub(super) fn ensure_home(home: &Path) -> io::Result<()> {
    open_root(home).map(drop)
}

pub(super) fn create_backup(home: &Path, backup_relative: &Path) -> io::Result<()> {
    let root = open_root(home)?;
    let backup_parts = normal_components(backup_relative)?;
    let (backup_parent_parts, backup_name) = backup_parts.split_at(backup_parts.len() - 1);
    let parent = ensure_directories(&root, backup_parent_parts)?;
    match fs::mkdirat(&parent, backup_name[0].as_os_str(), PRIVATE_DIR_MODE).map_err(to_io) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "backup destination already exists",
            ));
        }
        Err(error) => return Err(error),
    }
    let backup_root = open_child_dir(&parent, backup_name[0].as_os_str())?;
    fs::fchmod(&backup_root, PRIVATE_DIR_MODE).map_err(to_io)?;
    Ok(())
}

pub(super) fn write_backup_files(
    home: &Path,
    backup_relative: &Path,
    files: &[BackupFile],
) -> io::Result<()> {
    let backup_root = open_directory_relative(home, backup_relative)?;
    let files_dir = ensure_child_dir(&backup_root, OsStr::new("files"))?;

    for file in files {
        write_backup_file(&files_dir, &file.relative, &file.bytes)?;
    }

    let mapping = backup_mapping(files);
    write_new_file(
        &backup_root,
        OsStr::new("backup-map.json"),
        mapping.as_bytes(),
    )?;
    Ok(())
}

#[derive(Debug)]
pub(super) struct BackupFile {
    pub relative: std::path::PathBuf,
    pub bytes: Vec<u8>,
    pub original_mode: u32,
    pub sha256: String,
}

pub(super) fn write_execution_receipt(
    home: &Path,
    backup_relative: &Path,
    bytes: &[u8],
    create: bool,
) -> io::Result<()> {
    let backup_root = open_directory_relative(home, backup_relative)?;
    let name = OsStr::new("execution-receipt.json");
    if create {
        write_new_file(&backup_root, name, bytes)
    } else {
        replace_file_at(&backup_root, name, bytes, 0o600)
    }
}

pub(super) fn replace_file(
    home: &Path,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
) -> io::Result<()> {
    let (parent, name) = open_parent(home, relative)?;
    replace_file_at(&parent, &name, bytes, mode)
}

pub(super) fn remove_file(home: &Path, relative: &Path) -> io::Result<()> {
    let (parent, name) = open_parent(home, relative)?;
    let fd = fs::openat(
        &parent,
        &name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(to_io)?;
    if !File::from(fd).metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "target is not a regular file",
        ));
    }
    fs::unlinkat(&parent, &name, AtFlags::empty()).map_err(to_io)?;
    let _ = parent.sync_all();
    Ok(())
}

fn open_root(home: &Path) -> io::Result<File> {
    let fd = fs::open(
        home,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(to_io)?;
    Ok(File::from(fd))
}

fn open_parent(home: &Path, relative: &Path) -> io::Result<(File, OsString)> {
    let components = normal_components(relative)?;
    let (name, parents) = components.split_last().ok_or_else(invalid_path)?;
    let root = open_root(home)?;
    let parent = open_directories(&root, parents)?;
    Ok((parent, name.as_os_str().to_owned()))
}

fn open_directory_relative(home: &Path, relative: &Path) -> io::Result<File> {
    let components = normal_components(relative)?;
    let root = open_root(home)?;
    open_directories(&root, &components)
}

fn replace_file_at(parent: &File, name: &OsStr, bytes: &[u8], mode: u32) -> io::Result<()> {
    let temp_name = OsString::from(format!(
        ".akcleaner-{}-{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let fd = fs::openat(
        parent,
        &temp_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        PRIVATE_FILE_MODE,
    )
    .map_err(to_io)?;
    let mut file = File::from(fd);
    let result = (|| {
        file.write_all(bytes)?;
        fs::fchmod(&file, Mode::from_raw_mode(mode as _)).map_err(to_io)?;
        file.sync_all()?;
        fs::renameat(parent, &temp_name, parent, name).map_err(to_io)?;
        let _ = parent.sync_all();
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::unlinkat(parent, &temp_name, AtFlags::empty());
    }
    result
}

fn open_directories(root: &File, components: &[Component<'_>]) -> io::Result<File> {
    let mut current = root.try_clone()?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(invalid_path());
        };
        current = open_child_dir(&current, name)?;
    }
    Ok(current)
}

fn ensure_directories(root: &File, components: &[Component<'_>]) -> io::Result<File> {
    let mut current = root.try_clone()?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(invalid_path());
        };
        current = ensure_child_dir(&current, name)?;
    }
    Ok(current)
}

fn open_child_dir(parent: &File, name: &OsStr) -> io::Result<File> {
    let fd = fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(to_io)?;
    Ok(File::from(fd))
}

fn ensure_child_dir(parent: &File, name: &OsStr) -> io::Result<File> {
    let created = match fs::mkdirat(parent, name, PRIVATE_DIR_MODE).map_err(to_io) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error),
    };
    let directory = open_child_dir(parent, name)?;
    if created {
        fs::fchmod(&directory, PRIVATE_DIR_MODE).map_err(to_io)?;
    }
    Ok(directory)
}

fn write_backup_file(root: &File, relative: &Path, bytes: &[u8]) -> io::Result<()> {
    let components = normal_components(relative)?;
    let (name, parents) = components.split_last().ok_or_else(invalid_path)?;
    let parent = ensure_directories(root, parents)?;
    write_new_file(&parent, name.as_os_str(), bytes)
}

fn write_new_file(parent: &File, name: &OsStr, bytes: &[u8]) -> io::Result<()> {
    let fd = fs::openat(
        parent,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        PRIVATE_FILE_MODE,
    )
    .map_err(to_io)?;
    let mut file = File::from(fd);
    file.write_all(bytes)?;
    fs::fchmod(&file, PRIVATE_FILE_MODE).map_err(to_io)?;
    file.sync_all()?;
    Ok(())
}

fn backup_mapping(files: &[BackupFile]) -> String {
    let mapped = files
        .iter()
        .map(|source| {
            let path = source.relative.to_string_lossy();
            let source_bytes = source.relative.as_os_str().as_bytes();
            let mut backup_bytes = b"files/".to_vec();
            backup_bytes.extend_from_slice(source_bytes);
            serde_json::json!({
                "source": path,
                "backup": format!("files/{}", path),
                "source_path_bytes_hex": hex_lower(source_bytes),
                "backup_path_bytes_hex": hex_lower(&backup_bytes),
                "original_mode": source.original_mode,
                "sha256": source.sha256,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&serde_json::json!({"version": 1, "files": mapped}))
        .expect("serializing backup mapping into a string cannot fail")
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn normal_components(path: &Path) -> io::Result<Vec<Component<'_>>> {
    let components = path.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_path());
    }
    Ok(components)
}

fn invalid_path() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "path is not home-relative")
}

fn to_io(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}
