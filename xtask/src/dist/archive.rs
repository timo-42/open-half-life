//! `SHA256SUMS` generation and the final `.tar.gz`/`.zip` archive writers.

use std::fmt::Write as _;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// Lower-case hex encoding of `bytes`.
fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Every regular file under `root`, as a path relative to `root`, in a
/// stable (sorted) order and always using `/` as the separator so the
/// listing is reproducible across Windows and Unix.
fn relative_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else if path.is_file() {
                out.push(
                    path.strip_prefix(root)
                        .expect("walked path is under root")
                        .to_path_buf(),
                );
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

/// A relative path formatted with `/` separators, for the sums file and the
/// archives, regardless of host path convention.
fn portable_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Computes every file's SHA-256 digest and writes `SHA256SUMS` (in the
/// conventional `sha256sum`/`shasum -a 256 -c` format) at the root of
/// `dist_dir`, listing every other file already present there.
///
/// # Errors
/// Any [`io::Error`] hit while reading a file or writing `SHA256SUMS`.
pub fn write_sha256sums(dist_dir: &Path) -> io::Result<PathBuf> {
    let files = relative_files(dist_dir)?;
    let mut listing = String::new();
    for relative in &files {
        let mut hasher = Sha256::new();
        let mut file = std::fs::File::open(dist_dir.join(relative))?;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let digest = hasher.finalize();
        listing.push_str(&to_hex(&digest));
        listing.push_str("  ");
        listing.push_str(&portable_path(relative));
        listing.push('\n');
    }
    let sums_path = dist_dir.join("SHA256SUMS");
    std::fs::write(&sums_path, listing)?;
    Ok(sums_path)
}

/// Writes a gzip-compressed tar archive of `dist_dir` at `archive_path`,
/// with every entry prefixed by `dist_dir`'s own file name so extracting the
/// archive produces the same top-level folder.
///
/// Uses `flate2`'s pure-Rust `rust_backend` (`miniz_oxide`); no C zlib is
/// ever linked.
///
/// # Errors
/// Any [`io::Error`] hit while reading `dist_dir` or writing the archive.
pub fn create_tar_gz(dist_dir: &Path, archive_path: &Path) -> io::Result<()> {
    let folder_name = dist_dir
        .file_name()
        .ok_or_else(|| io::Error::other("dist directory has no file name"))?;
    let file = std::fs::File::create(archive_path)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
    let mut builder = tar::Builder::new(encoder);
    builder.append_dir_all(folder_name, dist_dir)?;
    let encoder = builder.into_inner()?;
    encoder.finish()?;
    Ok(())
}

/// Writes a zip archive (deflate method) of `dist_dir` at `archive_path`,
/// with every entry prefixed by `dist_dir`'s own file name.
///
/// # Errors
/// Any [`io::Error`] hit while reading `dist_dir` or writing the archive; a
/// zip-specific failure is mapped to [`io::Error`] via [`io::Error::other`].
pub fn create_zip(dist_dir: &Path, archive_path: &Path) -> io::Result<()> {
    let folder_name = dist_dir
        .file_name()
        .ok_or_else(|| io::Error::other("dist directory has no file name"))?
        .to_string_lossy()
        .into_owned();
    let file = std::fs::File::create(archive_path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for relative in relative_files(dist_dir)? {
        let entry_name = format!("{folder_name}/{}", portable_path(&relative));
        writer
            .start_file(entry_name, options)
            .map_err(io::Error::other)?;
        let mut source = std::fs::File::open(dist_dir.join(&relative))?;
        io::copy(&mut source, &mut writer)?;
    }
    writer.finish().map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{create_tar_gz, create_zip, portable_path, to_hex, write_sha256sums};
    use sha2::{Digest as _, Sha256};
    use std::io::Read as _;

    #[test]
    fn writes_one_correct_line_per_file_sorted_by_relative_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("b.txt"), b"second").expect("write");
        std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("sub").join("a.txt"), b"first").expect("write");

        let sums_path = write_sha256sums(dir.path()).expect("write sums");
        let contents = std::fs::read_to_string(&sums_path).expect("read sums");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);

        let mut second_hasher = Sha256::new();
        second_hasher.update(b"second");
        let second_hex = to_hex(&second_hasher.finalize());
        assert_eq!(lines[0], format!("{second_hex}  b.txt"));

        let mut first_hasher = Sha256::new();
        first_hasher.update(b"first");
        let first_hex = to_hex(&first_hasher.finalize());
        assert_eq!(lines[1], format!("{first_hex}  sub/a.txt"));
    }

    #[test]
    fn portable_path_always_uses_forward_slashes() {
        let path = std::path::Path::new("a").join("b").join("c.txt");
        assert_eq!(portable_path(&path), "a/b/c.txt");
    }

    fn fixture_dist_dir(root_name: &str) -> tempfile::TempDir {
        let parent = tempfile::tempdir().expect("tempdir");
        let dist_dir = parent.path().join(root_name);
        std::fs::create_dir_all(dist_dir.join("bin")).expect("mkdir");
        std::fs::write(dist_dir.join("bin").join("open-half-life"), b"binary").expect("write");
        std::fs::write(dist_dir.join("LICENSE"), b"license text").expect("write");
        parent
    }

    #[test]
    fn tar_gz_round_trips_the_dist_directory_under_its_own_folder_name() {
        let parent = fixture_dist_dir("open-half-life-0.1.0-x86_64-unknown-linux-gnu");
        let dist_dir = parent
            .path()
            .join("open-half-life-0.1.0-x86_64-unknown-linux-gnu");
        let archive_path = parent.path().join("out.tar.gz");

        create_tar_gz(&dist_dir, &archive_path).expect("create tar.gz");

        let file = std::fs::File::open(&archive_path).expect("open archive");
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        let mut names: Vec<String> = archive
            .entries()
            .expect("read entries")
            .map(|entry| {
                entry
                    .expect("read entry")
                    .path()
                    .expect("entry path")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert!(names.contains(
            &"open-half-life-0.1.0-x86_64-unknown-linux-gnu/bin/open-half-life".to_owned()
        ));
        assert!(
            names.contains(&"open-half-life-0.1.0-x86_64-unknown-linux-gnu/LICENSE".to_owned())
        );
    }

    #[test]
    fn zip_round_trips_the_dist_directory_under_its_own_folder_name() {
        let parent = fixture_dist_dir("open-half-life-0.1.0-x86_64-pc-windows-msvc");
        let dist_dir = parent
            .path()
            .join("open-half-life-0.1.0-x86_64-pc-windows-msvc");
        let archive_path = parent.path().join("out.zip");

        create_zip(&dist_dir, &archive_path).expect("create zip");

        let file = std::fs::File::open(&archive_path).expect("open archive");
        let mut archive = zip::ZipArchive::new(file).expect("read zip");
        let mut names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_owned())
            .collect();
        names.sort();
        assert!(names.contains(
            &"open-half-life-0.1.0-x86_64-pc-windows-msvc/bin/open-half-life".to_owned()
        ));

        let mut binary = archive
            .by_name("open-half-life-0.1.0-x86_64-pc-windows-msvc/bin/open-half-life")
            .expect("find binary entry");
        let mut contents = Vec::new();
        binary.read_to_end(&mut contents).expect("read entry");
        assert_eq!(contents, b"binary");
    }
}
