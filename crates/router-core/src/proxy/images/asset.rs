use std::{
    fs::{self, File},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    os::{
        fd::OwnedFd,
        unix::fs::{DirBuilderExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

use axum::body::Bytes;
use base64::{
    Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurposeConfig, general_purpose::GeneralPurpose},
};
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(super) const MCP_JSON_RESPONSE_LIMIT: usize = 65 * 1024 * 1024;
pub(super) const MAX_BASE64_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_COMPRESSED_PNG_BYTES: usize = 48 * 1024 * 1024;
pub(super) const PNG_DECODER_INTERNAL_LIMIT: usize = 16 * 1024 * 1024;
pub(super) const MAX_DECODED_FRAME_BYTES: usize = 66_355_200;
pub(super) const MCP_SINGLE_CALL_PEAK_BYTES: usize = 160 * 1024 * 1024;
pub(super) const READ_BACK_CHUNK_BYTES: usize = 64 * 1024;

const STRICT_STANDARD: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_encode_padding(true)
        .with_decode_padding_mode(DecodePaddingMode::RequireCanonical)
        .with_decode_allow_trailing_bits(false),
);
const ROOT_MODE: Mode = Mode::RWXU;
const FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ImageAssetErrorKind {
    StorageUnavailable,
    InvalidBase64,
    InvalidPng,
    TooLarge,
    WriteFailed,
}

impl ImageAssetErrorKind {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::StorageUnavailable => "image_asset_storage_unavailable",
            Self::InvalidBase64 => "image_result_invalid_base64",
            Self::InvalidPng => "image_result_invalid_png",
            Self::TooLarge => "image_result_too_large",
            Self::WriteFailed => "image_asset_write_failed",
        }
    }

    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::StorageUnavailable => "Image asset storage is unavailable.",
            Self::InvalidBase64 => "The generated image Base64 is invalid.",
            Self::InvalidPng => "The generated image is not a valid PNG.",
            Self::TooLarge => "The generated image exceeds local limits.",
            Self::WriteFailed => "The generated image could not be saved.",
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ImageAssetResult {
    status: &'static str,
    path: String,
    #[serde(rename = "mimeType")]
    mime_type: &'static str,
    width: u32,
    height: u32,
    bytes: u64,
    sha256: String,
    #[serde(rename = "assetId")]
    asset_id: Uuid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PublicationStage {
    AfterCreate,
    AfterWrite,
    AfterFileSync,
    AfterReadBack,
    AfterLink,
    AfterDirectorySync,
    AfterTemporaryUnlink,
    FinalValidation,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PublicationFault {
    stage: Option<PublicationStage>,
    cleanup_fails: bool,
    #[cfg(test)]
    delay_stage: Option<PublicationStage>,
    #[cfg(test)]
    delay: std::time::Duration,
}

impl PublicationFault {
    #[cfg(test)]
    pub(super) const fn at(stage: PublicationStage) -> Self {
        Self {
            stage: Some(stage),
            cleanup_fails: false,
            delay_stage: None,
            delay: std::time::Duration::ZERO,
        }
    }

    #[cfg(test)]
    pub(super) const fn with_cleanup_failure(stage: PublicationStage) -> Self {
        Self {
            stage: Some(stage),
            cleanup_fails: true,
            delay_stage: None,
            delay: std::time::Duration::ZERO,
        }
    }

    #[cfg(test)]
    pub(super) const fn with_delay(stage: PublicationStage, delay: std::time::Duration) -> Self {
        Self {
            stage: None,
            cleanup_fails: false,
            delay_stage: Some(stage),
            delay,
        }
    }

    fn triggers(self, stage: PublicationStage) -> bool {
        self.stage == Some(stage)
    }

    fn cleanup_fails(self) -> bool {
        self.cleanup_fails
    }
}

#[derive(Clone, Copy, Debug)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug)]
pub(super) struct AdmittedAssetRoot {
    configured_path: PathBuf,
    canonical_path: PathBuf,
    directory: OwnedFd,
    identity: FileIdentity,
}

impl AdmittedAssetRoot {
    pub(super) fn admit(configured_path: PathBuf) -> Result<Self, ImageAssetErrorKind> {
        validate_root_path(&configured_path)?;
        create_root_if_missing(&configured_path)?;
        let directory = open_root(&configured_path)?;
        rustix::fs::fchmod(&directory, ROOT_MODE)
            .map_err(|_| ImageAssetErrorKind::StorageUnavailable)?;
        rustix::fs::fsync(&directory).map_err(|_| ImageAssetErrorKind::StorageUnavailable)?;
        let stat = private_directory_stat(&directory)?;
        let canonical_path = configured_path
            .canonicalize()
            .map_err(|_| ImageAssetErrorKind::StorageUnavailable)?;
        if !canonical_path.is_absolute() {
            return Err(ImageAssetErrorKind::StorageUnavailable);
        }
        let root = Self {
            configured_path,
            canonical_path,
            directory,
            identity: stat_identity(&stat)?,
        };
        root.revalidate()?;
        Ok(root)
    }

    fn revalidate(&self) -> Result<(), ImageAssetErrorKind> {
        let held = private_directory_stat(&self.directory)?;
        if !same_identity(self.identity, &held) {
            return Err(ImageAssetErrorKind::StorageUnavailable);
        }
        let current = open_root(&self.configured_path)?;
        let current_stat = private_directory_stat(&current)?;
        if !same_identity(self.identity, &current_stat)
            || self.configured_path.canonicalize().ok().as_ref() != Some(&self.canonical_path)
        {
            return Err(ImageAssetErrorKind::StorageUnavailable);
        }
        Ok(())
    }
}

pub(super) fn process_image_response(
    body: Bytes,
    root: &AdmittedAssetRoot,
    fault: PublicationFault,
) -> Result<ImageAssetResult, ImageAssetErrorKind> {
    debug_assert!(lifecycle_budget_is_valid());
    let encoded = take_base64(body)?;
    let png = decode_base64(encoded)?;
    let (width, height) = validate_png(&png)?;
    publish_png(root, &png, width, height, Uuid::new_v4(), fault)
}

fn lifecycle_budget_is_valid() -> bool {
    [
        MCP_JSON_RESPONSE_LIMIT * 2,
        MAX_BASE64_BYTES + MAX_COMPRESSED_PNG_BYTES,
        MAX_COMPRESSED_PNG_BYTES + PNG_DECODER_INTERNAL_LIMIT + MAX_DECODED_FRAME_BYTES,
        MAX_COMPRESSED_PNG_BYTES + READ_BACK_CHUNK_BYTES,
    ]
    .into_iter()
    .all(|phase| phase <= MCP_SINGLE_CALL_PEAK_BYTES)
}

#[derive(Deserialize)]
struct ImagesResponse {
    data: Vec<ImagesResponseItem>,
}

#[derive(Deserialize)]
struct ImagesResponseItem {
    b64_json: Option<String>,
}

fn take_base64(body: Bytes) -> Result<String, ImageAssetErrorKind> {
    let parsed = serde_json::from_slice::<ImagesResponse>(&body)
        .map_err(|_| ImageAssetErrorKind::InvalidBase64);
    drop(body);
    let encoded = parsed?
        .data
        .into_iter()
        .find_map(|item| item.b64_json)
        .ok_or(ImageAssetErrorKind::InvalidBase64)?;
    if encoded.is_empty() {
        return Err(ImageAssetErrorKind::InvalidBase64);
    }
    if encoded.len() > MAX_BASE64_BYTES {
        return Err(ImageAssetErrorKind::TooLarge);
    }
    Ok(encoded)
}

fn decode_base64(encoded: String) -> Result<Vec<u8>, ImageAssetErrorKind> {
    let decoded = STRICT_STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| ImageAssetErrorKind::InvalidBase64);
    drop(encoded);
    let decoded = decoded?;
    if decoded.len() > MAX_COMPRESSED_PNG_BYTES {
        return Err(ImageAssetErrorKind::TooLarge);
    }
    Ok(decoded)
}

fn validate_png(png: &[u8]) -> Result<(u32, u32), ImageAssetErrorKind> {
    let limits = png::Limits {
        bytes: PNG_DECODER_INTERNAL_LIMIT,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(png), limits);
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let mut reader = decoder
        .read_info()
        .map_err(|error| classify_png_error(&error))?;
    let (width, height) = reader.info().size();
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ImageAssetErrorKind::TooLarge)?;
    if width == 0
        || height == 0
        || u64::from(width) >= super::MAX_IMAGE_EDGE_EXCLUSIVE
        || u64::from(height) >= super::MAX_IMAGE_EDGE_EXCLUSIVE
        || pixels > super::MAX_IMAGE_PIXELS
    {
        return Err(ImageAssetErrorKind::TooLarge);
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or(ImageAssetErrorKind::TooLarge)?;
    if output_size > MAX_DECODED_FRAME_BYTES {
        return Err(ImageAssetErrorKind::TooLarge);
    }
    let mut frame = Vec::new();
    frame
        .try_reserve_exact(output_size)
        .map_err(|_| ImageAssetErrorKind::TooLarge)?;
    frame.resize(output_size, 0);
    reader
        .next_frame(&mut frame)
        .map_err(|error| classify_png_error(&error))?;
    reader
        .finish()
        .map_err(|error| classify_png_error(&error))?;
    drop(reader);
    drop(frame);
    Ok((width, height))
}

fn classify_png_error(error: &png::DecodingError) -> ImageAssetErrorKind {
    match error {
        png::DecodingError::LimitsExceeded => ImageAssetErrorKind::TooLarge,
        png::DecodingError::IoError(_)
        | png::DecodingError::Format(_)
        | png::DecodingError::Parameter(_) => ImageAssetErrorKind::InvalidPng,
    }
}

fn publish_png(
    root: &AdmittedAssetRoot,
    png: &[u8],
    width: u32,
    height: u32,
    asset_id: Uuid,
    fault: PublicationFault,
) -> Result<ImageAssetResult, ImageAssetErrorKind> {
    root.revalidate()?;
    let final_name = format!("{asset_id}.png");
    let temporary_name = format!(".{asset_id}.{}.tmp", Uuid::new_v4());
    let mut cleanup = PublicationCleanup::new(root, temporary_name.clone(), fault);
    let temporary_fd = rustix::fs::openat(
        &root.directory,
        temporary_name.as_str(),
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        FILE_MODE,
    )
    .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    cleanup.temporary_owned = true;
    rustix::fs::fchmod(&temporary_fd, FILE_MODE).map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    let temporary_stat = private_regular_file_stat(&temporary_fd)?;
    injected_failure(fault, PublicationStage::AfterCreate)?;

    let mut temporary_file = File::from(temporary_fd);
    temporary_file
        .write_all(png)
        .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    temporary_file
        .flush()
        .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    injected_failure(fault, PublicationStage::AfterWrite)?;
    temporary_file
        .sync_all()
        .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    injected_failure(fault, PublicationStage::AfterFileSync)?;
    temporary_file
        .seek(SeekFrom::Start(0))
        .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    let (bytes, sha256) =
        verify_reader(&mut temporary_file, png).map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    injected_failure(fault, PublicationStage::AfterReadBack)?;

    let named_temporary = rustix::fs::statat(
        &root.directory,
        temporary_name.as_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    if !same_stat_identity(&temporary_stat, &named_temporary)
        || !stat_is_private_regular_file(&named_temporary)
    {
        return Err(ImageAssetErrorKind::WriteFailed);
    }
    rustix::fs::linkat(
        &root.directory,
        temporary_name.as_str(),
        &root.directory,
        final_name.as_str(),
        AtFlags::empty(),
    )
    .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    cleanup.final_name = Some(final_name.clone());
    let final_stat = rustix::fs::statat(
        &root.directory,
        final_name.as_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    if !same_stat_identity(&temporary_stat, &final_stat)
        || !stat_is_private_regular_file(&final_stat)
    {
        return Err(ImageAssetErrorKind::WriteFailed);
    }
    injected_failure(fault, PublicationStage::AfterLink)?;
    rustix::fs::fsync(&root.directory).map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    injected_failure(fault, PublicationStage::AfterDirectorySync)?;
    rustix::fs::unlinkat(&root.directory, temporary_name.as_str(), AtFlags::empty())
        .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    cleanup.temporary_owned = false;
    injected_failure(fault, PublicationStage::AfterTemporaryUnlink)?;
    rustix::fs::fsync(&root.directory).map_err(|_| ImageAssetErrorKind::WriteFailed)?;

    let path = validate_published_asset(root, &final_name, bytes, fault)?;
    cleanup.commit();
    Ok(ImageAssetResult {
        status: "success",
        path,
        mime_type: "image/png",
        width,
        height,
        bytes,
        sha256,
        asset_id,
    })
}

fn validate_published_asset(
    root: &AdmittedAssetRoot,
    final_name: &str,
    bytes: u64,
    fault: PublicationFault,
) -> Result<String, ImageAssetErrorKind> {
    let final_path = root.configured_path.join(final_name);
    let canonical_path = final_path
        .canonicalize()
        .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    if !canonical_path.is_absolute()
        || canonical_path.parent() != Some(root.canonical_path.as_path())
    {
        return Err(ImageAssetErrorKind::WriteFailed);
    }
    let metadata =
        fs::symlink_metadata(&canonical_path).map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    if !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.len() != bytes
    {
        return Err(ImageAssetErrorKind::WriteFailed);
    }
    root.revalidate()
        .map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    injected_failure(fault, PublicationStage::FinalValidation)?;
    let path = canonical_path
        .to_str()
        .ok_or(ImageAssetErrorKind::WriteFailed)?
        .to_owned();
    Ok(path)
}

fn verify_reader(reader: &mut impl Read, expected: &[u8]) -> Result<(u64, String), std::io::Error> {
    let mut buffer = vec![0_u8; READ_BACK_CHUNK_BYTES].into_boxed_slice();
    let mut offset = 0_usize;
    let mut digest = Sha256::new();
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let end = offset.checked_add(read).ok_or_else(invalid_file_data)?;
        if expected.get(offset..end) != Some(&buffer[..read]) {
            return Err(invalid_file_data());
        }
        digest.update(&buffer[..read]);
        offset = end;
    }
    if offset != expected.len() {
        return Err(invalid_file_data());
    }
    let bytes = u64::try_from(offset).map_err(|_| invalid_file_data())?;
    Ok((bytes, hex::encode(digest.finalize())))
}

fn invalid_file_data() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "asset verification failed")
}

struct PublicationCleanup<'a> {
    root: &'a AdmittedAssetRoot,
    temporary_name: String,
    temporary_owned: bool,
    final_name: Option<String>,
    committed: bool,
    fault: PublicationFault,
}

impl<'a> PublicationCleanup<'a> {
    fn new(root: &'a AdmittedAssetRoot, temporary_name: String, fault: PublicationFault) -> Self {
        Self {
            root,
            temporary_name,
            temporary_owned: false,
            final_name: None,
            committed: false,
            fault,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
        self.final_name = None;
    }

    fn cleanup(&mut self) {
        if self.committed || self.fault.cleanup_fails() {
            return;
        }
        if let Some(final_name) = self.final_name.take() {
            let _ =
                rustix::fs::unlinkat(&self.root.directory, final_name.as_str(), AtFlags::empty());
        }
        if self.temporary_owned {
            let _ = rustix::fs::unlinkat(
                &self.root.directory,
                self.temporary_name.as_str(),
                AtFlags::empty(),
            );
            self.temporary_owned = false;
        }
        let _ = rustix::fs::fsync(&self.root.directory);
    }
}

impl Drop for PublicationCleanup<'_> {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn injected_failure(
    fault: PublicationFault,
    stage: PublicationStage,
) -> Result<(), ImageAssetErrorKind> {
    #[cfg(test)]
    if fault.delay_stage == Some(stage) {
        std::thread::sleep(fault.delay);
    }
    if fault.triggers(stage) {
        Err(ImageAssetErrorKind::WriteFailed)
    } else {
        Ok(())
    }
}

fn validate_root_path(path: &Path) -> Result<(), ImageAssetErrorKind> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || !path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(ImageAssetErrorKind::StorageUnavailable);
    }
    Ok(())
}

fn create_root_if_missing(path: &Path) -> Result<(), ImageAssetErrorKind> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or(ImageAssetErrorKind::StorageUnavailable)?;
            let parent_metadata = fs::symlink_metadata(parent)
                .map_err(|_| ImageAssetErrorKind::StorageUnavailable)?;
            if !parent_metadata.file_type().is_dir() {
                return Err(ImageAssetErrorKind::StorageUnavailable);
            }
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(_) => Err(ImageAssetErrorKind::StorageUnavailable),
            }
        }
        Err(_) => Err(ImageAssetErrorKind::StorageUnavailable),
    }
}

fn open_root(path: &Path) -> Result<OwnedFd, ImageAssetErrorKind> {
    rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| ImageAssetErrorKind::StorageUnavailable)
}

fn private_directory_stat(directory: &OwnedFd) -> Result<rustix::fs::Stat, ImageAssetErrorKind> {
    let stat = rustix::fs::fstat(directory).map_err(|_| ImageAssetErrorKind::StorageUnavailable)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || Mode::from_raw_mode(stat.st_mode) != ROOT_MODE
    {
        return Err(ImageAssetErrorKind::StorageUnavailable);
    }
    Ok(stat)
}

fn private_regular_file_stat(file: &OwnedFd) -> Result<rustix::fs::Stat, ImageAssetErrorKind> {
    let stat = rustix::fs::fstat(file).map_err(|_| ImageAssetErrorKind::WriteFailed)?;
    if !stat_is_private_regular_file(&stat) {
        return Err(ImageAssetErrorKind::WriteFailed);
    }
    Ok(stat)
}

fn stat_is_private_regular_file(stat: &rustix::fs::Stat) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
        && Mode::from_raw_mode(stat.st_mode) == FILE_MODE
}

fn stat_identity(stat: &rustix::fs::Stat) -> Result<FileIdentity, ImageAssetErrorKind> {
    Ok(FileIdentity {
        device: stat
            .st_dev
            .try_into()
            .map_err(|_| ImageAssetErrorKind::StorageUnavailable)?,
        inode: stat.st_ino,
    })
}

fn same_identity(expected: FileIdentity, actual: &rustix::fs::Stat) -> bool {
    u64::try_from(actual.st_dev).ok() == Some(expected.device) && actual.st_ino == expected.inode
}

fn same_stat_identity(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    };

    use tempfile::TempDir;

    use super::*;

    fn valid_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("PNG header");
            writer
                .write_image_data(&[0x12, 0x34, 0x56, 0xff])
                .expect("PNG pixels");
            writer.finish().expect("PNG end");
        }
        bytes
    }

    fn temporary_asset_root() -> (TempDir, PathBuf) {
        let temporary = TempDir::new().expect("temporary app data");
        let path = temporary.path().join("mcp-images");
        (temporary, path)
    }

    fn directory_entries(path: &Path) -> Vec<PathBuf> {
        fs::read_dir(path)
            .expect("read asset root")
            .map(|entry| entry.expect("asset entry").path())
            .collect()
    }

    #[test]
    fn resource_limits_fit_the_reviewed_single_call_budget() {
        let encoded_limit = MAX_COMPRESSED_PNG_BYTES.div_ceil(3) * 4;
        assert_eq!(MAX_BASE64_BYTES, encoded_limit);
        let max_pixels = usize::try_from(super::super::MAX_IMAGE_PIXELS)
            .expect("pixel limit fits supported targets");
        assert_eq!(MAX_DECODED_FRAME_BYTES, max_pixels * 8);
        assert!(lifecycle_budget_is_valid());
    }

    struct RecordingReader<R> {
        inner: R,
        largest_request: usize,
    }

    impl<R: Read> Read for RecordingReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.largest_request = self.largest_request.max(buffer.len());
            self.inner.read(buffer)
        }
    }

    #[test]
    fn read_back_verification_uses_fixed_chunks_and_exact_bytes() {
        let expected = vec![0x5a; READ_BACK_CHUNK_BYTES * 2 + 17];
        let mut reader = RecordingReader {
            inner: Cursor::new(expected.as_slice()),
            largest_request: 0,
        };
        let (bytes, sha256) = verify_reader(&mut reader, &expected).expect("verified bytes");
        assert_eq!(bytes, expected.len() as u64);
        assert_eq!(sha256, hex::encode(Sha256::digest(&expected)));
        assert_eq!(reader.largest_request, READ_BACK_CHUNK_BYTES);

        let mut different = expected.clone();
        different[READ_BACK_CHUNK_BYTES] ^= 1;
        assert!(verify_reader(&mut Cursor::new(different), &expected).is_err());
        assert!(
            verify_reader(&mut Cursor::new(&expected[..expected.len() - 1]), &expected).is_err()
        );
    }

    #[test]
    fn root_admission_requires_an_absolute_private_real_directory() {
        assert_eq!(
            AdmittedAssetRoot::admit(PathBuf::from("relative/mcp-images"))
                .expect_err("relative root"),
            ImageAssetErrorKind::StorageUnavailable
        );
        assert_eq!(
            AdmittedAssetRoot::admit(PathBuf::from("/tmp/a/../mcp-images"))
                .expect_err("traversing root"),
            ImageAssetErrorKind::StorageUnavailable
        );

        let temporary = TempDir::new().expect("temporary root");
        let non_directory = temporary.path().join("not-a-directory");
        File::create(&non_directory).expect("create non-directory");
        assert_eq!(
            AdmittedAssetRoot::admit(non_directory).expect_err("non-directory root"),
            ImageAssetErrorKind::StorageUnavailable
        );

        let real = temporary.path().join("real-root");
        fs::create_dir(&real).expect("real root");
        let link = temporary.path().join("linked-root");
        symlink(&real, &link).expect("root symlink");
        assert_eq!(
            AdmittedAssetRoot::admit(link).expect_err("symlink root"),
            ImageAssetErrorKind::StorageUnavailable
        );

        let private = temporary.path().join("mcp-images");
        fs::create_dir(&private).expect("configured root");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o755))
            .expect("set permissive mode");
        AdmittedAssetRoot::admit(private.clone()).expect("admit and privatize root");
        assert_eq!(
            fs::metadata(private)
                .expect("root metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
    }

    #[test]
    fn strict_base64_and_full_png_decode_reject_invalid_or_oversized_results() {
        assert_eq!(
            decode_base64("AQ".to_owned()).expect_err("missing canonical padding"),
            ImageAssetErrorKind::InvalidBase64
        );
        assert_eq!(
            decode_base64("AQ==\n".to_owned()).expect_err("whitespace"),
            ImageAssetErrorKind::InvalidBase64
        );
        assert_eq!(
            validate_png(b"not a PNG").expect_err("non-PNG"),
            ImageAssetErrorKind::InvalidPng
        );
        let mut truncated = valid_png();
        truncated.truncate(truncated.len() - 8);
        assert_eq!(
            validate_png(&truncated).expect_err("truncated PNG"),
            ImageAssetErrorKind::InvalidPng
        );

        let mut oversized = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut oversized, 3_840, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("oversized PNG header");
            writer
                .write_image_data(&vec![0; 3_840])
                .expect("oversized PNG data");
            writer.finish().expect("oversized PNG end");
        }
        assert_eq!(
            validate_png(&oversized).expect_err("edge limit"),
            ImageAssetErrorKind::TooLarge
        );
    }

    #[test]
    fn publication_is_private_no_clobber_and_cleans_controllable_failures() {
        let png = valid_png();
        for stage in [
            PublicationStage::AfterCreate,
            PublicationStage::AfterWrite,
            PublicationStage::AfterFileSync,
            PublicationStage::AfterReadBack,
            PublicationStage::AfterLink,
            PublicationStage::AfterDirectorySync,
            PublicationStage::AfterTemporaryUnlink,
            PublicationStage::FinalValidation,
        ] {
            let (_temporary, path) = temporary_asset_root();
            let root = AdmittedAssetRoot::admit(path.clone()).expect("admitted root");
            assert_eq!(
                publish_png(
                    &root,
                    &png,
                    1,
                    1,
                    Uuid::new_v4(),
                    PublicationFault::at(stage),
                )
                .expect_err("injected publication failure"),
                ImageAssetErrorKind::WriteFailed,
                "stage {stage:?}"
            );
            assert!(
                directory_entries(&path).is_empty(),
                "residue after {stage:?}"
            );
        }

        let (_temporary, path) = temporary_asset_root();
        let root = AdmittedAssetRoot::admit(path.clone()).expect("admitted root");
        let asset_id = Uuid::nil();
        let existing_path = path.join(format!("{asset_id}.png"));
        let mut existing = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&existing_path)
            .expect("existing asset");
        existing.write_all(b"existing").expect("existing bytes");
        existing.sync_all().expect("existing sync");
        drop(existing);
        assert_eq!(
            publish_png(&root, &png, 1, 1, asset_id, PublicationFault::default(),)
                .expect_err("no-clobber publication"),
            ImageAssetErrorKind::WriteFailed
        );
        assert_eq!(
            fs::read(existing_path).expect("preserved asset"),
            b"existing"
        );
        assert_eq!(directory_entries(&path).len(), 1);
    }

    #[test]
    fn cleanup_failure_leaves_only_an_unreferenced_private_orphan() {
        let (_temporary, path) = temporary_asset_root();
        let root = AdmittedAssetRoot::admit(path.clone()).expect("admitted root");
        assert_eq!(
            publish_png(
                &root,
                &valid_png(),
                1,
                1,
                Uuid::new_v4(),
                PublicationFault::with_cleanup_failure(PublicationStage::AfterCreate),
            )
            .expect_err("cleanup failure"),
            ImageAssetErrorKind::WriteFailed
        );
        let entries = directory_entries(&path);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            fs::metadata(&entries[0])
                .expect("orphan metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
    }
}
